#!/usr/bin/env node
// Documentation-parity gate.
//
// ## Why
//
// The recurring second class of review finding is a user-facing surface — a CLI flag or a config
// key — that the code gained but the docs did not, or described inconsistently. PR #58's
// `--fail-on-unparseable` help contradicted the code; PR #52 shipped a flag consequence the docs
// omitted. The accuracy half (does the prose match the behaviour?) is a human read. This gate owns
// the mechanical half: **every CLI `--long` flag and every config-file key is mentioned somewhere in
// `docs/`.** A new flag or key that nobody documented fails the gate before a reviewer ever sees it.
//
// The source of truth is the code, not a hand-maintained list: flags come from clap's `#[arg(long)]`
// on `CheckOptions`, keys from the `RawConfig` deserialize struct (serde `rename_all = "camelCase"`),
// including frontend sections flattened into it.
// The parsing lives in ./lib/docs-parity.mjs and is unit-tested there.

import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { camel, cliFlags, configFields, configSurface, findConfigStruct, flagDocumented, structBody } from "./lib/docs-parity.mjs";

const root = resolve(process.argv[2] || ".");
const cliSource = readFileSync(join(root, "src/cli.rs"), "utf8");
// Frontend config sections are flattened into `RawConfig` from their own modules, so every source
// file is searched for the structs the walk reaches.
const rustSources = readdirSync(join(root, "src"), { recursive: true })
  .filter((name) => String(name).endsWith(".rs"))
  .map((name) => readFileSync(join(root, "src", String(name)), "utf8"));

// Surfaces intentionally not documented per flag/key. Each entry needs a reason: the gate ratchets
// forward — a new flag or key must be documented or explicitly justified here, never silently
// skipped. Keep this list short; prefer documenting the surface.
const UNDOCUMENTED_ALLOWED = new Map([
  // ["--some-flag", "reason it is intentionally undocumented"],
]);

// Recursive: rule and discovery pages live in `docs/rules/` and `docs/discovery/`. The option
// exists since Node 20.1; an older runtime would ignore it and scan the top level only, so fail
// loudly instead of passing on a partial scan.
const [nodeMajor, nodeMinor] = process.versions.node.split(".").map(Number);
assert.ok(nodeMajor > 20 || (nodeMajor === 20 && nodeMinor >= 1), `Node 20.1 or newer is required for a recursive docs scan; found ${process.versions.node}`);
const docs = readdirSync(join(root, "docs"), { recursive: true })
  .filter((name) => String(name).endsWith(".md"))
  .map((name) => readFileSync(join(root, "docs", String(name)), "utf8"))
  .join("\n");

const flags = cliFlags(structBody(cliSource, "CheckOptions"));
// `AdapterKeys` routes each claimed key to its adapter, so the walk stops there and reads each
// adapter's config struct instead.
// [file implementing `config_keys`, file holding the config struct, struct, key list it returns]
const ADAPTER_CONFIG_STRUCTS = [["src/ruby.rs", "src/ruby/config.rs", "RawRubyConfig", "CONFIG_KEYS"]];
/** A config struct body by name; `AdapterKeys` contributes no fields of its own. */
const findStruct = (name) => (name === "AdapterKeys" ? "" : findConfigStruct(rustSources, name));
const keys = [
  ...configSurface(findStruct, "RawConfig"),
  ...ADAPTER_CONFIG_STRUCTS.flatMap(([, , name]) => configSurface(findStruct, name)),
];
// Floors guard the parser itself: if a refactor makes the struct scan silently return fewer
// surfaces, that must fail loudly rather than pass by checking nothing.
assert.ok(flags.length >= 22, `expected to parse the CheckOptions flags, found only ${flags.length}`);
assert.ok(keys.length >= 18, `expected to parse the RawConfig keys, found only ${keys.length}`);

