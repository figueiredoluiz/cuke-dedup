// The behavior corpus runner, split like the recall runner: observing a case copies it, runs the
// binary and reads its outputs without judging; judging asserts every expectation against the
// observation. The corpus check runs both in order, and the evaluation report observes the same way.

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { cp, mkdir } from "node:fs/promises";
import { dirname, join } from "node:path";
import { parseRecords } from "./negative-isolation.mjs";
import { summaryComparisons } from "./behavior-summary.mjs";
import { readOptional } from "./read-optional.mjs";
import { latestClassifiedAt, validateAgainstRecords, validateCorpusLabels } from "./manifest-schema.mjs";

/**
 * Parses a run's report, naming the case and the run's exit status and stderr when it is not JSON.
 * @param {string} text Report text.
 * @param {string} name Case name.
 * @param {{status: number|null, stderr: string}} run The finished spawn.
 * @returns {object}
 */
function parseReport(text, name, run) {
  try {
    return JSON.parse(text);
  } catch (error) {
    throw new Error(`${name}: report is not valid JSON (${error.message}); exit ${run.status}: ${run.stderr}`);
  }
}

/**
 * Runs one behavior case through the binary and returns what it produced. Nothing is asserted: a
 * failed run still returns its status and whatever outputs exist. A report that exists but is not
 * valid JSON throws, since the run is then not observable.
 * @param {object} options
 * @param {string} options.binary Path to the analyzer binary.
 * @param {string} options.corpusRoot Directory holding the behavior case directories.
 * @param {string} options.scratch Directory the case's copy and outputs are written under.
 * @param {object} options.testCase Behavior manifest case.
 * @returns {Promise<{status: number|null, stderr: string, spawnFailure: string|null,
 *   report: object|null, fingerprintsText: string|null}>} Exit status, stderr, why the run did not
 *   reach an exit status (else `null`), JSON report and raw `--emit-fingerprints` text; the last two
 *   are `null` when the run wrote none.
 */
export async function observeBehaviorCase({ binary, corpusRoot, scratch, testCase }) {
  const caseRoot = join(scratch, "case");
  const output = join(scratch, "report");
  const fingerprints = join(scratch, "fingerprints.jsonl");
  await cp(join(corpusRoot, testCase.path), caseRoot, { recursive: true });
  for (const materialized of testCase.materialize || []) {
    const destination = join(caseRoot, materialized.destination);
    await mkdir(dirname(destination), { recursive: true });
    await cp(join(caseRoot, materialized.source), destination);
  }
  const run = spawnSync(
    binary,
    [".", ...testCase.arguments, "--reporters", "json", "--output", output, "--emit-fingerprints", fingerprints],
    { cwd: caseRoot, encoding: "utf8" },
  );
  const reportText = await readOptional(join(output, "cuke-dedup.json"));
  return {
    status: run.status,
    stderr: run.stderr,
    spawnFailure: run.error?.message ?? (run.signal ? `killed by ${run.signal}` : null),
    report: reportText === null ? null : parseReport(reportText, testCase.name, run),
    fingerprintsText: await readOptional(fingerprints),
  };
}

/**
 * Asserts a behavior case's observation against its expectations: exit, census, duplication
 * summary, per-rule counts, stderr messages and the labels the run can contradict.
 * @param {object} testCase Behavior manifest case.
 * @param {object} observation Result of {@link observeBehaviorCase}.
 */
export function judgeBehaviorCase(testCase, { status, stderr, spawnFailure, report, fingerprintsText }) {
  assert.equal(status, testCase.expectedExit, `${testCase.name}: ${spawnFailure ?? stderr}`);
  assert.equal(spawnFailure, null, `${testCase.name}: process error: ${spawnFailure}`);
  assert.ok(report, `${testCase.name}: the run wrote no JSON report (exit ${status}): ${stderr}`);
  assert.ok(fingerprintsText !== null, `${testCase.name}: the run wrote no fingerprint records (exit ${status}): ${stderr}`);
  const expected = testCase.expected;
  for (const { label, actual, expected: wanted } of summaryComparisons(testCase, report)) {
    assert.equal(actual, wanted, `${testCase.name}: ${label}`);
  }
  assert.deepEqual(report.summary.byRule, expected.byRule, `${testCase.name}: rules`);
  for (const message of testCase.stderrIncludes || []) {
    assert.ok(stderr.includes(message), `${testCase.name}: missing stderr message ${message}`);
  }
  // The labels the run can contradict are held against its fingerprint records, as for recall cases.
  validateCorpusLabels(testCase, testCase.name, latestClassifiedAt(), Object.keys(expected.byRule).length);
  validateAgainstRecords(
    { name: testCase.name, language: testCase.language, frameworks: testCase.frameworks, expectedDefinitions: expected.definitions },
    parseRecords(fingerprintsText),
  );
}
