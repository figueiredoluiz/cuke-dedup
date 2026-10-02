import assert from "node:assert/strict";
import test from "node:test";
import { normalizeOutcome, validateOutcomeContract } from "./behavior-spec.mjs";
import { outcome } from "./language-parity.mjs";

const finding = (overrides = {}) => ({ rule: "duplicate-handler", severity: "error",
  primary: { path: "steps.ts", line: 2 }, related: [{ path: "steps.ts", line: 4 }], suppression: null,
  ...overrides });
const report = (findings = [finding()], overrides = {}) => ({
  summary: { definitionsAnalyzed: 2, featureStepsAnalyzed: 1 },
  corpus: { incomplete: false }, findingsTruncated: 0, findings, ...overrides,
});
const languageSuffixes = { "steps.ts": "steps", "steps.rb": "steps" };

test("neutral outcome contracts accept valid zero and positive counts", () => {
  for (const expected of [
    { definitions: 0, featureSteps: 0, complete: false, requiredFindings: [{ rule: "duplicate-handler" }], requiredAbsent: [] },
    { definitions: 2, featureSteps: 1, complete: true, requiredFindings: [{ rule: "duplicate-handler", count: 3 }], requiredAbsent: [{ rule: "near-duplicate-step" }] },
  ]) assert.doesNotThrow(() => validateOutcomeContract(expected, "duplicate-handler"));
});

test("neutral outcome contracts reject each invalid expectation dimension", () => {
  const valid = () => ({ definitions: 2, featureSteps: 1, complete: true,
    requiredFindings: [{ rule: "duplicate-handler" }], requiredAbsent: [{ rule: "near-duplicate-step" }] });
  const invalid = [
    (expected) => { expected.definitions = -1; },
    (expected) => { expected.definitions = 1.5; },
    (expected) => { expected.definitions = "2"; },
    (expected) => { expected.featureSteps = -1; },
    (expected) => { expected.featureSteps = 1.5; },
    (expected) => { expected.featureSteps = "1"; },
    (expected) => { expected.complete = 1; },
    (expected) => { expected.requiredFindings = null; },
    (expected) => { expected.requiredAbsent = {}; },
    (expected) => { expected.requiredFindings[0].rule = ""; },
    (expected) => { expected.requiredFindings[0].rule = 3; },
    (expected) => { delete expected.requiredFindings[0].rule; },
    (expected) => { expected.requiredFindings[0].count = 0; },
    (expected) => { expected.requiredFindings[0].count = 1.5; },
    (expected) => { expected.requiredFindings[0].count = "1"; },
    (expected) => { expected.requiredFindings = []; },
    (expected) => { expected.requiredAbsent[0].rule = "duplicate-handler"; },
  ];
  for (const change of invalid) {
    const expected = valid();
    change(expected);
    assert.throws(() => validateOutcomeContract(expected, "duplicate-handler"));
  }
});

test("explicit source suffix mappings preserve paired logical locations", () => {
  const typescript = normalizeOutcome(report(), 1, languageSuffixes);
  const ruby = normalizeOutcome(report([finding({ primary: { path: "steps.rb", line: 2 },
    related: [{ path: "steps.rb", line: 4 }] })]), 1, languageSuffixes);
  assert.deepEqual(typescript, ruby);
  assert.deepEqual(typescript, { definitions: 2, featureSteps: 1, complete: true, truncated: 0, exit: 1,
    findings: [{ rule: "duplicate-handler", severity: "error", primary: { path: "steps", line: 2 },
      related: [{ path: "steps", line: 4 }] }] });
});

test("finding and related-location ordering does not change the outcome", () => {
  const first = finding({ related: [{ path: "steps.ts", line: 8 }, { path: "steps.ts", line: 4 }] });
  const second = finding({ rule: "unused-definition", primary: { path: "other.ts", line: 1 }, related: [] });
  assert.deepEqual(normalizeOutcome(report([first, second]), 1, languageSuffixes),
    normalizeOutcome(report([second, first]), 1, languageSuffixes));
});

test("suppressed findings are excluded while semantic outcome fields remain visible", () => {
  const baseline = normalizeOutcome(report([finding(), finding({ suppression: { reason: "ignored" } })]), 1, languageSuffixes);
  assert.equal(baseline.findings.length, 1);
  for (const changed of [
    { findings: [finding({ rule: "near-duplicate-step" })] },
    { findings: [finding({ severity: "warning" })] },
    { findings: [finding({ primary: { path: "steps.ts", line: 3 } })] },
    { findings: [finding({ related: [{ path: "steps.ts", line: 5 }] })] },
    { corpus: { incomplete: true } }, { findingsTruncated: 1 },
  ]) assert.notDeepEqual(normalizeOutcome(report(undefined, changed), 1, languageSuffixes), baseline);
  assert.notDeepEqual(normalizeOutcome(report(), 2, languageSuffixes), baseline);
});

test("only explicit terminal source suffixes are rewritten", () => {
  assert.equal(normalizeOutcome(report(), 1).findings[0].primary.path, "steps.ts");
  assert.equal(normalizeOutcome(report(), 1, { ".ts": ".src" }).findings[0].primary.path, "steps.src");
  assert.equal(normalizeOutcome(report(), 1, { "steps.ts": "steps" }).findings[0].primary.path, "steps");
});

test("language-parity outcome remains the established compatibility wrapper", () => {
  assert.deepEqual(outcome(report(), 1), normalizeOutcome(report(), 1, languageSuffixes));
});
