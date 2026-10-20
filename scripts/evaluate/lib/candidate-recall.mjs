// Candidate recall: of the pairs a case declares should be reported, how many did a candidate
// source propose, so the verifier ever saw them. A pair the verifier never saw cannot become a
// finding, whatever the matchers and handlers say, so this separates a generation miss from a
// verification miss. It reads `--emit-pairs` records and asserts nothing.

import { parseRecords } from "../../check/lib/negative-isolation.mjs";
import { partitionAmbiguous } from "../../check/lib/manifest-schema.mjs";
import { countMatches } from "../../check/lib/recall-oracle.mjs";
import { isOperationalExit } from "./classify.mjs";

/** Rules decided on definition pairs; the others have no pair record to join. */
export const PAIR_RULES = Object.freeze([
  "duplicate-matcher",
  "normalized-matcher",
  "duplicate-handler",
  "near-duplicate-step",
  "parameterization-candidate",
]);

/** The unordered key of a set of definition keys. */
function setKey(keys) {
  return [...keys].sort().join("|");
}

/**
 * The path as the JSON report writes it. Reports drop control characters other than line breaks and
 * tabs, plus bidirectional and invisible-format characters (`report_safe` in the reporters), while
 * `--emit-pairs` and `--emit-fingerprints` keep them raw; keys are compared in the report's form.
 * Two paths that differ only by such characters share a key.
 */
function reportPath(path) {
  return path.replace(/(?![\n\r\t])[\p{Cc}\u00ad\u061c\u200b-\u200f\u202a-\u202e\u2060-\u2069\ufeff\u{e0000}-\u{e007f}]/gu, "");
}

/** The key of a definition: path, line and column, since several registrations can share a line. */
function definitionKey({ path, line, column }) {
  return `${reportPath(path)}:${line}:${column}`;
}

/**
 * The definitions that could stand at each end of an expected pair: the ones at its declared
 * locations when it names both endpoints, else the ones with its matcher text.
 * @param {object} expected Manifest expectation.
 * @param {object[]} records Parsed `--emit-fingerprints` records.
 * @returns {{ends: string[][]}|null} The candidate definition keys per end; `null` when the
 *   expectation names neither, or names a selector this join cannot apply.
 */
function endpointsOf(expected, records) {
  /** The definition keys of the records a test selects. */
  const keysWhere = (test) => records.filter(test).map(definitionKey);
  const hasLocation = expected.primaryPath || expected.primaryLine || expected.relatedLocations;
  const located = expected.primaryPath && expected.primaryLine && Array.isArray(expected.relatedLocations) && expected.relatedLocations.length === 1;
  // A selector this join cannot apply would let an unrelated pair stand in for the expected one, so
  // the pair is left unresolved: a message, a cluster shape, a related count or a partial location.
  if (expected.messageIncludes || expected.cluster !== undefined || expected.relatedCount !== undefined || (hasLocation && !located)) return null;
  if (located) {
    const labels = [`${expected.primaryPath}:${expected.primaryLine}`, expected.relatedLocations[0]];
    return { ends: labels.map((label) => keysWhere((record) => `${reportPath(record.path)}:${record.line}` === label)) };
  }
  if (Array.isArray(expected.matchers) && expected.matchers.length === 2) {
    const [left, right] = expected.matchers;
    return { ends: [keysWhere((record) => record.matcher === left), keysWhere((record) => record.matcher === right)] };
  }
  return null;
}

/**
 * Whether the run evaluated every candidate pair it generated. When a limit stopped candidate
 * generation or verification, a pair with no record may have been skipped rather than never
 * proposed, because `--emit-pairs` records only the pairs that were evaluated.
 * @param {object|undefined} analysis The report's `analysis` section.
 * @returns {boolean}
 */
function complete(analysis) {
  return analysis?.truncated === false && analysis.skippedCandidateComparisons === 0 && analysis.truncatedStructuralClasses === 0;
}

/**
 * Where an expected pair stands when no finding was reported for it, resolved through the
 * definitions the run extracted: not extracted, extracted but not evaluated, or evaluated without a
 * finding. Pairs already credited to a reported finding are not evidence about the missing one.
 * @param {object} expected Manifest expectation naming its pair by locations or `matchers`.
 * @param {object} context
 * @param {object[]} context.records Parsed `--emit-fingerprints` records.
 * @param {Map<string, object>} context.pairsByKey Pair records by endpoint key.
 * @param {Set<string>} context.reported `rule|pair` keys credited to reported findings.
 * @param {boolean} context.complete Whether the run evaluated every pair it generated.
 * @returns {"definitions-not-extracted"|"never-proposed"|"skipped-by-limit"|"evaluated-no-finding"|"unresolved"}
 */
