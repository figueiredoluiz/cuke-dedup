// The recall corpus runner, split so a report can reuse what the corpus gate does. Observing a case
// (copying it, running the binary, reading its outputs) touches the filesystem and judges nothing;
// judging it (every assertion the gate makes) reads only the observation. The corpus check runs both
// in order. A report mode can observe the same way and classify the expectations instead of asserting.

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { cp } from "node:fs/promises";
import { join } from "node:path";
import { checkIsolation, isolatable, parseRecords } from "./negative-isolation.mjs";
import { readOptional } from "./read-optional.mjs";
import {
  exitAcceptable,
  partitionAmbiguous,
  validateAgainstRecords,
  validateCaseMetadata,
} from "./manifest-schema.mjs";
import {
  countMatches,
  expectedTotal as sumExpected,
  findingOwners,
  locationLabel,
} from "./recall-oracle.mjs";

/**
 * Asserts the case is well formed before it is run: its metadata is valid and every expectation
 * names a rule and the case pins something.
 * @param {object} testCase Recall manifest case.
 * @param {string} today `YYYY-MM-DD`, the latest allowed `classifiedAt`.
 */
export function assertCaseWellFormed(testCase, today) {
  // `findingMatches` compares `rule` first, so an expectation without one can never match any
  // finding: as an absence it is satisfied vacuously, and as a positive it can only fail. Either
  // way it asserts nothing, which counting entries alone would not notice — and a malformed
  // expectation is far likelier as an authoring slip than as an attempt to game the floor.
  validateCaseMetadata(testCase, today);
  const asserted = [
    ...(testCase.expectedFindings || []),
    ...(testCase.expectedAbsent || []),
    ...(testCase.knownMisses || []),
    ...(testCase.expectedAmbiguous || []),
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

/**
 * Runs one recall case through the binary and returns what it produced. Nothing is asserted: a
 * failed run still returns its status and whatever outputs exist. A report that exists but is not
 * valid JSON throws, since the run is then not observable.
 * @param {object} options
 * @param {string} options.binary Path to the analyzer binary.
 * @param {string} options.recallRoot Directory holding the recall case directories.
 * @param {string} options.scratch Directory the case's copy and outputs are written under.
 * @param {object} options.testCase Recall manifest case.
 * @param {boolean} [options.emitPairs] Also write `--emit-pairs` records; the gate does not.
 * @returns {Promise<{status: number|null, stderr: string, spawnFailure: string|null,
 *   report: object|null, fingerprintsText: string|null, pairsText: string|null}>} The run's exit
 *   status, stderr, why it did not run to an exit status (a spawn error or a signal, else `null`),
 *   JSON report, raw `--emit-fingerprints` text and raw `--emit-pairs` text (`null` unless
 *   requested); the report and the texts are `null` when the run wrote none.
 */
export async function observeRecallCase({ binary, recallRoot, scratch, testCase, emitPairs = false }) {
  const caseRoot = join(scratch, "case");
  const output = join(scratch, "report");
  const fingerprints = join(scratch, "fingerprints.jsonl");
  const pairs = join(scratch, "pairs.jsonl");
  await cp(join(recallRoot, testCase.path), caseRoot, { recursive: true });
  const run = spawnSync(
    binary,
    [".", ...(testCase.arguments || []), "--reporters", "json", "--output", output, "--emit-fingerprints", fingerprints, ...(emitPairs ? ["--emit-pairs", pairs] : [])],
    { cwd: caseRoot, encoding: "utf8" },
  );
  const reportText = await readOptional(join(output, "cuke-dedup.json"));
  return {
    status: run.status,
    stderr: run.stderr,
    spawnFailure: run.error?.message ?? (run.signal ? `killed by ${run.signal}` : null),
    report: reportText === null ? null : JSON.parse(reportText),
    fingerprintsText: await readOptional(fingerprints),
    pairsText: emitPairs ? await readOptional(pairs) : null,
  };
}

/**
 * Asserts a case's observation against its expectations and adds its counts to `totals`.
 * @param {object} testCase Recall manifest case.
 * @param {object} observation Result of {@link observeRecallCase}.
 * @param {object} context
 * @param {{detected: number, knownMisses: number, unisolated: number}} context.totals Corpus-wide
 *   counts, updated as each expectation is judged.
 * @param {Set<string>} context.knownIds Known-miss ids seen so far, for the uniqueness check.
 * @param {(label: string, check: () => Promise<void>) => Promise<void>} context.attemptCheck Runs a
 *   check that must not stop the case when it fails.
 */
export async function judgeRecallCase(testCase, observation, { totals, knownIds, attemptCheck }) {
  const { status, stderr, spawnFailure, report, fingerprintsText } = observation;
  assert.ok(
    exitAcceptable(status, testCase.expectedExit || 0, testCase),
    `${testCase.name}: exit ${status}, expected ${testCase.expectedExit || 0}: ${spawnFailure ?? stderr}`,
  );
  assert.ok(report, `${testCase.name}: the run wrote no JSON report`);
  assert.equal(
    report.summary.definitionsAnalyzed,
    testCase.expectedDefinitions,
    `${testCase.name}: definitions`,
  );
  assert.ok(Number.isSafeInteger(testCase.expectedFeatureSteps) && testCase.expectedFeatureSteps >= 0,
    `${testCase.name}: expectedFeatureSteps must declare a nonnegative integer`);
  assert.equal(report.summary.featureStepsAnalyzed, testCase.expectedFeatureSteps,
    `${testCase.name}: extracted feature steps`);
  for (const [source, expected] of Object.entries(testCase.expectedCandidateSources || {})) {
    assert.equal(
      report.analysis.candidateSources[source]?.evaluated,
      expected,
      `${testCase.name}: ${source} candidate count`,
    );
  }

  const activeFindings = report.findings.filter((finding) => finding.suppression === null);
  // Ambiguous findings are neither desired nor forbidden: they need no owner and stay out of the
  // recall ratio. Absences below still see them, so a finding cannot be both ambiguous and absent.
  const { owned: ownedFindings } = partitionAmbiguous(activeFindings, testCase.expectedAmbiguous || []);
  const expectedFindings = testCase.expectedFindings || [];
  // `count` lets one expectation stand for several findings sharing every asserted field. It
  // defaults to 1 so existing entries keep their exact meaning.
  const expectedTotal = sumExpected(expectedFindings);
  // Required findings are counted without the ambiguous ones, so an undecided pair of the same
  // rule cannot change a required count.
  for (const expected of expectedFindings) {
    assertFindingCount(
      ownedFindings,
      expected,
      expected.count ?? 1,
      `${testCase.name}: expected finding`,
    );
  }
  // Counting each expectation independently is not enough: one finding can satisfy a broad and a
  // narrow expectation at once, leaving room for a second unclassified finding while the totals
  // still balance. Requiring a one-to-one mapping closes that, and `count` still lets a single
  // expectation own several findings.
  for (const finding of ownedFindings) {
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

  const records = parseRecords(fingerprintsText ?? "");
  validateAgainstRecords(testCase, records);
  for (const expected of testCase.expectedAbsent || []) {
    assertFindingCount(activeFindings, expected, 0, `${testCase.name}: deliberate non-finding`);
    // `!== undefined`, so a null or false declaration is checked and rejected, not counted as absent.
    if (expected.isolation !== undefined) {
      await attemptCheck(
        `recall ${testCase.name}: isolation ${JSON.stringify(expected.matchers ?? expected.isolation.participants)}`,
        () => checkIsolation(records, expected, report.analysis),
      );
    } else if (isolatable(expected.rule)) {
      totals.unisolated += 1;
    }
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
    totals.knownMisses += 1;
  }
  assert.equal(
    ownedFindings.length,
    expectedTotal,
    `${testCase.name}: unclassified active findings`,
  );
  totals.detected += expectedTotal;
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
