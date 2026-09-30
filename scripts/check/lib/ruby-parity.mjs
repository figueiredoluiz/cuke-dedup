import assert from "node:assert/strict";
import { countMatches, findingOwners } from "./recall-oracle.mjs";

export function completionPassed(observations, blockers) {
  return observations.length > 0 && blockers.length === 0
    && observations.every((item) => item.deficits.length === 0);
}

// Counterpart coverage is independent of whether the analyzer implements the contract yet.
export function corpusCoverageDeficits(mappings, groups) {
  const gaps = [];
  for (const item of mappings) {
    const label = `${item.source}:${item.case}`;
    if (["language-inapplicable", "framework-inapplicable"].includes(item.requiredStatus)) {
      if (!item.rationale?.trim()) gaps.push(`${label}: exclusion needs a rationale`);
      continue;
    }
    if (item.requiredStatus !== "covered") gaps.push(`${label}: ${item.requiredStatus}`);
    if (!item.fixtureGroup || !groups[item.fixtureGroup]) gaps.push(`${label}: missing executable counterpart`);
  }
  if (!mappings.length) gaps.push("empty source corpus");
  return gaps;
}

// Desired outcomes are independent of observations. A missing positive, an unexpected finding,
// or lost completeness must remain a deficit even if extraction counts happen to match.
export function parityDeficits(oracle, report, exit) {
  const deficits = [];
  const check = (actual, expected, label) => {
    try {
      assert.deepEqual(actual, expected);
    } catch {
      deficits.push(`${label}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
    }
  };
  check(report.summary.definitionsAnalyzed, oracle.definitions, "definitions");
  check(report.summary.featureStepsAnalyzed, oracle.featureSteps, "feature steps");
  check(!report.corpus.incomplete, oracle.complete, "complete");
  check(exit, oracle.expectedExit, "strict exit");
  check(report.findingsTruncated, 0, "truncated findings");
  const active = report.findings.filter((finding) => finding.suppression === null);
  for (const expected of oracle.requiredFindings) {
    check(countMatches(active, expected), expected.count ?? 1, `required ${JSON.stringify(expected)}`);
  }
  for (const absent of oracle.requiredAbsent) {
    check(countMatches(active, absent), 0, `forbidden ${JSON.stringify(absent)}`);
  }
  for (const finding of active) {
    check(findingOwners(finding, oracle.requiredFindings).length, 1,
      `finding ownership ${finding.rule} at ${finding.primary.path}:${finding.primary.line}`);
  }
  for (const [key, value] of Object.entries(oracle.duplication ?? {})) {
    check(report.summary.duplication[key], value, `duplication.${key}`);
  }
  for (const [source, count] of Object.entries(oracle.expectedCandidateSources ?? {})) {
    check(report.analysis?.candidateSources?.[source]?.evaluated, count, `${source} candidates`);
  }
  return deficits;
}

export function validateOracle(oracle) {
  assert.equal(typeof oracle.path, "string");
  assert.ok(Number.isSafeInteger(oracle.definitions) && oracle.definitions >= 0);
  assert.ok(Number.isSafeInteger(oracle.featureSteps) && oracle.featureSteps >= 0);
  assert.equal(typeof oracle.complete, "boolean");
  assert.ok([0, 1, 2].includes(oracle.expectedExit), "strict exit must be explicit");
  for (const count of Object.values(oracle.expectedCandidateSources ?? {})) {
    assert.ok(Number.isSafeInteger(count) && count >= 0, "candidate counts must be nonnegative integers");
  }
  assert.ok(Array.isArray(oracle.requiredFindings));
  assert.ok(Array.isArray(oracle.requiredAbsent));
  for (const expectation of [...oracle.requiredFindings, ...oracle.requiredAbsent]) {
    assert.equal(typeof expectation.rule, "string", "expectations must name a rule");
    assert.ok(expectation.rule.length > 0);
    if (expectation.count !== undefined) {
      assert.ok(Number.isSafeInteger(expectation.count) && expectation.count > 0);
    }
  }
  for (const required of oracle.requiredFindings) {
    for (const absent of oracle.requiredAbsent) {
      const forbidsRequired = Object.entries(absent).every(([key, value]) => {
        const left = Array.isArray(value) ? [...value].sort() : value;
        const right = Array.isArray(required[key]) ? [...required[key]].sort() : required[key];
        return JSON.stringify(left) === JSON.stringify(right);
      });
      assert.ok(!forbidsRequired, "the same finding cannot be both required and forbidden");
    }
  }
}
