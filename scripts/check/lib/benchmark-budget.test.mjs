import assert from "node:assert/strict";
import test from "node:test";
import { validateBenchmark } from "./benchmark-budget.mjs";

const budgets = {
  schemaVersion: 2,
  binaryBytes: 100,
  scaling: { "usage-scale": { small: 20, large: 80, metric: "parsingMedianMs", maxRatio: 8 } },
  cases: {
    "usage-scale:20": {
      peakMemoryBytes: 200,
      findingCount: 0,
      analysisComplete: true,
      analysisTruncated: false,
      candidateComparisonsEvaluated: 0,
      skippedCandidateComparisons: 0,
      truncatedStructuralClasses: 0,
    },
    "usage-scale:80": {
      peakMemoryBytes: 200,
      findingCount: 0,
      analysisComplete: true,
      analysisTruncated: false,
      candidateComparisonsEvaluated: 0,
      skippedCandidateComparisons: 0,
      truncatedStructuralClasses: 0,
    },
  },
};
const result = {
  profile: "usage-scale",
  parsingMedianMs: 150,
  definitions: 20,
  findingCount: 0,
  analysisComplete: true,
  analysisTruncated: false,
  candidateComparisonsEvaluated: 0,
  skippedCandidateComparisons: 0,
  truncatedStructuralClasses: 0,
  peakMemoryBytes: 150,
  analysisMedianMs: 10,
};
const report = {
  schemaVersion: 3,
  smoke: false,
  binaryBytes: 90,
  results: [result, { ...result, definitions: 80, parsingMedianMs: 300 }],
};

/** Deterministic outcomes pass, and an extreme absolute time alone is not a failure. */
test("benchmark budgets accept deterministic outcomes while treating time as advisory", () => {
  assert.deepEqual(validateBenchmark(report, budgets).failures, []);
  assert.deepEqual(
    validateBenchmark({ ...report, results: [{ ...result, analysisMedianMs: 1_000_000 }, report.results[1]] }, budgets).failures,
    [],
  );
});

/** Checks benchmark budgets detect inventory, semantic, memory, and binary regressions. */
test("benchmark budgets reject inventory, semantic, memory, and binary regressions", () => {
  const failures = [
    validateBenchmark({ ...report, results: [] }, budgets).failures,
    validateBenchmark({ ...report, results: [result, result] }, budgets).failures,
    validateBenchmark({ ...report, results: [{ ...result, profile: "unexpected" }] }, budgets).failures,
    validateBenchmark({ ...report, results: [{ ...result, analysisComplete: false }] }, budgets).failures,
    validateBenchmark({ ...report, results: [{ ...result, analysisTruncated: true }] }, budgets).failures,
    validateBenchmark({ ...report, results: [{ ...result, peakMemoryBytes: null }] }, budgets).failures,
    validateBenchmark({ ...report, results: [{ ...result, peakMemoryBytes: 201 }] }, budgets).failures,
    validateBenchmark({ ...report, binaryBytes: 101 }, budgets).failures,
  ];
  assert.ok(failures.every((items) => items.length > 0));
});

/** Checks a missing benchmark result inventory raises a clear validation error. */
test("benchmark budgets reject a missing result inventory clearly", () => {
  assert.throws(
    () => validateBenchmark({ ...report, results: undefined }, budgets),
    /benchmark report has no results/,
  );
});

/** Checks full-profile validation rejects smoke reports and unsupported report schemas. */
test("full-profile budgets reject smoke reports and schema drift", () => {
  assert.throws(() => validateBenchmark({ ...report, smoke: true }, budgets));
  assert.throws(() => validateBenchmark({ ...report, schemaVersion: 4 }, budgets));
});

