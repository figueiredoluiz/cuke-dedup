import assert from "node:assert/strict";
import test from "node:test";
import { validateBenchmark } from "./benchmark-budget.mjs";

const budgets = {
  schemaVersion: 1,
  binaryBytes: 100,
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
