// Output equivalence against a released baseline.
//
// Refactors and performance work must not change what the analyzer reports. This check builds the
// analyzer at a baseline Git ref and runs the existing corpus, Ruby, language-parity and benchmark
// checkers with a shim in place of the binary. Every invocation runs both binaries on the same
// input and compares exit status, stdout, stderr and every written report, after removing timing
// metrics and the version string. Any difference fails the check, and so does a suite that made no
// invocation: a checker that silently stopped calling the binary would otherwise pass vacuously.
//
// The checkers still assert their own oracles against the candidate. A checker failure is reported
// separately from equivalence, so a run can show "behavior unchanged, oracle still failing" apart
// from "behavior changed".
//
// Usage: node scripts/check/check-output-equivalence.mjs [--ref REF] [--suites a,b] [CANDIDATE]
import assert from "node:assert/strict";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { summarize } from "./lib/output-equivalence.mjs";

const SUITES = {
  corpus: ["node", ["scripts/check/check-corpus.mjs", "{shim}", "fixtures/corpus"]],
  "corpus-window": ["node", ["scripts/check/check-corpus.mjs", "{shim}", "fixtures/corpus"], { CUKE_DEDUP_MATCHER_WINDOW: "1" }],
  ruby: ["node", ["scripts/check/check-ruby-parity.mjs", "--regression", "{shim}"]],
  language: ["node", ["scripts/check/check-language-parity.mjs", "--regression", "{shim}", "{scratch}/language-parity.json"]],
  benchmark: ["node", ["scripts/benchmark/benchmark-analysis.mjs", "{shim}"], { CUKE_DEDUP_BENCH_REPEATS: "1", CUKE_DEDUP_BENCH_TIMEOUT_MS: "1800000" }],
};

const args = process.argv.slice(2);
let ref = null;
let selected = Object.keys(SUITES);
let candidate = "target/release/cuke-dedup";
for (let index = 0; index < args.length; index += 1) {
  if (args[index] === "--ref") ref = args[++index];
  else if (args[index] === "--suites") selected = args[++index].split(",");
  else candidate = args[index];
}
for (const suite of selected) assert.ok(SUITES[suite], `unknown suite ${suite}; known: ${Object.keys(SUITES).join(", ")}`);
candidate = resolve(candidate);
// The previous release is the behavior every change is held to, so default to the newest version
// tag reachable from HEAD rather than a pinned one that goes stale after each release.
if (ref === null) {
  const described = spawnSync("git", ["describe", "--tags", "--abbrev=0", "--match", "v[0-9]*", "HEAD"], { encoding: "utf8" });
  assert.equal(described.status, 0,
    "no release tag is reachable from HEAD; pass --ref REF, or fetch tags with `git fetch --tags --unshallow`");
  ref = described.stdout.trim();
}
assert.ok(existsSync(candidate), `${candidate} is missing; run \`cargo build --release --locked\``);

/** Output of a command that must succeed. */
function run(command, commandArgs, options = {}) {
  const result = spawnSync(command, commandArgs, { encoding: "utf8", maxBuffer: 64 * 1024 * 1024, ...options });
  assert.equal(result.status, 0, `${command} ${commandArgs.join(" ")}: ${result.error?.message ?? result.stderr}`);
  return result.stdout;
}

/** Release binary built from `revision`, cached under `target/equivalence` by commit. */
function baselineBinary(revision) {
  const commit = run("git", ["rev-parse", "--verify", `${revision}^{commit}`]).trim();
  const root = resolve("target/equivalence");
  const binary = join(root, commit, "cuke-dedup");
  if (existsSync(binary)) return binary;
  const source = join(root, commit, "src");
  rmSync(source, { recursive: true, force: true });
  mkdirSync(source, { recursive: true });
  const archive = spawnSync("git", ["archive", commit], { maxBuffer: 1024 * 1024 * 1024 });
  assert.equal(archive.status, 0, `git archive ${commit}: ${archive.stderr}`);
  run("tar", ["-x", "-C", source], { input: archive.stdout });
  process.stderr.write(`building baseline ${revision} (${commit.slice(0, 12)})\n`);
  run("cargo", ["build", "--release", "--locked", "--manifest-path", join(source, "Cargo.toml"),
    "--target-dir", join(root, "target")], { stdio: ["ignore", "inherit", "inherit"] });
  writeFileSync(binary, readFileSync(join(root, "target", "release", "cuke-dedup")));
  chmodSync(binary, 0o755);
  rmSync(source, { recursive: true, force: true });
  return binary;
}

const baseline = baselineBinary(ref);
/** Version number printed by `binary --version`. */
const version = (binary) => run(binary, ["--version"]).trim().split(/\s+/).pop();
const versions = [[version(candidate), version(baseline)]].filter(([from, to]) => from !== to);
const scratch = mkdtempSync(join(tmpdir(), "cuke-output-equivalence-"));
try {
  const shim = join(scratch, "cuke-dedup");
  writeFileSync(shim, `#!/bin/sh\nexec node ${JSON.stringify(resolve("scripts/check/lib/equivalence-shim.mjs"))} "$@"\n`);
  chmodSync(shim, 0o755);
  const log = join(scratch, "equivalence.jsonl");
  writeFileSync(log, "");
  const checkerFailures = [];
  for (const suite of selected) {
    const [command, template, extraEnv = {}] = SUITES[suite];
    const commandArgs = template.map((arg) => arg.replace("{shim}", shim).replace("{scratch}", scratch));
    process.stderr.write(`\n== ${suite}\n`);
    const result = spawnSync(command, commandArgs, {
      stdio: ["ignore", "ignore", "inherit"],
      env: {
        ...process.env,
        ...extraEnv,
        CUKE_DEDUP_EQUIVALENCE_BASELINE: baseline,
        CUKE_DEDUP_EQUIVALENCE_CANDIDATE: candidate,
        CUKE_DEDUP_EQUIVALENCE_LOG: log,
        CUKE_DEDUP_EQUIVALENCE_SUITE: suite,
        CUKE_DEDUP_EQUIVALENCE_VERSIONS: JSON.stringify(versions),
      },
    });
    if (result.status !== 0) checkerFailures.push(`${suite} (exit ${result.status ?? result.signal})`);
  }
  const records = readFileSync(log, "utf8").split("\n").filter(Boolean).map((line) => JSON.parse(line));
  const verdict = summarize(records, selected);
  console.log(`\nbaseline ${ref}; candidate ${candidate}`);
  for (const [suite, count] of Object.entries(verdict.invocations)) console.log(`${suite}: ${count} invocations`);
  for (const record of verdict.mismatches.slice(0, 20)) {
    console.log(`\nMISMATCH [${record.suite}] ${record.cwd} ${record.args.join(" ")}`);
    for (const difference of record.differences) console.log(`  ${JSON.stringify(difference)}`);
  }
  if (verdict.mismatches.length > 20) console.log(`\n... ${verdict.mismatches.length - 20} more mismatches`);
  for (const suite of verdict.empty) console.log(`EMPTY ${suite}: the checker made no analyzer invocation`);
  for (const failure of checkerFailures) console.log(`CHECKER FAILED ${failure}`);
  console.log(verdict.ok
    ? `\noutput equivalent: ${records.length} invocations, 0 mismatches`
    : `\noutput differs: ${verdict.mismatches.length} of ${records.length} invocations`);
  process.exitCode = verdict.ok && checkerFailures.length === 0 ? 0 : 1;
} finally {
  rmSync(scratch, { recursive: true, force: true });
}
