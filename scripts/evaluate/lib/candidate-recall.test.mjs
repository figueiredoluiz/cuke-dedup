import { test } from "node:test";
import assert from "node:assert/strict";
import { aggregateCandidateRecall, candidateRecallForCase, recallForCase, renderCandidateMarkdown } from "./candidate-recall.mjs";

/** A recall case expecting one duplicate-matcher pair over matchers `a` and `a`, overridable. */
const baseCase = (over = {}) => ({
  name: "case",
  expectedFindings: [{ rule: "duplicate-matcher", matchers: ["a", "a"] }],
  ...over,
});

/** A pair finding of `rule` between `steps.ts:first` and `steps.ts:second` (column 1), plus extra related lines. */
const finding = (rule, first, second, extra = []) => ({
  rule,
  suppression: null,
  primary: { path: "steps.ts", line: first, column: 1 },
  related: [second, ...extra].map((line) => ({ path: "steps.ts", line, column: 1 })),
  message: "",
  evidence: { comparison: { leftMatcher: "a", rightMatcher: "a" } },
});

/** A pair record between two `steps.ts` lines (column 1), proposed by `sources` and producing `rules`. */
const pair = (left, right, sources = ["normalizedMatcher"], rules = ["duplicate-matcher"]) => ({
  left: { path: "steps.ts", line: left, column: 1 },
  right: { path: "steps.ts", line: right, column: 1 },
  candidateSources: sources,
  rules,
});

/** A fingerprint record for a definition with `matcher` at `steps.ts:line` and `column`. */
const definition = (line, matcher, column = 1) => ({ path: "steps.ts", line, column, matcher });

/** An observation of a clean run with the given findings, pair records and definitions. */
const observed = (findings, pairs = [], definitions = []) => ({
  status: 0,
  stderr: "",
  spawnFailure: null,
  report: { findings, analysis: { truncated: false, skippedCandidateComparisons: 0, truncatedStructuralClasses: 0 } },
  fingerprintsText: definitions.map((record) => JSON.stringify(record)).join("\n"),
  pairsText: pairs.map((record) => JSON.stringify(record)).join("\n"),
});

/** Checks a reported pair finding is joined to its record, with every source that proposed it. */
test("a reported pair reached verification through the sources that proposed it", () => {
  const result = candidateRecallForCase(baseCase(), observed([finding("duplicate-matcher", 1, 2)], [pair(2, 1, ["identicalHandler", "matcherBlocking"])]));
  assert.deepEqual(result.positives, [{
    case: "case", rule: "duplicate-matcher", cluster: false, at: ["steps.ts:1:1", "steps.ts:2:1"], reached: true, sources: ["identicalHandler", "matcherBlocking"],
  }]);
  assert.deepEqual(result.discarded, []);
  // Control: with no record for the pair, the same finding did not reach verification.
  assert.equal(candidateRecallForCase(baseCase(), observed([finding("duplicate-matcher", 1, 2)], [pair(1, 9)])).positives[0].reached, false);
});

/** Checks a cluster finding is joined through the pairs among its members. */
test("a cluster finding reaches verification through its member pairs", () => {
  const cluster = finding("duplicate-matcher", 1, 2, [3, 4, 5]);
  const result = candidateRecallForCase(baseCase(), observed([cluster], [pair(1, 2), pair(2, 3, ["structuralHandler"]), pair(8, 9, ["matcherBlocking"])]));
  assert.equal(result.positives[0].cluster, true);
  assert.deepEqual(result.positives[0].sources, ["normalizedMatcher", "structuralHandler"]);
  assert.equal(candidateRecallForCase(baseCase(), observed([cluster], [pair(8, 9)])).positives[0].reached, false);
});

