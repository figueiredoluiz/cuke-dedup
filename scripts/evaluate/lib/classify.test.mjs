import { test } from "node:test";
import assert from "node:assert/strict";
import { judgeRecallCase } from "../../check/lib/recall-runner.mjs";
import { classifyCase, evaluateCase } from "./classify.mjs";

/** A recall case with one duplicate-matcher expectation, overridable. */
const baseCase = (over = {}) => ({
  name: "case",
  expectedDefinitions: 2,
  expectedFeatureSteps: 0,
  expectedFindings: [{ rule: "duplicate-matcher", matchers: ["a", "a"] }],
  ...over,
});

/** A finding of `rule` over matchers `left` and `right`, at `line`. */
const finding = (rule, left = "a", right = "a", line = 1, over = {}) => ({
  rule,
  suppression: null,
  primary: { path: "steps.ts", line },
  related: [{ path: "steps.ts", line: line + 1 }],
  message: "msg",
  evidence: { comparison: { leftMatcher: left, rightMatcher: right }, matcherSimilarity: 1, handlerSimilarity: 0.5 },
  ...over,
});

/** An observation of a clean run holding `findings` and `records`. */
const observed = (findings, records = []) => ({
  status: 0,
  stderr: "",
  spawnFailure: null,
  report: { summary: { definitionsAnalyzed: 2, featureStepsAnalyzed: 0 }, analysis: { candidateSources: { matcherBlocking: { evaluated: 1 } } }, findings },
  fingerprintsText: records.map((record) => JSON.stringify(record)).join("\n"),
});

/** Checks a case that matches its expectations counts one true positive, one true negative and nothing to review. */
test("a matching case is all true positives and true negatives", () => {
  const result = classifyCase(baseCase({ expectedAbsent: [{ rule: "near-duplicate-step", matchers: ["x", "y"] }] }), observed([finding("duplicate-matcher")]));
  assert.deepEqual(result.rules["duplicate-matcher"], { tp: 1, fp: 0, fn: 0, tn: 0, knownMisses: 0 });
  assert.equal(result.rules["near-duplicate-step"].tn, 1);
  assert.deepEqual(result.entries, []);
});

/** Checks a missing finding is a false negative carrying the participants' digests and the candidate census. */
test("a missing finding is a false negative with its participants", () => {
  const records = [
    { matcher: "a", path: "steps.ts", line: 1, handler: { exact: "h1" } },
    { matcher: "other", path: "steps.ts", line: 9, handler: { exact: "h9" } },
  ];
  const result = classifyCase(baseCase({ expectedFindings: [{ rule: "duplicate-matcher", matchers: ["a", "a"], count: 2 }] }), observed([finding("duplicate-matcher")], records));
  assert.deepEqual(result.rules["duplicate-matcher"], { tp: 1, fp: 0, fn: 1, tn: 0, knownMisses: 0 });
  const [entry] = result.entries;
  assert.equal(entry.kind, "missing");
  assert.equal(entry.missing, 1);
  assert.deepEqual(entry.participants.map((participant) => participant.at), ["steps.ts:1"]);
  assert.deepEqual(entry.candidateSources, { matcherBlocking: { evaluated: 1 } });
});

/** Checks an unowned finding and one beyond its owner's count are false positives, flagged when declared absent. */
test("unowned and excess findings are false positives", () => {
  const absent = { rule: "near-duplicate-step", matchers: ["p", "q"] };
  const result = classifyCase(
    baseCase({ expectedAbsent: [absent] }),
    observed([finding("duplicate-matcher"), finding("duplicate-matcher", "a", "a", 5), finding("near-duplicate-step", "p", "q", 7)]),
  );
  // Two findings share one expectation of count 1: one true positive and one excess.
  assert.equal(result.rules["duplicate-matcher"].tp, 1);
  assert.equal(result.rules["duplicate-matcher"].fp, 1);
  assert.equal(result.rules["near-duplicate-step"].fp, 1);
  assert.deepEqual(result.entries.map((entry) => [entry.kind, entry.declaredAbsent]), [["unowned", true], ["excess", undefined]]);
  assert.equal(result.rules["near-duplicate-step"].tn, 0);
  // The unowned finding already names the violated absence, so the absence is not listed again.
  assert.equal(result.entries.some((entry) => entry.kind === "violated-absence"), false);
});

/** Checks a finding two expectations claim is an anomaly that is counted nowhere. */
test("a finding claimed by two expectations is an anomaly, not a count", () => {
  const result = classifyCase(baseCase({ expectedFindings: [{ rule: "duplicate-matcher" }, { rule: "duplicate-matcher", matchers: ["a", "a"] }] }), observed([finding("duplicate-matcher")]));
  assert.equal(result.entries[0].kind, "overlapping-expectations");
  assert.equal(result.entries[0].owners, 2);
  assert.equal(result.rules["duplicate-matcher"].tp, 0);
  assert.equal(result.rules["duplicate-matcher"].fn, 2);
});

