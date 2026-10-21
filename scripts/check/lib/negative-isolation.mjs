// An `expectedAbsent` entry passes whenever its finding is missing: when the analyzer told the
// participants apart, but also when it never analyzed one, excluded the pair from comparison, never
// proposed it, or the fixture differs in more than the dimension it claims to pin. An `isolation`
// declaration states the mechanism, and these checks hold it against the `--emit-fingerprints` and
// `--emit-pairs` records of the same run.

import { assertLocated, assertPairFile, pairOf, recordLabel } from "./pair-join.mjs";

/** Rules decided from per-definition records; the others also depend on feature steps. */
const HANDLER_RULES = new Set(["duplicate-handler", "near-duplicate-step", "parameterization-candidate"]);
const MATCHER_RULES = new Set(["duplicate-matcher", "normalized-matcher"]);

/** Record fields an isolation compares; locations and the composite fingerprint never isolate. */
export const ISOLATION_FIELDS = Object.freeze([
  "domain",
  "framework",
  "registration",
  "matcher",
  "matcherKind",
  "matcherFlags",
  "normalizedMatcher",
  "handler.exact",
  "handler.normalized",
  "handler.alphaNormalized",
  "handler.structural",
  "handler.behavior",
]);
const HANDLER_KEYS = ISOLATION_FIELDS.filter((field) => field.startsWith("handler."));
const DECLARATION_KEYS = new Set(["participants", "compared", "differs", "reason", "handlerDifference"]);
/** `compared` values naming a pair the analyzer never evaluated, each with the reason it requires. */
const UNEVALUATED = Object.freeze({ "not-proposed": "behavior-differs", "unevaluated-under-limit": "candidate-limit" });
/** The one explanation for handlers that differ although the pair is excluded from comparison. */
const UNRESOLVED_VALUE = "unresolved-value";

/**
 * Whether an `expectedAbsent` rule's isolation reads the run's pair records: the handler rules.
 * @param {string} rule
 * @returns {boolean}
 */
export function readsPairs(rule) {
  return HANDLER_RULES.has(rule);
}

/**
 * Whether an `expectedAbsent` rule can carry an isolation declaration.
 * @param {string} rule
 * @returns {boolean}
 */
export function isolatable(rule) {
  return HANDLER_RULES.has(rule) || MATCHER_RULES.has(rule);
}

/**
 * Parses `--emit-fingerprints` output into records.
 * @param {string} text
 * @returns {object[]}
 */
export function parseRecords(text) {
  return text.split("\n").filter(Boolean).map((line) => JSON.parse(line));
}

/**
 * A reader for the `--emit-pairs` file of a run. It parses on first use and replays its records, or
 * its error, afterwards, so a file that does not parse (one cut short, for example) fails the
 * isolations that read it and no other check of the case.
 * @param {string|null|undefined} text The file's contents, `null` when the run wrote none.
 * @returns {() => object[]|null} The records, `null` when the run wrote none; throws when the file does not parse.
 */
export function pairFileReader(text) {
  let outcome;
  return () => {
    if (outcome === undefined) {
      outcome = { records: null };
      if (typeof text === "string") {
        try {
          outcome = { records: parseRecords(text) };
        } catch (error) {
          outcome = { error: new Error(`the pair file does not parse: ${error.message}`) };
        }
      }
    }
    if (outcome.error !== undefined) throw outcome.error;
    return outcome.records;
  };
}

/** A record field by its dotted name; `framework` is an object, so it compares as JSON. */
function fieldValue(record, field) {
  const value = field.startsWith("handler.") ? record.handler[field.slice(8)] : record[field];
  return field === "framework" ? JSON.stringify(value) : value;
}

/** Throws unless a record carries schema 1 and every field an isolation reads. */
function assertRecord(record) {
  const missing = ISOLATION_FIELDS.filter((field) => record.handler === undefined || fieldValue(record, field) === undefined);
  if (record.schemaVersion !== "1" || missing.length > 0
    || typeof record.handler.comparable !== "boolean" || typeof record.handler.trivial !== "boolean") {
    throw new Error(`fingerprint record ${record.path}:${record.line} is not schema 1 with every isolation field (missing: ${missing.join(", ") || "comparable or trivial"})`);
  }
}

