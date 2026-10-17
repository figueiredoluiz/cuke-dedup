// An `expectedAbsent` entry passes whenever its finding is missing: when the analyzer told the
// participants apart, but also when it never analyzed one, excluded the pair from comparison, or the
// fixture differs in more than the dimension it claims to pin. An `isolation` declaration states the
// mechanism, and these checks hold it against the `--emit-fingerprints` records of the same run.

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
const DECLARATION_KEYS = new Set(["participants", "compared", "differs"]);

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
  const labels = selected.map((record) => `${record.path}:${record.line}`);
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
    ? isolation.participants.map((label) => [label, (record) => `${record.path}:${record.line}` === label])
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
 * Throws unless the declaration matches the records: participants resolve to one record each, the
 * declared fields are exactly those that differ, and a handler rule's pair was compared or excluded
 * as declared. An excluded pair must have equal handler keys, so differing handlers cannot also
 * explain the absence.
 * @param {object[]} records Records from the same run.
 * @param {object} expected The `expectedAbsent` entry carrying `isolation`.
 */
export function checkIsolation(records, expected) {
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
  if (HANDLER_RULES.has(expected.rule) !== (typeof isolation.compared === "boolean")) {
    throw new Error("compared is required for handler rules and not allowed for matcher rules");
  }
  const selected = participants(records, expected, isolation);
  if (new Set(selected).size !== selected.length) throw new Error("two participants select the same record");
  selected.forEach(assertRecord);
  assertCoversEntry(selected, expected);
  const actual = differingFields(selected);
  const declared = ISOLATION_FIELDS.filter((field) => isolation.differs.includes(field));
  if (actual.join() !== declared.join()) {
    throw new Error(`participants differ in [${actual.join(", ")}], declared [${declared.join(", ")}]`);
  }
  if (isolation.compared === undefined) return;
  const compared = selected.every((record) => record.handler.comparable && !record.handler.trivial);
  if (compared !== isolation.compared) {
    throw new Error(`the pair was ${compared ? "compared" : "excluded from comparison"}, declared ${isolation.compared ? "compared" : "excluded"}`);
  }
  if (!compared && actual.some((field) => HANDLER_KEYS.includes(field))) {
    throw new Error("an excluded pair whose handlers also differ is absent for two reasons; make the handlers equal");
  }
}