/** Checks a cluster is credited only through pairs that produced its own rule. */
test("a cluster is credited only through the pairs of its own rule", () => {
  const cluster = finding("duplicate-handler", 1, 2, [3]);
  const result = candidateRecallForCase(
    baseCase({ expectedFindings: [{ rule: "duplicate-handler" }] }),
    observed([cluster], [pair(1, 2, ["identicalHandler"], ["duplicate-handler"]), pair(2, 3, ["normalizedMatcher"], ["duplicate-matcher"])]),
  );
  assert.deepEqual(result.positives[0].sources, ["identicalHandler"]);
  // Control: a pair that also produced the cluster's rule is credited.
  const both = candidateRecallForCase(
    baseCase({ expectedFindings: [{ rule: "duplicate-handler" }] }),
    observed([cluster], [pair(1, 2, ["identicalHandler"], ["duplicate-handler"]), pair(2, 3, ["normalizedMatcher"], ["duplicate-matcher", "duplicate-handler"])]),
  );
  assert.deepEqual(both.positives[0].sources, ["identicalHandler", "normalizedMatcher"]);
});

/** Checks registrations sharing a line are told apart by column, so each finding gets its own pair's sources. */
test("definitions on one line are joined by column", () => {
  /** A `steps.ts` line 1 location at `column`. */
  const at = (column) => ({ path: "steps.ts", line: 1, column });
  const first = { ...finding("duplicate-matcher", 1, 1), primary: at(1), related: [at(10)] };
  const second = { ...finding("duplicate-matcher", 1, 1), primary: at(1), related: [at(20)] };
  /** A pair-record endpoint on line 1 at `column`. */
  const side = (column) => ({ path: "steps.ts", line: 1, column });
  const pairs = [
    { left: side(1), right: side(10), candidateSources: ["normalizedMatcher"], rules: ["duplicate-matcher"] },
    { left: side(1), right: side(20), candidateSources: ["identicalHandler"], rules: ["duplicate-matcher"] },
  ];
  const result = candidateRecallForCase(baseCase({ expectedFindings: [{ rule: "duplicate-matcher", count: 2 }] }), observed([first, second], pairs));
  assert.deepEqual(result.positives.map((positive) => positive.sources), [["normalizedMatcher"], ["identicalHandler"]]);
});

/** Checks usage rules, suppressed findings and unowned findings are not positives. */
test("only owned, active pair findings are positives", () => {
  const owned = finding("duplicate-matcher", 1, 2);
  const result = candidateRecallForCase(
    baseCase(),
    observed([
      owned,
      finding("ambiguous-step", 3, 4),
      { ...finding("duplicate-matcher", 5, 6), suppression: { reason: "x" } },
      finding("near-duplicate-step", 7, 8),
    ], [pair(1, 2)]),
  );
  assert.deepEqual(result.positives.map((positive) => positive.at), [["steps.ts:1:1", "steps.ts:2:1"]]);
});

/** Checks a missing pair is not blamed on candidate generation when a limit may have skipped it. */
test("a limit on the run turns never-proposed into skipped-by-limit", () => {
  /** The base observation with its report analysis replaced. */
  const withAnalysis = (analysis) => {
    const run = observed([], [], [definition(1, "a"), definition(2, "a")]);
    return { ...run, report: { ...run.report, analysis } };
  };
  /** The state of the missing pair under the given analysis. */
  const state = (analysis) => candidateRecallForCase(baseCase(), withAnalysis(analysis)).discarded[0].state;
  assert.equal(state({ truncated: false, skippedCandidateComparisons: 0, truncatedStructuralClasses: 0 }), "never-proposed");
  assert.equal(state({ truncated: true, skippedCandidateComparisons: 0, truncatedStructuralClasses: 0 }), "skipped-by-limit");
  assert.equal(state({ truncated: false, skippedCandidateComparisons: 2, truncatedStructuralClasses: 0 }), "skipped-by-limit");
  assert.equal(state({ truncated: false, skippedCandidateComparisons: 0, truncatedStructuralClasses: 1 }), "skipped-by-limit");
  assert.equal(state(undefined), "skipped-by-limit");
});

