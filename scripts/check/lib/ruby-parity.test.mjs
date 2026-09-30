import assert from "node:assert/strict";
import test from "node:test";
import { completionPassed, corpusCoverageDeficits, parityDeficits, validateOracle } from "./ruby-parity.mjs";

const oracle = {
  path: "example", definitions: 2, featureSteps: 0, complete: true, expectedExit: 0,
  requiredFindings: [{ rule: "duplicate-handler", primaryLine: 1 }],
  requiredAbsent: [{ rule: "near-duplicate-step" }],
};
const finding = {
  rule: "duplicate-handler", primary: { path: "steps.rb", line: 1 },
  related: [], suppression: null,
};
const report = {
  summary: { definitionsAnalyzed: 2, featureStepsAnalyzed: 0 },
  corpus: { incomplete: false }, findings: [finding], findingsTruncated: 0,
};

test("passing implemented fixtures cannot hide missing mappings or an empty run", () => {
  const passing = [{ deficits: [] }];
  assert.equal(completionPassed(passing, []), true);
  assert.equal(completionPassed(passing, ["unmapped scope test"]), false);
  assert.equal(completionPassed([{ deficits: ["missing positive"] }], []), false);
  assert.equal(completionPassed([], []), false);
});

test("source counterpart coverage cannot be inferred from inventory or passing implementations", () => {
  const mapping = { source: "source.json", case: "case", requiredStatus: "covered", fixtureGroup: "group" };
  const groups = { group: oracle };
  assert.deepEqual(corpusCoverageDeficits([mapping], groups), []);
  assert.ok(corpusCoverageDeficits([], groups).length);
  assert.ok(corpusCoverageDeficits([mapping], {}).length);
  assert.ok(corpusCoverageDeficits([{ ...mapping, requiredStatus: "partial" }], groups).length);
  assert.ok(corpusCoverageDeficits([{ ...mapping, requiredStatus: "framework-inapplicable" }], groups).length);
  assert.deepEqual(corpusCoverageDeficits([{ ...mapping, requiredStatus: "framework-inapplicable",
    rationale: "The positional framework has no Ruby global-registration counterpart." }], groups), []);
});

test("candidate-boundary oracles assert evaluated sources rather than just final finding counts", () => {
  const bounded = { ...oracle, expectedCandidateSources: { matcherBlocking: 1 } };
  validateOracle(bounded);
  for (const count of [-1, 1.5, "1"]) {
    assert.throws(() => validateOracle({ ...oracle, expectedCandidateSources: { matcherBlocking: count } }));
  }
  assert.ok(parityDeficits(bounded, report, 0).some((gap) => gap.includes("matcherBlocking candidates")));
  assert.deepEqual(parityDeficits(bounded, { ...report,
    analysis: { candidateSources: { matcherBlocking: { evaluated: 1 } } },
  }, 0), []);
});

test("Ruby parity rejects blanket suppression, unclassified findings and overlapping oracles", () => {
  validateOracle(oracle);
  assert.deepEqual(parityDeficits(oracle, report, 0), []);
  for (const findings of [[], [{ ...finding, suppression: {} }],
    [finding, { ...finding, rule: "near-duplicate-step" }]]) {
    assert.ok(parityDeficits(oracle, { ...report, findings }, 0).length > 0);
  }
  assert.ok(parityDeficits({ ...oracle,
    requiredFindings: [oracle.requiredFindings[0], { rule: "duplicate-handler" }],
  }, report, 0).some((message) => message.includes("ownership")));
});

test("Ruby parity checks strict completeness, counts and truncation independently", () => {
  for (const changed of [
    { corpus: { incomplete: true } }, { findingsTruncated: 1 },
    { summary: { definitionsAnalyzed: 0, featureStepsAnalyzed: 0 } },
  ]) {
    assert.ok(parityDeficits(oracle, { ...report, ...changed }, 0).length > 0);
  }
  assert.ok(parityDeficits(oracle, report, 2).length > 0);
  assert.throws(() => validateOracle({ ...oracle, requiredAbsent: [{}] }));
  assert.throws(() => validateOracle({ ...oracle, requiredAbsent: [{ rule: "duplicate-handler" }] }));
});
