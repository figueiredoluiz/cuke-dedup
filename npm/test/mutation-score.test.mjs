import assert from "node:assert/strict";
import test from "node:test";
import { checkMutationScore } from "../../scripts/check/check-mutation-score.mjs";

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

test("accepts the minimum score and excludes unviable mutants", () => {
  assert.equal(checkMutationScore(report(), 90), 90);
});

test("rejects weak scores, timeouts, inconsistent totals, and failed baselines", () => {
  assert.throws(() => checkMutationScore(report({ caught: 8, missed: 2 }), 90));
  assert.throws(() => checkMutationScore(report({ caught: 9, missed: 0, timeout: 1 }), 90));
  assert.throws(() => checkMutationScore(report({ total_mutants: 13 }), 90));
  assert.throws(() => checkMutationScore(report({ outcomes: [] }), 90));
  assert.throws(() => checkMutationScore(report({ caught: 8, success: 1 }), 90));
});
