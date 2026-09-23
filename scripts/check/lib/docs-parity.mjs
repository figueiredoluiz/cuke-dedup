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

export const kebab = (field) => field.replace(/_/g, "-");
export const camel = (field) => field.replace(/_([a-z])/g, (_, c) => c.toUpperCase());

// A struct field declaration, tolerating an optional `pub` / `pub(crate)` visibility.
const FIELD = /^(?:pub(?:\([^)]*\))?\s+)?([a-z][a-z0-9_]*)\s*:/;
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
 * Every camelCased config key declared on the given struct body (serde `rename_all = "camelCase"`).
 * @param {string} structBodyText Body from `structBody(configSource, "RawConfig")`.
 * @returns {string[]}
 */
export function configKeys(structBodyText) {
  const keys = [];
  for (const raw of structBodyText.split("\n")) {
    const field = raw.trim().match(FIELD);
    if (field) keys.push(camel(field[1]));
  }
  return keys;
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
