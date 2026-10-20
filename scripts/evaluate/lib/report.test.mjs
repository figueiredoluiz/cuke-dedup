import { test } from "node:test";
import assert from "node:assert/strict";
import { aggregate, metrics, renderMarkdown } from "./report.mjs";

/** An observable classified case with the given per-rule counts and entries. */
const classified = (name, rules, entries = [], over = {}) => ({ corpus: "recall", lane: null, name, observable: true, runFailure: null, rules, entries, ambiguous: 0, ...over });

/** Counts with every field defined. */
const counts = (over) => ({ tp: 0, fp: 0, fn: 0, tn: 0, knownMisses: 0, ...over });

/** Checks precision, recall and F1, with known misses desired and undefined ratios null rather than zero. */
test("metrics count known misses as desired and leave empty denominators undefined", () => {
  assert.deepEqual(metrics(counts({ tp: 3, fp: 1, fn: 0, knownMisses: 1 })), { precision: 0.75, recall: 0.75, f1: 0.75 });
  assert.deepEqual(metrics(counts({ tn: 5 })), { precision: null, recall: null, f1: null });
  assert.deepEqual(metrics(counts({ fp: 2 })), { precision: 0, recall: null, f1: 0 });
  // Nothing detected but something expected or reported is an F1 of zero, not undefined.
  assert.deepEqual(metrics(counts({ tp: 0, fn: 2, fp: 1 })), { precision: 0, recall: 0, f1: 0 });
  // F1 comes from the counts, so it is not rounded twice (2/7, not 2 * 0.1667 * 1 / 1.1667).
  assert.equal(metrics(counts({ tp: 1, fn: 5 })).f1, 0.2857);
});

/** Checks counts sum per rule, rules sort by name, and unobservable cases are listed but not summed. */
test("aggregate sums per rule and sets unobservable cases aside", () => {
  const report = aggregate([
    classified("a", { "near-duplicate-step": counts({ tp: 2, tn: 1 }), "duplicate-matcher": counts({ tp: 1 }) }),
    classified("b", { "duplicate-matcher": counts({ tp: 1, fn: 1 }) }),
    { corpus: "recall", lane: null, name: "c", observable: false, runFailure: "boom", rules: {}, entries: [], ambiguous: 0 },
  ]);
  assert.deepEqual(report.rules.map((row) => [row.rule, row.cases, row.tp, row.fn]), [["duplicate-matcher", 2, 2, 1], ["near-duplicate-step", 1, 2, 0]]);
  assert.equal(report.totals.tp, 4);
  assert.equal(report.totals.tn, 1);
  assert.deepEqual([report.cases, report.observed, report.unobservable], [3, 2, [{ corpus: "recall", lane: null, name: "c", runFailure: "boom" }]]);
});

/** Checks known misses are listed after every other entry and ambiguous findings are summed. */
test("known misses are listed last and ambiguous findings are summed", () => {
  const report = aggregate([
    classified("a", {}, [{ case: "a", rule: "r", kind: "known-miss", expected: {} }], { ambiguous: 2 }),
    classified("b", {}, [{ case: "b", rule: "r", kind: "unowned", observed: { primary: "x:1", matchers: null, message: "m" } }], { ambiguous: 1 }),
  ]);
  assert.deepEqual(report.entries.map((entry) => entry.kind), ["unowned", "known-miss"]);
  assert.equal(report.ambiguous, 3);
});

/** Checks the markdown carries the table, a total row, dashes for undefined ratios and the review list. */
test("markdown renders the table, totals, dashes and entries", () => {
  const empty = renderMarkdown(aggregate([classified("a", { r: counts({ tn: 2 }) })]));
  assert.match(empty, /\| recall \| - \| r \| 1 \| 0 \| 0 \| 0 \| 0 \| 2 \| - \| - \| - \|/);
  assert.match(empty, /\| all \| - \| \*\*all rules\*\* \|/);
  assert.match(empty, /None\./);
  const full = renderMarkdown(aggregate([
    classified("a", { r: counts({ fp: 1 }) }, [{ corpus: "recall", case: "a", rule: "r", kind: "unowned", observed: { primary: "s.ts:1", matchers: ["p", "q"], message: "m" } }]),
    { corpus: "recall", lane: null, name: "b", observable: false, runFailure: "spawn x", rules: {}, entries: [], ambiguous: 0 },
  ]));
  assert.match(full, /recall `a` r \*\*unowned\*\*: s\.ts:1 p \/ q/);
  assert.match(full, /recall `b`: spawn x/);
  assert.doesNotMatch(full, /None\./);
  // An isolation failure names its reason, so the markdown alone explains the uncredited absence.
  const failed = renderMarkdown(aggregate([classified("c", {}, [{ corpus: "recall", case: "c", rule: "r", kind: "isolation-failed", expected: { matchers: ["p", "q"] }, error: "participants differ in [matcher]" }])]));
  assert.match(failed, /\*\*isolation-failed\*\*: \["p","q"\] \(participants differ in \[matcher\]\)/);
  assert.doesNotMatch(full, /\(undefined\)/);
});

