import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { checkIsolation as checkWith, differingFields, isolatable, pairFileReader, parseRecords } from "./negative-isolation.mjs";
import { census } from "./pair-census-fixture.mjs";

/** Census options for a run whose pair-proposing sources skipped one comparison. */
const LIMITED = { skipped: 1 };

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
  definitionFingerprint: `definition-${line}`,
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

/** The pair record the analyzer would write for two fingerprint records, with overridable fields. */
const pair = (left, right, over = {}) => ({
  schemaVersion: "1",
  left: { path: left.path, line: left.line, column: left.column, definitionFingerprint: left.definitionFingerprint },
  right: { path: right.path, line: right.line, column: right.column, definitionFingerprint: right.definitionFingerprint },
  candidateSources: ["identicalHandler"],
  relationships: {
    exactMatcher: left.matcher === right.matcher,
    normalizedMatcher: left.normalizedMatcher === right.normalizedMatcher,
    sameHandler: left.handler.alphaNormalized === right.handler.alphaNormalized && left.handler.behavior === right.handler.behavior,
    sameHandlerStructure: left.handler.structural === right.handler.structural,
    sameStructure: left.handler.structural === right.handler.structural && left.handler.behavior === right.handler.behavior,
    sameDeferredAssertions: true,
  },
  positional: false,
  meaningfulHandlers: [left, right].every((end) => end.handler.comparable && !end.handler.trivial),
  rules: [],
  ...over,
});

/** Every unordered pair among the records, as if the run evaluated all of them. */
const everyPair = (records) => records.flatMap((left, index) => records.slice(index + 1).map((right) => pair(left, right)));

/**
 * Checks an isolation. The pair file defaults to every pair, and the census to one that counts the
 * file's records and skipped nothing; `limits` changes the census options and `analysis` replaces
 * the whole section (`null` passes none).
 */
const checkIsolation = (records, expected, { pairs = everyPair(records), limits = {}, analysis } = {}) => checkWith(
  records,
  expected,
  analysis === undefined ? census({ evaluated: pairs?.length, ...limits }) : analysis ?? undefined,
  () => pairs,
);

const MATCHER = ["matcher", "normalizedMatcher"];
const BEHAVIOR = [...MATCHER, "handler.behavior"];
const STRUCTURE = [...MATCHER, "handler.structural"];
/** A duplicate-handler absence over two matchers with an isolation declaration. */
const absent = (isolation, rule = "duplicate-handler") => ({ rule, matchers: ["one", "two"], isolation });
/** Two eligible records with equal handlers. */
const equalHandlers = () => [record(1, "one"), record(2, "two")];
/** Two eligible records whose behavior signatures differ. */
const differingBehavior = () => [record(1, "one"), record(2, "two", { behavior: "other" })];
/** Two eligible records whose structure differs, with equal behavior. */
const differingStructure = () => [record(1, "one"), record(2, "two", { structural: "other" })];
/** Three records: the first two differ in behavior, the third is unrelated to them. */
const differingThird = () => [record(1, "one"), record(2, "two", { behavior: "other" }), record(3, "three")];
/** Two eligible records with the same matcher text and different behavior, located by label. */
const equalMatchers = () => [record(1, "one"), record(2, "one", { behavior: "other" })];
/** The entry fields that select `equalMatchers` records by label. */
const SAME_TEXT = { matchers: ["one", "one"] };
/** An isolation over `equalMatchers` records. */
const sameText = (differs, extra = {}) => ({ compared: true, participants: ["steps.ts:1", "steps.ts:2"], differs, ...extra });
/** Two records whose handlers are not comparable and differ by an unresolved value. */
const unresolved = () => [record(1, "one", { comparable: false }), record(2, "two", { comparable: false, exact: "x", alphaNormalized: "x" })];

/**
 * One row per declared state against the evidence: a pair record present, absent, ambiguous or
 * unmodeled, and a census with and without skipped comparisons. Rows come in opposite-answer pairs
 * (the same records and declaration with only the evidence changed), so a check that ignored the
 * evidence would pass one side of every pair.
 */
