// Language-knowledge ratchet: counts how much of the language-neutral core still names a language,
// framework or grammar, and fails when a count differs from the committed baseline.
// Lexical, not Rust name resolution. Comments are skipped; string literals are skipped except
// `method:<lang>:` markers and whole-token strings (config keys). Inline `#[cfg(test)]` modules are
// counted; only dedicated `tests`/`tests.rs` files are outside the scan. Kinds overlap: an
// identifier that is part of an edge (`crate::ruby`) also counts under `name:`.
import { lstat, readFile, readdir, writeFile } from "node:fs/promises";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

// First match wins; a trailing `/` matches a directory prefix. `core` and `registry` are scanned.
const classes = [
  ["src/lib.rs", "exempt"], ["src/main.rs", "exempt"],
  ["src/source_adapter.rs", "registry"],
  ["src/typescript.rs", "adapter"], ["src/typescript/", "adapter"],
  ["src/ruby.rs", "adapter"], ["src/ruby/", "adapter"],
  ["src/reporters/templates/", "asset"],
];
const sections = ["core", "registry"];
const words = ["ruby", "typescript", "javascript", "tsx", "jest", "vitest", "playwright", "cypress", "cucumberjs"];
const shapes = {
  module: /^crate::(ruby|typescript)$/,
  marker: /^marker:method:[A-Za-z0-9_-]+$/,
  name: new RegExp(`^name:(${words.join("|")})$`),
  key: new RegExp(`^key:(${words.join("|")})$`),
  grammar: /^grammar:tree_sitter_\w+$/,
};
const identifier = /^[A-Za-z_][A-Za-z_0-9]*$/;
/** Order strings by code unit so the baseline sorts identically on every platform. */
const cmp = (a, b) => (a < b ? -1 : a > b ? 1 : 0);
/** Order inventory rows by file, then edge. */
const cmpRow = (a, b) => cmp(a[0], b[0]) || cmp(a[1], b[1]);
/** Return the kind of an edge key, or undefined when the key matches no known shape. */
const kindOf = (key) => Object.keys(shapes).find((kind) => shapes[kind].test(key));
// Words that may also be spelled as two segments (`TypeScript`, `java_script`, `cucumber-js`).
const compounds = { typescript: ["type", "script"], javascript: ["java", "script"], cucumberjs: ["cucumber", "js"] };
const nameCache = new Map();
/** Return the language words a name spells as whole snake_case, camelCase or digit-delimited segments (memoized per distinct name). */
const named = (text) => {
  if (!nameCache.has(text)) {
    const parts = ` ${text.split(/[_.-]|(?<=[a-z0-9])(?=[A-Z])|(?<=[A-Z])(?=[A-Z][a-z])|(?<=[A-Za-z])(?=\d)|(?<=\d)(?=[A-Za-z])/).filter(Boolean).join(" ").toLowerCase()} `;
    nameCache.set(text, words.filter((word) => [[word], compounds[word] ?? []].some((seq) => seq.length && parts.includes(` ${seq.join(" ")} `))));
  }
  return nameCache.get(text);
};

