import assert from "node:assert/strict";
import {
  cp,
  mkdir,
  mkdtemp,
  readFile,
  rm,
} from "node:fs/promises";
import { readdirSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import {
  countMatches,
  expectedTotal as sumExpected,
  findingOwners,
  locationLabel,
} from "./lib/recall-oracle.mjs";

// The corpus check runs whatever binary is on disk. A binary older than the sources silently
// analyzes with stale behavior, so a passing or failing run reflects code that is no longer
// checked out — a repeatedly confusing failure mode locally. Warn (never fail: CI always builds
// fresh, and the warning is for the local edit-then-check loop) when the binary predates any
// source or is missing.
function newestSource() {
  let newest = { mtimeMs: 0, path: null };
  const consider = (path) => {
    const { mtimeMs } = statSync(path);
    if (mtimeMs > newest.mtimeMs) newest = { mtimeMs, path };
  };
  const walk = (directory) => {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const full = join(directory, entry.name);
      if (entry.isDirectory()) walk(full);
      else consider(full);
    }
  };
  walk(resolve("src"));
  consider(resolve("Cargo.toml"));
  // A dependency-only bump touches the lockfile without touching `src` or the manifest, and can
  // change analyzer behavior through a rebuilt dependency, so it counts toward staleness too.
  consider(resolve("Cargo.lock"));
  return newest;
}

function warnIfBinaryStale(binaryPath) {
  let binaryMtimeMs;
  try {
    binaryMtimeMs = statSync(binaryPath).mtimeMs;
  } catch {
    process.stderr.write(
      `\n⚠️  ${binaryPath} is missing — run \`cargo build --release\` before the corpus check.\n\n`,
    );
    return;
  }
  const newest = newestSource();
  if (newest.mtimeMs > binaryMtimeMs) {
    process.stderr.write(
      `\n⚠️  ${binaryPath} is older than ${newest.path} — it may analyze with stale behavior.\n` +
        `    Run \`cargo build --release\` before the corpus check.\n\n`,
    );
  }
}

const binary = resolve(process.argv[2] || "target/release/cuke-dedup");
warnIfBinaryStale(binary);
const corpus = resolve(process.argv[3] || "fixtures/corpus");
const manifest = JSON.parse(await readFile(join(corpus, "manifest.json"), "utf8"));
assert.equal(manifest.schemaVersion, 1);
const recallRoot = resolve("fixtures/recall");
const recallManifest = JSON.parse(
  await readFile(join(recallRoot, "manifest.json"), "utf8"),
);
assert.equal(recallManifest.schemaVersion, 1);

// Narrows a run to one recall case, so a deliberate break can be attributed to the case meant to
// catch it. The aggregate run stops at the first failure and cannot show that.
const onlyCase = process.env.CUKE_DEDUP_CORPUS_CASE;

