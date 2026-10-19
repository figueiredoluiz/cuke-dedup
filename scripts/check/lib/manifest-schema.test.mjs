import { test } from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import {
  AXES,
  latestClassifiedAt,
  partitionAmbiguous,
  validateCorpusLabels,
  exitAcceptable,
  validateAgainstRecords,
  validateCaseMetadata,
  validateVariantReferences,
} from "./manifest-schema.mjs";

const TODAY = "2026-10-18";

/** A valid positive case with overridable fields. */
const positive = (over = {}) => ({
  name: "case",
  role: "positive",
  origin: "synthetic",
  language: "ecmascript",
  frameworks: ["cucumber-js"],
  axes: ["import-form"],
  verdict: "reviewed",
  classifiedAt: "2026-10-18",
  expectedFindings: [{ rule: "duplicate-matcher" }],
  ...over,
});

/** A finding of `rule` with the comparison shape `findingMatches` reads. */
const finding = (rule, left = "a", right = "b") => ({
  rule,
  primary: { path: "steps.ts", line: 1 },
  related: [],
  message: "",
  evidence: { comparison: { leftMatcher: left, rightMatcher: right } },
});

/** Checks a complete case passes and each required field is rejected when missing. */
test("a complete case passes and each required field is rejected when missing", () => {
  validateCaseMetadata(positive(), TODAY);
  for (const field of ["role", "origin", "language", "frameworks", "axes", "verdict", "classifiedAt"]) {
    const { [field]: _removed, ...rest } = positive();
    assert.throws(() => validateCaseMetadata(rest, TODAY), new RegExp(field), field);
  }
});

/** Checks vocabulary values outside the set are rejected. */
test("vocabulary values outside the set are rejected", () => {
  for (const [field, value] of [["role", "maybe"], ["origin", "scraped"], ["language", "python"], ["verdict", "approved"]]) {
    assert.throws(() => validateCaseMetadata(positive({ [field]: value }), TODAY), new RegExp(field), field);
  }
});

/** Checks frameworks and axes must be non-empty distinct lowercase slugs, and axes must belong to the vocabulary. */
test("frameworks and axes must be distinct slugs and axes must be in the vocabulary", () => {
  for (const bad of [[], ["Cucumber"], ["a", "a"], "cucumber-js"]) {
    assert.throws(() => validateCaseMetadata(positive({ frameworks: bad }), TODAY), /frameworks/);
    assert.throws(() => validateCaseMetadata(positive({ axes: bad }), TODAY), /axes/);
  }
  validateCaseMetadata(positive({ axes: ["import-form", "scope"] }), TODAY);
  // A well-formed slug outside the closed vocabulary is rejected and named; every listed axis is accepted.
  assert.throws(() => validateCaseMetadata(positive({ axes: ["import-form", "made-up-axis"] }), TODAY), /not in the vocabulary: made-up-axis/);
  for (const axis of Object.keys(AXES)) validateCaseMetadata(positive({ axes: [axis] }), TODAY);
  assert.ok(Object.keys(AXES).length >= 10);
  for (const [axis, meaning] of Object.entries(AXES)) assert.ok(/^[a-z][a-z0-9-]*$/.test(axis) && meaning.length > 10, axis);
});

/** Checks classifiedAt must be a real calendar date that is not in the future. */
test("classifiedAt must be a real calendar date that is not in the future", () => {
  for (const bad of ["2026-02-30", "2026-1-1", "18/10/2026", "2026-10-19", 20261018]) {
    assert.throws(() => validateCaseMetadata(positive({ classifiedAt: bad }), TODAY), /classifiedAt/, String(bad));
  }
  validateCaseMetadata(positive({ classifiedAt: TODAY }), TODAY);
});

/** Checks role must agree with the case's own expectations. */
test("role must agree with the case's own expectations", () => {
  assert.throws(() => validateCaseMetadata(positive({ expectedFindings: [] }), TODAY), /role positive needs/);
  assert.throws(() => validateCaseMetadata(positive({ role: "negative" }), TODAY), /role negative has/);
  validateCaseMetadata(positive({ role: "negative", expectedFindings: [], expectedAbsent: [{ rule: "duplicate-matcher" }] }), TODAY);
});

