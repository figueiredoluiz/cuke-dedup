import assert from "node:assert/strict";
import test from "node:test";
import { classify, deficits, outcome, validateManifest } from "./language-parity.mjs";

const finding = { rule: "duplicate-handler", severity: "error", primary: { path: "steps.ts", line: 2 },
  related: [{ path: "steps.ts", line: 3 }], suppression: null };
const report = { summary: { definitionsAnalyzed: 2, featureStepsAnalyzed: 1 }, corpus: { incomplete: false },
  findingsTruncated: 0, findings: [finding] };
const expected = { definitions: 2, featureSteps: 1, complete: true,
  requiredFindings: [{ rule: "duplicate-handler" }], requiredAbsent: [{ rule: "parameterization-candidate" }] };
const passing = { outcome: outcome(report, 1), deficits: [] };

test("classifications distinguish shared failures, adapter gaps, missing measurements and drift", () => {
  const failed = { ...passing, deficits: ["missing finding"] };
  assert.equal(classify(passing, passing), "equivalent");
  assert.equal(classify(passing, failed), "ruby-behind");
  assert.equal(classify(failed, passing), "typescript-behind");
  assert.equal(classify(failed, failed), "shared-limitation");
  assert.equal(classify(passing, { error: "timeout" }), "unmeasured");
  assert.equal(classify(passing, { ...passing, outcome: { ...passing.outcome, definitions: 0 } }), "outcome-difference");
  assert.equal(classify(failed, { ...failed, outcome: {} }), "both-have-gaps");
});

test("outcomes ignore source extensions but retain finding ownership, severity and strict exits", () => {
  const ruby = { ...report, findings: [{ ...finding, primary: { path: "steps.rb", line: 2 },
    related: [{ path: "steps.rb", line: 3 }] }] };
  assert.deepEqual(outcome(report, 1), outcome(ruby, 1));
  for (const changed of [{ primary: { path: "steps.ts", line: 4 } }, { related: [] }, { severity: "warning" }]) {
    assert.notDeepEqual(outcome(report, 1), outcome({ ...report, findings: [{ ...finding, ...changed }] }, 1));
  }
  assert.notDeepEqual(outcome(report, 1), outcome(report, 2));
});

test("independent expectations reject suppression, false positives and false completeness", () => {
  assert.deepEqual(deficits(expected, report, 1), []);
  for (const changed of [{ findings: [] }, { findings: [{ ...finding, suppression: {} }] },
    { findings: [finding, { ...finding, rule: "parameterization-candidate" }] },
    { summary: { definitionsAnalyzed: 0, featureStepsAnalyzed: 1 } },
    { corpus: { incomplete: true } }, { findingsTruncated: 1 }]) {
    assert.ok(deficits(expected, { ...report, ...changed }, 1).length);
  }
  assert.ok(deficits(expected, report, null).length);
  assert.ok(deficits(expected, report, 0).length);
  assert.ok(deficits(expected, report, 2).length);
});

test("manifest rejects missing or contradictory expectations and unsafe case names", () => {
  const item = { id: "paired", rule: "duplicate-handler", sources: { typescript: "", ruby: "" }, feature: "",
    intent: "exact duplicate", expected };
  const manifest = { schemaVersion: 1, baselineCommit: "a".repeat(40), cases: [item] };
  validateManifest(manifest);
  for (const cases of [[], [item, item], [{ ...item, id: "../escape" }],
    [{ ...item, expected: { ...expected, requiredAbsent: expected.requiredFindings } }]]) {
    assert.throws(() => validateManifest({ ...manifest, cases }));
  }
});