/** Checks a counted shortfall is diagnosed from pairs other than the ones already reported. */
test("a counted shortfall ignores the pairs already credited", () => {
  const counted = baseCase({ expectedFindings: [{ rule: "duplicate-matcher", matchers: ["a", "a"], count: 2 }] });
  const definitions = [definition(1, "a"), definition(2, "a"), definition(3, "a")];
  // Only the credited pair was evaluated; the second expected pair was never seen.
  const credited = candidateRecallForCase(counted, observed([finding("duplicate-matcher", 1, 2)], [pair(1, 2)], definitions));
  assert.equal(credited.discarded[0].state, "never-proposed");
  // Control: another evaluated pair among the same definitions is "evaluated, no finding".
  const other = candidateRecallForCase(counted, observed([finding("duplicate-matcher", 1, 2)], [pair(1, 2), pair(1, 3)], definitions));
  assert.equal(other.discarded[0].state, "evaluated-no-finding");
});

/** Checks a cluster whose related locations the report truncated is named, not scored from partial membership. */
test("a cluster with truncated related locations is an anomaly, not a positive", () => {
  const truncated = { ...finding("duplicate-matcher", 1, 2, [3, 4, 5]), relatedLocationsTruncated: true };
  const result = candidateRecallForCase(baseCase(), observed([truncated], [pair(1, 2)]));
  assert.deepEqual(result.positives, []);
  assert.equal(result.anomalies[0].reason, "related locations truncated");
  // Control: the same cluster not truncated is joined.
  const whole = candidateRecallForCase(baseCase(), observed([{ ...truncated, relatedLocationsTruncated: false }], [pair(1, 2)]));
  assert.equal(whole.positives[0].reached, true);
});

/** Checks paths the report sanitizes still join to the raw paths in the pair and fingerprint records. */
test("a path the report sanitized joins to its raw record path", () => {
  const raw = "ste\u200bps.ts";
  const sanitized = { ...finding("duplicate-matcher", 1, 2) };
  const rawPair = { left: { path: raw, line: 1, column: 1 }, right: { path: raw, line: 2, column: 1 }, candidateSources: ["identicalHandler"], rules: ["duplicate-matcher"] };
  const joined = candidateRecallForCase(baseCase(), observed([sanitized], [rawPair]));
  assert.deepEqual(joined.positives[0].sources, ["identicalHandler"]);
  // Control: a path that only looks similar is not joined, and tabs and line breaks are kept as the report keeps them.
  const different = { ...rawPair, left: { ...rawPair.left, path: "other.ts" } };
  assert.equal(candidateRecallForCase(baseCase(), observed([sanitized], [different])).positives[0].reached, false);
  const located = baseCase({ expectedFindings: [{ rule: "duplicate-matcher", primaryPath: "steps.ts", primaryLine: 1, relatedLocations: ["steps.ts:2"] }] });
  const result = candidateRecallForCase(located, observed([], [rawPair], [{ path: raw, line: 1, column: 1, matcher: "a" }, { path: raw, line: 2, column: 1, matcher: "a" }]));
  assert.equal(result.discarded[0].state, "evaluated-no-finding");
});

/** Checks a pair credited under one rule is not evidence about the same pair expected under another. */
test("a credited pair is scoped to its rule when diagnosing a shortfall", () => {
  const dual = baseCase({ expectedFindings: [{ rule: "duplicate-handler", matchers: ["a", "a"] }, { rule: "near-duplicate-step", matchers: ["a", "a"] }] });
  const definitions = [definition(1, "a"), definition(2, "a")];
  // Only duplicate-handler was reported; the pair was evaluated, so near-duplicate-step was a verification miss.
  const result = candidateRecallForCase(dual, observed([finding("duplicate-handler", 1, 2)], [pair(1, 2, ["identicalHandler"], ["duplicate-handler"])], definitions));
  assert.deepEqual(result.discarded.map((entry) => [entry.rule, entry.state]), [["near-duplicate-step", "evaluated-no-finding"]]);
});

