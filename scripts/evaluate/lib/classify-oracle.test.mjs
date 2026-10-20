import { test } from "node:test";
import assert from "node:assert/strict";
import { classifyOracle } from "./classify-oracle.mjs";

// Matrix. Corpus: paired language-parity lane (ownership off), Ruby-parity group (ownership on).
// Outcome: match, missing, excess, forbidden, unowned. Operational: spawn failure, no report, exit
// outside 0-2, truncated findings. Label: oracle absent, requiredFindings not a list, rule missing,
// bad count, census not integers. Census: definitions, feature steps, candidate sources. Each row
// is paired with the opposite answer in the same test.

/** An oracle requiring one duplicate-matcher finding and no near-duplicate-step. */
const oracle = (over = {}) => ({
  definitions: 2,
  featureSteps: 1,
  complete: true,
  requiredFindings: [{ rule: "duplicate-matcher" }],
  requiredAbsent: [{ rule: "near-duplicate-step" }],
  ...over,
});

/** An active finding of `rule` at `line`. */
const finding = (rule, line = 1) => ({ rule, severity: "error", suppression: null, primary: { path: "steps.rb", line }, related: [], message: "m", evidence: {} });

/** A completed observation holding `findings`, matching `oracle` otherwise. */
const observed = (findings, over = {}) => ({
  status: 1,
  stderr: "",
  spawnFailure: null,
  report: {
    summary: { definitionsAnalyzed: 2, featureStepsAnalyzed: 1 },
    corpus: { incomplete: false },
    findingsTruncated: 0,
    analysis: { candidateSources: { matcherBlocking: { evaluated: 1 } } },
    findings,
  },
  ...over,
});

/** The target of a Ruby group or a paired lane, overridable. */
const target = (over = {}) => ({ corpus: "ruby-parity", name: "group", lane: null, oracle: oracle(), ownership: true, ...over });

/** Checks a matching run scores one true positive and one true negative, with nothing to review. */
test("a run matching its oracle is true positives and true negatives", () => {
  const result = classifyOracle(target(), observed([finding("duplicate-matcher")]));
  assert.deepEqual(result.rules["duplicate-matcher"], { tp: 1, fp: 0, fn: 0, tn: 0, knownMisses: 0 });
  assert.equal(result.rules["near-duplicate-step"].tn, 1);
  assert.deepEqual([result.corpus, result.lane, result.entries], ["ruby-parity", null, []]);
});

/** Checks a missing required finding is a false negative, its control counts a true positive, and counts are honored. */
test("missing and counted findings", () => {
  const missing = classifyOracle(target(), observed([], { status: 0 }));
  assert.deepEqual(missing.rules["duplicate-matcher"], { tp: 0, fp: 0, fn: 1, tn: 0, knownMisses: 0 });
  assert.deepEqual(missing.entries.map((entry) => entry.kind), ["missing"]);
  const counted = target({ oracle: oracle({ requiredFindings: [{ rule: "duplicate-matcher", count: 2 }] }) });
  const one = classifyOracle(counted, observed([finding("duplicate-matcher")]));
  assert.deepEqual([one.rules["duplicate-matcher"].tp, one.rules["duplicate-matcher"].fn], [1, 1]);
  const two = classifyOracle(counted, observed([finding("duplicate-matcher", 1), finding("duplicate-matcher", 2)]));
  assert.deepEqual([two.rules["duplicate-matcher"].tp, two.rules["duplicate-matcher"].fn, two.entries], [2, 0, []]);
});

/** Checks a finding beyond the count is a false positive and a suppressed finding is not counted. */
test("excess findings are false positives and suppressed ones are ignored", () => {
  const excess = classifyOracle(target(), observed([finding("duplicate-matcher", 1), finding("duplicate-matcher", 2)]));
  assert.deepEqual([excess.rules["duplicate-matcher"].tp, excess.rules["duplicate-matcher"].fp], [1, 1]);
  assert.deepEqual(excess.entries.map((entry) => entry.kind), ["excess"]);
  const suppressed = classifyOracle(target(), observed([finding("duplicate-matcher"), { ...finding("duplicate-matcher", 2), suppression: { reason: "x" } }]));
  assert.deepEqual([suppressed.rules["duplicate-matcher"].fp, suppressed.entries], [0, []]);
});