/** Checks known misses count while absent and are reported as fixed when the finding appears. */
test("a known miss counts while absent and is flagged once fixed", () => {
  const miss = { id: "m", reason: "r", rule: "near-duplicate-step", matchers: ["p", "q"] };
  const open = classifyCase(baseCase({ knownMisses: [miss] }), observed([finding("duplicate-matcher")]));
  assert.equal(open.rules["near-duplicate-step"].knownMisses, 1);
  assert.equal(open.entries[0].kind, "known-miss");
  const fixed = classifyCase(baseCase({ knownMisses: [miss] }), observed([finding("duplicate-matcher"), finding("near-duplicate-step", "p", "q", 4)]));
  // Found now: a true positive for the rule, no longer a known miss and not a false positive.
  assert.deepEqual(fixed.rules["near-duplicate-step"], { tp: 1, fp: 0, fn: 0, tn: 0, knownMisses: 0 });
  assert.deepEqual(fixed.entries.map((entry) => entry.kind), ["fixed-known-miss"]);
  // A known miss is one desired finding: reported twice, the second is an excess.
  const twice = classifyCase(baseCase({ knownMisses: [miss] }), observed([finding("duplicate-matcher"), finding("near-duplicate-step", "p", "q", 4), finding("near-duplicate-step", "p", "q", 9)]));
  assert.deepEqual(twice.rules["near-duplicate-step"], { tp: 1, fp: 1, fn: 0, tn: 0, knownMisses: 0 });
  assert.deepEqual(twice.entries.map((entry) => entry.kind).sort(), ["excess", "fixed-known-miss"]);
});

/** Checks known misses that both claim a finding report the overlap instead of letting their order decide. */
test("overlapping known misses are an anomaly, independent of their order", () => {
  const broad = { id: "b", reason: "r", rule: "near-duplicate-step" };
  const narrow = { id: "n", reason: "r", rule: "near-duplicate-step", matchers: ["p", "q"] };
  /** Classifies the case with the given known misses. */
  const run = (knownMisses) => classifyCase(baseCase({ knownMisses }), observed([finding("duplicate-matcher"), finding("near-duplicate-step", "p", "q", 4)]));
  const forward = run([broad, narrow]);
  const reversed = run([narrow, broad]);
  assert.deepEqual(forward.rules, reversed.rules);
  assert.equal(forward.entries.filter((entry) => entry.kind === "overlapping-expectations").length, 1);
  assert.equal(forward.rules["near-duplicate-step"].tp, 0);
});

/** Checks a finding claimed by a positive expectation and a known miss is an anomaly, not a silent attribution. */
test("a finding claimed by an expectation and a known miss is an anomaly", () => {
  /** Classifies the case with the given positive expectations. */
  const classify = (expectedFindings) => classifyCase(
    baseCase({ expectedFindings, knownMisses: [{ id: "m", reason: "r", rule: "duplicate-matcher", matchers: ["a", "a"] }] }),
    observed([finding("duplicate-matcher")]),
  );
  const overlapping = classify([{ rule: "duplicate-matcher" }]);
  // Neither claimant is credited: the anomaly is listed and both stay unresolved.
  assert.deepEqual(overlapping.entries.map((entry) => entry.kind).sort(), ["known-miss", "missing", "overlapping-expectations"]);
  assert.deepEqual(overlapping.rules["duplicate-matcher"], { tp: 0, fp: 0, fn: 1, tn: 0, knownMisses: 1 });
  // Control: with an expectation naming a different pair, the same known miss is recovered normally.
  const apart = classify([{ rule: "duplicate-matcher", matchers: ["z", "z"] }]);
  assert.equal(apart.rules["duplicate-matcher"].tp, 1);
  assert.deepEqual(apart.entries.map((entry) => entry.kind).sort(), ["fixed-known-miss", "missing"]);
});

/** Checks a violated absence is listed and not counted as a true negative, and suppressed findings are ignored. */
test("a violated absence is listed, and suppressed findings do not count", () => {
  const result = classifyCase(
    baseCase({ expectedAbsent: [{ rule: "duplicate-matcher", matchers: ["a", "a"] }] }),
    observed([finding("duplicate-matcher"), finding("duplicate-matcher", "z", "z", 8, { suppression: { reason: "x" } })]),
  );
  assert.equal(result.rules["duplicate-matcher"].tn, 0);
  assert.deepEqual(result.entries.map((entry) => entry.kind), ["violated-absence"]);
  assert.equal(result.rules["duplicate-matcher"].fp, 0);
});