function pairState(expected, { records, pairsByKey, reported, complete: exhaustive }) {
  const endpoints = endpointsOf(expected, records);
  if (endpoints === null) return "unresolved";
  const [lefts, rights] = endpoints.ends;
  // The distinct unordered pairs that could stand for the expectation and are not already credited
  // to a finding of its rule. None left means the run extracted too few definitions, not that the
  // generator missed one.
  const remaining = new Map();
  for (const a of lefts) for (const b of rights) {
    const key = setKey([a, b]);
    if (a !== b && !reported.has(`${expected.rule}|${key}`)) remaining.set(key, pairsByKey.has(key));
  }
  if (remaining.size === 0) return "definitions-not-extracted";
  if ([...remaining.values()].some(Boolean)) return "evaluated-no-finding";
  return exhaustive ? "never-proposed" : "skipped-by-limit";
}

/**
 * Joins a case's declared pair positives and known misses to its pair records.
 * @param {object} testCase Recall manifest case.
 * @param {{status: number|null, stderr: string, spawnFailure: string|null, report: object|null,
 *   fingerprintsText: string|null, pairsText: string|null}} observation Result of
 *   `observeRecallCase` with `emitPairs`.
 * @returns {{name: string, observable: boolean, runFailure: string|null, positives: object[],
 *   discarded: object[]}} Reported positives with the sources that proposed them, and the declared
 *   pairs that were never reported, with where each stopped.
 */
export function candidateRecallForCase(testCase, observation) {
  const { report, status, spawnFailure, pairsText } = observation;
  // A tool failure, or a run that wrote no pair file, cannot be joined.
  if (report === null || pairsText === null || spawnFailure !== null || isOperationalExit(status, testCase)) {
    const cause = spawnFailure ?? `exit ${status}${report === null ? " wrote no JSON report" : pairsText === null ? " wrote no pair records" : ""}: ${observation.stderr}`.trim();
    return { name: testCase.name, observable: false, runFailure: cause, positives: [], discarded: [], anomalies: [] };
  }
  const pairs = parseRecords(pairsText);
  const records = parseRecords(observation.fingerprintsText ?? "");
  const pairsByKey = new Map(pairs.map((pair) => [setKey([definitionKey(pair.left), definitionKey(pair.right)]), pair]));
  // Ownership is decided as the classifier decides it: ambiguous findings are set aside, and one pass
  // over positive expectations and known misses attributes each finding to its single claimant.
  const claimants = [
    ...(testCase.expectedFindings || []).map((expected) => ({ expected, miss: false })),
    ...(testCase.knownMisses || []).map((expected) => ({ expected, miss: true })),
  ].filter(({ expected }) => PAIR_RULES.includes(expected.rule));
  const active = report.findings.filter((finding) => finding.suppression === null && PAIR_RULES.includes(finding.rule));
  const { owned } = partitionAmbiguous(active, testCase.expectedAmbiguous || []);
  const attributed = new Map(claimants.map((claimant) => [claimant, []]));
  const anomalies = [];
  for (const finding of owned) {
    const owners = claimants.filter(({ expected }) => countMatches([finding], expected) > 0);
    if (owners.length === 1) attributed.get(owners[0]).push(finding);
    // Two claimants make the finding's attribution ambiguous, so it joins no positive; the gate rejects
    // such a corpus, and the report names it instead of dropping it silently.
    else if (owners.length > 1) anomalies.push({ case: testCase.name, rule: finding.rule, reason: `claimed by ${owners.length} expectations`, at: [finding.primary, ...finding.related].map(definitionKey) });
  }

  const positives = [];
  const discarded = [];
  // Pairs credited to a reported finding, so a counted shortfall is diagnosed from the others.
  const reported = new Set();
  const context = { records, pairsByKey, reported, complete: complete(report.analysis) };
  for (const [{ expected, miss }, findings] of attributed) {
    for (const finding of findings.slice(0, miss ? 1 : expected.count ?? 1)) {
      // The report keeps a bounded number of related locations. A cluster beyond it has members the
      // join cannot see, so its supporting pairs cannot be told from the rest.
      if (finding.relatedLocationsTruncated) {
        anomalies.push({ case: testCase.name, rule: finding.rule, reason: "related locations truncated", at: [finding.primary, ...finding.related].map(definitionKey) });
        continue;
      }
      const locations = [finding.primary, ...finding.related];
      const keys = locations.map(definitionKey);
      const members = new Set(keys);
      // A pair finding has one record. A cluster finding stands for the pairs among its members that
      // produced this rule: another rule's pair between the same members says nothing about it.
      const proposing = keys.length === 2
        ? [pairsByKey.get(setKey(keys))].filter(Boolean)
        : pairs.filter((pair) => members.has(definitionKey(pair.left)) && members.has(definitionKey(pair.right)) && pair.rules.includes(finding.rule));
      for (const pair of proposing) reported.add(`${finding.rule}|${setKey([definitionKey(pair.left), definitionKey(pair.right)])}`);
      positives.push({
        case: testCase.name,
        rule: finding.rule,
        cluster: keys.length > 2,
        at: keys,
        reached: proposing.length > 0,
        sources: [...new Set(proposing.flatMap((pair) => pair.candidateSources))].sort(),
      });
    }
  }
  // Shortfalls are diagnosed after every reported pair is known, so no credited pair stands in for a
  // missing one whichever expectation came first.
  for (const [{ expected, miss }, findings] of attributed) {
    const wanted = miss ? 1 : expected.count ?? 1;
    const missing = wanted - Math.min(findings.length, wanted);
    if (missing > 0) discarded.push({ case: testCase.name, rule: expected.rule, kind: miss ? "known-miss" : "expected", missing, expected, state: pairState(expected, context) });
  }
  return { name: testCase.name, observable: true, runFailure: null, positives, discarded, anomalies };
}

