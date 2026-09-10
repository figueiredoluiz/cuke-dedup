import assert from "node:assert/strict";
import {
  cp,
  mkdir,
  mkdtemp,
  readFile,
  rm,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";

const binary = resolve(process.argv[2] || "target/release/cuke-dedup");
const corpus = resolve(process.argv[3] || "fixtures/corpus");
const manifest = JSON.parse(await readFile(join(corpus, "manifest.json"), "utf8"));
assert.equal(manifest.schemaVersion, 1);
const recallRoot = resolve("fixtures/recall");
const recallManifest = JSON.parse(
  await readFile(join(recallRoot, "manifest.json"), "utf8"),
);
assert.equal(recallManifest.schemaVersion, 1);

const temporary = await mkdtemp(join(tmpdir(), "cuke-dedup-corpus-"));
try {
  for (const testCase of manifest.cases) {
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
  assert.equal(jsonlSummary.schemaVersion, "1");
  assert.equal(typeof jsonlSummary.toolVersion, "string");
  assert.equal(jsonlSummary.recordCount, jsonl.length);
  assert.equal(jsonlSummary.truncated, false);
  assert.deepEqual(jsonlSummary.summary, json.summary);
  assert.ok(jsonlSummary.metrics.filesDiscovered > 0);
  for (const record of jsonlFindings) {
    assert.equal(record.schemaVersion, "1");
    assert.equal(typeof record.toolVersion, "string");
    assert.match(record.fingerprint, /^[0-9a-f]{16}$/);
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

async function validateRecallCorpus(temporary) {
  const knownIds = new Set();
  let detected = 0;
  let knownMisses = 0;

  for (const testCase of recallManifest.cases) {
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

    const activeFindings = report.findings.filter((finding) => finding.suppression === null);
    const expectedFindings = testCase.expectedFindings || [];
    for (const expected of expectedFindings) {
      assertFindingCount(activeFindings, expected, 1, `${testCase.name}: expected finding`);
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
      expectedFindings.length,
      `${testCase.name}: unclassified active findings`,
    );
    detected += expectedFindings.length;
  }

  assert.equal(
    knownMisses,
    recallManifest.knownMissBaseline,
    "known-miss baseline changed; fixes must move entries to expectedFindings and reduce the baseline",
  );
  const desired = detected + knownMisses;
  const ratio = desired === 0 ? 1 : detected / desired;
  assert.ok(
    ratio >= recallManifest.minimumRecall,
    `recall ${(ratio * 100).toFixed(1)}% is below ${(recallManifest.minimumRecall * 100).toFixed(1)}%`,
  );
  return { cases: recallManifest.cases.length, detected, desired, ratio, knownMisses };
}

function assertFindingCount(findings, expected, count, context) {
  const matches = findings.filter((finding) => findingMatches(finding, expected));
  assert.equal(matches.length, count, `${context}: ${JSON.stringify(expected)}`);
}

function findingMatches(finding, expected) {
  if (finding.rule !== expected.rule) return false;
  if (expected.primaryPath && finding.primary.path !== expected.primaryPath) return false;
  if (expected.primaryLine && finding.primary.line !== expected.primaryLine) return false;
  if (expected.matchers) {
    const comparison = finding.evidence.comparison;
    if (!comparison) return false;
    const actual = [comparison.leftMatcher, comparison.rightMatcher].sort();
    const wanted = [...expected.matchers].sort();
    return actual.length === wanted.length
      && actual.every((matcher, index) => matcher === wanted[index]);
  }
  return true;
}

function escapeRegex(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}