/** Checks ambiguous findings are set aside, counted apart from false positives. */
test("ambiguous findings are set aside", () => {
  const ambiguous = [{ rule: "near-duplicate-step", primaryPath: "steps.ts", primaryLine: 7, relatedLocations: ["steps.ts:8"], reason: "r" }];
  const result = classifyCase(baseCase({ expectedAmbiguous: ambiguous }), observed([finding("duplicate-matcher"), finding("near-duplicate-step", "p", "q", 7)]));
  assert.equal(result.ambiguous, 1);
  assert.equal(result.rules["near-duplicate-step"], undefined);
  assert.deepEqual(result.entries, []);
});

/** Checks a run with no report is unobservable with its cause, and nothing is classified. */
test("a run that wrote no report is unobservable", () => {
  const none = { status: 2, stderr: "bad", spawnFailure: null, report: null, fingerprintsText: null };
  assert.match(classifyCase(baseCase(), none).runFailure, /exit 2 wrote no JSON report: bad/);
  assert.equal(classifyCase(baseCase(), { ...none, status: null, spawnFailure: "spawn ENOENT" }).runFailure, "spawn ENOENT");
  assert.equal(classifyCase(baseCase(), none).observable, false);
  // A process error with a report and an allowed exit is unobservable; the control without one is scored.
  assert.equal(classifyCase(baseCase(), { ...observed([]), spawnFailure: "spawnSync ENOBUFS" }).runFailure, "spawnSync ENOBUFS");
  assert.equal(classifyCase(baseCase(), observed([])).observable, true);
  assert.deepEqual(classifyCase(baseCase(), none).rules, {});
});

/** Checks the classification agrees with the gate: its true positives and known misses are the gate's totals. */
test("true positives and known misses equal the totals the gate computes", async () => {
  const testCase = {
    ...baseCase({ knownMisses: [{ id: "m", reason: "r", rule: "near-duplicate-step", matchers: ["p", "q"] }] }),
    language: "ecmascript", frameworks: ["cucumber-js"], role: "positive",
  };
  const observation = observed([finding("duplicate-matcher")], [
    { domain: "ecmascript", framework: "cucumber-js", matcher: "a" },
    { domain: "ecmascript", framework: "cucumber-js", matcher: "b" },
  ]);
  const totals = { detected: 0, knownMisses: 0, unisolated: 0 };
  await judgeRecallCase(testCase, observation, { totals, knownIds: new Set(), attemptCheck: (_label, check) => check() });
  const { rules } = classifyCase(testCase, observation);
  /** The corpus-wide sum of one count. */
  const sum = (key) => Object.values(rules).reduce((total, counts) => total + counts[key], 0);
  assert.equal(sum("tp"), totals.detected);
  assert.equal(sum("knownMisses"), totals.knownMisses);
  assert.equal(sum("fp") + sum("fn"), 0);
});

/** Checks a tool failure after a report was written is unobservable, while finding-dependent exits are not. */
test("an operational exit status is unobservable and a finding-dependent one is not", () => {
  /** An observation of the base findings with the given exit status. */
  const withStatus = (status) => ({ ...observed([finding("duplicate-matcher")]), status, stderr: "boom" });
  assert.equal(classifyCase(baseCase(), withStatus(2)).observable, false);
  assert.match(classifyCase(baseCase(), withStatus(2)).runFailure, /exit 2: boom/);
  assert.equal(classifyCase(baseCase(), withStatus(null)).observable, false);
  for (const status of [0, 1]) assert.equal(classifyCase(baseCase(), withStatus(status)).observable, true);
  // A case that declares exit 2 expects it.
  assert.equal(classifyCase(baseCase({ expectedExit: 2 }), withStatus(2)).observable, true);
});

/** Checks an observation that throws leaves that case unobservable and does not stop the next one. */
test("a throwing observation becomes an unobservable case", async () => {
  const failed = await evaluateCase(baseCase(), async () => { throw new Error("Unexpected end of JSON input"); });
  assert.equal(failed.observable, false);
  assert.match(failed.runFailure, /observation failed: Unexpected end of JSON input/);
  const fine = await evaluateCase(baseCase(), async () => observed([finding("duplicate-matcher")]));
  assert.equal(fine.rules["duplicate-matcher"].tp, 1);
});

/** A fingerprint record with every field an isolation reads. */
const fingerprint = (line, matcher, handler = {}) => ({
  schemaVersion: "1", path: "steps.ts", line, column: 1, domain: "ecmascript", framework: "cucumber-js", registration: "Given",
  matcher, matcherKind: "cucumber-expression", matcherFlags: "", normalizedMatcher: matcher,
  handler: { exact: "e", normalized: "n", alphaNormalized: "a", structural: "s", behavior: "b", comparable: true, trivial: false, ...handler },
});