/** Same items with the same multiplicity, in any order. */
function sameMultiset(left, right) {
  return JSON.stringify([...left].sort()) === JSON.stringify([...right].sort());
}

/**
 * Throws unless the participants are the entry's own: exactly its `matchers` and, with
 * `relatedLocations`, exactly its primary and related locations; a primary alone must be included.
 * Labels therefore cannot certify another pair, and an extra record cannot decide `compared`.
 */
function assertCoversEntry(selected, expected) {
  const matchers = selected.map((record) => record.matcher);
  const labels = selected.map(recordLabel);
  if (Boolean(expected.primaryPath) !== Boolean(expected.primaryLine)) {
    throw new Error("an isolated entry names primaryPath and primaryLine together or neither");
  }
  const primary = expected.primaryPath ? [`${expected.primaryPath}:${expected.primaryLine}`] : [];
  const located = [...primary, ...(expected.relatedLocations ?? [])];
  if (expected.matchers && !sameMultiset(matchers, expected.matchers)) {
    throw new Error(`participants have matchers ${JSON.stringify(matchers)}, the entry names ${JSON.stringify(expected.matchers)}`);
  }
  if (expected.relatedLocations ? !sameMultiset(labels, located) : !primary.every((label) => labels.includes(label))) {
    throw new Error(`participants are ${JSON.stringify(labels)}, the entry locates ${JSON.stringify(located)}`);
  }
}

/** One record per participant label (`path:line`) or, without labels, per entry matcher. */
function participants(records, expected, isolation) {
  const selectors = isolation.participants
    ? isolation.participants.map((label) => [label, (record) => recordLabel(record) === label])
    : (expected.matchers ?? []).map((matcher) => [matcher, (record) => record.matcher === matcher]);
  if (selectors.length < 2) {
    throw new Error("isolation needs two or more participants, from `participants` or the entry's `matchers`");
  }
  return selectors.map(([name, selects]) => {
    const selected = records.filter(selects);
    if (selected.length !== 1) {
      throw new Error(`participant ${JSON.stringify(name)} selects ${selected.length} records; it must select one (use \`participants\` labels)`);
    }
    return selected[0];
  });
}

/**
 * Fields whose values are not all equal across the records, in `ISOLATION_FIELDS` order.
 * @param {object[]} records
 * @returns {string[]}
 */
export function differingFields(records) {
  return ISOLATION_FIELDS.filter((field) => new Set(records.map((record) => fieldValue(record, field))).size > 1);
}

/**
 * Throws unless the pair record agrees with the fingerprint records: the analyzer's own equalities
 * must match the fields the declaration says differ. A matcher equality is claimed only where the
 * fields decide it: raw flags that differ, or read by another dialect, may still have equal effective flags, and a positional
 * definition is its own matcher class, so a positional pair is never normalized-equal.
 */
function assertRelationships(pair, actual) {
  /** Whether any of the fields is among the ones the participants differ in. */
  const differ = (...fields) => fields.some((field) => actual.includes(field));
  const { relationships } = pair;
  /** Whether the matcher fields say the matchers are equal, `undefined` when the flags or their dialect leave it open. */
  const matchersEqual = (...fields) => (differ(...fields) ? false : differ("matcherFlags", "domain") ? undefined : true);
  const expected = {
    sameHandler: !differ("handler.alphaNormalized", "handler.behavior"),
    sameHandlerStructure: !differ("handler.structural"),
    sameStructure: !differ("handler.structural", "handler.behavior"),
    exactMatcher: matchersEqual("matcher", "matcherKind"),
    normalizedMatcher: pair.positional ? false : matchersEqual("normalizedMatcher", "matcherKind"),
  };
  const wrong = Object.entries(expected)
    .filter(([name, value]) => value !== undefined && relationships[name] !== value)
    .map(([name]) => name);
  // Deferred assertions are part of the behavior signature, so a pair that differs in them differs in behavior.
  if (relationships.sameDeferredAssertions === false && !differ("handler.behavior")) wrong.push("sameDeferredAssertions");
  if (wrong.length > 0) {
    throw new Error(`the pair record's relationships disagree with the fingerprint records on: ${wrong.join(", ")}`);
  }
}