const temporary = await mkdtemp(join(tmpdir(), "cuke-dedup-corpus-"));
try {
  for (const testCase of onlyCase ? [] : manifest.cases) {
    const caseRoot = join(temporary, testCase.name, "case");
    const output = join(temporary, testCase.name, "report");
    await cp(join(corpus, testCase.path), caseRoot, { recursive: true });
    for (const materialized of testCase.materialize || []) {
      const destination = join(caseRoot, materialized.destination);
      await mkdir(dirname(destination), { recursive: true });
      await cp(join(caseRoot, materialized.source), destination);
    }
    const run = spawnSync(
      binary,
      [".", ...testCase.arguments, "--reporters", "json", "--output", output],
      { cwd: caseRoot, encoding: "utf8" },
    );
    assert.equal(run.status, testCase.expectedExit, `${testCase.name}: ${run.stderr}`);
    const report = JSON.parse(await readFile(join(output, "cuke-dedup.json"), "utf8"));
    const expected = testCase.expected;
    assert.equal(report.summary.definitionsAnalyzed, expected.definitions, `${testCase.name}: definitions`);
    assert.equal(report.summary.featureStepsAnalyzed, expected.featureSteps, `${testCase.name}: feature steps`);
    assert.equal(report.summary.duplication.duplicatedDefinitions, expected.duplicatedDefinitions, `${testCase.name}: duplicated definitions`);
    assert.equal(report.summary.duplication.percentage, expected.percentage, `${testCase.name}: percentage`);
    assert.equal(report.summary.duplication.threshold, expected.threshold, `${testCase.name}: threshold`);
    assert.equal(report.summary.duplication.passed, expected.passed, `${testCase.name}: result`);
    assert.deepEqual(report.summary.byRule, expected.byRule, `${testCase.name}: rules`);
    for (const message of testCase.stderrIncludes || []) {
      assert.ok(run.stderr.includes(message), `${testCase.name}: missing stderr message ${message}`);
    }
  }

  const recall = await validateRecallCorpus(temporary);

  const parityRoot = join(corpus, "threshold-group");
  const parityOutput = join(temporary, "reporter-parity");
  const parity = spawnSync(
    binary,
    [".", "--reporters", "terminal,json,html,sarif", "--output", parityOutput],
    { cwd: parityRoot, encoding: "utf8" },
  );
  assert.equal(parity.status, 0, parity.stderr);
  const json = JSON.parse(await readFile(join(parityOutput, "cuke-dedup.json"), "utf8"));
  const html = await readFile(join(parityOutput, "cuke-dedup.html"), "utf8");
  const sarif = JSON.parse(await readFile(join(parityOutput, "cuke-dedup.sarif"), "utf8"));
  const jsonlRun = spawnSync(
    binary,
    [".", "--reporters", "jsonl"],
    { cwd: parityRoot, encoding: "utf8" },
  );
  assert.equal(jsonlRun.status, 0, jsonlRun.stderr);
  const jsonl = jsonlRun.stdout.trimEnd().split("\n").map((line) => JSON.parse(line));
  const jsonlFindings = jsonl.filter((record) => record.type === "finding");
  const jsonlSummary = jsonl.at(-1);
  const embedded = html.match(/<script type="application\/json" id="report-data">([\s\S]*?)<\/script>/);
  assert.ok(embedded, "HTML report is missing embedded report data");
  assert.deepEqual(JSON.parse(embedded[1]), json, "HTML and JSON report data differ");

  const active = json.findings.filter((finding) => finding.suppression === null);
  assert.equal(json.summary.findings, active.length);
  assert.equal((parity.stdout.match(/^  \[(?:error|warning)\]/gm) || []).length, active.length);
  assert.equal((html.match(/<article class="finding"/g) || []).length, active.length);
  assert.equal(sarif.runs[0].results.length, active.length);
  assert.equal(jsonlFindings.length, json.findings.length);
  assert.equal(jsonlSummary.type, "summary");
  assert.equal(jsonlSummary.schemaVersion, "3");
  assert.equal(typeof jsonlSummary.toolVersion, "string");
  assert.equal(jsonlSummary.recordCount, jsonl.length);
  assert.equal(jsonlSummary.truncated, false);
  assert.deepEqual(jsonlSummary.summary, json.summary);
  assert.ok(jsonlSummary.metrics.filesDiscovered > 0);
  for (const record of jsonlFindings) {
    assert.equal(record.schemaVersion, "3");
    assert.equal(typeof record.toolVersion, "string");
    assert.match(record.fingerprint, /^[0-9a-f]{32}$/);
    assert.equal(typeof record.active, "boolean");
    assert.equal(typeof record.contributesToThreshold, "boolean");
    assert.equal(typeof record.primary.path, "string");
    assert.equal(typeof record.primary.line, "number");
    assert.equal(typeof record.evidence, "object");
    assert.ok(Array.isArray(record.truncatedFields));
  }
  for (const finding of active) {
    assert.match(parity.stdout, new RegExp(escapeRegex(finding.rule)));
    assert.match(parity.stdout, new RegExp(escapeRegex(finding.message)));
    assert.match(parity.stdout, new RegExp(escapeRegex(finding.primary.path)));
    assert.ok(
      sarif.runs[0].results.some((result) =>
        result.ruleId === finding.rule
        && result.message.text === finding.message
        && result.locations[0].physicalLocation.artifactLocation.uri === finding.primary.path),
      `SARIF lost ${finding.rule} at ${finding.primary.path}`,
    );
    assert.ok(
      jsonlFindings.some((record) =>
        record.active
        && record.rule === finding.rule
        && record.message === finding.message
        && record.primary.path === finding.primary.path),
      `JSONL lost ${finding.rule} at ${finding.primary.path}`,
    );
  }
  console.log(
    `Validated ${manifest.cases.length} behavior cases, ${recall.cases} recall cases, and ${active.length} findings across terminal, JSON, JSONL, HTML, and SARIF.`,
  );
  console.log(
    `Recall baseline: ${recall.detected}/${recall.desired} desired findings (${(recall.ratio * 100).toFixed(1)}%); ${recall.knownMisses} explicit known misses.`,
  );
} finally {
  await rm(temporary, { recursive: true, force: true });
}