/** Checks ambiguous needs reasoned entries, and entries need the ambiguous role. */
test("ambiguous needs reasoned entries, and entries need the ambiguous role", () => {
  const ambiguous = { rule: "near-duplicate-step", primaryPath: "a.ts", primaryLine: 3, relatedLocations: ["a.ts:5"], reason: "wording distance is a judgement call" };
  validateCaseMetadata(positive({ role: "ambiguous", expectedFindings: [], expectedAmbiguous: [ambiguous] }), TODAY);
  assert.throws(() => validateCaseMetadata(positive({ role: "ambiguous", expectedAmbiguous: [] }), TODAY), /needs expectedAmbiguous/);
  assert.throws(() => validateCaseMetadata(positive({ role: "ambiguous", expectedAmbiguous: [{ rule: "x", primaryPath: "a.ts", primaryLine: 3, relatedLocations: ["a.ts:5"] }] }), TODAY), /reason/);
  assert.throws(() => validateCaseMetadata(positive({ role: "ambiguous", expectedAmbiguous: [{ rule: "x", reason: "r" }] }), TODAY), /identify one pair/);
  assert.throws(() => validateCaseMetadata(positive({ role: "ambiguous", expectedAmbiguous: [{ primaryPath: "a.ts", primaryLine: 3, relatedLocations: ["a.ts:5"], reason: "r" }] }), TODAY), /names no rule/);
  assert.throws(() => validateCaseMetadata(positive({ expectedAmbiguous: [ambiguous] }), TODAY), /needs role ambiguous/);
});

/** Checks an ambiguity entry must name both endpoints of one pair; any narrower selector is rejected. */
test("ambiguity selectors must identify one pair", () => {
  /** A purely ambiguous case whose one entry is the default plus `over`. */
  const entry = (over) => positive({ role: "ambiguous", expectedFindings: [], expectedAmbiguous: [{ rule: "x", reason: "r", ...over }] });
  const both = { primaryPath: "a.ts", primaryLine: 3, relatedLocations: ["a.ts:5"] };
  const bad = [
    {}, { messageIncludes: "x" }, { matchers: ["a", "b"] }, { primaryPath: "a.ts", primaryLine: 3 }, { relatedLocations: ["a.ts:5"] },
    { ...both, primaryLine: 0 }, { ...both, primaryPath: "" }, { ...both, primaryLine: "3" }, { ...both, relatedLocations: [] }, { ...both, relatedLocations: [""] },
  ];
  for (const over of bad) assert.throws(() => validateCaseMetadata(entry(over), TODAY), /identify one pair/, JSON.stringify(over));
  validateCaseMetadata(entry(both), TODAY);
  validateCaseMetadata(entry({ ...both, matchers: ["a", "b"] }), TODAY);
});

/** Checks variantOf is required for variants, forbidden otherwise, and must name a base case. */
test("variantOf is required for variants, forbidden otherwise, and must name a base case", () => {
  validateCaseMetadata(positive({ role: "variant", variantOf: "base" }), TODAY);
  assert.throws(() => validateCaseMetadata(positive({ role: "variant" }), TODAY), /needs variantOf/);
  assert.throws(() => validateCaseMetadata(positive({ variantOf: "base" }), TODAY), /needs role variant/);
  const base = positive({ name: "base" });
  const variant = positive({ name: "v", role: "variant", variantOf: "base" });
  validateVariantReferences([base, variant]);
  assert.throws(() => validateVariantReferences([variant]), /names no case/);
  assert.throws(() => validateVariantReferences([base, variant, positive({ name: "w", role: "variant", variantOf: "v" })]), /not another variant/);
  assert.throws(() => validateVariantReferences([positive({ name: "base", role: "variant", variantOf: "base" })]), /not another variant/);
});

/** Checks language and frameworks are held against the analyzed records. */
test("language and frameworks are held against the analyzed records", () => {
  const records = [
    { domain: "ecmascript", framework: "cucumber-js" },
    { domain: "ecmascript", framework: "playwright-bdd" },
  ];
  validateAgainstRecords(positive({ frameworks: ["playwright-bdd", "cucumber-js"] }), records);
  assert.throws(() => validateAgainstRecords(positive({ frameworks: ["cucumber-js"] }), records), /frameworks label/);
  assert.throws(() => validateAgainstRecords(positive({ frameworks: ["cucumber-js", "playwright-bdd", "cypress-cucumber"] }), records), /frameworks label/);
  assert.throws(() => validateAgainstRecords(positive({ language: "ruby", frameworks: ["cucumber-js", "playwright-bdd"] }), records), /language label/);
  // No records is acceptable only for a case that expects no definitions.
  validateAgainstRecords(positive({ language: "ruby", expectedDefinitions: 0 }), []);
  assert.throws(() => validateAgainstRecords(positive({ expectedDefinitions: 2 }), []), /expects definitions/);
  // Control: a zero-definition case with matching labels still fails when records exist.
  assert.throws(() => validateAgainstRecords(positive({ expectedDefinitions: 0, frameworks: ["cucumber-js"] }), [{ domain: "ecmascript", framework: "cucumber-js" }]), /expects no definitions/);
});

