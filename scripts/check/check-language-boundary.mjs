// Lexical architecture ratchet: file-wide aliases are conservative, not Rust name resolution.
// Dedicated test files are excluded; inline tests remain part of the inventory.
import { lstat, readFile, readdir } from "node:fs/promises";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const modules = ["analysis", "cli", "discovery", "config", "source_filter", "model"];
const exclusions = {
  "src/lib.rs": "composition root", "src/main.rs": "composition root",
  "src/source_adapter.rs": "adapter routing/session composition",
  "src/source_adapter/": "adapter routing tests/support",
  "src/typescript.rs": "language frontend", "src/typescript/": "language frontend",
  "src/ruby.rs": "language frontend", "src/ruby/": "language frontend",
};
const identifier = /^[A-Za-z_][A-Za-z_0-9]*$/;
function tokens(source) {
  const result = [];
  result.rawIdentifiers = new Set();
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
      i = end + 1 + raw[1].length; continue;
    }
    if (source[i] === '"') {
      i++;
      while (i < source.length && source[i] !== '"') i += source[i] === "\\" ? 2 : 1;
      if (i >= source.length) throw Error("unterminated string");
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
function edge(path) {
  // Relative paths are possible adapter references even inside inline modules.
  // Count conservatively; proving a local same-named module is outside this lexical guard.
  let relative = 0;
  while (["self", "super"].includes(path[relative])) relative++;
  if (relative) {
    path = path.slice(relative);
    path = path[0] === "Framework" ? ["crate", "model", ...path] : ["crate", ...path];
  }
  if (path[0] !== "crate") return null;
  if (["ruby", "typescript"].includes(path[1])) return path.slice(0, 2).join("::");
  if (path[1] === "model" && path[2] === "Framework") return path.slice(0, 4).join("::");
  return null;
}
function imports(input) {
  let position = 0;
  const leaves = [];
  function branch(prefix) {
    let path = [...prefix];
    if (input[position] === "::") position++;
    while (identifier.test(input[position] ?? "") || input[position] === "*") {
      const part = input[position++];
      if (part === "as") throw Error("invalid import alias");
      if (part !== "self" || !path.length) path.push(part);
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
    if (input[position] === "as") {
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

export function inventorySources(sources) {
  const inventory = {};
  for (const [file, source] of Object.entries(sources).sort(([a], [b]) => a.localeCompare(b))) {
    try {
      const input = tokens(source), aliases = new Map(), leaves = [], counts = {}, ignored = new Set();
      const add = (path) => { const key = edge(path); if (key) counts[key] = (counts[key] ?? 0) + 1; };
      for (let i = 0; i < input.length; i++) {
        if (!isImport(input, i)) continue;
        const end = input.indexOf(";", i);
        if (end < 0) throw Error("unterminated use declaration");
        // All import syntax is parsed: forward aliases must not hide unsupported targeted forms.
        const parsed = imports(input.slice(i + 1, end));
        for (const leaf of parsed) {
          leaves.push(leaf);
          const origins = aliases.get(leaf.alias) ?? [];
          origins.push(leaf.path); aliases.set(leaf.alias, origins);
        }
        for (let n = i; n <= end; n++) ignored.add(n);
        i = end;
      }
      function canonical(path, visited = new Set()) {
        if (path[0] === "crate") return path;
        if (aliases.has(path[0])) {
          if (visited.has(path[0])) throw Error(`cyclic import alias ${path[0]}`);
          visited.add(path[0]);
          const choices = aliases.get(path[0]).map((origin) => canonical([...origin, ...path.slice(1)], new Set(visited)));
          const unique = new Map(choices.map((choice) => [choice.join("::"), choice]));
          if (unique.size === 1) return choices[0];
          if (choices.some((choice) => edge(choice) || choice.join("::") === "crate" || choice.join("::") === "crate::model")) throw Error(`ambiguous targeted alias ${path[0]}`);
          return path;
        }
        // Framework may arrive through a parent module or an inline-test glob.
        return path[0] === "Framework" ? ["crate", "model", ...path] : path;
      }
      for (const { path } of leaves) {
        const resolved = canonical(path);
        if (resolved.includes("*") && (edge(resolved) || resolved.join("::") === "crate::*" || resolved.join("::") === "crate::model::*")) throw Error("unsupported targeted wildcard import");
        add(resolved);
      }
      for (let i = 0; i < input.length; i++) {
        if (ignored.has(i) || !identifier.test(input[i]) || input[i - 1] === "::") continue;
        const path = [input[i]];
        while (input[i + 1] === "::" && identifier.test(input[i + 2] ?? "")) { path.push(input[i + 2]); i += 2; }
        add(canonical(path));
      }
      inventory[file] = Object.fromEntries(Object.entries(counts).sort(([a], [b]) => a.localeCompare(b)));
    } catch (error) { throw Error(`${file}: ${error.message}`); }
  }
  return inventory;
}

export function compareInventory(actual, baseline) {
  const differences = [];
  for (const file of [...new Set([...Object.keys(actual), ...Object.keys(baseline)])].sort()) {
    if (!(file in actual)) { differences.push(`${file}: recorded file missing; reconcile inventory`); continue; }
    if (!(file in baseline)) { differences.push(`${file}: new scoped file; record inventory`); continue; }
    for (const key of [...new Set([...Object.keys(actual[file]), ...Object.keys(baseline[file])])].sort()) {
      const found = actual[file][key] ?? 0, expected = baseline[file][key] ?? 0;
      if (found !== expected) differences.push(`${file}: ${key}: ${found} != ${expected}; ${found > expected ? "boundary growth" : "tighten inventory after removal"}`);
    }
  }
  return differences;
}

async function scopedSources(root) {
  const sources = {};
  async function visit(path, optional = false) {
    let entries;
    try {
      if ((await lstat(path)).isSymbolicLink()) throw Error(`unsupported symbolic link in boundary scope: ${path}`);
      entries = await readdir(path, { withFileTypes: true });
    }
    catch (error) { if (optional && error.code === "ENOENT") return; throw error; }
    for (const entry of entries.sort((a, b) => a.name.localeCompare(b.name))) {
      if (entry.name === "tests" || entry.name === "tests.rs") continue;
      const child = join(path, entry.name);
      if (entry.isSymbolicLink()) throw Error(`unsupported symbolic link in boundary scope: ${child}`);
      if (entry.isDirectory()) await visit(child);
      else if (entry.isFile() && entry.name.endsWith(".rs")) sources[relative(root, child).replaceAll("\\", "/")] = await readFile(child, "utf8");
    }
  }
  for (const module of modules) {
    const path = `src/${module}.rs`;
    if ((await lstat(join(root, path))).isSymbolicLink()) throw Error(`unsupported symbolic link in boundary scope: ${path}`);
    sources[path] = await readFile(join(root, path), "utf8");
    await visit(join(root, "src", module), true);
  }
  return sources;
}
function validateManifest(manifest) {
  if (manifest?.schemaVersion !== 1 || !manifest.files || typeof manifest.files !== "object" || Array.isArray(manifest.files)) throw Error("invalid boundary manifest schema");
  if (JSON.stringify(manifest.exclusions) !== JSON.stringify(exclusions)) throw Error("invalid boundary manifest exclusions");
  for (const [file, counts] of Object.entries(manifest.files)) {
    if (!file.startsWith("src/") || !counts || typeof counts !== "object" || Array.isArray(counts)) throw Error(`invalid manifest file ${file}`);
    for (const [key, count] of Object.entries(counts)) {
      if (edge(key.split("::")) !== key || !Number.isSafeInteger(count) || count <= 0) throw Error(`invalid manifest edge ${file}: ${key}`);
    }
  }
}
async function main() {
  const args = process.argv.slice(2);
  let root = resolve(dirname(fileURLToPath(import.meta.url)), "../.."), manifestPath;
  for (let i = 0; i < args.length; i += 2) {
    if (!["--root", "--manifest"].includes(args[i]) || !args[i + 1]) throw Error("Usage: check-language-boundary.mjs [--root PATH] [--manifest PATH]");
    if (args[i] === "--root") root = resolve(args[i + 1]); else manifestPath = resolve(args[i + 1]);
  }
  const manifest = JSON.parse(await readFile(manifestPath ?? join(root, "scripts/check/language-boundary.json"), "utf8"));
  validateManifest(manifest);
  const actual = inventorySources(await scopedSources(root));
  const differences = compareInventory(actual, manifest.files);
  if (differences.length) throw Error(differences.join("\n"));
  console.log(`Language boundary: ${Object.keys(actual).length} files match the reviewed inventory.`);
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  main().catch((error) => { console.error(`Language boundary: ${error.message}`); process.exitCode = 1; });
}