/** A classified case of another corpus, optionally with a lane. */
const inCorpus = (corpus, name, rules, lane = null, over = {}) => classified(name, rules, [], { corpus, lane, ...over });

/** Checks corpora stay separate, each row names its corpus and lane, and the totals sum every corpus. */
test("aggregate keeps corpora and lanes apart and totals per rule across all", () => {
  const report = aggregate([
    inCorpus("ruby-parity", "g", { "duplicate-matcher": counts({ tp: 3 }) }),
    inCorpus("recall", "a", { "duplicate-matcher": counts({ tp: 1, fp: 1 }) }),
    inCorpus("language-parity", "p", { "duplicate-matcher": counts({ tp: 1, fn: 1 }) }, "typescript"),
    inCorpus("language-parity", "p", { "duplicate-matcher": counts({ tp: 1 }) }, "ruby"),
    inCorpus("behavior", "b", { "ambiguous-step": counts({ fn: 2 }) }),
  ]);
  assert.deepEqual(report.rules.map((row) => [row.corpus, row.lane, row.rule, row.tp]), [
    ["recall", null, "duplicate-matcher", 1],
    ["behavior", null, "ambiguous-step", 0],
    ["language-parity", "ruby", "duplicate-matcher", 1],
    ["language-parity", "typescript", "duplicate-matcher", 1],
    ["ruby-parity", null, "duplicate-matcher", 3],
  ]);
  assert.deepEqual(report.ruleTotals.map((row) => [row.corpus, row.rule, row.cases, row.tp, row.fp, row.fn]), [
    ["all", "ambiguous-step", 1, 0, 0, 2],
    ["all", "duplicate-matcher", 3, 6, 1, 1],
  ]);
  assert.deepEqual(report.corpora.map((row) => [row.corpus, row.cases, row.totals.tp]), [
    ["recall", 1, 1], ["behavior", 1, 0], ["language-parity", 1, 2], ["ruby-parity", 1, 3],
  ]);
  // One paired case run in two languages is one case; the grand totals count both runs.
  assert.deepEqual([report.cases, report.totals.tp, report.totals.fn], [4, 6, 3]);
});

/** Checks a case named alike in two corpora counts twice and an unobservable lane is listed with its corpus and lane. */
test("same-named cases in two corpora stay distinct and unscored lanes are named", () => {
  const report = aggregate([
    inCorpus("recall", "same", { r: counts({ tp: 1 }) }),
    inCorpus("behavior", "same", { r: counts({ tp: 1 }) }),
    inCorpus("language-parity", "p", {}, "ruby", { observable: false, runFailure: "no report" }),
    inCorpus("language-parity", "p", { r: counts({ tp: 1 }) }, "typescript"),
  ]);
  assert.deepEqual([report.cases, report.observed, report.ruleTotals[0].cases], [3, 2, 3]);
  assert.deepEqual(report.unobservable, [{ corpus: "language-parity", lane: "ruby", name: "p", runFailure: "no report" }]);
  const markdown = renderMarkdown(report);
  assert.match(markdown, /- language-parity \(ruby\) `p`: no report/);
  assert.match(markdown, /\| language-parity \| typescript \| r \|/);
  // Total rows count cases whose every run was scored: the paired case lost a lane, so it is not one.
  assert.match(markdown, /\| language-parity \| - \| \*\*all rules\*\* \| 0 \|/);
  assert.match(markdown, /\| all \| - \| \*\*all rules\*\* \| 2 \|/);
});

/** Checks entries name their corpus and lane, and a detail replaces the finding description. */
test("entries render with corpus, lane and detail", () => {
  const markdown = renderMarkdown(aggregate([
    inCorpus("ruby-parity", "g", {}, null, { entries: [{ corpus: "ruby-parity", lane: null, case: "g", rule: "r", kind: "missing", detail: "expected 2, observed 1" }] }),
  ]));
  assert.match(markdown, /- ruby-parity `g` r \*\*missing\*\*: expected 2, observed 1/);
  assert.match(markdown, /entries to review: 1\./);
});
