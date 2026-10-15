import assert from "node:assert/strict";
import test from "node:test";
import { validateBenchmark } from "./benchmark-budget.mjs";

const budgets = {
  schemaVersion: 2,
  binaryBytes: 100,
  scaling: { "usage-scale": { small: 20, large: 20, metric: "parsingMedianMs", maxRatio: 8 } },
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
const report = { schemaVersion: 3, smoke: false, binaryBytes: 90, results: [result] };

test("benchmark budgets accept deterministic outcomes while treating time as advisory", () => {
  assert.deepEqual(validateBenchmark(report, budgets).failures, []);
  assert.deepEqual(
    validateBenchmark({ ...report, results: [{ ...result, analysisMedianMs: 1_000_000 }] }, budgets).failures,
    [],
  );
});

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

test("benchmark budgets reject a missing result inventory clearly", () => {
  assert.throws(
    () => validateBenchmark({ ...report, results: undefined }, budgets),
    /benchmark report has no results/,
  );
});

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

test("scaling budgets accept growth at or under the ratio and reject growth above it", () => {
  assert.deepEqual(validateBenchmark(growthReport(200, 1600), growthBudgets).failures, []);
  assert.deepEqual(validateBenchmark(growthReport(200, 800), growthBudgets).failures, []);
  const failures = validateBenchmark(growthReport(200, 1602), growthBudgets).failures;
  assert.equal(failures.length, 1);
  assert.match(failures[0], /grows 8\.01x from 10 to 40 definitions, above 8x/);
});

test("scaling budgets fail closed on a base too fast to measure, a missing case or metric", () => {
  assert.match(validateBenchmark(growthReport(49, 100), growthBudgets).failures.join(), /too small to gate growth/);
  assert.deepEqual(validateBenchmark(growthReport(50, 100), growthBudgets).failures, []);
  const missing = { ...growthReport(200, 400), results: growthReport(200, 400).results.slice(0, 1) };
  assert.match(validateBenchmark(missing, growthBudgets).failures.join(), /scaling needs cases 10 and 40/);
  assert.match(validateBenchmark(growthReport(200, undefined), growthBudgets).failures.join(), /was not measured/);
});

test("budgets of the previous schema are rejected", () => {
  assert.throws(() => validateBenchmark(report, { ...budgets, schemaVersion: 1 }), /unsupported benchmark-budget schema/);
});

test("budgets without scaling rules, or with a rule outside the case inventory, are rejected", () => {
  for (const scaling of [undefined, {}]) {
    assert.match(validateBenchmark(report, { ...budgets, scaling }).failures.join(), /define no scaling rules/);
  }
  const orphan = { ...budgets, scaling: { "usage-scale": { small: 20, large: 80, metric: "parsingMedianMs", maxRatio: 8 } } };
  assert.match(validateBenchmark(report, orphan).failures.join(), /usage-scale:80: scaling case has no budget/);
});
