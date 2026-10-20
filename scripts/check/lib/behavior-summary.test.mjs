import { test } from "node:test";
import assert from "node:assert/strict";
import { summaryComparisons } from "./behavior-summary.mjs";

/** A behavior case expecting two definitions and a non-passing duplication summary. */
const testCase = {
  name: "case",
  expected: { definitions: 2, featureSteps: 0, duplicatedDefinitions: 2, percentage: 100, threshold: 0, passed: false },
};

/** Checks the census and summary comparisons pair every observed field with its expected one. */
test("summaryComparisons lists the census first and each summary field once", () => {
  const report = { summary: { definitionsAnalyzed: 2, featureStepsAnalyzed: 1, duplication: { duplicatedDefinitions: 2, percentage: 50, threshold: 0, passed: false } } };
  const rows = summaryComparisons(testCase, report);
  assert.deepEqual(rows.filter((row) => row.census).map((row) => row.label), ["definitions", "feature steps"]);
  assert.deepEqual(rows.filter((row) => row.actual !== row.expected).map((row) => row.label), ["feature steps", "percentage"]);
  // A report without a duplication summary compares as undefined instead of throwing.
  const bare = summaryComparisons(testCase, { summary: { definitionsAnalyzed: 2, featureStepsAnalyzed: 0 } });
  assert.deepEqual(bare.filter((row) => row.actual === undefined).length, 4);
});