/** Checks a forbidden finding loses the true negative and is one false positive in either ownership mode. */
test("a forbidden finding is a false positive once", () => {
  for (const ownership of [true, false]) {
    const result = classifyOracle(target({ ownership }), observed([finding("duplicate-matcher"), finding("near-duplicate-step", 5)]));
    assert.deepEqual(result.rules["near-duplicate-step"], { tp: 0, fp: 1, fn: 0, tn: 0, knownMisses: 0 }, `ownership ${ownership}`);
    assert.ok(result.entries.some((entry) => entry.kind === "forbidden"), `ownership ${ownership}`);
  }
});

/** Checks an unowned finding is a false positive only where the gate requires ownership. */
test("unowned findings are scored only with ownership", () => {
  const findings = [finding("duplicate-matcher"), finding("unused-definition", 7)];
  const owned = classifyOracle(target({ ownership: true }), observed(findings));
  assert.deepEqual(owned.rules["unused-definition"], { tp: 0, fp: 1, fn: 0, tn: 0, knownMisses: 0 });
  assert.deepEqual(owned.entries.map((entry) => [entry.rule, entry.kind]), [["unused-definition", "unowned"]]);
  const lane = classifyOracle(target({ corpus: "language-parity", lane: "ruby", ownership: false }), observed(findings));
  assert.equal(lane.rules["unused-definition"], undefined);
  assert.deepEqual([lane.lane, lane.entries], ["ruby", []]);
});

/** Checks a finding two expectations own is an anomaly counted nowhere. */
test("overlapping expectations are an anomaly", () => {
  const broad = target({ oracle: oracle({ requiredFindings: [{ rule: "duplicate-matcher" }, { rule: "duplicate-matcher", primaryLine: 1 }] }) });
  const result = classifyOracle(broad, observed([finding("duplicate-matcher")]));
  assert.deepEqual(result.entries.map((entry) => entry.kind), ["overlapping-expectations", "missing", "missing"]);
  assert.deepEqual([result.rules["duplicate-matcher"].tp, result.rules["duplicate-matcher"].fp], [0, 0]);
});

/** Checks completeness and exit disagreements are listed without changing scores. */
test("completeness and exit disagreements are entries", () => {
  const incomplete = observed([finding("duplicate-matcher")]);
  incomplete.report.corpus.incomplete = true;
  const result = classifyOracle(target({ oracle: oracle({ expectedExit: 0 }) }), incomplete);
  assert.deepEqual(result.entries.map((entry) => entry.kind), ["completeness", "exit"]);
  assert.equal(result.rules["duplicate-matcher"].tp, 1);
  // Control: the matching completeness and exit raise neither.
  const control = classifyOracle(target({ oracle: oracle({ expectedExit: 1 }) }), observed([finding("duplicate-matcher")]));
  assert.deepEqual(control.entries, []);
});

/** Checks every operational failure is unscored with its cause and names the corpus and lane. */
test("operational failures are unscored", () => {
  const rows = [
    ["spawn failure", observed([], { status: null, spawnFailure: "spawn ENOENT", report: null }), /spawn ENOENT/],
    ["no report", observed([], { report: null, stderr: "boom" }), /exit 1 wrote no JSON report: boom/],
    ["panic exit", observed([], { status: 101 }), /exit 101/],
    ["null status", observed([], { status: null, spawnFailure: "killed by SIGKILL" }), /killed by SIGKILL/],
    // An allowed exit with a report does not excuse a process error such as an output-limit kill.
    ["process error with report", observed([], { spawnFailure: "spawnSync ENOBUFS" }), /ENOBUFS/],
  ];
  for (const [label, observation, cause] of rows) {
    const result = classifyOracle(target({ corpus: "language-parity", lane: "typescript" }), observation);
    assert.equal(result.observable, false, label);
    assert.deepEqual([result.corpus, result.lane, result.rules], ["language-parity", "typescript", {}], label);
    assert.match(result.runFailure, cause, label);
  }
  // Control: strict incompleteness (exit 2) is a completed run.
  assert.equal(classifyOracle(target(), observed([finding("duplicate-matcher")], { status: 2 })).observable, true);
});