/** Checks a run that analyzed other inputs than declared is unobservable, with each mismatch named. */
test("an extraction mismatch makes the case unobservable instead of scoring it", () => {
  /** The base observation with its report summary and analysis overridden. */
  const withSummary = (summary, analysis = {}) => {
    const run = observed([finding("duplicate-matcher")]);
    return { ...run, report: { ...run.report, summary: { ...run.report.summary, ...summary }, analysis: { candidateSources: {}, ...analysis } } };
  };
  assert.match(classifyCase(baseCase(), withSummary({ definitionsAnalyzed: 1 })).runFailure, /definitions 1, expected 2/);
  assert.match(classifyCase(baseCase(), withSummary({ featureStepsAnalyzed: 3 })).runFailure, /feature steps 3, expected 0/);
  const sources = baseCase({ expectedCandidateSources: { normalizedMatcher: 4 } });
  assert.match(classifyCase(sources, withSummary({}, { candidateSources: { normalizedMatcher: { evaluated: 2 } } })).runFailure, /normalizedMatcher candidates 2, expected 4/);
  // Controls: the matching run is scored, so each result above is caused by its own mismatch.
  assert.equal(classifyCase(baseCase(), withSummary({})).observable, true);
  assert.equal(classifyCase(sources, withSummary({}, { candidateSources: { normalizedMatcher: { evaluated: 4 } } })).observable, true);
});

/** Checks a true negative needs its declared isolation to hold, and a failing isolation is listed instead. */
test("a declared isolation must hold before an absence is credited", () => {
  const records = [fingerprint(1, "one"), fingerprint(2, "two", { behavior: "other" })];
  /** A duplicate-handler absence over the two matchers with the given isolation. */
  const absence = (isolation) => ({ rule: "duplicate-handler", matchers: ["one", "two"], isolation });
  // `compared: true` needs a run whose candidate limits skipped nothing.
  const unlimited = { truncated: false, skippedCandidateComparisons: 0, truncatedStructuralClasses: 0, candidateSources: {} };
  /** Classifies the absence with its run's analysis reporting no skipped comparisons. */
  const run = (isolation) => {
    const observation = observed([], records);
    observation.report.analysis = unlimited;
    return classifyCase(baseCase({ expectedFindings: [], expectedAbsent: [absence(isolation)] }), observation);
  };
  const holds = run({ compared: true, differs: ["matcher", "normalizedMatcher", "handler.behavior"] });
  assert.equal(holds.rules["duplicate-handler"].tn, 1);
  assert.deepEqual(holds.entries, []);
  const broken = run({ compared: true, differs: ["matcher"] });
  assert.equal(broken.rules["duplicate-handler"]?.tn ?? 0, 0);
  assert.equal(broken.entries[0].kind, "isolation-failed");
  assert.match(broken.entries[0].error, /declared \[matcher\]/);
  // An absence with no isolation is still credited when the finding is absent.
  assert.equal(classifyCase(baseCase({ expectedFindings: [], expectedAbsent: [{ rule: "duplicate-handler", matchers: ["one", "two"] }] }), observed([], records)).rules["duplicate-handler"].tn, 1);
});

/** Checks recall results name their corpus, and a failed observation keeps the corpus and lane it was asked for. */
test("results carry their corpus and lane through a failed observation", async () => {
  const recall = classifyCase(baseCase(), observed([finding("duplicate-matcher")]));
  assert.deepEqual([recall.corpus, recall.lane], ["recall", null]);
  /** An observation that always fails. */
  const failing = async () => { throw new Error("copy failed"); };
  const defaulted = await evaluateCase({ name: "x" }, failing);
  assert.deepEqual([defaulted.corpus, defaulted.lane, defaulted.observable], ["recall", null, false]);
  const lane = await evaluateCase({ name: "x" }, failing, classifyCase, { corpus: "language-parity", lane: "ruby" });
  assert.deepEqual([lane.corpus, lane.lane, lane.runFailure], ["language-parity", "ruby", "observation failed: copy failed"]);
  // Control: a successful observation is classified by the classifier given, not the default.
  const custom = await evaluateCase({ name: "x" }, async () => 1, (named, value) => ({ named, value }));
  assert.deepEqual(custom, { named: { name: "x" }, value: 1 });
  // A classifier defect is named as such, not blamed on the observation.
  const broken = await evaluateCase({ name: "x" }, async () => 1, () => { throw new Error("bad shape"); }, { corpus: "behavior" });
  assert.deepEqual([broken.corpus, broken.observable, broken.runFailure], ["behavior", false, "classification failed: bad shape"]);
});
