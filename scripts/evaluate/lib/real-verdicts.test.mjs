import { test } from "node:test";
import assert from "node:assert/strict";
import { buildLabels, labelsPathProblem, precisionReport, renderPrecision, validateLabels } from "./real-verdicts.mjs";

/** A 16-hex id from a number. */
const id = (n) => n.toString(16).padStart(16, "0");

/** A snapshot with `count` findings for each named rule. */
const snapshotOf = (counts, suite = "s") => ({
  suite,
  commit: "a".repeat(40),
  findings: Object.fromEntries(Object.entries(counts).map(([rule, count], r) => [rule, Array.from({ length: count }, (_, n) => ({ id: id(r * 1000 + n), fp: id(0) }))])),
});

/** Locations for every id in a snapshot. */
const locationsOf = (snapshot) => new Map(Object.values(snapshot.findings).flat().map((found) => [found.id, [`f.ts:${found.id}:1`]]));

/** Checks the draw is capped per rule, reproducible, seed dependent and never carries a verdict. */
test("buildLabels draws a reproducible capped sample with no verdicts", () => {
  const snapshot = snapshotOf({ "duplicate-handler": 20, "near-duplicate-step": 2 });
  /** A draw from the snapshot with the given seed and cap. */
  const draw = (seed, perRule = 5) => buildLabels(snapshot, locationsOf(snapshot), { perRule, seed });
  const labels = draw("1");
  assert.deepEqual(draw("1"), labels);
  assert.equal(labels.samples.filter((sample) => sample.rule === "duplicate-handler").length, 5);
  assert.equal(labels.samples.filter((sample) => sample.rule === "near-duplicate-step").length, 2);
  assert.ok(labels.samples.every((sample) => sample.verdict === null && sample.note === "" && sample.locations.length === 1));
  assert.deepEqual([labels.suite, labels.commit, labels.seed, labels.perRule], ["s", "a".repeat(40), "1", 5]);
  const other = draw("2").samples.filter((sample) => sample.rule === "duplicate-handler").map((sample) => sample.id);
  assert.notDeepEqual(other, labels.samples.filter((sample) => sample.rule === "duplicate-handler").map((sample) => sample.id));
  // Order of the snapshot's findings does not change the draw.
  const reversed = { ...snapshot, findings: Object.fromEntries(Object.entries(snapshot.findings).map(([rule, list]) => [rule, [...list].reverse()])) };
  assert.deepEqual(buildLabels(reversed, locationsOf(reversed), { perRule: 5, seed: "1" }).samples.map((sample) => sample.id).sort(), labels.samples.map((sample) => sample.id).sort());
  for (const bad of [0, -1, 1.5, Number.NaN]) assert.throws(() => draw("1", bad), /positive integer/);
});

/** Checks labels are written only to a fresh path that cannot be committed. */
test("labelsPathProblem refuses an existing or committable path", () => {
  assert.equal(labelsPathProblem("p", { exists: false, ignored: true }), null);
  assert.match(labelsPathProblem("p", { exists: true, ignored: true }), /may hold reviewed verdicts/);
  assert.match(labelsPathProblem("p", { exists: false, ignored: false }), /not ignored by git/);
});

/** Labels for the snapshot below, overridable. */
const labelsFor = (snapshot, over = {}, samples) => ({
  schemaVersion: 1,
  suite: snapshot.suite,
  commit: snapshot.commit,
  seed: "1",
  perRule: 1,
  samples: samples ?? [{ id: id(0), rule: "duplicate-handler", locations: [], verdict: null, note: "" }],
  ...over,
});

/** Checks each way a labels file can disagree with the snapshot it labels. */
test("validateLabels fails closed on stale, foreign and malformed labels", () => {
  // One finding and one sample per rule, so the draw is the whole rule.
  const snapshot = snapshotOf({ "duplicate-handler": 1 });
  assert.equal(validateLabels(labelsFor(snapshot), snapshot).length, 1);
  /** The samples of labels carrying `verdict`, validated against the snapshot. */
  const ok = (verdict) => validateLabels(labelsFor(snapshot, {}, [{ id: id(0), rule: "duplicate-handler", verdict }]), snapshot);
  for (const verdict of ["true-positive", "false-positive", "unclear", null]) assert.equal(ok(verdict).length, 1);
  for (const verdict of ["maybe", "TP", "", undefined, true]) assert.throws(() => ok(verdict), /verdict/, String(verdict));
  assert.throws(() => validateLabels(labelsFor(snapshot, { commit: "b".repeat(40) }), snapshot), /revision/);
  assert.throws(() => validateLabels(labelsFor(snapshot, { suite: "other" }), snapshot), /suite other/);
  assert.throws(() => validateLabels(labelsFor(snapshot, { schemaVersion: 2 }), snapshot), /schemaVersion/);
  assert.throws(() => validateLabels(labelsFor(snapshot, { samples: "x" }), snapshot), /samples must be an array/);
  for (const bad of [null, 3, "x"]) assert.throws(() => validateLabels(labelsFor(snapshot, {}, [bad]), snapshot), /is not an object/);
  const duplicate = [{ id: id(0), rule: "duplicate-handler", verdict: null }, { id: id(0), rule: "duplicate-handler", verdict: null }];
  assert.throws(() => validateLabels(labelsFor(snapshot, {}, duplicate), snapshot), /duplicate sample/);
  assert.throws(() => validateLabels(labelsFor(snapshot, {}, [{ id: id(99), rule: "duplicate-handler", verdict: null }]), snapshot), /not a duplicate-handler finding/);
  assert.throws(() => validateLabels(labelsFor(snapshot, {}, [{ id: id(0), rule: "near-duplicate-step", verdict: null }]), snapshot), /not a near-duplicate-step finding/);
  // Inherited property names are not rules.
  for (const rule of ["constructor", "__proto__", "toString"]) assert.throws(() => validateLabels(labelsFor(snapshot, {}, [{ id: id(0), rule, verdict: null }]), snapshot), /finding in the snapshot/, rule);
});