/**
 * Runs every recall case and returns the corpus-wide recall figures.
 *
 * Recall is deliberately a ratio of *desired* findings rather than a pass/fail count: a case may
 * assert a finding the analyzer cannot yet produce, recorded as a known miss, and the ratio is what
 * keeps those visible instead of letting them read as absence.
 *
 * @param {string} temporary Scratch directory each case is copied into before it runs.
 * @returns {Promise<{cases: number, detected: number, desired: number, ratio: number,
 *   knownMisses: number}>} Per-run totals; `ratio` is `detected / desired`.
 */
async function validateRecallCorpus(temporary) {
  const knownIds = new Set();
  let detected = 0;
  let knownMisses = 0;

  // The recall baseline is only asserted over a full run, since a subset cannot meet a
  // corpus-wide ratio.
  const only = onlyCase;
  const cases = only
    ? recallManifest.cases.filter((testCase) => testCase.name === only)
    : recallManifest.cases;
  assert.ok(cases.length > 0, `no recall case named ${only}`);

  // Counting cases protects the count, not the coverage. Without these two, a case carrying real
  // expectations could be deleted and replaced by a vacuous one — or by a copy of an easier case
  // under a new name — leaving `minimumCases` satisfied and the recall ratio unharmed.
  const names = cases.map((testCase) => testCase.name);
  const repeated = names.filter((name, index) => names.indexOf(name) !== index);
  assert.deepEqual(repeated, [], `duplicate recall case name(s): ${repeated.join(", ")}`);
  for (const testCase of cases) {
    // `findingMatches` compares `rule` first, so an expectation without one can never match any
    // finding: as an absence it is satisfied vacuously, and as a positive it can only fail. Either
    // way it asserts nothing, which counting entries alone would not notice — and a malformed
    // expectation is far likelier as an authoring slip than as an attempt to game the floor.
    const asserted = [
      ...(testCase.expectedFindings || []),
      ...(testCase.expectedAbsent || []),
      ...(testCase.knownMisses || []),
    ];
    for (const expectation of asserted) {
      assert.ok(
        expectation.rule,
        `${testCase.name}: an expectation names no rule, so it can never match a finding: ${JSON.stringify(expectation)}`,
      );
    }
    assert.ok(
      asserted.length > 0,
      `${testCase.name}: asserts no finding, non-finding or known miss, so it occupies a corpus slot without pinning anything`,
    );
  }

  for (const testCase of cases) {
    const caseRoot = join(temporary, "recall", testCase.name, "case");
    const output = join(temporary, "recall", testCase.name, "report");
    await cp(join(recallRoot, testCase.path), caseRoot, { recursive: true });
    const run = spawnSync(
      binary,
      [".", ...(testCase.arguments || []), "--reporters", "json", "--output", output],
      { cwd: caseRoot, encoding: "utf8" },
    );
    assert.equal(run.status, testCase.expectedExit || 0, `${testCase.name}: ${run.stderr}`);
    const report = JSON.parse(await readFile(join(output, "cuke-dedup.json"), "utf8"));
    assert.equal(
      report.summary.definitionsAnalyzed,
      testCase.expectedDefinitions,
      `${testCase.name}: definitions`,
    );
    for (const [source, expected] of Object.entries(testCase.expectedCandidateSources || {})) {
      assert.equal(
        report.analysis.candidateSources[source]?.evaluated,
        expected,
        `${testCase.name}: ${source} candidate count`,
      );
    }

    const activeFindings = report.findings.filter((finding) => finding.suppression === null);
    const expectedFindings = testCase.expectedFindings || [];
    // `count` lets one expectation stand for several findings sharing every asserted field. It
    // defaults to 1 so existing entries keep their exact meaning.
    const expectedTotal = sumExpected(expectedFindings);
    for (const expected of expectedFindings) {
      assertFindingCount(
        activeFindings,
        expected,
        expected.count ?? 1,
        `${testCase.name}: expected finding`,
      );
    }
    // Counting each expectation independently is not enough: one finding can satisfy a broad and a
    // narrow expectation at once, leaving room for a second unclassified finding while the totals
    // still balance. Requiring a one-to-one mapping closes that, and `count` still lets a single
    // expectation own several findings.
    for (const finding of activeFindings) {
      const owners = findingOwners(finding, expectedFindings);
      assert.equal(
        owners.length,
        1,
        `${testCase.name}: each finding needs exactly one expectation, ${
          owners.length === 0 ? "none" : owners.length
        } matched ${JSON.stringify({
          rule: finding.rule,
          primary: locationLabel(finding.primary),
          related: finding.related.map(locationLabel),
        })}`,
      );
    }

    for (const expected of testCase.expectedAbsent || []) {
      assertFindingCount(activeFindings, expected, 0, `${testCase.name}: deliberate non-finding`);
    }
    for (const miss of testCase.knownMisses || []) {
      assert.ok(miss.reason, `${testCase.name}: known miss ${miss.id} needs a reason`);
      assert.ok(!knownIds.has(miss.id), `duplicate known-miss id ${miss.id}`);
      knownIds.add(miss.id);
      assertFindingCount(
        activeFindings,
        miss,
        0,
        `${testCase.name}: known miss ${miss.id} was fixed; move it to expectedFindings`,
      );
      knownMisses += 1;
    }
    assert.equal(
      activeFindings.length,
      expectedTotal,
      `${testCase.name}: unclassified active findings`,
    );
    detected += expectedTotal;
  }

  const desired = detected + knownMisses;
  const ratio = desired === 0 ? 1 : detected / desired;
  if (!only) {
    // Recall is a ratio, so deleting an unsatisfied case raises it. Exact equality — matching how
    // `knownMissBaseline` below is asserted — makes the corpus ratchet in one direction only. A
    // `>=` floor would let the corpus grow to 60 and then shed all seven additions while staying
    // green, so additions would never become durable. Raising the floor is one line, and it is the
    // line that records the gain.
    assert.equal(
      cases.length,
      recallManifest.minimumCases,
      `recall corpus has ${cases.length} cases against a floor of ${recallManifest.minimumCases}; `
        + "raise minimumCases when adding a case, and never lower it to drop one",
    );
    assert.equal(
      knownMisses,
      recallManifest.knownMissBaseline,
      "known-miss baseline changed; fixes must move entries to expectedFindings and reduce the baseline",
    );
    assert.ok(
      ratio >= recallManifest.minimumRecall,
      `recall ${(ratio * 100).toFixed(1)}% is below ${(recallManifest.minimumRecall * 100).toFixed(1)}%`,
    );
  }
  return { cases: cases.length, detected, desired, ratio, knownMisses };
}

/**
 * Asserts that exactly `count` findings match an expectation.
 *
 * The same helper serves positives, deliberate non-findings (`count` 0) and known misses, so all
 * three are held to one matching rule and cannot drift apart.
 *
 * @param {object[]} findings Active findings from one case.
 * @param {object} expected Expectation to match against, as written in the manifest.
 * @param {number} count Exact number of findings that must match.
 * @param {string} context Message prefix identifying the case and the kind of expectation.
 */
function assertFindingCount(findings, expected, count, context) {
  assert.equal(countMatches(findings, expected), count, `${context}: ${JSON.stringify(expected)}`);
}

/**
 * Escapes regex metacharacters so a literal string can be embedded in a pattern.
 *
 * @param {string} value Literal text.
 * @returns {string} Text safe to interpolate into a `RegExp`.
 */
function escapeRegex(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