/** Checks declared ambiguous findings are set aside, and a recovered known miss is a positive. */
test("ambiguous findings are not positives and a recovered known miss is", () => {
  const ambiguous = [{ rule: "duplicate-matcher", primaryPath: "steps.ts", primaryLine: 1, relatedLocations: ["steps.ts:2"], reason: "r" }];
  const broad = baseCase({ expectedFindings: [{ rule: "duplicate-matcher" }], expectedAmbiguous: ambiguous });
  const only = candidateRecallForCase(broad, observed([finding("duplicate-matcher", 1, 2)], [pair(1, 2)]));
  // The ambiguous pair cannot stand in for the required finding, which is still missing.
  assert.deepEqual(only.positives, []);
  assert.equal(only.discarded[0].kind, "expected");
  const miss = baseCase({ expectedFindings: [], knownMisses: [{ id: "m", reason: "r", rule: "duplicate-matcher", matchers: ["a", "a"] }] });
  const recovered = candidateRecallForCase(miss, observed([finding("duplicate-matcher", 1, 2)], [pair(1, 2, ["identicalHandler"])]));
  assert.deepEqual(recovered.positives.map((positive) => positive.sources), [["identicalHandler"]]);
  assert.deepEqual(recovered.discarded, []);
});

/** Checks a shortfall with no uncredited pair left is an extraction shortfall, not a generation miss. */
test("a counted shortfall with every possible pair credited is a missing definition", () => {
  const counted = baseCase({ expectedFindings: [{ rule: "duplicate-matcher", matchers: ["a", "a"], count: 2 }] });
  // Two definitions extracted of three declared: their one pair is credited and nothing else could be proposed.
  const result = candidateRecallForCase(counted, observed([finding("duplicate-matcher", 1, 2)], [pair(1, 2)], [definition(1, "a"), definition(2, "a")]));
  assert.equal(result.discarded[0].state, "definitions-not-extracted");
});

/** Checks selectors the join cannot apply leave the pair unresolved instead of borrowing an unrelated pair. */
test("selectors the join cannot apply leave the missing pair unresolved", () => {
  const definitions = [definition(1, "a"), definition(2, "a")];
  /** The state of the pair when the expectation carries `selectors` as well as matchers. */
  const state = (selectors) => candidateRecallForCase(
    baseCase({ expectedFindings: [{ rule: "duplicate-matcher", matchers: ["a", "a"], ...selectors }] }),
    observed([], [pair(1, 2)], definitions),
  ).discarded[0].state;
  // Control: matchers alone resolve through the unrelated evaluated pair.
  assert.equal(state({}), "evaluated-no-finding");
  for (const selectors of [{ primaryPath: "steps.ts", primaryLine: 10 }, { primaryPath: "steps.ts" }, { relatedLocations: ["steps.ts:9"] }, { messageIncludes: "x" }, { relatedCount: 3 }, { cluster: true }]) {
    assert.equal(state(selectors), "unresolved", JSON.stringify(selectors));
  }
});

/** Checks a finding two expectations claim is not a positive, so an overlap cannot inflate the counts. */
test("a finding claimed by two expectations is not a positive", () => {
  const overlapping = baseCase({ expectedFindings: [{ rule: "duplicate-matcher" }, { rule: "duplicate-matcher", matchers: ["a", "a"] }] });
  const result = candidateRecallForCase(overlapping, observed([finding("duplicate-matcher", 1, 2)], [pair(1, 2)]));
  assert.deepEqual(result.positives, []);
  // The overlap is named instead of dropped silently.
  assert.deepEqual(result.anomalies, [{ case: "case", rule: "duplicate-matcher", reason: "claimed by 2 expectations", at: ["steps.ts:1:1", "steps.ts:2:1"] }]);
  // Control: one claimant makes it a positive.
  assert.equal(candidateRecallForCase(baseCase(), observed([finding("duplicate-matcher", 1, 2)], [pair(1, 2)])).positives.length, 1);
  assert.deepEqual(candidateRecallForCase(baseCase(), observed([finding("duplicate-matcher", 1, 2)], [pair(1, 2)])).anomalies, []);
});

