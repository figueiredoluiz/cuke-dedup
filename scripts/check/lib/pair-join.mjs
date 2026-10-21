// Joins `--emit-pairs` records to `--emit-fingerprints` records for the isolation gate. The gate and
// the candidate-recall report share the definition key and the pair-source census; only the gate
// requires the join to be exact.

/**
 * The path as the JSON report writes it. Reports drop control characters other than line breaks and
 * tabs, plus bidirectional and invisible-format characters (`report_safe` in the reporters), while
 * `--emit-pairs` and `--emit-fingerprints` keep them raw; keys are compared in the report's form.
 * Two paths that differ only by such characters share a key.
 * @param {string} path
 * @returns {string}
 */
export function reportPath(path) {
  return path.replace(/(?![\n\r\t])[\p{Cc}\u00ad\u061c\u200b-\u200f\u202a-\u202e\u2060-\u2069\ufeff\u{e0000}-\u{e007f}]/gu, "");
}

/**
 * The `path:line` label of a record, with the path in the form the JSON report writes it.
 * @param {{path: string, line: number}} record
 * @returns {string}
 */
export function recordLabel({ path, line }) {
  return `${reportPath(path)}:${line}`;
}

/**
 * The key of a definition: path, line and column, since several registrations can share a line.
 * @param {{path: string, line: number, column: number}} location
 * @returns {string}
 */
export function definitionKey({ path, line, column }) {
  return `${reportPath(path)}:${line}:${column}`;
}

/** The census source that compares steps and writes no `--emit-pairs` record. */
const NON_PAIR_SOURCE = "matcherOverlap";

/** The sources that write `--emit-pairs` records. */
export const PAIR_SOURCES = Object.freeze(["normalizedMatcher", "identicalHandler", "structuralHandler", "matcherBlocking"]);

/** The relationships a pair record carries, each a boolean. */
const RELATIONSHIPS = Object.freeze(["exactMatcher", "normalizedMatcher", "sameHandler", "sameHandlerStructure", "sameStructure", "sameDeferredAssertions"]);

/**
 * Reads the census of the pair-proposing sources: pairs evaluated and comparisons skipped, summed
 * over `PAIR_SOURCES`, and the structural classes truncated. The report's totals and its `truncated`
 * flag also cover matcher overlap, which writes no pair record, so they are not read.
 * @param {object|undefined} analysis The report's `analysis` section.
 * @returns {{census: {evaluated: number, skipped: number, limited: boolean}}|{problem: string}} The
 *   census, where `limited` is true when a source skipped a comparison or a structural class was
 *   truncated; or why it is unreadable: a source this module does not know could hold pairs or skips
 *   it would miss, and a missing source or a count that is not an integer says nothing.
 */
export function readCensus(analysis) {
  const unmodeled = Object.keys(analysis?.candidateSources ?? {}).filter((name) => !PAIR_SOURCES.includes(name) && name !== NON_PAIR_SOURCE);
  if (unmodeled.length > 0) return { problem: `names sources this check does not model: ${unmodeled.join(", ")}` };
  const sources = PAIR_SOURCES.map((name) => analysis?.candidateSources?.[name]);
  const evaluated = sources.map((source) => source?.evaluated);
  const skipped = sources.map((source) => source?.skipped);
  const truncatedClasses = analysis?.truncatedStructuralClasses;
  if (![...evaluated, ...skipped, truncatedClasses].every(Number.isSafeInteger)) {
    return { problem: "is missing a pair-proposing source or holds a count that is not an integer" };
  }
  /** Sum of a list of counts. */
  const add = (counts) => counts.reduce((total, count) => total + count, 0);
  return { census: { evaluated: add(evaluated), skipped: add(skipped), limited: add(skipped) > 0 || truncatedClasses > 0 } };
}

/**
 * Throws unless the pair file is the whole set of pairs the run evaluated: the census counts every
 * evaluated pair, so a file with another count was cut short or does not belong to the run, and a
 * missing pair is no evidence of anything.
 * @param {object[]|null} pairs Parsed `--emit-pairs` records, `null` when the run wrote none.
 * @param {object|undefined} analysis The report's `analysis` section.
 * @returns {{evaluated: number, skipped: number, limited: boolean}} The census that matched.
 */
export function assertPairFile(pairs, analysis) {
  if (pairs === null) throw new Error("the run wrote no --emit-pairs file, so no pair can be shown evaluated or absent");
  const { census, problem } = readCensus(analysis);
  if (census === undefined) throw new Error(`the report's candidate census ${problem}`);
  if (census.evaluated !== pairs.length) {
    throw new Error(`the pair file holds ${pairs.length} records, the report's pair-source census counts ${census.evaluated} evaluated pairs`);
  }
  return census;
}