/** Checks a labels file whose samples no longer match the draw it records is refused, so dropping inconvenient samples cannot raise precision. */
test("validateLabels re-derives the draw", () => {
  const snapshot = snapshotOf({ "duplicate-handler": 6, "near-duplicate-step": 2 });
  const drawn = buildLabels(snapshot, locationsOf(snapshot), { perRule: 3, seed: "1" });
  assert.equal(validateLabels(drawn, snapshot).length, 5);
  /** The drawn labels without the samples `predicate` selects. */
  const without = (predicate) => ({ ...drawn, samples: drawn.samples.filter((sample) => !predicate(sample)) });
  const dropped = drawn.samples.find((sample) => sample.rule === "duplicate-handler");
  assert.throws(() => validateLabels(without((sample) => sample.id === dropped.id), snapshot), /duplicate-handler differ from the draw/);
  assert.throws(() => validateLabels(without((sample) => sample.rule === "near-duplicate-step"), snapshot), /near-duplicate-step differ from the draw/);
  const spare = snapshot.findings["duplicate-handler"].find((entry) => !drawn.samples.some((sample) => sample.id === entry.id));
  const swapped = { ...drawn, samples: drawn.samples.map((sample) => (sample.id === dropped.id ? { ...sample, id: spare.id } : sample)) };
  assert.throws(() => validateLabels(swapped, snapshot), /differ from the draw/);
  assert.throws(() => validateLabels({ ...drawn, perRule: 1 }, snapshot), /differ from the draw/);
  for (const bad of [{ perRule: undefined }, { perRule: 0 }, { seed: 1 }]) assert.throws(() => validateLabels({ ...drawn, ...bad }, snapshot), /perRule and seed/);
  // Verdicts and notes are free to change.
  assert.equal(validateLabels({ ...drawn, samples: drawn.samples.map((sample) => ({ ...sample, verdict: "unclear", note: "x" })) }, snapshot).length, 5);
});

/** Samples of `rule` with the given verdict counts. */
const samples = (rule, { tp = 0, fp = 0, unclear = 0, none = 0 }) => [
  ...Array(tp).fill("true-positive"),
  ...Array(fp).fill("false-positive"),
  ...Array(unclear).fill("unclear"),
  ...Array(none).fill(null),
].map((verdict) => ({ rule, verdict }));

/** Checks precision uses reviewed true and false positives only, and an unreviewed rule has no ratio. */
test("precisionReport counts reviewed verdicts only", () => {
  const { suites, pooled } = precisionReport([
    {
      suite: "a",
      samples: [
        ...samples("duplicate-handler", { tp: 3, fp: 1, unclear: 2, none: 4 }),
        ...samples("near-duplicate-step", { none: 5 }),
        ...samples("overlapping-matcher", { fp: 2 }),
        ...samples("duplicate-matcher", { tp: 2 }),
      ],
    },
    { suite: "b", samples: samples("duplicate-handler", { tp: 1, fp: 3 }) },
  ]);
  /** The row of `rule` in `rows`. */
  const row = (rule, rows = suites[0].rows) => rows.find((candidate) => candidate.rule === rule);
  assert.deepEqual(row("duplicate-handler"), { rule: "duplicate-handler", sampled: 10, tp: 3, fp: 1, unclear: 2, unreviewed: 4, reviewed: 4, precision: 0.75 });
  // Nothing reviewed is no ratio, not 0 and not 1; all false positives is 0; all true positives is 1.
  assert.equal(row("near-duplicate-step").precision, null);
  assert.equal(row("near-duplicate-step").reviewed, 0);
  assert.equal(row("overlapping-matcher").precision, 0);
  assert.equal(row("duplicate-matcher").precision, 1);
  assert.deepEqual(suites[0].rows.map((candidate) => candidate.rule), ["duplicate-handler", "duplicate-matcher", "near-duplicate-step", "overlapping-matcher"]);
  assert.equal(row("duplicate-handler", suites[1].rows).precision, 0.25);
  assert.deepEqual([row("duplicate-handler", pooled).tp, row("duplicate-handler", pooled).fp, row("duplicate-handler", pooled).precision], [4, 4, 0.5]);
});

/** Checks every ratio prints beside its sample size and a missing ratio prints as n/a. */
test("renderPrecision prints the sample size next to every ratio", () => {
  const report = precisionReport([
    { suite: "a", samples: [...samples("duplicate-handler", { tp: 1, fp: 2 }), ...samples("near-duplicate-step", { none: 3 })] },
  ]);
  const text = renderPrecision(report);
  assert.match(text, /\| duplicate-handler \| 3 \| 1 \| 2 \| 0 \| 0 \| 33\.3% \(n=3\) \|/);
  assert.match(text, /\| near-duplicate-step \| 3 \| 0 \| 0 \| 0 \| 3 \| n\/a \(n=0\) \|/);
  assert.ok(!text.includes("all suites"));
  assert.match(renderPrecision(precisionReport([{ suite: "a", samples: [] }, { suite: "b", samples: [] }])), /### all suites/);
});
