import { test } from "node:test";
import assert from "node:assert/strict";
import { checkIsolation as checkWith, differingFields, isolatable, parseRecords } from "./negative-isolation.mjs";

/** A run whose candidate limits skipped nothing. */
const UNLIMITED = { truncated: false, skippedCandidateComparisons: 0, truncatedStructuralClasses: 0 };
/** Checks an isolation against records from a run with no skipped comparisons. */
const checkIsolation = (records, expected, analysis = UNLIMITED) => checkWith(records, expected, analysis);

/** A comparable fingerprint record with overridable top-level and handler fields. */
const record = (line, matcher, handler = {}, over = {}) => ({
  schemaVersion: "1",
  path: "steps.ts",
  line,
  column: 1,
  domain: "ecmascript",
  framework: { cucumber: "cucumber-js" },
  registration: "Given",
  matcher,
  matcherKind: "cucumber-expression",
  matcherFlags: "",
  normalizedMatcher: matcher,
  handler: {
    exact: "e",
    normalized: "n",
    alphaNormalized: "a",
    structural: "s",
    behavior: "b",
    comparable: true,
    trivial: false,
    ...handler,
  },
  ...over,
});
const MATCHER = ["matcher", "normalizedMatcher"];
/** A duplicate-handler absence over two matchers with an isolation declaration. */
const absent = (isolation, rule = "duplicate-handler") => ({ rule, matchers: ["one", "two"], isolation });

