// Pure source parsing for the documentation-parity gate, extracted from check-docs-parity.mjs so
// the struct/flag/key extraction can be unit-tested without the real source tree. The parser must
// never silently return fewer surfaces than exist — a dropped flag is exactly what would let an
// undocumented one pass this gate — so `cliFlags` scans line by line (no backtracking-prone regex),
// handles multi-line `#[arg(...)]` blocks, `long = "override"`, and an optional `pub`/`pub(crate)`
// visibility on the field. The caller floors the counts as a second guard.
import assert from "node:assert/strict";

/** Returns the brace-matched body of the first `struct <name> { ... }` in `source`. */
export function structBody(source, name) {
  const start = source.indexOf(`struct ${name} {`);
  assert.notEqual(start, -1, `struct ${name} not found`);
  let depth = 0;
  const open = source.indexOf("{", start);
  for (let i = open; i < source.length; i += 1) {
    if (source[i] === "{") depth += 1;
    else if (source[i] === "}") {
      depth -= 1;
      if (depth === 0) return source.slice(open + 1, i);
    }
  }
  throw new Error(`unterminated struct ${name}`);
}

/** Convert underscore-separated field names to kebab case for derived CLI flags. */
export const kebab = (field) => field.replace(/_/g, "-");
/** Convert underscore-plus-lowercase-letter sequences to camel case for config keys. */
export const camel = (field) => field.replace(/_([a-z])/g, (_, c) => c.toUpperCase());

// A struct field declaration, tolerating an optional `pub` / `pub(crate)` visibility.
const FIELD = /^(?:pub(?:\([^)]*\))?\s+)?([a-z][a-z0-9_]*)\s*:/;
// Capitalized config value types that are not sections.
const SCALAR_TYPES = new Set(["PathBuf", "String"]);
/** Return the opening-minus-closing square-bracket count in an attribute line. */
const netBrackets = (line) =>
  (line.match(/\[/g) || []).length - (line.match(/\]/g) || []).length;

/**
 * Every `--long` flag declared by clap on the given struct body. `long = "x"` overrides the
 * kebab-cased field name; a bare `long` derives it. A linear line scan tracks a multi-line
 * `#[arg(...)]` block by bracket depth, so no attribute is skipped and no regex can backtrack.
 * @param {string} structBodyText Body from `structBody(cliSource, "CheckOptions")`.
 * @returns {string[]}
 */
export function cliFlags(structBodyText) {
  const flags = [];
  let pendingArg = ""; // the most recent #[arg(...)] attribute awaiting its field
  let attr = ""; // an attribute currently being accumulated across lines
  let depth = 0; // unclosed `[` while inside a multi-line attribute
  /** Retain a completed arg attribute containing a long flag and clear the current attribute buffer. */
  const flush = () => {
    if (/^\s*#\[arg\b/.test(attr) && /\blong\b/.test(attr)) pendingArg = attr;
    attr = "";
  };
  for (const line of structBodyText.split("\n")) {
    if (depth > 0) {
      attr += `\n${line}`;
      depth += netBrackets(line);
      if (depth <= 0) flush();
      continue;
    }
    const trimmed = line.trim();
    if (trimmed.startsWith("#[")) {
      attr = line;
      depth = netBrackets(line);
      if (depth <= 0) flush();
      continue;
    }
    if (trimmed === "" || trimmed.startsWith("///")) continue; // keep pendingArg across doc/blank
    const field = trimmed.match(FIELD);
    if (field && pendingArg) {
      const override = pendingArg.match(/long\s*=\s*"([^"]+)"/);
      flags.push(`--${override ? override[1] : kebab(field[1])}`);
    }
    pendingArg = ""; // a field consumes (or, without a pending arg, clears) the pending attribute
  }
  return flags;
}

/**
 * Every field of a config struct body with its camelCased key, declared type and whether a
 * `#[serde(flatten)]` attribute precedes it (its keys then sit at this level, not under its name).
 * @param {string} structBodyText A struct body.
 * @returns {{key: string, type: string, flatten: boolean}[]}
 */
export function configFields(structBodyText) {
  const fields = [];
  let flatten = false;
  for (const raw of structBodyText.split("\n")) {
    const line = raw.trim();
    if (line.startsWith("#[")) {
      flatten ||= /^#\[serde\([^)]*\bflatten\b/.test(line);
      continue;
    }
    const field = line.match(FIELD);
    if (!field) continue;
    const type = line.slice(line.indexOf(":") + 1).replace(/,\s*$/, "").trim();
    fields.push({ key: camel(field[1]), type, flatten });
    flatten = false;
  }
  return fields;
}

/**
 * Every config key reachable from struct `name`: flattened fields contribute their struct's keys at
 * the same level, and a field typed `Option<Section>` contributes `key.sectionKey` entries beside
 * `key`. A flattened or section type `findStruct` cannot resolve throws, so a moved struct fails
 * the gate instead of dropping its keys.
 * @param {(name: string) => string | null} findStruct Body of a struct by name, or null.
 * @param {string} name Struct to start from.
 * @param {string} [prefix] Dotted section path for nested keys.
 * @returns {string[]}
 */
export function configSurface(findStruct, name, prefix = "") {
  const body = findStruct(name);
  if (body === null) throw new Error(`config struct ${name} not found`);
  const keys = [];
  for (const field of configFields(body)) {
    if (field.flatten) {
      keys.push(...configSurface(findStruct, field.type.split("::").pop(), prefix));
      continue;
    }
    keys.push(prefix + field.key);
    // A capitalized `Option<T>` is a section unless it is a known scalar; an unresolved one throws
    // like a lost flattened type, so a renamed section cannot drop its keys.
    const section = field.type.match(/^Option<([A-Z]\w*)>$/);
    if (section && !SCALAR_TYPES.has(section[1])) {
      keys.push(...configSurface(findStruct, section[1], `${prefix}${field.key}.`));
    }
  }
  return keys;
}

/**
 * Body of struct `name` in any of `sources`, following `type Name = path::Target;` aliases and
 * `use path::Target as Name;` declarations to the target; null when no source declares it.
 * @param {string[]} sources Rust source texts.
 * @param {string} name A struct or alias name.
 * @returns {string | null}
 */
export function findConfigStruct(sources, name) {
  for (const source of sources) {
    const alias =
      source.match(new RegExp(`\\btype ${name}\\s*=\\s*([\\w:]+)\\s*;`)) ||
      source.match(new RegExp(`\\buse\\s+[\\w:]*?(\\w+)\\s+as\\s+${name}\\s*;`));
    if (alias) return findConfigStruct(sources, alias[1].split("::").pop());
    if (new RegExp(`\\bstruct ${name} \\{`).test(source)) return structBody(source, name);
  }
  return null;
}

/**
 * Whether `flag` appears in `docs` as a complete flag token, not merely as a prefix of a longer one
 * (`--exclude` must not be satisfied by `--exclude-defaults`). Boundaries are fixed-width lookarounds
 * over the trusted docs text, so there is no backtracking risk.
 * @param {string} docs Concatenated documentation text.
 * @param {string} flag A `--long` flag.
 * @returns {boolean}
 */
export function flagDocumented(docs, flag) {
  const escaped = flag.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return new RegExp(`(?<![\\w-])${escaped}(?![\\w-])`).test(docs);
}