const MATRIX = [
  // compared: true needs the analyzer's own record of a meaningful pair.
  { name: "true, record present", records: equalHandlers, declared: { compared: true, differs: MATCHER } },
  { name: "true, record absent from an unlimited run", records: equalHandlers, pairs: () => [], declared: { compared: true, differs: MATCHER }, error: /never evaluated/ },
  { name: "true, record absent from a limited run", records: equalHandlers, pairs: () => [], limits: LIMITED, declared: { compared: true, differs: MATCHER }, error: /never evaluated/ },
  { name: "true, record judges the handlers runtime-incompatible", records: equalHandlers, pairs: (r) => [pair(r[0], r[1], { meaningfulHandlers: false })], declared: { compared: true, differs: MATCHER }, error: /runtime-incompatible/ },
  { name: "true, record lists the absent rule", records: equalHandlers, pairs: (r) => [pair(r[0], r[1], { rules: ["duplicate-handler"] })], declared: { compared: true, differs: MATCHER }, error: /suppressed or collapsed/ },
  { name: "true, record lists another rule", records: equalHandlers, pairs: (r) => [pair(r[0], r[1], { rules: ["near-duplicate-step"] })], declared: { compared: true, differs: MATCHER } },
  { name: "true, ineligible participant", records: () => [record(1, "one", { comparable: false }), record(2, "two", { comparable: false })], declared: { compared: true, differs: MATCHER }, error: /excluded from comparison, declared compared/ },
  { name: "true, record says handlers differ that the declaration says are equal", records: equalHandlers, pairs: (r) => [pair(r[0], r[1], { relationships: { ...pair(r[0], r[1]).relationships, sameHandler: false } })], declared: { compared: true, differs: MATCHER }, error: /disagree.*sameHandler/ },
  { name: "true, record says handlers are equal that the declaration says differ", records: differingBehavior, pairs: (r) => [pair(r[0], r[1], { relationships: { ...pair(r[0], r[1]).relationships, sameHandler: true } })], declared: { compared: true, differs: BEHAVIOR }, error: /disagree.*sameHandler/ },
  { name: "true, record says matchers are equal that the declaration says differ", records: equalHandlers, pairs: (r) => [pair(r[0], r[1], { relationships: { ...pair(r[0], r[1]).relationships, exactMatcher: true } })], declared: { compared: true, differs: MATCHER }, error: /disagree.*exactMatcher/ },
  { name: "true, record says normalized matchers are equal that the declaration says differ", records: equalHandlers, pairs: (r) => [pair(r[0], r[1], { relationships: { ...pair(r[0], r[1]).relationships, normalizedMatcher: true } })], declared: { compared: true, differs: MATCHER }, error: /disagree.*normalizedMatcher/ },
  { name: "true, record says structures are equal that the declaration says differ", records: differingStructure, pairs: (r) => [pair(r[0], r[1], { relationships: { ...pair(r[0], r[1]).relationships, sameHandlerStructure: true } })], declared: { compared: true, differs: STRUCTURE }, error: /disagree.*sameHandlerStructure/ },
  { name: "true, record says handlers share a structure and behavior that the declaration says differ", records: differingStructure, pairs: (r) => [pair(r[0], r[1], { relationships: { ...pair(r[0], r[1]).relationships, sameStructure: true } })], declared: { compared: true, differs: STRUCTURE }, error: /disagree.*sameStructure/ },
  { name: "true, record says structures differ that the declaration says are equal", records: equalHandlers, pairs: (r) => [pair(r[0], r[1], { relationships: { ...pair(r[0], r[1]).relationships, sameHandlerStructure: false, sameStructure: false } })], declared: { compared: true, differs: MATCHER }, error: /disagree.*sameHandlerStructure, sameStructure/ },
  { name: "true, record says deferred assertions differ although behavior is equal", records: equalHandlers, pairs: (r) => [pair(r[0], r[1], { relationships: { ...pair(r[0], r[1]).relationships, sameDeferredAssertions: false } })], declared: { compared: true, differs: MATCHER }, error: /disagree.*sameDeferredAssertions/ },
  { name: "true, deferred assertions differ with behavior", records: differingBehavior, pairs: (r) => [pair(r[0], r[1], { relationships: { ...pair(r[0], r[1]).relationships, sameDeferredAssertions: false } })], declared: { compared: true, differs: BEHAVIOR } },
  // Matcher equalities are claimed where the fields decide them, in both directions.
  { name: "true, equal matchers match the record", records: equalMatchers, entry: SAME_TEXT, declared: sameText(["handler.behavior"]) },
  { name: "true, record says equal matchers differ", records: equalMatchers, entry: SAME_TEXT, pairs: (r) => [pair(r[0], r[1], { relationships: { ...pair(r[0], r[1]).relationships, exactMatcher: false } })], declared: sameText(["handler.behavior"]), error: /disagree.*exactMatcher/ },
  { name: "true, record says equal normalized matchers differ", records: equalMatchers, entry: SAME_TEXT, pairs: (r) => [pair(r[0], r[1], { relationships: { ...pair(r[0], r[1]).relationships, normalizedMatcher: false } })], declared: sameText(["handler.behavior"]), error: /disagree.*normalizedMatcher/ },
  { name: "true, positional pair is not normalized-equal", records: equalMatchers, entry: SAME_TEXT, pairs: (r) => [pair(r[0], r[1], { positional: true, relationships: { ...pair(r[0], r[1]).relationships, normalizedMatcher: false } })], declared: sameText(["handler.behavior"]) },
  { name: "true, positional pair claimed normalized-equal", records: equalMatchers, entry: SAME_TEXT, pairs: (r) => [pair(r[0], r[1], { positional: true })], declared: sameText(["handler.behavior"]), error: /disagree.*normalizedMatcher/ },
  { name: "true, flags that differ leave the equality open (equal)", records: () => [record(1, "one"), record(2, "one", { behavior: "other" }, { matcherFlags: "g" })], entry: SAME_TEXT, declared: sameText(["matcherFlags", "handler.behavior"]) },
  { name: "true, a dialect that differs leaves the equality open", records: () => [record(1, "one"), record(2, "one", { behavior: "other" }, { domain: "ruby" })], entry: SAME_TEXT, pairs: (r) => [pair(r[0], r[1], { relationships: { ...pair(r[0], r[1]).relationships, exactMatcher: false, normalizedMatcher: false } })], declared: sameText(["domain", "handler.behavior"]) },
  { name: "true, flags that differ leave the equality open (different)", records: () => [record(1, "one"), record(2, "one", { behavior: "other" }, { matcherFlags: "g" })], entry: SAME_TEXT, pairs: (r) => [pair(r[0], r[1], { relationships: { ...pair(r[0], r[1]).relationships, exactMatcher: false, normalizedMatcher: false } })], declared: sameText(["matcherFlags", "handler.behavior"]) },
  { name: "true, record says matchers of another kind are equal", records: () => [record(1, "one"), record(2, "one", { behavior: "other" }, { matcherKind: "regular-expression" })], entry: SAME_TEXT, pairs: (r) => [pair(r[0], r[1])], declared: sameText(["matcherKind", "handler.behavior"]), error: /disagree.*exactMatcher, normalizedMatcher/ },
  { name: "true, differing structure matches the record", records: differingStructure, declared: { compared: true, differs: STRUCTURE } },
  { name: "true, differing handlers beyond the declaration", records: differingBehavior, declared: { compared: true, differs: MATCHER }, error: /differ in \[matcher, normalizedMatcher, handler.behavior\], declared \[matcher, normalizedMatcher\]/ },
  // compared: false needs an exclusion visible in the fingerprints or in the pair record.
  { name: "false, ineligible participants, no record", records: () => [record(1, "one", { comparable: false }), record(2, "two", { comparable: false })], pairs: () => [], declared: { compared: false, differs: MATCHER } },
  { name: "false, ineligible participants, record excludes them", records: () => [record(1, "one", { trivial: true }), record(2, "two", { trivial: true })], declared: { compared: false, differs: MATCHER } },
  { name: "false, ineligible participants, record says meaningful", records: () => [record(1, "one", { comparable: false }), record(2, "two", { comparable: false })], pairs: (r) => [pair(r[0], r[1], { meaningfulHandlers: true })], declared: { compared: false, differs: MATCHER }, error: /contradicts the fingerprints/ },
  { name: "false, eligible participants, record judges them runtime-incompatible", records: () => [record(1, "one"), record(2, "two", {}, { framework: { cucumber: "other" } })], pairs: (r) => [pair(r[0], r[1], { meaningfulHandlers: false })], declared: { compared: false, differs: [...MATCHER, "framework"] } },
  { name: "false, runtime-incompatible record but no visible difference", records: equalHandlers, pairs: (r) => [pair(r[0], r[1], { meaningfulHandlers: false })], declared: { compared: false, differs: MATCHER }, error: /nothing visible explains it/ },
  { name: "false, runtime-incompatible record and another Ruby file", records: () => [record(1, "one", {}, { domain: "ruby" }), record(2, "two", {}, { domain: "ruby", path: "other.rb" })], pairs: (r) => [pair(r[0], r[1], { meaningfulHandlers: false })], declared: { compared: false, differs: MATCHER } },
  { name: "false, runtime-incompatible record and another ECMAScript file only", records: () => [record(1, "one"), record(2, "two", {}, { path: "other.ts" })], pairs: (r) => [pair(r[0], r[1], { meaningfulHandlers: false })], declared: { compared: false, differs: MATCHER }, error: /nothing visible explains it/ },
  { name: "false, runtime-incompatible record and another framework", records: () => [record(1, "one"), record(2, "two", {}, { framework: { cucumber: "other" } })], pairs: (r) => [pair(r[0], r[1], { meaningfulHandlers: false })], declared: { compared: false, differs: [...MATCHER, "framework"] } },
  { name: "false, runtime-incompatible record and another registration only", records: () => [record(1, "one"), record(2, "two", {}, { registration: "When" })], pairs: (r) => [pair(r[0], r[1], { meaningfulHandlers: false })], declared: { compared: false, differs: [...MATCHER, "registration"] }, error: /nothing visible explains it/ },
  { name: "false, runtime-incompatible record and another domain only", records: () => [record(1, "one"), record(2, "two", {}, { domain: "ruby" })], pairs: (r) => [pair(r[0], r[1], { meaningfulHandlers: false })], declared: { compared: false, differs: [...MATCHER, "domain"] }, error: /nothing visible explains it/ },
  { name: "false, eligible participants, record says meaningful", records: equalHandlers, declared: { compared: false, differs: MATCHER }, error: /was compared, declared excluded/ },
  { name: "false, eligible participants, no record", records: equalHandlers, pairs: () => [], declared: { compared: false, differs: MATCHER }, error: /nothing shows the pair was excluded/ },
  { name: "false, differing handlers without a declared reason", records: unresolved, pairs: () => [], declared: { compared: false, differs: [...MATCHER, "handler.exact", "handler.alphaNormalized"] }, error: /absent for two reasons/ },
  { name: "false, differing handlers with an unresolved value", records: unresolved, pairs: () => [], declared: { compared: false, differs: [...MATCHER, "handler.exact", "handler.alphaNormalized"], handlerDifference: "unresolved-value" } },
  { name: "false, unresolved value with a pair record", records: unresolved, declared: { compared: false, differs: [...MATCHER, "handler.exact", "handler.alphaNormalized"], handlerDifference: "unresolved-value" } },
  { name: "false, unresolved value with equal handlers", records: () => [record(1, "one", { comparable: false }), record(2, "two", { comparable: false })], pairs: () => [], declared: { compared: false, differs: MATCHER, handlerDifference: "unresolved-value" }, error: /only for an excluded pair whose handlers differ/ },
  { name: "false, unresolved value with another name", records: unresolved, pairs: () => [], declared: { compared: false, differs: [...MATCHER, "handler.exact", "handler.alphaNormalized"], handlerDifference: "shadowed" }, error: /only for an excluded pair whose handlers differ/ },
  { name: "false, unresolved value but one participant is comparable", records: () => [record(1, "one"), record(2, "two", { comparable: false, exact: "x" })], pairs: () => [], declared: { compared: false, differs: [...MATCHER, "handler.exact"], handlerDifference: "unresolved-value" }, error: /both participants non-comparable/ },
  { name: "false, unresolved value but behavior differs", records: () => [record(1, "one", { comparable: false }), record(2, "two", { comparable: false, behavior: "x" })], pairs: () => [], declared: { compared: false, differs: BEHAVIOR, handlerDifference: "unresolved-value" }, error: /equal in handler.behavior/ },
  // not-proposed needs no record in a run that skipped nothing, and keys that explain it.
  { name: "not-proposed, no record, unlimited, behavior differs", records: differingBehavior, pairs: () => [], declared: { compared: "not-proposed", reason: "behavior-differs", differs: BEHAVIOR } },
  { name: "not-proposed, record present", records: differingBehavior, declared: { compared: "not-proposed", reason: "behavior-differs", differs: BEHAVIOR }, error: /has a record/ },
  { name: "not-proposed, run skipped a comparison", records: differingBehavior, pairs: () => [], limits: LIMITED, declared: { compared: "not-proposed", reason: "behavior-differs", differs: BEHAVIOR }, error: /skipped no candidate/ },
  { name: "not-proposed, structural census unreadable", records: differingBehavior, pairs: () => [], analysis: { ...census(), truncatedStructuralClasses: undefined }, declared: { compared: "not-proposed", reason: "behavior-differs", differs: BEHAVIOR }, error: /candidate census is missing/ },
  { name: "not-proposed, behavior equal (an identical-handler source would propose it)", records: equalHandlers, pairs: () => [], declared: { compared: "not-proposed", reason: "behavior-differs", differs: MATCHER }, error: /differ in handler.behavior and normalizedMatcher/ },
  { name: "not-proposed, normalized matchers equal (the normalized-matcher source would propose it)", records: () => [record(1, "one"), record(2, "two", { behavior: "other" }, { normalizedMatcher: "one" })], pairs: () => [], declared: { compared: "not-proposed", reason: "behavior-differs", differs: ["matcher", "handler.behavior"] }, error: /differ in handler.behavior and normalizedMatcher/ },
  { name: "not-proposed, ineligible participant", records: () => [record(1, "one", { comparable: false }), record(2, "two", { comparable: false, behavior: "x" })], pairs: () => [], declared: { compared: "not-proposed", reason: "behavior-differs", differs: BEHAVIOR }, error: /excluded, not not-proposed/ },
  { name: "not-proposed, reason missing", records: differingBehavior, pairs: () => [], declared: { compared: "not-proposed", differs: BEHAVIOR }, error: /needs reason: / },
  { name: "not-proposed, another state's reason", records: differingBehavior, pairs: () => [], declared: { compared: "not-proposed", reason: "candidate-limit", differs: BEHAVIOR }, error: /needs reason: / },
  // unevaluated-under-limit needs no record in a run whose limits skipped comparisons.
  { name: "unevaluated-under-limit, no record, limited", records: differingBehavior, pairs: () => [], limits: LIMITED, declared: { compared: "unevaluated-under-limit", reason: "candidate-limit", differs: BEHAVIOR } },
  { name: "unevaluated-under-limit, structural classes truncated", records: differingBehavior, pairs: () => [], limits: { structural: 1 }, declared: { compared: "unevaluated-under-limit", reason: "candidate-limit", differs: BEHAVIOR } },
  { name: "unevaluated-under-limit, only skipped comparisons", records: differingBehavior, pairs: () => [], limits: { skipped: 2 }, declared: { compared: "unevaluated-under-limit", reason: "candidate-limit", differs: BEHAVIOR } },
  { name: "not-proposed, only skipped comparisons", records: differingBehavior, pairs: () => [], limits: { skipped: 2 }, declared: { compared: "not-proposed", reason: "behavior-differs", differs: BEHAVIOR }, error: /skipped no candidate/ },
  { name: "not-proposed, only truncated structural classes", records: differingBehavior, pairs: () => [], limits: { structural: 1 }, declared: { compared: "not-proposed", reason: "behavior-differs", differs: BEHAVIOR }, error: /skipped no candidate/ },
  { name: "unevaluated-under-limit, unlimited run", records: differingBehavior, pairs: () => [], declared: { compared: "unevaluated-under-limit", reason: "candidate-limit", differs: BEHAVIOR }, error: /limits skipped comparisons/ },
  { name: "unevaluated-under-limit, structural census unreadable", records: differingBehavior, pairs: () => [], analysis: { ...census(), truncatedStructuralClasses: undefined }, declared: { compared: "unevaluated-under-limit", reason: "candidate-limit", differs: BEHAVIOR }, error: /candidate census is missing/ },
  { name: "unevaluated-under-limit, behavior equal (the keys leave the absence unexplained)", records: equalHandlers, pairs: () => [], limits: LIMITED, declared: { compared: "unevaluated-under-limit", reason: "candidate-limit", differs: MATCHER }, error: /differ in handler.behavior and normalizedMatcher/ },
  { name: "unevaluated-under-limit, record present", records: differingBehavior, limits: LIMITED, pairs: (r) => [pair(r[0], r[1])], declared: { compared: "unevaluated-under-limit", reason: "candidate-limit", differs: BEHAVIOR }, error: /has a record/ },
  { name: "unevaluated-under-limit, reason missing", records: differingBehavior, pairs: () => [], limits: LIMITED, declared: { compared: "unevaluated-under-limit", differs: BEHAVIOR }, error: /needs reason: / },
  // A declaration or evidence shape the checker does not model fails closed.
  { name: "a state given as a list", records: differingBehavior, pairs: () => [], limits: LIMITED, declared: { compared: ["unevaluated-under-limit"], reason: "candidate-limit", differs: BEHAVIOR }, error: /compared must be true, false, not-proposed or unevaluated-under-limit/ },
  { name: "unmodeled state", records: equalHandlers, declared: { compared: "maybe", differs: MATCHER }, error: /compared must be true, false, not-proposed or unevaluated-under-limit/ },
  { name: "reason on an evaluated pair", records: equalHandlers, declared: { compared: true, reason: "behavior-differs", differs: MATCHER }, error: /reason is allowed only with/ },
  { name: "handlerDifference on an evaluated pair", records: unresolved, declared: { compared: true, handlerDifference: "unresolved-value", differs: [...MATCHER, "handler.exact", "handler.alphaNormalized"] }, error: /only for compared: false/ },
  { name: "handlerDifference on an unevaluated pair", records: differingBehavior, pairs: () => [], declared: { compared: "not-proposed", reason: "behavior-differs", handlerDifference: "unresolved-value", differs: BEHAVIOR }, error: /only for compared: false/ },
];

