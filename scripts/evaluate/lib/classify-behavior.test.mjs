import { test } from "node:test";
import assert from "node:assert/strict";
import { judgeBehaviorCase } from "../../check/lib/behavior-runner.mjs";
import { classifyBehaviorCase } from "./classify-behavior.mjs";

// Matrix. Outcome: match, missing count, excess count, unlisted rule, clean case. Operational:
// spawn failure, no report, exit outside {0, 1, expectedExit}, signal. Label: no expected block, no
// or malformed byRule, non-integer exit, malformed report byRule. Census: definitions, feature steps.
// Each scored outcome has a control returning the opposite answer in the same table.

/** A behavior case expecting two duplicate-matcher findings and exit 1. */
const baseCase = (over = {}) => ({
  name: "case",
  expectedExit: 1,
  expected: { definitions: 3, featureSteps: 0, duplicatedDefinitions: 2, percentage: 66.7, threshold: 0, passed: false, byRule: { "duplicate-matcher": 2 } },
  ...over,
});

/** An observation of a run reporting `byRule`, matching `baseCase` otherwise. */
const observed = (byRule, over = {}) => ({
  status: 1,
  stderr: "",
  spawnFailure: null,
  report: { summary: { definitionsAnalyzed: 3, featureStepsAnalyzed: 0, byRule, duplication: { duplicatedDefinitions: 2, percentage: 66.7, threshold: 0, passed: false } } },
  ...over,
});

/** Checks the per-rule counts of count outcomes, each against the opposite-answer control. */
test("rule counts map expected against observed", () => {
  const rows = [
    ["match", { "duplicate-matcher": 2 }, { tp: 2, fp: 0, fn: 0 }, []],
    ["missing", { "duplicate-matcher": 1 }, { tp: 1, fp: 0, fn: 1 }, ["missing"]],
    ["absent", {}, { tp: 0, fp: 0, fn: 2 }, ["missing"]],
    ["excess", { "duplicate-matcher": 3 }, { tp: 2, fp: 1, fn: 0 }, ["unexpected"]],
  ];
  for (const [label, byRule, counts, kinds] of rows) {
    const result = classifyBehaviorCase(baseCase(), observed(byRule));
    assert.ok(result.rules["duplicate-matcher"], `${label}: the rule has a row`);
    assert.deepEqual(result.rules["duplicate-matcher"], { ...counts, tn: 0, knownMisses: 0 }, label);
    assert.deepEqual(result.entries.map((entry) => entry.kind), kinds, label);
    assert.equal(result.corpus, "behavior");
    assert.equal(result.observable, true);
  }
});

/** Checks a rule the case does not list is a false positive, and a clean case with a clean run scores nothing. */
test("an unlisted rule is a false positive and a clean case has no rules", () => {
  const unlisted = classifyBehaviorCase(baseCase(), observed({ "duplicate-matcher": 2, "ambiguous-step": 1 }));
  assert.deepEqual(unlisted.rules["ambiguous-step"], { tp: 0, fp: 1, fn: 0, tn: 0, knownMisses: 0 });
  assert.deepEqual(unlisted.entries.map((entry) => [entry.rule, entry.kind]), [["ambiguous-step", "unexpected"]]);
  const clean = classifyBehaviorCase(
    baseCase({ expectedExit: 0, expected: { ...baseCase().expected, byRule: {} } }),
    observed({}, { status: 0 }),
  );
  assert.deepEqual([clean.observable, clean.rules, clean.entries.map((entry) => entry.kind)], [true, {}, []]);
  // A rule listed with a zero count on both sides is neither expected nor observed, so it has no row.
  const zero = classifyBehaviorCase(baseCase(), observed({ "duplicate-matcher": 2, "ambiguous-step": 0 }));
  assert.deepEqual(Object.keys(zero.rules), ["duplicate-matcher"]);
});