/** Classify a repository-relative `src/` path; a trailing `/` marks a directory. Unknown non-Rust files fail closed. */
export function classify(path) {
  if (!path.startsWith("src/")) throw Error(`${path}: unclassified file in boundary scope`);
  const rule = classes.find(([prefix]) => (prefix.endsWith("/") ? path.startsWith(prefix) : path === prefix));
  if (rule) return rule[1];
  if (path.split("/").some((part) => part === "tests" || part === "tests.rs")) return "test";
  if (path.endsWith(".rs") || path.endsWith("/")) return "core";
  throw Error(`${path}: unclassified file in boundary scope`);
}
/** Tokenize Rust outside comments and literals, record raw-identifier positions and string contents, and reject unterminated comments or strings. */
function tokens(source) {
  const result = [];
  result.rawIdentifiers = new Set();
  result.strings = [];
  for (let i = 0; i < source.length;) {
    if (/\s/.test(source[i])) { i++; continue; }
    if (source.startsWith("//", i)) { i = source.indexOf("\n", i); if (i < 0) break; continue; }
    if (source.startsWith("/*", i)) {
      let depth = 1; i += 2;
      while (i < source.length && depth) {
        if (source.startsWith("/*", i)) { depth++; i += 2; }
        else if (source.startsWith("*/", i)) { depth--; i += 2; }
        else i++;
      }
      if (depth) throw Error("unterminated block comment");
      continue;
    }
    const raw = /^(?:b|c)?r(#{0,255})"/.exec(source.slice(i));
    if (raw) {
      const end = source.indexOf(`"${raw[1]}`, i + raw[0].length);
      if (end < 0) throw Error("unterminated raw string");
      result.strings.push(source.slice(i + raw[0].length, end));
      i = end + 1 + raw[1].length; continue;
    }
    if (source[i] === '"') {
      const start = ++i;
      while (i < source.length && source[i] !== '"') i += source[i] === "\\" ? 2 : 1;
      if (i >= source.length) throw Error("unterminated string");
      result.strings.push(source.slice(start, i));
      i++; continue;
    }
    const character = /^'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^'\\\r\n])'/u.exec(source.slice(i));
    if (character) { i += character[0].length; continue; }
    const token = /^(?:r#[A-Za-z_][A-Za-z_0-9]*|[A-Za-z_][A-Za-z_0-9]*|::|.)/s.exec(source.slice(i))[0];
    if (token.startsWith("r#")) result.rawIdentifiers.add(result.length);
    result.push(token.startsWith("r#") ? token.slice(2) : token); i += token.length;
  }
  return result;
}
/** Resolve relative prefixes and return the module or grammar edge key of a path, or null; a lone `tree_sitter_*` name is a grammar edge only when imported. */
function edge(path, imported = false) {
  // Relative paths are possible adapter references even inside inline modules.
  // Count conservatively; proving a local same-named module is outside this lexical guard.
  let relative = 0;
  while (["self", "super"].includes(path[relative])) relative++;
  if (relative) path = ["crate", ...path.slice(relative)];
  if (path[0] === "crate" && ["ruby", "typescript"].includes(path[1])) return path.slice(0, 2).join("::");
  return /^tree_sitter_\w+$/.test(path[0] ?? "") && (imported || path.length > 1) ? `grammar:${path[0]}` : null;
}
/** Collect the `a::b::c` identifier chain starting at an index and return it with the index of its last token. */
function chain(input, start) {
  const path = [input[start]];
  let end = start;
  while (input[end + 1] === "::" && identifier.test(input[end + 2] ?? "")) { path.push(input[end + 2]); end += 2; }
  return { path, end };
}
/** Return the end index of the item starting at `from` (a `;` at bracket depth 0 or the enclosing close bracket) and its depth-0 `=` index, or -1. */
function itemSpan(input, from) {
  let depth = 0, eq = -1;
  for (let n = from + 1; n < input.length; n++) {
    if (["(", "[", "{"].includes(input[n])) depth++;
    else if ([")", "]", "}"].includes(input[n])) { if (--depth < 0) return { end: n, eq }; }
    else if (input[n] === ";" && !depth) return { end: n, eq };
    else if (input[n] === "=" && !depth && eq < 0) eq = n;
  }
  return { end: input.length, eq };
}
/** Count the inline `mod name {` blocks that enclose the token at `at`. */
function moduleDepth(input, at) {
  const stack = [];
  for (let n = 0; n < at; n++) {
    if (input[n] === "{") stack.push(input[n - 2] === "mod" && !input.rawIdentifiers.has(n - 2));
    else if (input[n] === "}") stack.pop();
  }
  return stack.filter(Boolean).length;
}
/** Parse lexical use-tree leaves and aliases, rejecting unsupported or malformed import syntax. */
function imports(input, rawIdentifiers, offset) {
  /** Recognize a keyword token only when its original spelling was not a raw identifier. */
  const keyword = (index, word) => input[index] === word && !rawIdentifiers.has(offset + index);
  let position = 0;
  const leaves = [];
  /** Consume one use-tree branch, recursively expanding groups into path and alias leaves. */
  function branch(prefix) {
    let path = [...prefix];
    if (input[position] === "::") position++;
    while (identifier.test(input[position] ?? "") || input[position] === "*") {
      const part = input[position++];
      if (keyword(position - 1, "as")) throw Error("invalid import alias");
      if (!keyword(position - 1, "self") || !path.length) path.push(part);
      if (input[position] !== "::") break;
      position++;
    }
    if (input[position] === "{") {
      position++;
      while (input[position] !== "}") {
        branch(path);
        if (input[position] === ",") position++;
        else if (input[position] !== "}") throw Error("unsupported grouped import");
        if (position >= input.length) throw Error("unterminated grouped import");
      }
      position++; return;
    }
    let alias = path.at(-1);
    if (keyword(position, "as")) {
      position++; alias = input[position++];
      if (!identifier.test(alias ?? "")) throw Error("unsupported import alias");
    }
    if (!path.length) throw Error("unsupported import");
    leaves.push({ path, alias });
  }
  branch([]);
  if (position !== input.length) throw Error("unsupported import syntax");
  return leaves;
}