/**
 * Observes and joins one case; a failure while observing makes only that case unobservable.
 * @param {object} testCase Recall manifest case.
 * @param {() => Promise<object>} observe Produces the case's observation.
 * @returns {Promise<object>} The join, or an unobservable result with the error message.
 */
export async function recallForCase(testCase, observe) {
  try {
    return candidateRecallForCase(testCase, await observe());
  } catch (error) {
    return { name: testCase.name, observable: false, runFailure: `observation failed: ${error.message}`, positives: [], discarded: [], anomalies: [] };
  }
}

/**
 * Sums joined cases into per-rule candidate recall and per-source counts. A source's `sole` count is
 * the positives only it proposed: the findings lost if that source were removed.
 * @param {object[]} results Results of {@link recallForCase}.
 * @returns {{cases: number, observed: number, unobservable: object[], rules: object[], totals: object,
 *   sources: object[], discarded: object[], anomalies: object[]}} Rules and sources sorted by name.
 */
export function aggregateCandidateRecall(results) {
  const observed = results.filter((result) => result.observable);
  const positives = observed.flatMap((result) => result.positives);
  /** Positives, those that reached verification and their ratio, for a set of rows. */
  const summarize = (rows) => ({
    positives: rows.length,
    reached: rows.filter((row) => row.reached).length,
    candidateRecall: rows.length === 0 ? null : Math.round((rows.filter((row) => row.reached).length / rows.length) * 10000) / 10000,
  });
  const rules = PAIR_RULES.map((rule) => ({ rule, ...summarize(positives.filter((row) => row.rule === rule)) }))
    .filter((row) => row.positives > 0);
  const names = [...new Set(positives.flatMap((row) => row.sources))].sort();
  const sources = names.map((source) => ({
    source,
    proposed: positives.filter((row) => row.sources.includes(source)).length,
    sole: positives.filter((row) => row.sources.length === 1 && row.sources[0] === source).length,
  }));
  return {
    cases: results.length,
    observed: observed.length,
    unobservable: results.filter((result) => !result.observable).map(({ name, runFailure }) => ({ name, runFailure })),
    rules,
    totals: summarize(positives),
    sources,
    discarded: observed.flatMap((result) => result.discarded),
    anomalies: observed.flatMap((result) => result.anomalies ?? []),
  };
}

/** A ratio as a percentage with one decimal, or a dash when undefined. */
function percent(value) {
  return value === null ? "-" : `${(value * 100).toFixed(1)}%`;
}

/**
 * Renders candidate recall as markdown.
 * @param {object} report Result of {@link aggregateCandidateRecall}.
 * @returns {string}
 */
export function renderCandidateMarkdown(report) {
  const lines = [
    "# Candidate recall",
    "",
    `${report.observed} of ${report.cases} recall cases observed. A positive is a reported pair finding the case expects; it reached verification when a candidate source proposed its pair.`,
    "",
    "A reported finding was necessarily evaluated, so \"reached\" is 100% unless the join fails. The source table and the list of declared pairs that were never reported carry the information.",
    "",
    "| Rule | Positives | Reached verification | Candidate recall |",
    "| --- | ---: | ---: | ---: |",
  ];
  for (const row of [...report.rules, { rule: "**all**", ...report.totals }]) {
    lines.push(`| ${row.rule} | ${row.positives} | ${row.reached} | ${percent(row.candidateRecall)} |`);
  }
  lines.push("", "| Source | Proposed | Sole proposer |", "| --- | ---: | ---: |");
  for (const row of report.sources) lines.push(`| ${row.source} | ${row.proposed} | ${row.sole} |`);
  if (report.unobservable.length > 0) {
    lines.push("", "## Cases that could not be observed", "");
    for (const { name, runFailure } of report.unobservable) lines.push(`- \`${name}\`: ${runFailure}`);
  }
  lines.push("", "## Declared pairs that were never reported", "");
  if (report.discarded.length === 0) lines.push("None.");
  for (const row of report.discarded) {
    lines.push(`- \`${row.case}\` ${row.rule} **${row.kind}** (${row.state}): ${JSON.stringify(row.expected.matchers ?? row.expected)}`);
  }
  if (report.anomalies.length > 0) {
    lines.push("", "## Findings that could not be joined", "");
    for (const row of report.anomalies) lines.push(`- \`${row.case}\` ${row.rule} ${row.reason} at ${row.at.join(", ")}`);
  }
  return `${lines.join("\n")}\n`;
}
