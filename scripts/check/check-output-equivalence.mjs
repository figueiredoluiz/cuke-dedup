// Output equivalence against a released baseline.
//
// Refactors and performance work must not change what the analyzer reports. This check builds the
// analyzer at a baseline Git ref and runs the existing corpus, Ruby, language-parity and benchmark
// checkers, and the `tests/cli.rs` integration tests, with a shim in place of the binary. Every
// invocation runs both binaries on the same input and compares exit status, stdout, stderr and every
// written report, after removing timing metrics and the version string. Any difference fails the
// check, and so does a suite that made no invocation: a checker that silently stopped calling the
// binary would otherwise pass vacuously.
//
// The checkers still assert their own oracles against the candidate. A checker failure is reported
// separately from equivalence, so a run can show "behavior unchanged, oracle still failing" apart
// from "behavior changed".
//
// Usage: node scripts/check/check-output-equivalence.mjs [--ref REF] [--suites a,b] [CANDIDATE]
import assert from "node:assert/strict";
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { readRecords, summarize } from "./lib/output-equivalence.mjs";

const SUITES = {
  corpus: ["node", ["scripts/check/check-corpus.mjs", "{shim}", "fixtures/corpus"]],
  "corpus-window": ["node", ["scripts/check/check-corpus.mjs", "{shim}", "fixtures/corpus"], { CUKE_DEDUP_MATCHER_WINDOW: "1" }],
  ruby: ["node", ["scripts/check/check-ruby-parity.mjs", "--regression", "{shim}"]],
  language: ["node", ["scripts/check/check-language-parity.mjs", "--regression", "{shim}", "{scratch}/language-parity.json"]],
  benchmark: ["node", ["scripts/benchmark/benchmark-analysis.mjs", "{shim}"], { CUKE_DEDUP_BENCH_REPEATS: "1", CUKE_DEDUP_BENCH_TIMEOUT_MS: "1800000" }],
  // assert_cmd's `Command::cargo_bin` reads this variable at run time, so the tests' own invocations
  // go through the shim while their assertions see the candidate's output.
  cli: [cliTestBinary, [], { "CARGO_BIN_EXE_cuke-dedup": "{shim}" }],
};

// The shim is a POSIX shell script and the binary path has no `.exe`; fail clearly rather than
// partway through a suite.
assert.notEqual(process.platform, "win32", "check:equivalence runs on macOS and Linux only");

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
// The previous release is the behavior every change is held to, so default to the newest release
// reachable from HEAD rather than a pinned one that goes stale after each release.
if (ref === null) {
  // Highest version among tags reachable from HEAD; `git describe` would pick the nearest instead.
  const tags = spawnSync("git", ["tag", "--merged", "HEAD", "--list", "v[0-9]*", "--sort=-v:refname"], { encoding: "utf8" });
  ref = tags.status === 0 ? tags.stdout.split("\n")[0].trim() : "";
  assert.ok(ref, "no release tag is reachable from HEAD; pass --ref REF, or fetch tags with `git fetch --tags --unshallow`");
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
  // Source and Cargo output are private to this process, so concurrent runs, for the same or
  // different refs, never delete or copy each other's build.
  const build = join(root, commit, `build-${process.pid}`);
  const source = join(build, "src");
  const target = join(build, "target");
  try {
    mkdirSync(source, { recursive: true });
    const archive = spawnSync("git", ["archive", commit], { maxBuffer: 1024 * 1024 * 1024 });
    assert.equal(archive.status, 0, `git archive ${commit}: ${archive.stderr}`);
    run("tar", ["-x", "-C", source], { input: archive.stdout });
    process.stderr.write(`building baseline ${revision} (${commit.slice(0, 12)})\n`);
    run("cargo", ["build", "--release", "--locked", "--manifest-path", join(source, "Cargo.toml"),
      "--target-dir", target], { stdio: ["ignore", "inherit", "inherit"] });
    // Stage under a private name and rename into place, so a reader never sees a partial binary.
    const staged = join(build, "cuke-dedup");
    copyFileSync(join(target, "release", "cuke-dedup"), staged);
    chmodSync(staged, 0o755);
    renameSync(staged, binary);
  } finally {
    rmSync(build, { recursive: true, force: true });
  }
  return binary;
}

/** The `tests/cli.rs` test executable of the checked-out tree, built by Cargo when stale. */
function cliTestBinary() {
  const messages = run("cargo", ["test", "--locked", "--no-run", "--test", "cli", "--message-format", "json-render-diagnostics"],
    { stdio: ["ignore", "pipe", "inherit"] });
  const executables = messages.split("\n").filter(Boolean).map((line) => JSON.parse(line))
    .filter((message) => message.reason === "compiler-artifact" && message.target.name === "cli" && message.executable)
    .map((message) => message.executable);
  assert.equal(executables.length, 1, `expected one cli test executable, found ${executables.length}`);
  return executables[0];
}

const baseline = baselineBinary(ref);
/** Version number printed by `binary --version`. */
const version = (binary) => run(binary, ["--version"]).trim().split(/\s+/).pop();
const versions = [[version(candidate), version(baseline)]].filter(([from, to]) => from !== to);
const scratch = mkdtempSync(join(tmpdir(), "cuke-output-equivalence-"));
try {
  const shim = join(scratch, "cuke-dedup");
  // POSIX single quotes: nothing inside them expands, whatever the checkout path contains.
  const shimScript = `'${resolve("scripts/check/lib/equivalence-shim.mjs").replaceAll("'", "'\\''")}'`;
  writeFileSync(shim, `#!/bin/sh\nexec node ${shimScript} "$@"\n`);
  chmodSync(shim, 0o755);
  // One record file per shim invocation: the cli suite's parallel tests run many shims at once.
  const log = join(scratch, "equivalence-records");
  mkdirSync(log);
  const checkerFailures = [];
  for (const suite of selected) {
    const [command, template, extraEnv = {}] = SUITES[suite];
    const commandArgs = template.map((arg) => arg.replace("{shim}", shim).replace("{scratch}", scratch));
    process.stderr.write(`\n== ${suite}\n`);
    const result = spawnSync(typeof command === "function" ? command() : command, commandArgs, {
      stdio: ["ignore", "pipe", "inherit"],
      encoding: "utf8",
      maxBuffer: 64 * 1024 * 1024,
      env: {
        ...process.env,
        ...Object.fromEntries(Object.entries(extraEnv).map(([name, value]) => [name, value.replace("{shim}", shim)])),
        CUKE_DEDUP_EQUIVALENCE_BASELINE: baseline,
        CUKE_DEDUP_EQUIVALENCE_CANDIDATE: candidate,
        CUKE_DEDUP_EQUIVALENCE_LOG: log,
        CUKE_DEDUP_EQUIVALENCE_SUITE: suite,
        CUKE_DEDUP_EQUIVALENCE_VERSIONS: JSON.stringify(versions),
      },
    });
    if (result.status !== 0) {
      // Test harnesses report failures on stdout; keep its tail so a checker failure is diagnosable.
      process.stderr.write(`${(result.stdout ?? "").split("\n").slice(-40).join("\n")}\n`);
      checkerFailures.push(`${suite} (exit ${result.status ?? result.signal})`);
    }
  }
  const records = readRecords(log);
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