/** Recognize a lexical use declaration boundary, accounting for visibility and raw identifiers. */
function isImport(input, index) {
  if (input[index] !== "use" || input.rawIdentifiers.has(index) || input[index + 1] === "<") return false;
  let before = index - 1;
  if (input[before] === ")") {
    let depth = 1;
    while (--before >= 0 && depth) {
      if (input[before] === ")") depth++;
      if (input[before] === "(") depth--;
    }
  }
  if (input[before] === "pub") before--;
  return before < 0 || [";", "{", "}", "]"].includes(input[before]);
}

/** Count language-knowledge edges per source file, resolving file-wide aliases and rejecting ambiguous or unsupported targeted imports. */
export function inventorySources(sources) {
  const inventory = {};
  for (const [file, source] of Object.entries(sources).sort(([a], [b]) => cmp(a, b))) {
    try {
      const input = tokens(source), aliases = new Map(), leaves = [], counts = {}, ignored = new Set();
      /** Increment the count of one edge key. */
      const bump = (key) => { counts[key] = (counts[key] ?? 0) + 1; };
      /** Increment the inventory count when a path resolves to a boundary edge. */
      const add = (path, imported) => { const key = edge(path, imported); if (key) bump(key); };
      for (let i = 0; i < input.length; i++) {
        if (!isImport(input, i)) continue;
        const end = input.indexOf(";", i);
        if (end < 0) throw Error("unterminated use declaration");
        // All import syntax is parsed: forward aliases must not hide unsupported targeted forms.
        const parsed = imports(input.slice(i + 1, end), input.rawIdentifiers, i + 1);
        for (const leaf of parsed) {
          leaves.push({ ...leaf, at: i });
          // `use tree_sitter_ruby;` binds a name to itself; recording it would read as an alias cycle.
          if (leaf.path.length === 1 && leaf.path[0] === leaf.alias) continue;
          const origins = aliases.get(leaf.alias) ?? [];
          origins.push(leaf.path); aliases.set(leaf.alias, origins);
        }
        for (let n = i; n <= end; n++) ignored.add(n);
        i = end;
      }
      /** Resolve file-wide aliases recursively, rejecting cycles and ambiguous targeted origins while preserving unrelated ambiguity. */
      function canonical(path, visited = new Set()) {
        if (path[0] === "crate") return path;
        if (aliases.has(path[0])) {
          if (visited.has(path[0])) throw Error(`cyclic import alias ${path[0]}`);
          visited.add(path[0]);
          const choices = aliases.get(path[0]).map((origin) => canonical([...origin, ...path.slice(1)], new Set(visited)));
          /** Report whether every origin maps to one value. */
          const same = (key) => new Set(choices.map(key)).size === 1;
          // Origins that differ in path but reach one edge (two leaves of one adapter module) are not ambiguous.
          if (same((choice) => choice.join("::")) || (edge(choices[0]) && same(edge))) return choices[0];
          if (choices.some((choice) => edge(choice) || choice.join("::") === "crate")) throw Error(`ambiguous targeted alias ${path[0]}`);
          return path;
        }
        return path;
      }
      for (const { path, at } of leaves) {
        const resolved = canonical(path);
        if (resolved.includes("*") && (edge(resolved) || resolved.join("::") === "crate::*")) throw Error("unsupported targeted wildcard import");
        // A wildcard that climbs out of the file's own module imports names this scan cannot see (a parent's `use crate::ruby;`), and a path that continues after `super` may go through a parent's alias (`use crate as root;`). Only `super::*` chains that stay inside inline modules (`mod tests { use super::*; }`) are accepted.
        const relative = resolved.slice(resolved[0] === "self" ? 1 : 0), climbs = relative.findIndex((part) => part !== "super");
        if (resolved.at(-1) === "*" && climbs > 0 && (climbs > moduleDepth(input, at) || relative.length !== climbs + 1)) throw Error("unsupported wildcard import beyond the file module");
        add(resolved, true);
      }
      for (let i = 0; i < input.length; i++) {
        if (input[i] === "type" && !input.rawIdentifiers.has(i) && input[i - 1] !== "$") {
          // Aliases are not resolved, so a `type` item naming a tracked path would hide its uses. A bodiless `type Item;` has no `=`.
          const { end, eq } = itemSpan(input, i);
          for (let n = eq + 1; eq >= 0 && n < end; n++) {
            if (identifier.test(input[n]) && !(input[n - 1] === "::" && identifier.test(input[n - 2] ?? "")) && edge(canonical(chain(input, n).path))) throw Error("unsupported `type` item naming a tracked path (alias or associated type)");
          }
        }
      }
      for (const token of input) if (identifier.test(token)) for (const word of named(token)) bump(`name:${word}`);
      for (let i = 0; i < input.length; i++) {
        // A leading `::` starts a path; `::` after an identifier continues one.
        if (ignored.has(i) || !identifier.test(input[i]) || (input[i - 1] === "::" && identifier.test(input[i - 2] ?? ""))) continue;
        const { path, end } = chain(input, i);
        i = end;
        add(canonical(path));
      }
      for (const text of input.strings) {
        for (const marker of text.matchAll(/(?<!\w)method:([A-Za-z0-9_-]+):/g)) bump(`marker:method:${marker[1]}`);
        if (/^[\w.-]+$/.test(text)) for (const word of named(text)) bump(`key:${word}`);
      }
      inventory[file] = Object.fromEntries(Object.entries(counts).sort(([a], [b]) => cmp(a, b)));
    } catch (error) { throw Error(`${file}: ${error.message}`); }
  }
  return inventory;
}