/** Runs every state-by-evidence row and expects its outcome. */
test("each declared state is held against its pair evidence", () => {
  for (const row of MATRIX) {
    const records = row.records();
    const options = { pairs: row.pairs?.(records), limits: row.limits, analysis: row.analysis };
    if (options.pairs === undefined) delete options.pairs;
    if (options.limits === undefined) delete options.limits;
    if (options.analysis === undefined) delete options.analysis;
    const entry = { ...absent(row.declared), ...row.entry };
    if (row.error === undefined) checkIsolation(records, entry, options);
    else assert.throws(() => checkIsolation(records, entry, options), row.error, row.name);
  }
});

/** Checks every state is rejected for a pair file or census the join cannot trust. */
test("a missing, miscounted, ambiguous or unmodeled pair file is rejected for every state", () => {
  const states = [
    [equalHandlers, { compared: true, differs: MATCHER }, {}],
    [equalHandlers, { compared: false, differs: MATCHER }, {}],
    [differingBehavior, { compared: "not-proposed", reason: "behavior-differs", differs: BEHAVIOR }, {}],
    [differingBehavior, { compared: "unevaluated-under-limit", reason: "candidate-limit", differs: BEHAVIOR }, LIMITED],
  ];
  /** The census count a file carries unless a row overrides it. */
  const records = (file) => file?.length;
  /** Damaged pair files: how to build one from the fingerprint records, the census count it carries, and the error. */
  const damage = [
    { name: "no file", file: () => null, count: records, error: /wrote no --emit-pairs file/ },
    { name: "a record fewer than the census counts", file: () => [], count: () => 1, error: /holds 0 records, the report's pair-source census counts 1/ },
    { name: "a record more than the census counts", file: (r) => [pair(r[0], r[1])], count: () => 0, error: /holds 1 records, the report's pair-source census counts 0/ },
    { name: "a census count of another type", file: (r) => [pair(r[0], r[1])], count: () => "one", error: /candidate census is missing/ },
    { name: "two records for one pair", file: (r) => [pair(r[0], r[1]), pair(r[1], r[0])], count: records, error: /several pair records join/ },
    { name: "a null entry", file: () => [null], count: records, error: /pair record 1 is not schema 1/ },
    { name: "another schema", file: (r) => [pair(r[0], r[1], { schemaVersion: "2" })], count: records, error: /not schema 1/ },
    { name: "no relationships", file: (r) => [pair(r[0], r[1], { relationships: undefined })], count: records, error: /not schema 1/ },
    { name: "rules that are not a list", file: (r) => [pair(r[0], r[1], { rules: "none" })], count: records, error: /not schema 1/ },
    { name: "a rule that is not a name", file: (r) => [pair(r[0], r[1], { rules: [1] })], count: records, error: /not schema 1/ },
    { name: "no positional flag", file: (r) => [pair(r[0], r[1], { positional: undefined })], count: records, error: /not schema 1/ },
    { name: "a relationship that is not a flag", file: (r) => [pair(r[0], r[1], { relationships: { ...pair(r[0], r[1]).relationships, sameHandler: "yes" } })], count: records, error: /not schema 1/ },
    { name: "a relationship missing", file: (r) => [pair(r[0], r[1], { relationships: { exactMatcher: true } })], count: records, error: /not schema 1/ },
    { name: "relationships as a list", file: (r) => [pair(r[0], r[1], { relationships: [] })], count: records, error: /not schema 1/ },
    { name: "a bad record after a good one is located", file: (r) => [pair(r[0], r[1]), { ...pair(r[0], r[1]), schemaVersion: "2" }], count: records, error: /pair record 2 is not schema 1/ },
    { name: "a non-boolean meaningfulHandlers", file: (r) => [pair(r[0], r[1], { meaningfulHandlers: "yes" })], count: records, error: /not schema 1/ },
    { name: "an endpoint without a fingerprint", file: (r) => [pair(r[0], { ...r[1], definitionFingerprint: undefined })], count: records, error: /not schema 1/ },
    { name: "a fingerprint other than the record's", file: (r) => [pair(r[0], { ...r[1], definitionFingerprint: "other" })], count: records, error: /fingerprint for steps.ts:2:1 is not/ },
  ];
  for (const [build, declared, limits] of states) {
    for (const { name, file, count, error } of damage) {
      const list = build();
      const pairs = file(list);
      assert.throws(
        () => checkWith(list, absent(declared), census({ ...limits, evaluated: count(pairs) }), () => pairs),
        error,
        `${declared.compared} with ${name}`,
      );
    }
  }
});

/** Checks a key shared by several fingerprint records cannot be joined to one definition. */
test("a pair is not joined to a key several definitions share", () => {
  const records = [...equalHandlers(), record(2, "three", {}, { definitionFingerprint: "other" })];
  assert.throws(
    () => checkIsolation(records, absent({ compared: true, differs: MATCHER }), { pairs: [pair(records[0], records[1])] }),
    /several definitions share the key steps.ts:2:1/,
  );
  // Control: the same pair joins once the key is unique.
  const unique = [...equalHandlers(), record(3, "three")];
  checkIsolation(unique, absent({ compared: true, differs: MATCHER }), { pairs: [pair(unique[0], unique[1])] });
});

/** Checks paths are joined in the report's form, so invisible characters neither split nor merge keys. */
test("a pair joins by the report's form of the path", () => {
  /** A record whose raw path holds a bidirectional override the report drops. */
  const hidden = (line, matcher) => record(line, matcher, {}, { path: "st\u202eeps.ts" });
  const records = [hidden(1, "one"), hidden(2, "two")];
  checkIsolation(records, absent({ compared: true, differs: MATCHER }), { pairs: [pair(records[0], records[1])] });
  // Participant labels are in the report's form too: the raw path with its invisible character selects the same records.
  const labelled = { compared: true, differs: MATCHER, participants: ["steps.ts:1", "steps.ts:2"] };
  checkIsolation(records, absent(labelled), { pairs: [pair(records[0], records[1])] });
  // A record whose path differs only by an invisible character shares the participant's key.
  const clash = [...records, record(2, "three")];
  assert.throws(
    () => checkIsolation(clash, absent({ compared: true, differs: MATCHER }), { pairs: [pair(records[0], records[1])] }),
    /several definitions share the key steps.ts:2:1/,
  );
});

/** Checks a pair file is read as records, an empty one as none, a missing one as no file and a cut one as unusable. */
test("a pair file is read whole or not at all", () => {
  assert.equal(pairFileReader(null)(), null);
  assert.equal(pairFileReader(undefined)(), null);
  assert.deepEqual(pairFileReader("")(), []);
  assert.deepEqual(pairFileReader('{"a":1}\n{"a":2}\n')(), [{ a: 1 }, { a: 2 }]);
  const cut = pairFileReader('{"a":1}\n{"a":');
  assert.throws(cut, /the pair file does not parse/);
  // The failure is replayed, not parsed again, so every reader of the file sees the same error.
  assert.throws(cut, /the pair file does not parse/);
  // Control: nothing is parsed until the first read, and the records are parsed once.
  pairFileReader('{"a":1}\n{"a":');
  const read = pairFileReader('{"a":1}');
  assert.equal(read(), read());
});

/** Checks the pair file is validated once however many entries read it, observed through a counting getter. */
test("the pair file is validated once per run", () => {
  const records = [...equalHandlers(), record(3, "three")];
  let reads = 0;
  const unrelated = pair(records[0], records[2]);
  const counted = { ...unrelated, get rules() { reads += 1; return []; } };
  const pairs = [pair(records[0], records[1]), counted];
  checkIsolation(records, absent({ compared: true, differs: MATCHER }), { pairs });
  const validation = reads;
  assert.ok(validation > 0);
  for (let entry = 0; entry < 3; entry += 1) checkIsolation(records, absent({ compared: true, differs: MATCHER }), { pairs });
  assert.equal(reads, validation);
});

/** Checks the join index is rebuilt when the same pair file is read against another record list. */
test("the join index follows the record list it was built from", () => {
  const records = equalHandlers();
  const pairs = [pair(records[0], records[1])];
  checkIsolation(records, absent({ compared: true, differs: MATCHER }), { pairs });
  const clashing = [...records, record(2, "three", {}, { definitionFingerprint: "other" })];
  assert.throws(() => checkIsolation(clashing, absent({ compared: true, differs: MATCHER }), { pairs }), /several definitions share the key/);
});

/** Checks a fingerprint record elsewhere in the run without a location fails the join, naming it. */
test("a fingerprint record without a location is named, not a type error", () => {
  const records = [...equalHandlers(), record(3, "three", {}, { column: undefined })];
  assert.throws(
    () => checkIsolation(records, absent({ compared: true, differs: MATCHER }), { pairs: [pair(records[0], records[1])] }),
    /fingerprint record 3 has no path, line and column/,
  );
});

/** Checks a pair record is found whichever way round its endpoints are written. */
test("a pair record joins in either orientation", () => {
  const records = equalHandlers();
  checkIsolation(records, absent({ compared: true, differs: MATCHER }), { pairs: [pair(records[1], records[0])] });
});

/** Checks a pair record for other definitions is not evidence about the participants. */
test("a record for other definitions leaves the participants without one", () => {
  const records = differingThird();
  const other = [pair(records[0], records[2])];
  checkIsolation(records, absent({ compared: "not-proposed", reason: "behavior-differs", differs: BEHAVIOR }), { pairs: other });
  assert.throws(() => checkIsolation(records, absent({ compared: true, differs: BEHAVIOR }), { pairs: other }), /never evaluated/);
});

/** Checks a handler rule compares exactly two participants, while a matcher rule may take more. */
test("a handler isolation compares exactly two participants", () => {
  const records = [record(1, "one"), record(2, "two"), record(3, "three")];
  /** A labelled declaration over all three records. */
  const three = (rule, compared) => ({ rule, matchers: ["one", "two", "three"], isolation: { differs: MATCHER, participants: ["steps.ts:1", "steps.ts:2", "steps.ts:3"], ...compared } });
  assert.throws(() => checkIsolation(records, three("duplicate-handler", { compared: true })), /exactly two participants/);
  checkIsolation(records, three("duplicate-matcher", {}), { pairs: null, analysis: null });
});

/** Checks a matcher rule is decided by the fingerprint records alone, with or without pair records. */
test("a matcher rule needs no pair evidence", () => {
  const records = [record(1, "one"), record(2, "two")];
  const matcher = absent({ differs: MATCHER }, "normalized-matcher");
  checkIsolation(records, matcher, { pairs: null, analysis: null });
  checkIsolation(records, matcher, { pairs: [pair(records[0], records[1], { rules: ["normalized-matcher"] })] });
  assert.throws(() => checkIsolation(records, absent({ compared: true, differs: MATCHER }, "duplicate-matcher")), /not allowed for matcher rules/);
  // The reader is never called for a matcher rule, and a handler rule calls it.
  /** A pair-file reader for a file that does not parse. */
  const unreadable = () => { throw new Error("the pair file does not parse"); };
  checkWith(records, matcher, undefined, unreadable);
  assert.throws(() => checkWith(records, absent({ compared: true, differs: MATCHER }), census({ evaluated: 0 }), unreadable), /does not parse/);
});

/** Checks the pair-record error messages are not reachable for a handler rule without a census. */
test("a handler rule with no analysis section is rejected", () => {
  const records = equalHandlers();
  assert.throws(() => checkWith(records, absent({ compared: true, differs: MATCHER }), undefined, () => everyPair(records)), /candidate census is missing/);
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
  assert.throws(() => checkIsolation([null, record(2, "two")], absent({ compared: true, differs: MATCHER }), { pairs: [] }), /fingerprint record 1 has no path/);
  for (const located of [{ column: undefined }, { line: "1" }, { path: undefined }]) {
    assert.throws(() => checkIsolation([record(1, "one", {}, located), record(2, "two")], absent({ compared: true, differs: MATCHER }), { pairs: [] }), /has no path, line and column/, JSON.stringify(located));
  }
  const foreign = [record(1, "one", {}, { schemaVersion: "2" }), record(2, "two")];
  assert.throws(() => checkIsolation(foreign, absent({ compared: true, differs: MATCHER })), /not schema 1/);
  const headless = [{ ...record(1, "one"), handler: undefined }, record(2, "two")];
  assert.throws(() => checkIsolation(headless, absent({ compared: true, differs: MATCHER }), { pairs: [] }), /not schema 1/);
});

/** Checks malformed declarations and inapplicable rules fail instead of passing. */
test("malformed declarations and feature-step rules are rejected", () => {
  const records = [record(1, "one"), record(2, "two")];
  assert.throws(() => checkIsolation(records, absent({ compared: true, differs: MATCHER, note: "x" })), /unknown isolation keys: note/);
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
  assert.throws(() => checkIsolation(records, absent({ differs: MATCHER, handlerDifference: "unresolved-value" }, "normalized-matcher")), /only for compared: false/);
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

/** The isolated handler and matcher negatives whose rule the case's `.cuke-dedup.json` turns off. */
function disabledTargets(manifest, readConfig) {
  return manifest.cases.flatMap((testCase) => {
    const rules = readConfig(testCase.path)?.rules ?? {};
    return (testCase.expectedAbsent ?? [])
      .filter((entry) => entry.isolation !== undefined && rules[entry.rule] === "off")
      .map((entry) => `${testCase.name}: ${entry.rule}`);
  });
}

/**
 * Checks no isolated negative targets a rule its case switches off. A disabled rule also leaves the
 * pair record's `rules` without it, so the evidence cannot tell; the corpus is audited here instead.
 */
test("no isolated negative targets a rule its case configuration disables", () => {
  const root = new URL("../../../fixtures/recall/", import.meta.url);
  /** The case directory's config, `null` when it has none. */
  const readConfig = (path) => (existsSync(new URL(`${path}/.cuke-dedup.json`, root))
    ? JSON.parse(readFileSync(new URL(`${path}/.cuke-dedup.json`, root), "utf8"))
    : null);
  const manifest = JSON.parse(readFileSync(new URL("manifest.json", root), "utf8"));
  assert.deepEqual(disabledTargets(manifest, readConfig), []);
  // Case arguments select inputs and budgets; a new flag needs a look at whether it can switch a rule off.
  const flags = new Set(manifest.cases.flatMap((testCase) => (testCase.arguments ?? []).filter((argument) => argument.startsWith("--"))));
  assert.deepEqual([...flags].sort(), ["--definitions", "--features", "--max-candidate-comparisons"]);
  // Control: the audit does name an entry whose rule is off, and not one whose rule is on or unconfigured.
  const synthetic = { cases: [{ name: "c", path: "p", expectedAbsent: [{ rule: "duplicate-handler", isolation: {} }, { rule: "near-duplicate-step", isolation: {} }, { rule: "duplicate-handler" }] }] };
  assert.deepEqual(disabledTargets(synthetic, () => ({ rules: { "duplicate-handler": "off" } })), ["c: duplicate-handler"]);
  assert.deepEqual(disabledTargets(synthetic, () => null), []);
});