/** Checks truncated findings and every census mismatch make the run unscored, since absences would certify nothing. */
test("truncation and extraction mismatches are unscored", () => {
  const truncated = observed([finding("duplicate-matcher")]);
  truncated.report.findingsTruncated = 3;
  assert.match(classifyOracle(target(), truncated).runFailure, /truncated/);
  const untracked = observed([finding("duplicate-matcher")]);
  delete untracked.report.findingsTruncated;
  assert.equal(classifyOracle(target(), untracked).observable, false);
  const census = observed([finding("duplicate-matcher")]);
  census.report.summary.definitionsAnalyzed = 5;
  census.report.summary.featureStepsAnalyzed = 0;
  assert.match(classifyOracle(target(), census).runFailure, /definitions 5, expected 2; feature steps 0, expected 1/);
  const sources = target({ oracle: oracle({ expectedCandidateSources: { matcherBlocking: 2 } }) });
  assert.match(classifyOracle(sources, observed([finding("duplicate-matcher")])).runFailure, /matcherBlocking candidates 1, expected 2/);
  assert.equal(classifyOracle(target({ oracle: oracle({ expectedCandidateSources: { matcherBlocking: 1 } }) }), observed([finding("duplicate-matcher")])).observable, true);
});

/** Checks an oracle this classifier does not model is unscored, never an empty clean oracle. */
test("unmodeled oracles are unscored", () => {
  const bad = [
    ["no oracle", undefined],
    ["findings not a list", oracle({ requiredFindings: undefined })],
    ["absent not a list", oracle({ requiredAbsent: {} })],
    ["rule missing", oracle({ requiredFindings: [{ primaryLine: 1 }] })],
    ["empty rule", oracle({ requiredFindings: [{ rule: "" }] })],
    ["zero count", oracle({ requiredFindings: [{ rule: "duplicate-matcher", count: 0 }] })],
    ["census not integers", oracle({ definitions: "2" })],
    ["candidate sources not a map", oracle({ expectedCandidateSources: [] })],
    ["candidate source count fractional", oracle({ expectedCandidateSources: { matcherBlocking: 1.5 } })],
    ["complete not boolean", oracle({ complete: "true" })],
    ["exit not integer", oracle({ expectedExit: "1" })],
  ];
  for (const [label, value] of bad) {
    const result = classifyOracle(target({ oracle: value }), observed([finding("duplicate-matcher")]));
    assert.equal(result.observable, false, label);
    assert.match(result.runFailure, /^unmodeled label/, label);
  }
});

/** Checks a paired oracle with no declared exit implies one as its gate does: 2 when incomplete, else 1 only for an error finding. */
test("the exit is implied when the oracle declares none", () => {
  /** The entry kinds of a result. */
  const kinds = (result) => result.entries.map((entry) => entry.kind);
  const warning = { ...finding("duplicate-matcher"), severity: "warning" };
  assert.deepEqual(kinds(classifyOracle(target(), observed([finding("duplicate-matcher")], { status: 1 }))), []);
  assert.deepEqual(kinds(classifyOracle(target(), observed([finding("duplicate-matcher")], { status: 0 }))), ["exit"]);
  assert.deepEqual(kinds(classifyOracle(target(), observed([warning], { status: 0 }))), []);
  assert.deepEqual(kinds(classifyOracle(target(), observed([warning], { status: 1 }))), ["exit"]);
  const incomplete = observed([finding("duplicate-matcher")], { status: 1 });
  incomplete.report.corpus.incomplete = true;
  assert.deepEqual(kinds(classifyOracle(target({ oracle: oracle({ complete: false }) }), incomplete)), ["exit"]);
  incomplete.status = 2;
  assert.deepEqual(kinds(classifyOracle(target({ oracle: oracle({ complete: false }) }), incomplete)), []);
});

/** Checks a report without a summary or findings list is unscored with a controlled cause. */
test("an unmodeled report shape is unscored", () => {
  for (const mutate of [(report) => delete report.summary, (report) => delete report.findings, (report) => { report.findings = {}; }]) {
    const run = observed([finding("duplicate-matcher")]);
    mutate(run.report);
    const result = classifyOracle(target(), run);
    assert.equal(result.observable, false);
    assert.match(result.runFailure, /unmodeled report/);
  }
});