/** Checks a declared pair with no finding is named with where it stopped, for each stage. */
test("a declared pair with no finding is named with the stage where it stopped", () => {
  /** The state of the declared pair given the extracted definitions and evaluated pairs. */
  const state = (definitions, pairs) => candidateRecallForCase(baseCase(), observed([], pairs, definitions)).discarded[0].state;
  assert.equal(state([], []), "definitions-not-extracted");
  assert.equal(state([definition(1, "a")], []), "definitions-not-extracted");
  assert.equal(state([definition(1, "a"), definition(2, "a")], []), "never-proposed");
  assert.equal(state([definition(1, "a"), definition(2, "a")], [pair(1, 2)]), "evaluated-no-finding");
  const distinct = baseCase({ expectedFindings: [{ rule: "duplicate-matcher", matchers: ["a", "b"] }] });
  assert.equal(candidateRecallForCase(distinct, observed([], [], [definition(1, "a")])).discarded[0].state, "definitions-not-extracted");
  assert.equal(candidateRecallForCase(distinct, observed([], [], [definition(1, "a"), definition(2, "b")])).discarded[0].state, "never-proposed");
  // An expectation naming both locations is resolved at those locations, not at any same-matcher pair.
  const located = baseCase({ expectedFindings: [{ rule: "duplicate-matcher", primaryPath: "steps.ts", primaryLine: 10, relatedLocations: ["steps.ts:20"] }] });
  const elsewhere = [definition(1, "a"), definition(2, "a"), definition(10, "a")];
  assert.equal(candidateRecallForCase(located, observed([], [pair(1, 2)], elsewhere)).discarded[0].state, "definitions-not-extracted");
  assert.equal(candidateRecallForCase(located, observed([], [pair(1, 2)], [...elsewhere, definition(20, "a")])).discarded[0].state, "never-proposed");
  assert.equal(candidateRecallForCase(located, observed([], [pair(10, 20)], [...elsewhere, definition(20, "a")])).discarded[0].state, "evaluated-no-finding");
  const ruleOnly = baseCase({ expectedFindings: [{ rule: "duplicate-matcher" }] });
  assert.equal(candidateRecallForCase(ruleOnly, observed([], [], [])).discarded[0].state, "unresolved");
});

/** Checks a known miss of a pair rule is reported once, and a usage-rule one is ignored. */
test("known misses of pair rules are discarded entries", () => {
  const miss = { id: "m", reason: "r", rule: "near-duplicate-step", matchers: ["p", "q"] };
  const result = candidateRecallForCase(baseCase({ expectedFindings: [], knownMisses: [miss, { ...miss, id: "u", rule: "unused-definition" }] }), observed([], [], []));
  assert.deepEqual(result.discarded.map((entry) => [entry.kind, entry.rule, entry.state]), [["known-miss", "near-duplicate-step", "definitions-not-extracted"]]);
});

/** Checks a count above one stands for several findings, and a shortfall is discarded. */
test("a count above one expects several findings and reports the shortfall", () => {
  const counted = baseCase({ expectedFindings: [{ rule: "duplicate-matcher", count: 2 }] });
  const result = candidateRecallForCase(counted, observed([finding("duplicate-matcher", 1, 2)], [pair(1, 2)]));
  assert.equal(result.positives.length, 1);
  assert.equal(result.discarded[0].missing, 1);
});

/** Checks a run that cannot be joined is unobservable with its cause, and observation errors do not stop the batch. */
test("a run with no report, no pair records or a tool failure is unobservable", async () => {
  const ok = observed([]);
  assert.match(candidateRecallForCase(baseCase(), { ...ok, report: null, status: 2, stderr: "bad" }).runFailure, /exit 2 wrote no JSON report: bad/);
  assert.match(candidateRecallForCase(baseCase(), { ...ok, pairsText: null }).runFailure, /wrote no pair records/);
  assert.match(candidateRecallForCase(baseCase(), { ...ok, status: 2, stderr: "boom" }).runFailure, /exit 2: boom/);
  assert.equal(candidateRecallForCase(baseCase({ expectedExit: 2 }), { ...ok, status: 2 }).observable, true);
  assert.equal(candidateRecallForCase(baseCase(), { ...ok, status: null, spawnFailure: "spawn ENOENT" }).runFailure, "spawn ENOENT");
  const failed = await recallForCase(baseCase(), async () => { throw new Error("bad JSON"); });
  assert.match(failed.runFailure, /observation failed: bad JSON/);
});

