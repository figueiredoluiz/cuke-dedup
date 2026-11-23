// Run with a built CukeDedup binary and the Cucumber-Ruby 10.2.0 gem.
// Pass --controls to validate the oracle on versions that still require explicit Ruby selection.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, relative, resolve, sep } from "node:path";

const [, , binaryPath, mode] = process.argv;
assert.ok(binaryPath && (mode === undefined || mode === "--controls"),
  "usage: node scripts/check/probe-ruby-default-discovery.mjs <binary> [--controls]");
const binary = resolve(binaryPath);
const cucumberVersion = "10.2.0";
const steps = "features/step_definitions/booking.rb";
const support = "features/support/gate.rb";
const outside = "sandbox/example.rb";
const files = {
  "features/booking.feature": "Feature: Booking\n  Scenario: Arrival\n    Given a booking is active\n",
  [steps]: "Given('a booking is active') { check_booking }\n",
  [support]: "Given('the cargo gate is locked') { check_gate }\n",
  [outside]: "Given('a sandbox example') { try_example }\n",
};

/** Runs a tool in the owned fixture with pinned Git exclusions and no inherited tool options. */
function run(command, args, project) {
  const { PATH, SystemRoot, GEM_HOME, GEM_PATH, TMPDIR, TEMP, TMP } = process.env;
  const env = { PATH, SystemRoot, GEM_HOME, GEM_PATH, TMPDIR, TEMP, TMP,
    GIT_CONFIG_NOSYSTEM: "1", GIT_CONFIG_GLOBAL: join(project, "empty-git-config"), XDG_CONFIG_HOME: project };
  const result = spawnSync(command, args, {
    cwd: project, env, encoding: "utf8", timeout: 30_000, maxBuffer: 1024 * 1024,
  });
  if (result.error) throw new Error(`could not run ${command}`, { cause: result.error });
  assert.equal(result.status, 0, `${command} failed (${result.signal ?? result.status}):\n${result.stderr}`);
  return result;
}

/** Converts fixture-relative paths from the observed tool to slash-separated paths. */
function fixturePath(project, path) {
  return relative(project, resolve(project, path)).split(sep).join("/");
}

/** Parses the complete newline-delimited record stream. */
function jsonLines(text) {
  return text.trim().split(/\r?\n/).map((line) => JSON.parse(line));
}

/** Reads Cucumber's actual file-load log and registration messages. */
async function observeCucumber(project) {
  const launcher = `load Gem.bin_path("cucumber", "cucumber", "${cucumberVersion}")`;
  const { stdout } = run("ruby", ["-e", launcher, "--", "--dry-run", "--verbose", "--publish-quiet",
    "--format", "message", "--out", "cucumber.ndjson"], project);
  const loaded = stdout.match(/^Code:\r?\n((?:  \* [^\r\n]+\r?\n)+)/m);
  assert.ok(loaded, "Cucumber did not emit its code load list");
  const selected = loaded[1].trimEnd().split(/\r?\n/).map((line) => fixturePath(project, line.slice(4))).sort();
  const registrations = jsonLines(await readFile(join(project, "cucumber.ndjson"), "utf8"))
    .filter((row) => row.stepDefinition)
    .map(({ stepDefinition: { sourceReference, pattern } }) =>
      `${fixturePath(project, sourceReference.uri)}:${sourceReference.location.line} ${pattern.source}`).sort();
  assert.deepEqual(selected, [steps, support], "Cucumber selected different fixture files");
  assert.deepEqual(registrations, [`${steps}:1 a booking is active`, `${support}:1 the cargo gate is locked`],
    "Cucumber registered different fixture steps");
  return { selected, registrations };
}

/** Observes the analyzer with reporting flags that do not supply a file selection. */
async function observeAnalyzer(project, selection) {
  const { stdout, stderr } = run(binary, [...selection, "--reporters", "jsonl", "--no-metrics",
    "--explain-discovery", "--emit-fingerprints", "fingerprints.jsonl"], project);
  const selected = Array.from(stderr.matchAll(/^cuke-dedup: definition ([^\r\n]+)$/gm),
    ([, path]) => fixturePath(project, path)).sort();
  const registrations = (await readFile(join(project, "fingerprints.jsonl"), "utf8")).trim()
    .split(/\r?\n/).filter(Boolean).map((line) => JSON.parse(line))
    .map(({ path, line, matcher }) => `${fixturePath(project, path)}:${line} ${matcher}`).sort();
  const rows = jsonLines(stdout);
  const summary = rows.at(-1);
  assert.equal(summary.type, "summary", "expected final analyzer summary");
  assert.equal(summary.summary.featureStepsAnalyzed, 1, "expected the booking feature step");
  assert.equal(summary.corpus.incomplete, false);
  assert.equal(summary.analysis.truncated, false);
  assert.equal(summary.findingsTruncated, 0);
  const findings = rows.filter((row) => row.type === "finding").map(({ rule, active, primary }) =>
    [rule, active, fixturePath(project, primary.path), primary.line]);
  return { selected, registrations, findings };
}

/** Checks selection and outcomes against the independently observed suite. */
function assertParity(actual, cucumber) {
  assert.deepEqual(actual.selected, cucumber.selected, "Ruby file selection parity");
  assert.deepEqual(actual.registrations, cucumber.registrations, "registration parity");
  assert.deepEqual(actual.findings, [["unused-definition", true, support, 1]], "exact analyzer findings");
}

const project = await mkdtemp(join(tmpdir(), "cuke-ruby-default-"));
try {
  for (const [name, source] of Object.entries(files)) {
    const path = join(project, name);
    await mkdir(dirname(path), { recursive: true });
    await writeFile(path, source);
  }
  const cucumber = await observeCucumber(project);
  const explicit = await observeAnalyzer(project, ["--definitions", cucumber.selected.join(",")]);
  assertParity(explicit, cucumber);

  const omitted = await observeAnalyzer(project, ["--definitions", steps]);
  const admitted = await observeAnalyzer(project, ["--definitions", [steps, support, outside].join(",")]);
  assert.deepEqual(omitted.selected, [steps], "support file omission must reach the analyzer");
  assert.deepEqual(omitted.findings, [], "omitting support drops the unused-definition finding");
  assert.deepEqual(admitted.selected, [...cucumber.selected, outside], "unrelated file admission must reach the analyzer");
  assert.deepEqual(admitted.findings, [
    ["unused-definition", true, support, 1],
    ["unused-definition", true, outside, 1],
  ], "admitting unrelated Ruby adds an unused-definition finding");
  for (const wrongSelection of [omitted, admitted]) {
    assert.throws(() => assertParity(wrongSelection, cucumber), {
      code: "ERR_ASSERTION", message: /^Ruby file selection parity/,
    });
  }
  console.log("Cucumber parity with explicit selection and both opposite selection controls passed");

  if (mode !== "--controls") {
    const terminal = run(binary, [], project);
    const defaults = await observeAnalyzer(project, []);
    assertParity(defaults, cucumber);
    assert.match(terminal.stdout, /Analyzed 2 definitions and 1 feature step/);
    assert.match(terminal.stdout, /^unused-definition$/m);
    assert.match(terminal.stdout, /features[\\/]support[\\/]gate\.rb:1:/);
    console.log("Zero-argument Ruby discovery and exact findings passed");
  }
} finally {
  await rm(project, { recursive: true, force: true });
}