/** Checks a compared pair passes only with the exact differing fields declared. */
test("a compared pair passes with exactly its differing fields declared", () => {
  const records = [record(1, "one"), record(2, "two", { behavior: "other" })];
  checkIsolation(records, absent({ compared: true, differs: [...MATCHER, "handler.behavior"] }));
  assert.throws(
    () => checkIsolation(records, absent({ compared: true, differs: MATCHER })),
    /differ in \[matcher, normalizedMatcher, handler.behavior\], declared \[matcher, normalizedMatcher\]/,
  );
  assert.throws(
    () => checkIsolation(records, absent({ compared: true, differs: [...MATCHER, "handler.behavior", "domain"] })),
    /declared \[domain, /,
  );
});

/** Checks the declared mechanism must match comparability, for both mechanisms. */
test("compared and excluded are each checked against the records", () => {
  const excluded = [record(1, "one", { comparable: false }), record(2, "two", { comparable: false })];
  checkIsolation(excluded, absent({ compared: false, differs: MATCHER }));
  assert.throws(() => checkIsolation(excluded, absent({ compared: true, differs: MATCHER })), /was excluded from comparison, declared compared/);
  const trivial = [record(1, "one", { trivial: true }), record(2, "two")];
  assert.throws(() => checkIsolation(trivial, absent({ compared: true, differs: MATCHER })), /excluded from comparison/);
  const compared = [record(1, "one"), record(2, "two", { exact: "x" })];
  assert.throws(() => checkIsolation(compared, absent({ compared: false, differs: [...MATCHER, "handler.exact"] })), /was compared, declared excluded/);
});

/** Checks a compared pair is rejected when candidate limits skipped work, and an excluded one is not. */
test("compared needs a run whose candidate limits skipped nothing", () => {
  const records = [record(1, "one"), record(2, "two", { exact: "x" })];
  const compared = absent({ compared: true, differs: [...MATCHER, "handler.exact"] });
  for (const limited of [
    { ...UNLIMITED, truncated: true },
    { ...UNLIMITED, skippedCandidateComparisons: 3 },
    { ...UNLIMITED, truncatedStructuralClasses: 1 },
    {},
    undefined,
  ]) {
    assert.throws(() => checkWith(records, compared, limited), /candidate limits skipped comparisons/);
  }
  const excluded = [record(1, "one", { comparable: false }), record(2, "two", { comparable: false })];
  checkWith(excluded, absent({ compared: false, differs: MATCHER }), { ...UNLIMITED, truncated: true });
});

/** Checks an excluded pair whose handlers also differ is rejected as absent for two reasons. */
test("an excluded pair with differing handlers is absent for two reasons", () => {
  const records = [record(1, "one", { comparable: false }), record(2, "two", { comparable: false, alphaNormalized: "x" })];
  assert.throws(
    () => checkIsolation(records, absent({ compared: false, differs: [...MATCHER, "handler.alphaNormalized"] })),
    /absent for two reasons/,
  );
});

/** Checks participant selection by matcher and by label fails closed on zero or several records. */
test("participants resolve to exactly one record each", () => {
  const records = [record(1, "one"), record(2, "two"), record(3, "two", {}, { path: "other.ts" })];
  assert.throws(() => checkIsolation(records, absent({ compared: true, differs: MATCHER })), /"two" selects 2 records/);
  const labelled = { compared: true, differs: MATCHER, participants: ["steps.ts:1", "steps.ts:2"] };
  checkIsolation(records, absent(labelled));
  assert.throws(
    () => checkIsolation(records, absent({ ...labelled, participants: ["steps.ts:1", "steps.ts:9"] })),
    /"steps.ts:9" selects 0 records/,
  );
  assert.throws(
    () => checkIsolation(records, { rule: "duplicate-handler", isolation: { compared: true, differs: [] } }),
    /two or more participants/,
  );
});

/** Checks participants equal the entry's matchers and located set, with a primary-only entry included. */
test("participants are exactly the entry's matchers and located pair", () => {
  const records = [record(1, "one"), record(2, "two"), record(3, "three"), record(4, "four")];
  const stale = { compared: true, differs: MATCHER, participants: ["steps.ts:3", "steps.ts:4"] };
  assert.throws(() => checkIsolation(records, absent(stale)), /matchers \["three","four"\], the entry names \["one","two"\]/);
  const extra = { compared: true, differs: MATCHER, participants: ["steps.ts:1", "steps.ts:2", "steps.ts:3"] };
  assert.throws(() => checkIsolation(records, absent(extra)), /the entry names/);
  const repeated = { rule: "duplicate-handler", matchers: ["one", "one"], isolation: { compared: true, differs: MATCHER, participants: ["steps.ts:1", "steps.ts:2"] } };
  assert.throws(() => checkIsolation(records, repeated), /matchers \["one","two"\], the entry names \["one","one"\]/);
  /** A duplicate-matcher absence located by its primary, with labelled participants. */
  const located = (participants, over = {}) => ({
    rule: "duplicate-matcher",
    primaryPath: "steps.ts",
    primaryLine: 1,
    ...over,
    isolation: { differs: MATCHER, participants },
  });
  checkIsolation(records, located(["steps.ts:1", "steps.ts:2"]));
  checkIsolation(records, located(["steps.ts:1", "steps.ts:2", "steps.ts:3"]));
  assert.throws(() => checkIsolation(records, located(["steps.ts:3", "steps.ts:4"])), /the entry locates \["steps.ts:1"\]/);
  checkIsolation(records, located(["steps.ts:4", "steps.ts:1"], { relatedLocations: ["steps.ts:4"] }));
  assert.throws(
    () => checkIsolation(records, located(["steps.ts:1", "steps.ts:2"], { relatedLocations: ["steps.ts:4"] })),
    /the entry locates \["steps.ts:1","steps.ts:4"\]/,
  );
  assert.throws(
    () => checkIsolation(records, located(["steps.ts:1", "steps.ts:2", "steps.ts:4"], { relatedLocations: ["steps.ts:4"] })),
    /the entry locates/,
  );
  for (const partial of [{ primaryLine: undefined }, { primaryPath: undefined }]) {
    assert.throws(() => checkIsolation(records, located(["steps.ts:1", "steps.ts:2"], partial)), /together or neither/);
  }
});

/** Checks records missing a field or with non-boolean flags fail instead of comparing as equal. */
test("incomplete or foreign records are rejected before comparison", () => {
  /** A record whose handler carries no fields. */
  const bare = (line, matcher) => ({ ...record(line, matcher), handler: {} });
  assert.throws(() => checkIsolation([bare(1, "one"), bare(2, "two")], absent({ compared: false, differs: MATCHER })), /not schema 1.*handler.exact/);
  const untyped = [record(1, "one", { trivial: undefined }), record(2, "two")];
  assert.throws(() => checkIsolation(untyped, absent({ compared: true, differs: MATCHER })), /comparable or trivial/);
  const noComparable = [record(1, "one", { comparable: "yes" }), record(2, "two")];
  assert.throws(() => checkIsolation(noComparable, absent({ compared: true, differs: MATCHER })), /comparable or trivial/);
  const foreign = [record(1, "one", {}, { schemaVersion: "2" }), record(2, "two")];
  assert.throws(() => checkIsolation(foreign, absent({ compared: true, differs: MATCHER })), /not schema 1/);
  const headless = [{ ...record(1, "one"), handler: undefined }, record(2, "two")];
  assert.throws(() => checkIsolation(headless, absent({ compared: true, differs: MATCHER })), /not schema 1/);
});

/** Checks malformed declarations and inapplicable rules fail instead of passing. */
test("malformed declarations and feature-step rules are rejected", () => {
  const records = [record(1, "one"), record(2, "two")];
  assert.throws(() => checkIsolation(records, absent({ compared: true, differs: MATCHER, reason: "x" })), /unknown isolation keys: reason/);
  assert.throws(() => checkIsolation(records, absent({ compared: true, differs: ["line"] })), /differs must list distinct fields/);
  assert.throws(() => checkIsolation(records, absent({ compared: true })), /differs must list distinct fields/);
  assert.throws(() => checkIsolation(records, absent({ compared: true, differs: [...MATCHER, "matcher"] })), /distinct fields/);
  for (const isolation of [null, true, []]) {
    assert.throws(() => checkIsolation(records, absent(isolation)), /isolation must be an object/);
  }
  assert.throws(
    () => checkIsolation(records, absent({ compared: true, differs: [], participants: ["steps.ts:1", "steps.ts:1"] })),
    /select the same record/,
  );
  assert.throws(() => checkIsolation(records, absent({ differs: MATCHER })), /compared is required for handler rules/);
  assert.throws(
    () => checkIsolation(records, absent({ compared: true, differs: MATCHER }, "duplicate-matcher")),
    /not allowed for matcher rules/,
  );
  checkIsolation(records, absent({ differs: MATCHER }, "normalized-matcher"));
  assert.throws(() => checkIsolation(records, absent({ differs: MATCHER }, "unused-definition")), /depends on feature steps/);
  assert.equal(isolatable("ambiguous-step"), false);
  assert.equal(isolatable("parameterization-candidate"), true);
});

/** Checks every isolation field is compared, framework by value, and three participants by any pair. */
test("differing fields cover every field, compare framework by value, and span all participants", () => {
  assert.deepEqual(differingFields([record(1, "one"), record(2, "one")]), []);
  assert.deepEqual(
    differingFields([record(1, "one"), record(2, "one", {}, { framework: { cucumber: "cucumber-js" } })]),
    [],
  );
  const changed = {
    domain: "ruby",
    framework: { cucumber: "cucumber-ruby" },
    registration: "When",
    matcherKind: "regular-expression",
    matcherFlags: "i",
    normalizedMatcher: "x",
  };
  const handler = { exact: "x", normalized: "x", alphaNormalized: "x", structural: "x", behavior: "x" };
  assert.deepEqual(differingFields([record(1, "one"), record(2, "two", handler, changed)]), [
    "domain", "framework", "registration", "matcher", "matcherKind", "matcherFlags", "normalizedMatcher",
    "handler.exact", "handler.normalized", "handler.alphaNormalized", "handler.structural", "handler.behavior",
  ]);
  assert.deepEqual(differingFields([record(1, "one"), record(2, "one"), record(3, "one", { structural: "x" })]), ["handler.structural"]);
  assert.deepEqual(parseRecords('{"line":1}\n{"line":2}\n'), [{ line: 1 }, { line: 2 }]);
  assert.deepEqual(parseRecords(""), []);
});