/** Checks ambiguous findings leave the owned set; every other finding stays. */
test("ambiguous findings leave the owned set; every other finding stays", () => {
  const near = finding("near-duplicate-step");
  const exact = finding("duplicate-matcher");
  const { owned, ambiguous } = partitionAmbiguous([near, exact], [{ rule: "near-duplicate-step", reason: "r" }]);
  assert.deepEqual(owned, [exact]);
  assert.deepEqual(ambiguous, [near]);
  // Control: with no declaration nothing is exempt, so the split is not vacuous.
  assert.deepEqual(partitionAmbiguous([near, exact], []).owned, [near, exact]);
  // A declaration naming other matchers exempts nothing.
  assert.deepEqual(partitionAmbiguous([near], [{ rule: "near-duplicate-step", primaryPath: "other.ts", primaryLine: 9, relatedLocations: ["other.ts:1"], reason: "r" }]).owned, [near]);
});

/** Checks a purely ambiguous case takes either finding-dependent exit status but never an operational failure. */
test("ambiguity makes the finding-dependent exit status neutral but not an operational failure", () => {
  const ambiguous = positive({ role: "ambiguous", expectedFindings: [], expectedAmbiguous: [{ rule: "x", primaryPath: "a.ts", primaryLine: 3, relatedLocations: ["a.ts:5"], reason: "r" }] });
  // Both outcomes of the undecided finding pass: emitted (1) and absent (0), whichever is declared.
  for (const expected of [0, 1]) {
    for (const status of [0, 1]) assert.equal(exitAcceptable(status, expected, ambiguous), true, `${status}/${expected}`);
  }
  // Controls: without ambiguity the status is exact, and a status 2 is never neutral.
  assert.equal(exitAcceptable(0, 1, positive()), false);
  assert.equal(exitAcceptable(1, 0, positive()), false);
  assert.equal(exitAcceptable(2, 1, ambiguous), false);
  assert.equal(exitAcceptable(null, 0, ambiguous), false);
  assert.equal(exitAcceptable(0, 2, ambiguous), false);
  assert.equal(exitAcceptable(2, 2, ambiguous), false);
  assert.equal(exitAcceptable(2, 2, positive()), true);
  // A case that also requires findings keeps its exact status.
  const mixed = { ...ambiguous, expectedFindings: [{ rule: "duplicate-matcher" }] };
  assert.equal(exitAcceptable(0, 1, mixed), false);
});

/** Checks the recall README documents exactly the axes in the vocabulary, with the same meaning. */
test("the recall README lists every axis in the vocabulary and no other", async () => {
  const readme = await readFile("fixtures/recall/README.md", "utf8");
  const documented = new Map([...readme.matchAll(/^\| `([a-z][a-z0-9-]*)` \| (.+) \|$/gm)].map((match) => [match[1], match[2]]));
  // The metadata table above it also has backticked first cells (role, origin, ...); keep the axes.
  const axes = new Map([...documented].filter(([name]) => Object.hasOwn(AXES, name) || !["role", "origin", "language", "frameworks", "verdict", "classifiedAt", "axes"].includes(name)));
  assert.deepEqual([...axes.keys()].sort(), Object.keys(AXES).sort());
  for (const [axis, meaning] of Object.entries(AXES)) assert.equal(axes.get(axis), meaning, axis);
});

/** Checks labels in the other corpora: the shared fields are required, and the role follows the required findings. */
test("corpus labels need the shared fields and a role that follows the required findings", () => {
  /** A labelled Ruby group, with fields overridden. */
  const labelled = (over = {}) => ({ role: "positive", origin: "synthetic", language: "ruby", frameworks: ["cucumber-ruby"], verdict: "reviewed", classifiedAt: "2026-10-18", axes: ["provider-context"], ...over });
  validateCorpusLabels(labelled(), "group", TODAY, 2);
  validateCorpusLabels(labelled({ role: "negative" }), "group", TODAY, 0);
  validateCorpusLabels(labelled({ language: "both" }), "group", TODAY, 1);
  assert.throws(() => validateCorpusLabels(labelled(), "group", TODAY, 0), /role positive needs a required finding/);
  assert.throws(() => validateCorpusLabels(labelled({ role: "negative" }), "group", TODAY, 1), /role negative requires a finding/);
  // Roles that only recall cases can have are rejected here.
  for (const role of ["ambiguous", "variant", undefined]) assert.throws(() => validateCorpusLabels(labelled({ role }), "group", TODAY, 1), /role must be one of/);
  for (const field of ["origin", "language", "frameworks", "verdict", "classifiedAt", "axes"]) {
    assert.throws(() => validateCorpusLabels(labelled({ [field]: undefined }), "group", TODAY, 1), new RegExp(field), field);
  }
});

/** Checks the newest allowed label date is the date anywhere on Earth, so a local date ahead of UTC is accepted. */
test("the latest label date is the calendar date at UTC+14", () => {
  assert.equal(latestClassifiedAt(Date.UTC(2026, 9, 19, 12)), "2026-10-20");
  assert.equal(latestClassifiedAt(Date.UTC(2026, 9, 19, 9)), "2026-10-19");
  assert.equal(latestClassifiedAt(Date.UTC(2026, 11, 31, 23)), "2027-01-01");
});