/** Throws unless the pair record at `position` has schema 1 and the fields an isolation reads. */
function assertPairRecord(pair, position) {
  const sides = [pair?.left, pair?.right];
  if (
    pair?.schemaVersion !== "1"
    || sides.some((side) => typeof side?.path !== "string" || !Number.isSafeInteger(side.line) || !Number.isSafeInteger(side.column) || typeof side.definitionFingerprint !== "string")
    || typeof pair.meaningfulHandlers !== "boolean"
    || typeof pair.positional !== "boolean"
    || !Array.isArray(pair.rules) || pair.rules.some((rule) => typeof rule !== "string")
    || RELATIONSHIPS.some((name) => typeof pair.relationships?.[name] !== "boolean")
  ) {
    throw new Error(`pair record ${position + 1} is not schema 1 with its endpoints, meaningfulHandlers, positional, relationships and rules`);
  }
}

/**
 * Throws unless every fingerprint record has the path, line and column a join keys on.
 * @param {object[]} records Every fingerprint record of the run.
 */
export function assertLocated(records) {
  records.forEach((record, position) => {
    if (typeof record?.path !== "string" || !Number.isSafeInteger(record.line) || !Number.isSafeInteger(record.column)) {
      throw new Error(`fingerprint record ${position + 1} has no path, line and column to join on`);
    }
  });
}

/**
 * The unordered key of a set of definition keys.
 * @param {string[]} keys
 * @returns {string}
 */
export function setKey(keys) {
  return JSON.stringify([...keys].sort());
}

/** The unordered key of two definition keys. */
const pairKey = (first, second) => setKey([first, second]);

/**
 * Throws unless every pair record has the schema an isolation reads and no endpoint pair has two
 * records, since the join could not tell which one stands for the pair.
 * @param {object[]} pairs Every pair record of the run.
 */
export function assertPairRecords(pairs) {
  pairs.forEach(assertPairRecord);
  const seen = new Set();
  for (const pair of pairs) {
    const key = pairKey(definitionKey(pair.left), definitionKey(pair.right));
    if (seen.has(key)) throw new Error(`several pair records join ${key}`);
    seen.add(key);
  }
}

/** Join indexes, held weakly by the fingerprint record array and then the pair array they were built from. */
const indexes = new WeakMap();

/**
 * Validates every pair record and counts the fingerprint records and pair records at each key.
 * @param {object[]} records Every fingerprint record of the run.
 * @param {object[]} pairs Every pair record of the run.
 * @returns {{definitions: Map<string, number>, joined: Map<string, object>}}
 */
function indexOfRun(records, pairs) {
  const cached = indexes.get(records)?.get(pairs);
  if (cached !== undefined) return cached;
  assertPairRecords(pairs);
  const definitions = new Map();
  for (const record of records) {
    const key = definitionKey(record);
    definitions.set(key, (definitions.get(key) ?? 0) + 1);
  }
  const joined = new Map(pairs.map((pair) => [pairKey(definitionKey(pair.left), definitionKey(pair.right)), pair]));
  const index = { definitions, joined };
  if (!indexes.has(records)) indexes.set(records, new WeakMap());
  indexes.get(records).set(pairs, index);
  return index;
}

/**
 * The pair record for two definitions, or `null` when the run evaluated none. Fails closed when the
 * join is not exact: a key shared by several definitions, a pair file with several records for one
 * pair, or an endpoint whose definition fingerprint differs from the fingerprint record's.
 * @param {object} left Fingerprint record of one participant.
 * @param {object} right Fingerprint record of the other.
 * @param {object[]} records Every fingerprint record of the run.
 * @param {object[]} pairs Every pair record of the run.
 * @returns {object|null}
 */
export function pairOf(left, right, records, pairs) {
  const index = indexOfRun(records, pairs);
  const [leftKey, rightKey] = [definitionKey(left), definitionKey(right)];
  for (const key of [leftKey, rightKey]) {
    if (index.definitions.get(key) !== 1) {
      throw new Error(`several definitions share the key ${key}, so a pair record cannot be joined to one definition`);
    }
  }
  const pair = index.joined.get(pairKey(leftKey, rightKey));
  if (pair === undefined) return null;
  const byKey = new Map([[definitionKey(pair.left), pair.left], [definitionKey(pair.right), pair.right]]);
  for (const record of [left, right]) {
    if (byKey.get(definitionKey(record)).definitionFingerprint !== record.definitionFingerprint) {
      throw new Error(`the pair record's fingerprint for ${definitionKey(record)} is not the fingerprint record's`);
    }
  }
  return pair;
}
