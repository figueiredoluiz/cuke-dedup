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
// on `CheckOptions`, keys from the `RawConfig` deserialize struct (serde `rename_all = "camelCase"`).
// The parsing lives in ./lib/docs-parity.mjs and is unit-tested there.

import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { cliFlags, configKeys, flagDocumented, structBody } from "./lib/docs-parity.mjs";

const root = resolve(process.argv[2] || ".");
const cliSource = readFileSync(join(root, "src/cli.rs"), "utf8");
const configSource = readFileSync(join(root, "src/config.rs"), "utf8");

// Surfaces intentionally not documented per flag/key. Each entry needs a reason: the gate ratchets
// forward — a new flag or key must be documented or explicitly justified here, never silently
// skipped. Keep this list short; prefer documenting the surface.
const UNDOCUMENTED_ALLOWED = new Map([
  // ["--some-flag", "reason it is intentionally undocumented"],
]);

const docs = readdirSync(join(root, "docs"))
  .filter((name) => name.endsWith(".md"))
  .map((name) => readFileSync(join(root, "docs", name), "utf8"))
  .join("\n");

const flags = cliFlags(structBody(cliSource, "CheckOptions"));
const keys = configKeys(structBody(configSource, "RawConfig"));
// Floors guard the parser itself: if a refactor makes the struct scan silently return fewer
// surfaces, that must fail loudly rather than pass by checking nothing.
assert.ok(flags.length >= 22, `expected to parse the CheckOptions flags, found only ${flags.length}`);
assert.ok(keys.length >= 18, `expected to parse the RawConfig keys, found only ${keys.length}`);

const missing = [];
for (const flag of flags) {
  // Match the whole flag token, so `--exclude` is not satisfied by `--exclude-defaults`.
  if (flagDocumented(docs, flag) || UNDOCUMENTED_ALLOWED.has(flag)) continue;
  missing.push(`CLI flag ${flag}`);
}
// A config key is matched as a quoted or code-spanned JSON key so a coincidental prose word does not
// satisfy it.
for (const key of keys) {
  if (docs.includes(`"${key}"`) || docs.includes(`\`${key}\``)) continue;
  if (UNDOCUMENTED_ALLOWED.has(key)) continue;
  missing.push(`config key ${key}`);
}

if (missing.length > 0) {
  console.error(
    `Documentation parity: ${missing.length} user-facing surface(s) are undocumented in docs/.\n` +
      missing.map((m) => `  - ${m}`).join("\n") +
      `\n\nDocument each in docs/ (and CHANGELOG.md if new), or justify it in UNDOCUMENTED_ALLOWED.`,
  );
  process.exit(1);
}

console.log(`Documentation parity: ${flags.length} CLI flags and ${keys.length} config keys documented.`);