/** Checks the exit, duplication summary and stderr disagreements are listed without changing the rule counts. */
test("exit, summary and stderr disagreements are entries, not scores", () => {
  const result = classifyBehaviorCase(
    baseCase({ expectedExit: 0, stderrIncludes: ["needle"] }),
    observed({ "duplicate-matcher": 2 }, { stderr: "hay" }),
  );
  assert.deepEqual(result.rules["duplicate-matcher"], { tp: 2, fp: 0, fn: 0, tn: 0, knownMisses: 0 });
  assert.deepEqual(result.entries.map((entry) => entry.kind), ["exit", "stderr"]);
  const summary = classifyBehaviorCase(baseCase(), observed({ "duplicate-matcher": 2 }, {
    report: { summary: { definitionsAnalyzed: 3, featureStepsAnalyzed: 0, byRule: { "duplicate-matcher": 2 }, duplication: { duplicatedDefinitions: 1, percentage: 66.7, threshold: 0, passed: false } } },
  }));
  assert.deepEqual(summary.entries.map((entry) => entry.kind), ["summary"]);
  assert.match(summary.entries[0].detail, /duplicated definitions expected 2, got 1/);
});

/** Checks every operational failure is unscored with its cause, while an expected exit 2 is not one. */
test("operational failures are unscored", () => {
  const rows = [
    ["spawn failure", observed({}, { status: null, spawnFailure: "spawn ENOENT", report: null }), /spawn ENOENT/],
    ["no report", observed({}, { report: null, stderr: "boom" }), /exit 1 wrote no JSON report: boom/],
    ["unexpected exit", observed({}, { status: 101, stderr: "panic" }), /exit 101: panic/],
    ["signal", observed({}, { status: null, spawnFailure: "killed by SIGKILL", report: null }), /killed by SIGKILL/],
  ];
  for (const [label, observation, cause] of rows) {
    const result = classifyBehaviorCase(baseCase(), observation);
    assert.equal(result.observable, false, label);
    assert.deepEqual([result.corpus, result.rules, result.entries], ["behavior", {}, []], label);
    assert.match(result.runFailure, cause, label);
  }
  // Control: a case that declares exit 2 and gets it is scored.
  const strict = classifyBehaviorCase(baseCase({ expectedExit: 2 }), observed({ "duplicate-matcher": 2 }, { status: 2 }));
  assert.equal(strict.observable, true);
});

/** Checks a case or report this classifier does not model is unscored, never an empty clean case. */
test("unmodeled labels and report shapes are unscored", () => {
  const unlabelled = [
    ["no expected block", { expected: undefined }],
    ["no byRule", { expected: { ...baseCase().expected, byRule: undefined } }],
    ["array byRule", { expected: { ...baseCase().expected, byRule: [] } }],
    ["fractional count", { expected: { ...baseCase().expected, byRule: { "duplicate-matcher": 1.5 } } }],
    ["negative count", { expected: { ...baseCase().expected, byRule: { "duplicate-matcher": -1 } } }],
    ["no census", { expected: { ...baseCase().expected, definitions: undefined } }],
    ["no percentage", { expected: { ...baseCase().expected, percentage: undefined } }],
    ["string threshold", { expected: { ...baseCase().expected, threshold: "0" } }],
    ["no passed", { expected: { ...baseCase().expected, passed: undefined } }],
    ["no duplicated definitions", { expected: { ...baseCase().expected, duplicatedDefinitions: undefined } }],
    ["non-integer exit", { expectedExit: "1" }],
  ];
  for (const [label, over] of unlabelled) {
    const result = classifyBehaviorCase(baseCase(over), observed({ "duplicate-matcher": 2 }));
    assert.equal(result.observable, false, label);
    assert.match(result.runFailure, /^unmodeled label/, label);
  }
  for (const byRule of [undefined, [], { "duplicate-matcher": "2" }]) {
    const result = classifyBehaviorCase(baseCase(), observed(byRule));
    assert.equal(result.observable, false);
    assert.match(result.runFailure, /unmodeled report/);
  }
});