/**
 * Throws unless an excluded pair whose handlers differ declares why, and the records back it: the
 * difference is a value the analyzer could not resolve, so both participants are non-comparable and
 * their behavior is equal. Without that, differing handlers would explain the absence on their own.
 */
function assertUnresolvedValue(isolation, selected, actual) {
  const differing = actual.some((field) => HANDLER_KEYS.includes(field));
  if (isolation.handlerDifference === undefined) {
    if (differing) throw new Error("an excluded pair whose handlers also differ is absent for two reasons; make the handlers equal or declare handlerDifference");
    return;
  }
  if (isolation.handlerDifference !== UNRESOLVED_VALUE || !differing) {
    throw new Error(`handlerDifference is "${UNRESOLVED_VALUE}", only for an excluded pair whose handlers differ`);
  }
  if (selected.some((record) => record.handler.comparable) || actual.includes("handler.behavior")) {
    throw new Error(`${UNRESOLVED_VALUE} needs both participants non-comparable and equal in handler.behavior`);
  }
}

/**
 * Throws unless the declared `compared` state is what the records and pair evidence show for a
 * handler rule's pair. `true` needs a pair record the analyzer judged meaningful; `false` needs an
 * exclusion visible in the fingerprints or in the pair record; `not-proposed` and `unevaluated-under-limit`
 * need no record and the matching census, and name why through `reason`.
 */
function checkHandlerPair(expected, isolation, selected, actual, evidence) {
  const state = isolation.compared;
  const census = assertPairFile(evidence.pairs, evidence.analysis);
  const skipped = census.limited;
  const pair = pairOf(selected[0], selected[1], evidence.records, evidence.pairs);
  if (pair !== null) {
    assertRelationships(pair, actual);
    if (pair.rules.includes(expected.rule)) {
      throw new Error(`the pair record lists ${expected.rule}, so the finding was produced and then suppressed or collapsed; the absence is not isolated`);
    }
  }
  const eligible = selected.every((record) => record.handler.comparable && !record.handler.trivial);
  if (state === true) {
    if (!eligible) throw new Error("the pair was excluded from comparison, declared compared");
    if (pair === null) throw new Error("the pair has no record, so the analyzer never evaluated it; declare not-proposed or unevaluated-under-limit with a reason, or rework the fixture");
    if (!pair.meaningfulHandlers) throw new Error("the pair was excluded from comparison (the analyzer judged the handlers runtime-incompatible), declared compared");
    return;
  }
  if (state === false) {
    if (!eligible && pair?.meaningfulHandlers) throw new Error("the pair record contradicts the fingerprints: meaningful handlers although a participant is not comparable or is trivial");
    if (eligible && pair === null) throw new Error("both handlers are comparable and the analyzer recorded no evaluation, so nothing shows the pair was excluded");
    if (eligible && pair.meaningfulHandlers) throw new Error("the pair was compared, declared excluded");
    // The analyzer's runtime-compatibility verdict turns on the handler domain (a function of the
    // framework: scenario-local, plugin-global or shared), the lexical file (Ruby only) and the method
    // semantics in the behavior signature. A pair excluded for the first two differs in `framework`
    // or, in Ruby, in file; the third would be a handler difference, a second reason for the absence.
    const otherRubyFile = selected.every((record) => record.domain === "ruby") && selected[0].path !== selected[1].path;
    if (eligible && !actual.includes("framework") && !otherRubyFile) {
      throw new Error("the analyzer excluded the pair, but the participants share a framework (and, for Ruby, a file), so nothing visible explains it");
    }
    assertUnresolvedValue(isolation, selected, actual);
    return;
  }
  if (!eligible) throw new Error(`a participant is not comparable or is trivial, so the pair is excluded, not ${state}; declare compared: false`);
  if (pair !== null) throw new Error(`the pair has a record, so the analyzer evaluated it; declared ${state}`);
  // Either way an unevaluated pair with equal grouping keys would be unexplained: the sources group by them.
  if (!["handler.behavior", "normalizedMatcher"].every((field) => actual.includes(field))) {
    throw new Error("behavior-differs needs participants that differ in handler.behavior and normalizedMatcher, the keys the candidate sources group by");
  }
  if (state === "not-proposed" && skipped) throw new Error("not-proposed needs a run that skipped no candidate: a limit may have dropped the pair");
  if (state === "unevaluated-under-limit" && !skipped) throw new Error("unevaluated-under-limit needs a run whose candidate limits skipped comparisons");
}

