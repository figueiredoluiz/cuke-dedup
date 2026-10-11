import assert from "node:assert/strict";
import test from "node:test";
import { regressionFailures, snapshot } from "./parity-regression.mjs";

const outcome = (findings = []) => ({
  definitions: 2,
  complete: true,
  exit: 0,
  findings,
});

const observation = (id, values = {}) => ({
  id,
  outcome: values.outcome ?? outcome(),
  deficits: values.deficits ?? [],
  ...(values.difference === undefined ? {} : { difference: values.difference }),
  ...(values.error === undefined ? {} : { error: values.error }),
});

const entry = (item, status, reason) => ({
  status,
  digest: snapshot({ outcome: item.outcome, deficits: item.deficits }),
  ...(reason === undefined ? {} : { reason }),
});

const manifest = (cases) => ({ schemaVersion: 1, cases });

test("unchanged pass, known-gap and accepted-difference observations are positive controls", () => {
  const items = [
    observation("clean"),
    observation("known-gap", { deficits: ["required finding missing"] }),
    observation("difference", { difference: true }),
  ];
  const baseline = manifest({
    clean: entry(items[0], "pass"),
    "known-gap": entry(items[1], "known-gap", "Existing Ruby outcome gap"),
    difference: entry(items[2], "accepted-difference", "Intentional framework difference"),
  });
  assert.deepEqual(regressionFailures(items, baseline), []);
});

test("changed outcomes and changed independent-oracle deficits fail", () => {
  const baselineItem = observation("case", { deficits: ["missing required finding"] });
  const baseline = manifest({ case: entry(baselineItem, "known-gap", "Observed gap") });
  const changedOutcome = observation("case", {
    outcome: outcome([{ rule: "duplicate-handler", line: 4 }]),
    deficits: baselineItem.deficits,
  });
  const changedDeficit = observation("case", {
    outcome: baselineItem.outcome,
    deficits: ["unexpected finding"],
  });
  const failures = regressionFailures([changedOutcome], baseline);
  assert.ok(failures.some((failure) => failure.includes(baseline.cases.case.digest)
    && failure.includes(snapshot({ outcome: changedOutcome.outcome, deficits: changedOutcome.deficits }))));
  assert.ok(regressionFailures([changedDeficit], baseline).length > 0);
});

test("a known gap that starts passing requires explicit reclassification", () => {
  const failing = observation("case", { deficits: ["required finding missing"] });
  const baseline = manifest({ case: entry(failing, "known-gap", "Existing gap") });
  const improved = observation("case");
  assert.ok(regressionFailures([improved], baseline).some((failure) => failure.includes("disposition changed")));
});

test("missing, new and duplicate observation IDs fail exact inventory matching", () => {
  const first = observation("first");
  const second = observation("second");
  const baseline = manifest({ first: entry(first, "pass"), second: entry(second, "pass") });
  assert.ok(regressionFailures([first], baseline).length > 0);
  assert.ok(regressionFailures([first, second, observation("new")], baseline).length > 0);
  assert.ok(regressionFailures([first, first, second], baseline).length > 0);
});

test("invalid manifests and malformed observations cannot produce a passing gate", () => {
  const item = observation("case");
  const valid = manifest({ case: entry(item, "pass") });
  for (const invalid of [
    { ...valid, schemaVersion: 2 },
    manifest({ case: { ...valid.cases.case, status: "unknown" } }),
    manifest({ case: { ...valid.cases.case, digest: "bad" } }),
    manifest({ case: { status: "known-gap", digest: valid.cases.case.digest } }),
    manifest({ case: { ...valid.cases.case, reasn: "Typo" } }),
    manifest({ case: { ...valid.cases.case, reason: " " } }),
    manifest({}),
  ]) {
    assert.throws(() => regressionFailures([item], invalid));
  }
  for (const malformed of [
    { outcome: item.outcome, deficits: [] },
    { id: "", outcome: item.outcome, deficits: [] },
    { id: "case", deficits: [] },
    { id: "case", outcome: item.outcome, deficits: "none" },
  ]) {
    assert.ok(regressionFailures([malformed], valid).length > 0);
  }
});

test("operational errors always fail, even when their apparent outcome matches a known gap", () => {
  const gap = observation("case", { deficits: ["required finding missing"] });
  const baseline = manifest({ case: entry(gap, "known-gap", "Observed analyzer gap") });
  for (const error of ["launch failure", "timeout", "process crash", "missing report", "invalid report"]) {
    assert.ok(regressionFailures([{ ...gap, error }], baseline).length > 0, error);
  }
});

test("snapshot canonicalizes object keys but preserves array order", () => {
  assert.equal(snapshot({ a: 1, b: { x: 2, y: 3 } }), snapshot({ b: { y: 3, x: 2 }, a: 1 }));
  assert.notEqual(snapshot({ findings: ["first", "second"] }), snapshot({ findings: ["second", "first"] }));
});

test("accepted differences cannot hide oracle deficits or silently become equivalent", () => {
  const difference = observation("case", { difference: true });
  const baseline = manifest({ case: entry(difference, "accepted-difference", "Reviewed difference") });
  assert.ok(regressionFailures([{ ...difference, deficits: ["missing finding"] }], baseline).length);
  assert.ok(regressionFailures([{ ...difference, difference: false }], baseline).length);
});

test("invalid JSON values, empty observations and present empty errors fail closed", () => {
  const item = observation("case");
  const baseline = manifest({ case: entry(item, "pass") });
  assert.ok(regressionFailures([], baseline).length);
  for (const error of ["", undefined, null]) assert.ok(regressionFailures([{ ...item, error }], baseline).length);
  for (const value of [undefined, NaN, Infinity, new Date(), [ , 1]]) {
    assert.throws(() => snapshot({ value }));
    assert.ok(regressionFailures([{ ...item, outcome: { value } }], baseline).length);
  }
});

test("normalized finding snapshots use host-independent path ordering", async () => {
  const { outcome: normalizedOutcome } = await import("./language-parity.mjs");
  const location = (name) => ({ path: `${name}/steps.ts`, line: 1 });
  const report = {
    summary: { definitionsAnalyzed: 2, featureStepsAnalyzed: 0 },
    corpus: { incomplete: false }, findingsTruncated: 0,
    findings: ["ä", "z"].map((name) => ({ suppression: null, rule: "duplicate-handler",
      severity: "warning", primary: location(name), related: [location("ä"), location("z")] })),
  };
  const actual = normalizedOutcome(report, 0);
  assert.deepEqual(actual.findings.map((finding) => finding.primary.path), ["z/steps", "ä/steps"]);
  assert.deepEqual(actual.findings[0].related.map((item) => item.path), ["z/steps", "ä/steps"]);
});
