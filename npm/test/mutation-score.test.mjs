import assert from "node:assert/strict";
import test from "node:test";
import { checkMutationScore } from "../../scripts/check/check-mutation-score.mjs";

/** Build a valid mutation report with caller overrides for individual rejection cases. */
function report(overrides = {}) {
  return {
    caught: 9,
    missed: 1,
    timeout: 0,
    unviable: 2,
    success: 0,
    total_mutants: 12,
    outcomes: [{ scenario: "Baseline", summary: "Success" }],
    ...overrides,
  };
}

/** Verify the minimum accepted score excludes unviable mutants from its denominator. */
test("accepts the minimum score and excludes unviable mutants", () => {
  assert.equal(checkMutationScore(report(), 90), 90);
});

/** Verify weak scores, timeouts, inconsistent totals, missing baselines, and unexplained successes are rejected. */
test("rejects weak scores, timeouts, inconsistent totals, and failed baselines", () => {
  assert.throws(() => checkMutationScore(report({ caught: 8, missed: 2 }), 90));
  assert.throws(() => checkMutationScore(report({ caught: 9, missed: 0, timeout: 1 }), 90));
  assert.throws(() => checkMutationScore(report({ total_mutants: 13 }), 90));
  assert.throws(() => checkMutationScore(report({ outcomes: [] }), 90));
  assert.throws(() => checkMutationScore(report({ caught: 8, success: 1 }), 90));
});