// Adapters may not claim a core key, so the core's list must be exactly RawConfig's own fields.
const configSource = readFileSync(join(root, "src/config.rs"), "utf8");
const coreList = configSource.match(/CORE_CONFIG_KEYS: \[&str; \d+\] = \[([^\]]*)\]/);
assert.ok(coreList, "CORE_CONFIG_KEYS not found in src/config.rs");
const coreKeys = [...coreList[1].matchAll(/"([^"]+)"/g)].map((match) => match[1]);
// `Config`'s serialized fields: the ones without `#[serde(skip)]` and not flattened.
const printedKeys = [];
let skipped = false;
for (const line of structBody(configSource, "Config").split("\n").map((line) => line.trim())) {
  if (line.startsWith("#[")) {
    skipped ||= /^#\[serde\((skip|flatten)\b/.test(line);
    continue;
  }
  const field = line.match(/^(?:pub\s+)?([a-z_][a-z0-9_]*)\s*:/);
  if (!field) continue;
  if (!skipped) printedKeys.push(camel(field[1]));
  skipped = false;
}
assert.deepEqual(coreKeys, printedKeys, "CORE_CONFIG_KEYS must list Config's serialized fields in order");
const readKeys = configFields(findConfigStruct(rustSources, "RawConfig"))
  .filter((field) => !field.flatten)
  .map((field) => field.key);
assert.deepEqual(readKeys.filter((key) => !coreKeys.includes(key)), [], "every RawConfig field must be in CORE_CONFIG_KEYS");
// Every adapter implementing `config_keys` is listed above, so none escapes these checks, however
// it spells its key list. The trait and its test adapters live in `source_adapter*` and test files.
const declaring = readdirSync(join(root, "src"), { recursive: true })
  .map((name) => String(name).replace(/\\/g, "/"))
  .filter((name) => name.endsWith(".rs") && !/^source_adapter(\.rs|\/)|(^|\/)tests?(\.rs|\/)/.test(name))
  .filter((name) => /\bfn config_keys\(&self\)/.test(readFileSync(join(root, "src", name), "utf8")))
  .map((name) => `src/${name}`)
  .sort();
assert.deepEqual(declaring, ADAPTER_CONFIG_STRUCTS.map(([impl]) => impl).sort(), "list each adapter implementing config_keys");
// An adapter's claimed keys are its config struct's top-level fields, or registration would let
// another adapter claim one the struct reads.
for (const [impl, file, name, list] of ADAPTER_CONFIG_STRUCTS) {
  const returned = new RegExp(`fn config_keys\\(&self\\)[^{]*\\{\\s*&(?:\\w+::)*${list}\\b`);
  assert.ok(returned.test(readFileSync(join(root, impl), "utf8")), `${impl} config_keys must return ${list}`);
  const source = readFileSync(join(root, file), "utf8");
  const declared = source.match(new RegExp(`${list}: \\[&str; \\d+\\] = \\[([^\\]]*)\\]`));
  assert.ok(declared, `${list} not found in ${file}`);
  const claimed = [...declared[1].matchAll(/"([^"]+)"/g)].map((match) => match[1]);
  const fields = configFields(findConfigStruct(rustSources, name)).map((field) => field.key);
  assert.deepEqual(claimed, fields, `${file} ${list} must list ${name}'s fields in order`);
}

const missing = [];
for (const flag of flags) {
  // Match the whole flag token, so `--exclude` is not satisfied by `--exclude-defaults`.
  if (flagDocumented(docs, flag) || UNDOCUMENTED_ALLOWED.has(flag)) continue;
  missing.push(`CLI flag ${flag}`);
}
// A config key is matched as a quoted or code-spanned JSON key so a coincidental prose word does not
// satisfy it. A section key (`ruby.loadPaths`) needs its dotted path in a code span.
for (const key of keys) {
  if (!key.includes(".") && docs.includes(`"${key}"`)) continue;
  if (docs.includes(`\`${key}\``)) continue;
  if (UNDOCUMENTED_ALLOWED.has(key)) continue;
  missing.push(`config key ${key}`);
}

if (missing.length > 0) {
  console.error(
    `Documentation parity: ${missing.length} user-facing surface(s) are undocumented in docs/.\n` +
      missing.map((m) => `  - ${m}`).join("\n") +
      `\n\nDocument each in docs/, or justify it in UNDOCUMENTED_ALLOWED.`,
  );
  process.exit(1);
}

console.log(`Documentation parity: ${flags.length} CLI flags and ${keys.length} config keys documented.`);