/** Checks without ownership each expectation counts independently as the paired gate does, and with it an overlap is an anomaly. */
test("overlap is independent without ownership", () => {
  const broad = oracle({ requiredFindings: [{ rule: "duplicate-matcher" }, { rule: "duplicate-matcher", primaryLine: 1 }] });
  const lane = classifyOracle(target({ corpus: "language-parity", lane: "ruby", ownership: false, oracle: broad }), observed([finding("duplicate-matcher")]));
  assert.deepEqual([lane.rules["duplicate-matcher"].tp, lane.rules["duplicate-matcher"].fn, lane.entries.map((entry) => entry.kind)], [2, 0, []]);
  const owned = classifyOracle(target({ oracle: broad }), observed([finding("duplicate-matcher")]));
  assert.deepEqual(owned.entries.map((entry) => entry.kind), ["overlapping-expectations", "missing", "missing"]);
});

/** Checks a forbidden finding no expectation owns is one false positive without ownership. */
test("a forbidden finding is a false positive without ownership", () => {
  const result = classifyOracle(target({ ownership: false }), observed([finding("duplicate-matcher"), finding("near-duplicate-step", 3)]));
  assert.equal(result.rules["near-duplicate-step"].fp, 1);
  assert.equal(classifyOracle(target({ ownership: false }), observed([finding("duplicate-matcher")])).rules["near-duplicate-step"].fp, 0);
});

/** Checks a signal-killed run names the signal. */
test("a run killed by a signal names it", () => {
  const result = classifyOracle(target(), observed([], { status: null, signal: "SIGSEGV", report: null }));
  assert.match(result.runFailure, /killed by SIGSEGV/);
});

/** Checks a report without the completeness flag is unscored, and an invalid report names its parse error. */
test("a missing completeness flag and an invalid report are unscored with their cause", () => {
  const flagless = observed([finding("duplicate-matcher")]);
  delete flagless.report.corpus;
  assert.match(classifyOracle(target(), flagless).runFailure, /no corpus\.incomplete flag/);
  const invalid = classifyOracle(target(), observed([], { report: null, reportError: "Unexpected end of JSON input", stderr: "crashed" }));
  assert.match(invalid.runFailure, /exit 1 wrote an invalid JSON report \(Unexpected end of JSON input\): crashed/);
});

/** Checks a rule may be required at one location and absent at another, scoring each finding once in both modes. */
test("a rule required in one place and absent in another is scored per finding", () => {
  const split = oracle({ requiredFindings: [{ rule: "duplicate-matcher", primaryLine: 1 }], requiredAbsent: [{ rule: "duplicate-matcher", primaryLine: 2 }] });
  for (const ownership of [true, false]) {
    /** The rule counts for findings at the given lines. */
    const at = (...lines) => classifyOracle(target({ ownership, oracle: split }), observed(lines.map((line) => finding("duplicate-matcher", line)))).rules["duplicate-matcher"];
    assert.deepEqual([at(1).tp, at(1).fp, at(1).tn], [1, 0, 1], `ownership ${ownership}: only the required finding`);
    assert.deepEqual([at(1, 2).tp, at(1, 2).fp, at(1, 2).tn], [1, 1, 0], `ownership ${ownership}: the forbidden one is one false positive`);
  }
  // A finding both required and forbidden is a contradiction the gate fails: it is a false positive too.
  const both = oracle({ requiredFindings: [{ rule: "duplicate-matcher" }], requiredAbsent: [{ rule: "duplicate-matcher", primaryLine: 1 }] });
  const clash = classifyOracle(target({ oracle: both }), observed([finding("duplicate-matcher", 1)])).rules["duplicate-matcher"];
  assert.deepEqual([clash.tp, clash.fp], [1, 1]);
  // Two absences matching one finding count it once.
  const twice = oracle({ requiredFindings: [], requiredAbsent: [{ rule: "duplicate-matcher" }, { rule: "duplicate-matcher", primaryLine: 5 }] });
  assert.equal(classifyOracle(target({ ownership: false, oracle: twice }), observed([finding("duplicate-matcher", 5)])).rules["duplicate-matcher"].fp, 1);
  // An excess finding that is also forbidden is not counted twice.
  const excess = classifyOracle(target({ ownership: false, oracle: both }), observed([finding("duplicate-matcher", 2), finding("duplicate-matcher", 1)])).rules["duplicate-matcher"];
  assert.deepEqual([excess.tp, excess.fp], [1, 1]);
});