/** Flatten a per-file inventory into sorted `[file, edge, count]` rows. */
export function rows(inventory) {
  return Object.entries(inventory).flatMap(([file, counts]) => Object.entries(counts).map(([key, count]) => [file, key, count])).sort(cmpRow);
}
/** Return per-edge differences between actual and baseline rows, sorted by file and edge. */
export function diffRows(actual, baseline) {
  const found = new Map(actual.map(([file, key, count]) => [`${file}\0${key}`, count]));
  const expected = new Map(baseline.map(([file, key, count]) => [`${file}\0${key}`, count]));
  return [...new Set([...found.keys(), ...expected.keys()])].map((id) => {
    const [file, key] = id.split("\0");
    return { file, key, was: expected.get(id) ?? 0, now: found.get(id) ?? 0 };
  }).filter(({ was, now }) => was !== now).sort((a, b) => cmpRow([a.file, a.key], [b.file, b.key]));
}
/** Format one difference naming the file, kind, old and new count, and the required action. */
function describe(section, { file, key, was, now }) {
  const action = now > was ? "boundary growth: remove the edge" : "stale baseline: lower it with --update";
  const kind = kindOf(key), label = key.startsWith(`${kind}:`) ? key : `${kind} ${key}`;
  return `${section} ${file}: ${label}: baseline ${was}, found ${now}; ${action}`;
}
/** Sum edge counts per kind over inventory rows. */
export function totals(list) {
  const result = Object.fromEntries(Object.keys(shapes).map((kind) => [kind, 0]));
  for (const [, key, count] of list) result[kindOf(key)] += count;
  return result;
}