/** Checks aggregation sums per rule and source, counts sole proposers, and sets unobservable cases aside. */
test("aggregation sums rules and sources and counts sole proposers", () => {
  /** A positive of `rule` proposed by `sources`. */
  const row = (rule, sources, reached = true) => ({ case: "c", rule, cluster: false, at: [], reached, sources });
  const report = aggregateCandidateRecall([
    { name: "a", observable: true, runFailure: null, positives: [row("duplicate-matcher", ["normalizedMatcher"]), row("near-duplicate-step", ["identicalHandler", "matcherBlocking"]), row("near-duplicate-step", [], false)], discarded: [] },
    { name: "b", observable: true, runFailure: null, positives: [], discarded: [{ case: "b", rule: "duplicate-matcher", kind: "known-miss", state: "never-proposed", expected: { matchers: ["x", "y"] } }] },
    { name: "c", observable: false, runFailure: "boom", positives: [], discarded: [] },
  ]);
  assert.deepEqual(report.rules.map((entry) => [entry.rule, entry.positives, entry.reached, entry.candidateRecall]), [["duplicate-matcher", 1, 1, 1], ["near-duplicate-step", 2, 1, 0.5]]);
  assert.deepEqual(report.totals, { positives: 3, reached: 2, candidateRecall: 0.6667 });
  assert.deepEqual(report.sources, [
    { source: "identicalHandler", proposed: 1, sole: 0 },
    { source: "matcherBlocking", proposed: 1, sole: 0 },
    { source: "normalizedMatcher", proposed: 1, sole: 1 },
  ]);
  assert.deepEqual([report.cases, report.observed, report.unobservable], [3, 2, [{ name: "c", runFailure: "boom" }]]);
});

/** Checks the markdown carries the tables, the unobservable list and the discarded pairs, or "None". */
test("markdown renders the tables, unobservable cases and discarded pairs", () => {
  const empty = renderCandidateMarkdown(aggregateCandidateRecall([{ name: "a", observable: true, runFailure: null, positives: [], discarded: [] }]));
  assert.match(empty, /\| \*\*all\*\* \| 0 \| 0 \| - \|/);
  assert.match(empty, /None\./);
  const full = renderCandidateMarkdown(aggregateCandidateRecall([
    { name: "a", observable: true, runFailure: null, positives: [{ case: "a", rule: "duplicate-matcher", cluster: false, at: [], reached: true, sources: ["normalizedMatcher"] }], discarded: [{ case: "a", rule: "duplicate-matcher", kind: "known-miss", state: "never-proposed", expected: { matchers: ["x", "y"] } }] },
    { name: "b", observable: false, runFailure: "spawn x", positives: [], discarded: [] },
  ]));
  assert.match(full, /\| duplicate-matcher \| 1 \| 1 \| 100\.0% \|/);
  assert.match(full, /\| normalizedMatcher \| 1 \| 1 \|/);
  assert.match(full, /`b`: spawn x/);
  assert.match(full, /`a` duplicate-matcher \*\*known-miss\*\* \(never-proposed\)/);
  assert.match(full, /"reached" is 100% unless the join fails/);
  assert.doesNotMatch(full, /could not be joined/);
  const withAnomaly = renderCandidateMarkdown(aggregateCandidateRecall([{ name: "a", observable: true, runFailure: null, positives: [], discarded: [], anomalies: [{ case: "a", rule: "duplicate-matcher", reason: "claimed by 2 expectations", at: ["steps.ts:1:1"] }] }]));
  assert.match(withAnomaly, /could not be joined[\s\S]*`a` duplicate-matcher claimed by 2 expectations at steps.ts:1:1/);
  assert.doesNotMatch(full, /None\./);
});