/** Checks a run that analyzed other inputs than declared is unscored, naming each mismatch. */
test("an extraction mismatch is unscored", () => {
  const wrong = observed({ "duplicate-matcher": 2 });
  wrong.report.summary.definitionsAnalyzed = 2;
  wrong.report.summary.featureStepsAnalyzed = 1;
  const result = classifyBehaviorCase(baseCase(), wrong);
  assert.equal(result.observable, false);
  assert.match(result.runFailure, /definitions 2, expected 3; feature steps 1, expected 0/);
});

/** Checks the judge half of the runner asserts what the classifier only counts. */
test("judgeBehaviorCase fails on a rule count the classifier scores as a miss", () => {
  const good = { ...observed({ "duplicate-matcher": 2 }), fingerprintsText: "" };
  const labelled = baseCase({ role: "positive", origin: "synthetic", language: "ecmascript", frameworks: ["unknown"], verdict: "reviewed", classifiedAt: "2026-10-20", axes: ["control"] });
  assert.throws(() => judgeBehaviorCase(labelled, { ...good, report: { ...good.report, summary: { ...good.report.summary, byRule: { "duplicate-matcher": 1 } } } }), /rules/);
  assert.throws(() => judgeBehaviorCase(labelled, { ...good, fingerprintsText: null }), /no fingerprint records/);
  // A binary that cannot start names why, instead of a bare null status.
  assert.throws(() => judgeBehaviorCase(labelled, { ...good, status: null, spawnFailure: "spawn ENOENT", stderr: "" }), /case: spawn ENOENT/);
});

/** Checks a malformed observed summary is unscored, while the same fields well typed but different stay entries. */
test("a malformed duplication summary is unscored, not scored", () => {
  /** The matching summary with fields overridden. */
  const summaryOf = (over) => ({ ...observed({ "duplicate-matcher": 2 }).report.summary, ...over });
  const rows = [
    ["no duplication", { duplication: undefined }, /summary\.duplication is missing/],
    ["null duplication", { duplication: null }, /summary\.duplication is missing/],
    ["empty duplication", { duplication: {} }, /incomplete or mistyped/],
    ["string percentage", { duplication: { duplicatedDefinitions: 2, percentage: "66.7", threshold: 0, passed: false } }, /incomplete or mistyped/],
    ["string passed", { duplication: { duplicatedDefinitions: 2, percentage: 66.7, threshold: 0, passed: "false" } }, /incomplete or mistyped/],
    ["fractional count", { duplication: { duplicatedDefinitions: 1.5, percentage: 66.7, threshold: 0, passed: false } }, /incomplete or mistyped/],
    ["string census", { definitionsAnalyzed: "3" }, /not a nonnegative integer/],
    ["no summary", null, /summary is missing/],
  ];
  for (const [label, over, cause] of rows) {
    const report = { summary: over === null ? undefined : summaryOf(over) };
    const result = classifyBehaviorCase(baseCase(), observed({ "duplicate-matcher": 2 }, { report }));
    assert.equal(result.observable, false, label);
    assert.match(result.cause ?? result.runFailure ?? JSON.stringify(result), cause, label);
  }
  // Control: the same summary well typed but disagreeing with the case is scored and listed.
  const control = classifyBehaviorCase(baseCase(), observed({ "duplicate-matcher": 2 }, {
    report: { summary: summaryOf({ duplication: { duplicatedDefinitions: 1, percentage: 33.3, threshold: 0, passed: true } }) },
  }));
  assert.equal(control.observable, true);
  assert.deepEqual(control.rules["duplicate-matcher"], { tp: 2, fp: 0, fn: 0, tn: 0, knownMisses: 0 });
  assert.ok(control.entries.some((entry) => entry.kind === "summary"));
});