const growthCase = {
  peakMemoryBytes: 200,
  findingCount: 0,
  analysisComplete: true,
  analysisTruncated: false,
  candidateComparisonsEvaluated: 0,
  skippedCandidateComparisons: 0,
  truncatedStructuralClasses: 0,
};
const growthBudgets = {
  ...budgets,
  cases: { "growth:10": growthCase, "growth:40": growthCase },
  scaling: { growth: { small: 10, large: 40, metric: "parsingMedianMs", maxRatio: 8 } },
};
/** Report with the two growth cases at the given parse medians. */
const growthReport = (smallMs, largeMs) => ({
  ...report,
  results: [
    { ...result, ...growthCase, profile: "growth", definitions: 10, parsingMedianMs: smallMs },
    { ...result, ...growthCase, profile: "growth", definitions: 40, parsingMedianMs: largeMs },
  ],
});

/** Growth at or under the ceiling passes; growth above it fails, compared before rounding. */
test("scaling budgets accept growth at or under the ratio and reject growth above it", () => {
  assert.deepEqual(validateBenchmark(growthReport(200, 1600), growthBudgets).failures, []);
  assert.deepEqual(validateBenchmark(growthReport(200, 800), growthBudgets).failures, []);
  const failures = validateBenchmark(growthReport(200, 1602), growthBudgets).failures;
  assert.equal(failures.length, 1);
  assert.match(failures[0], /grows 8\.01x from 10 to 40 definitions, above 8x/);
  // Rounding happens only for display: 8.004x shows as 8.00x and still fails an 8x ceiling.
  const unrounded = validateBenchmark(growthReport(1000, 8004), growthBudgets).failures;
  assert.equal(unrounded.length, 1);
  assert.match(unrounded[0], /grows 8\.00x from 10 to 40 definitions, above 8x/);
});

/** Each malformed rule field fails with the field named instead of disabling the check. */
test("malformed scaling rules fail instead of passing silently", () => {
  const rule = growthBudgets.scaling.growth;
  for (const [field, value, message] of [
    ["maxRatio", undefined, /maxRatio must be a finite number above 1/],
    ["maxRatio", "6", /maxRatio must be a finite number above 1/],
    ["maxRatio", 1, /maxRatio must be a finite number above 1/],
    ["metric", "parseMs", /metric must be one of/],
    ["small", 40, /small and large must be positive integers with small < large/],
    ["large", 2.5, /small and large must be positive integers with small < large/],
  ]) {
    const budgetsWithRule = { ...growthBudgets, scaling: { growth: { ...rule, [field]: value } } };
    assert.match(validateBenchmark(growthReport(200, 400), budgetsWithRule).failures.join(), message, `${field}=${value}`);
  }
});

/** A base under the floor, a missing case or a missing metric fails rather than passes. */
test("scaling budgets fail closed on a base too fast to measure, a missing case or metric", () => {
  assert.match(validateBenchmark(growthReport(49, 100), growthBudgets).failures.join(), /too small to gate growth/);
  assert.deepEqual(validateBenchmark(growthReport(50, 100), growthBudgets).failures, []);
  const missing = { ...growthReport(200, 400), results: growthReport(200, 400).results.slice(0, 1) };
  assert.match(validateBenchmark(missing, growthBudgets).failures.join(), /scaling needs cases 10 and 40/);
  assert.match(validateBenchmark(growthReport(200, undefined), growthBudgets).failures.join(), /was not measured/);
});

/** Schema-1 budgets, which had no scaling rules, are rejected. */
test("budgets of the previous schema are rejected", () => {
  assert.throws(() => validateBenchmark(report, { ...budgets, schemaVersion: 1 }), /unsupported benchmark-budget schema/);
});

/** An absent or empty scaling map, or a rule naming an unbudgeted case, fails. */
test("budgets without scaling rules, or with a rule outside the case inventory, are rejected", () => {
  for (const scaling of [undefined, {}]) {
    assert.match(validateBenchmark(report, { ...budgets, scaling }).failures.join(), /define no scaling rules/);
  }
  const orphan = { ...budgets, scaling: { "usage-scale": { small: 20, large: 160, metric: "parsingMedianMs", maxRatio: 8 } } };
  assert.match(validateBenchmark(report, orphan).failures.join(), /usage-scale:160: scaling case has no budget/);
});