/** Read scoped Rust sources per section, classifying every file and rejecting unclassified files and symbolic links. */
async function scopedSources(root) {
  const sources = { core: {}, registry: {} };
  /** Traverse one directory in sorted order, classifying files and rejecting symbolic links. */
  async function visit(path) {
    if ((await lstat(path)).isSymbolicLink()) throw Error(`unsupported symbolic link in boundary scope: ${path}`);
    for (const entry of (await readdir(path, { withFileTypes: true })).sort((a, b) => cmp(a.name, b.name))) {
      // Hidden non-Rust files (`.DS_Store`, editor swap files) and hidden links (Emacs lock files such as `.#a.rs`) are never modules; hidden `.rs` files and directories are scanned.
      if (entry.name.startsWith(".") && (entry.isSymbolicLink() || (entry.isFile() && !entry.name.endsWith(".rs")))) continue;
      const child = join(path, entry.name), name = relative(root, child).replaceAll("\\", "/");
      // Adapter, test and asset paths are skipped before any link or type check, so their contents may be anything.
      let kind;
      try { kind = classify(entry.isDirectory() ? `${name}/` : name); } catch (error) {
        // A link or special file is not a file type problem: it takes the directory class so the link check reports it.
        if (entry.isFile()) throw error;
        kind = classify(`${name}/`);
      }
      if (!sections.includes(kind) && !(entry.isDirectory() && kind === "core")) continue;
      if (entry.isSymbolicLink()) throw Error(`unsupported symbolic link in boundary scope: ${child}`);
      if (entry.isDirectory()) await visit(child);
      else if (!entry.isFile()) throw Error(`${name}: unclassified file in boundary scope`);
      else sources[kind][name] = await readFile(child, "utf8");
    }
  }
  await visit(join(root, "src"));
  return sources;
}
/** Reject a baseline with an invalid schema, section, row, edge, count or ordering. */
export function validateManifest(manifest) {
  if (manifest?.schemaVersion !== 2 || JSON.stringify(Object.keys(manifest).sort()) !== JSON.stringify(["core", "registry", "schemaVersion"])) throw Error("invalid boundary manifest schema");
  for (const section of sections) {
    if (!Array.isArray(manifest[section])) throw Error(`invalid boundary manifest section ${section}`);
    manifest[section].forEach((row, index) => {
      const [file, key, count] = Array.isArray(row) && row.length === 3 ? row : [];
      // A row for a file that moved to an adapter path is not malformed: it fails as stale until `--update` drops it.
      if (typeof file !== "string" || typeof key !== "string" || !/^src\/.+\.rs$/.test(file) || !kindOf(key) || !Number.isSafeInteger(count) || count <= 0) throw Error(`invalid manifest row ${section}: ${JSON.stringify(row)}`);
      if (index && cmpRow(manifest[section][index - 1], row) >= 0) throw Error(`manifest rows must be sorted and unique: ${section}: ${JSON.stringify(row)}`);
    });
  }
}
/** Serialize a baseline with one row per line so diffs show single edge changes. */
export function render(manifest) {
  /** Render one section as an array with a row per line. */
  const block = (list) => (list.length ? `[\n${list.map((row) => `    ${JSON.stringify(row)}`).join(",\n")}\n  ]` : "[]");
  return `{\n  "schemaVersion": 2,\n  "core": ${block(manifest.core)},\n  "registry": ${block(manifest.registry)}\n}\n`;
}
/** Compare the scanned tree with the baseline, or rewrite the baseline under `--update`. */
async function main() {
  const usage = "Usage: check-language-boundary.mjs [--root PATH] [--manifest PATH] [--update [--allow-growth]]";
  const args = process.argv.slice(2);
  let root = resolve(dirname(fileURLToPath(import.meta.url)), "../.."), manifestPath, update = false, growth = false;
  for (let i = 0; i < args.length; i++) {
    if (args[i] === "--update") update = true;
    else if (args[i] === "--allow-growth") growth = true;
    else if (["--root", "--manifest"].includes(args[i]) && args[i + 1] && !args[i + 1].startsWith("--")) { if (args[i++] === "--root") root = resolve(args[i]); else manifestPath = resolve(args[i]); }
    else throw Error(usage);
  }
  if (growth && !update) throw Error(usage);
  manifestPath ??= join(root, "scripts/check/language-boundary.json");
  let manifest = { core: [], registry: [] }, unreadable, current;
  try {
    manifest = JSON.parse(current = await readFile(manifestPath, "utf8"));
    validateManifest(manifest);
  } catch (error) {
    // --update may regenerate an unreadable baseline (merge conflict), but then every edge counts as growth.
    if (!update) throw error;
    manifest = { core: [], registry: [] }; unreadable = error.message;
  }
  const sources = await scopedSources(root);
  const actual = Object.fromEntries(sections.map((section) => [section, rows(inventorySources(sources[section]))]));
  const differences = sections.flatMap((section) => diffRows(actual[section], manifest[section]).map((item) => ({ ...item, text: describe(section, item) })));
  const blocked = differences.filter((item) => update ? item.now > item.was && !growth : true);
  if (update && !blocked.length) {
    const next = render({ core: actual.core, registry: actual.registry });
    if (next !== current) await writeFile(manifestPath, next);
  } else if (blocked.length) {
    const grew = blocked.some((item) => item.now > item.was);
    const hint = update ? `${unreadable ? `Baseline unreadable (${unreadable}). ` : ""}Growth is not recorded by --update; pass --allow-growth only for a reviewed increase or to regenerate a missing or unreadable baseline.`
      : `${grew ? "Remove the added edges, or record a reviewed increase with `--update --allow-growth`. " : ""}${blocked.some((item) => item.now < item.was) ? "After removing edges, run `node scripts/check/check-language-boundary.mjs --update` and commit the lowered baseline." : ""}`.trim();
    throw Error(`${blocked.map((item) => item.text).join("\n")}\n${hint}`);
  }
  for (const section of sections) {
    const parts = Object.entries(totals(actual[section])).map(([kind, count]) => `${kind} ${count}`).join(", ");
    console.log(`Language boundary ${section}: ${new Set(actual[section].map(([file]) => file)).size} files with edges; ${parts}.`);
  }
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error) => { console.error(`Language boundary: ${error.message}`); process.exitCode = 1; });
}