/**
 * Throws unless the declaration matches the records: participants resolve to one record each, the
 * declared fields are exactly those that differ, and a handler rule's pair stands as declared. The
 * fingerprint records show only eligibility, so a handler rule's `compared` is also held against
 * the run's `--emit-pairs` records: `true` needs the analyzer's own record of the pair, `false` an
 * exclusion visible in the evidence, and `not-proposed` or `unevaluated-under-limit` no record plus a
 * reason the census supports. Matcher rules are decided by the records' keys alone.
 * @param {object[]} records Fingerprint records from the same run.
 * @param {object} expected The `expectedAbsent` entry carrying `isolation`.
 * @param {object} analysis The JSON report's `analysis` section from the same run.
 * @param {() => object[]|null} [readPairs] Reads the pair records of the same run, `null` when it
 *   wrote none; only a handler rule calls it, and it may throw when the file does not parse.
 */
export function checkIsolation(records, expected, analysis, readPairs = () => null) {
  const { isolation } = expected;
  if (!isolatable(expected.rule)) {
    throw new Error(`${expected.rule} also depends on feature steps; isolation covers handler and matcher rules only`);
  }
  if (typeof isolation !== "object" || isolation === null || Array.isArray(isolation)) {
    throw new Error("isolation must be an object");
  }
  const unknown = Object.keys(isolation).filter((key) => !DECLARATION_KEYS.has(key));
  if (unknown.length > 0) throw new Error(`unknown isolation keys: ${unknown.join(", ")}`);
  if (
    !Array.isArray(isolation.differs)
    || isolation.differs.some((field) => !ISOLATION_FIELDS.includes(field))
    || new Set(isolation.differs).size !== isolation.differs.length
  ) {
    throw new Error(`differs must list distinct fields from: ${ISOLATION_FIELDS.join(", ")}`);
  }
  const handlerRule = readsPairs(expected.rule);
  if (handlerRule === (isolation.compared === undefined)) {
    throw new Error("compared is required for handler rules and not allowed for matcher rules");
  }
  const unevaluated = typeof isolation.compared === "string" && Object.hasOwn(UNEVALUATED, isolation.compared);
  if (handlerRule && !unevaluated && typeof isolation.compared !== "boolean") {
    throw new Error(`compared must be true, false, ${Object.keys(UNEVALUATED).join(" or ")}`);
  }
  if (unevaluated && isolation.reason !== UNEVALUATED[isolation.compared]) {
    throw new Error(`${isolation.compared} needs reason: ${UNEVALUATED[isolation.compared]}`);
  }
  if (!unevaluated && isolation.reason !== undefined) {
    throw new Error("reason is allowed only with not-proposed and unevaluated-under-limit");
  }
  if (isolation.handlerDifference !== undefined && isolation.compared !== false) {
    throw new Error("handlerDifference is only for compared: false");
  }
  assertLocated(records);
  const selected = participants(records, expected, isolation);
  if (new Set(selected).size !== selected.length) throw new Error("two participants select the same record");
  selected.forEach(assertRecord);
  assertCoversEntry(selected, expected);
  if (handlerRule && selected.length !== 2) throw new Error("a handler rule's isolation compares exactly two participants");
  const actual = differingFields(selected);
  const declared = ISOLATION_FIELDS.filter((field) => isolation.differs.includes(field));
  if (actual.join() !== declared.join()) {
    throw new Error(`participants differ in [${actual.join(", ")}], declared [${declared.join(", ")}]`);
  }
  if (handlerRule) checkHandlerPair(expected, isolation, selected, actual, { records, pairs: readPairs(), analysis });
}
