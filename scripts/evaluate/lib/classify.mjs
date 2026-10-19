// Classifies one observed recall case against its expectations without asserting anything. The
// corpus gate fails on the first disagreement; this counts them, so a report can say how many
// findings were detected, missed or unexpected per rule and name each one. A finding is attributed
// to the one `expectedFindings` entry that owns it, exactly as the gate attributes it.

import { partitionAmbiguous } from "../../check/lib/manifest-schema.mjs";
import { checkIsolation, parseRecords } from "../../check/lib/negative-isolation.mjs";
import { countMatches, locationLabel } from "../../check/lib/recall-oracle.mjs";

/** Counts for one rule, all zero. */
function emptyCounts() {
  return { tp: 0, fp: 0, fn: 0, tn: 0, knownMisses: 0 };
}

/**
 * The ways a report's extraction differs from what the case declares: definitions, feature steps
 * and the evaluated count of each declared candidate source.
 * @param {object} testCase Recall manifest case.
 * @param {object} report The run's JSON report.
 * @returns {string[]} One message per mismatch; empty when the run analyzed what the case declares.
 */
function extractionMismatches(testCase, report) {
  const found = [];
  if (report.summary.definitionsAnalyzed !== testCase.expectedDefinitions) {
    found.push(`definitions ${report.summary.definitionsAnalyzed}, expected ${testCase.expectedDefinitions}`);
  }
  if (report.summary.featureStepsAnalyzed !== testCase.expectedFeatureSteps) {
    found.push(`feature steps ${report.summary.featureStepsAnalyzed}, expected ${testCase.expectedFeatureSteps}`);
  }
  for (const [source, expected] of Object.entries(testCase.expectedCandidateSources || {})) {
    const evaluated = report.analysis?.candidateSources?.[source]?.evaluated;
    if (evaluated !== expected) found.push(`${source} candidates ${evaluated}, expected ${expected}`);
  }
  return found;
}

/**
 * Why a declared isolation does not hold against the run, or `null` when it holds or none is declared.
 * @param {object[]} records Parsed fingerprint records of the run.
 * @param {object} absent An `expectedAbsent` entry.
 * @param {object} analysis The report's `analysis` section.
 * @returns {string|null}
 */
function isolationFailure(records, absent, analysis) {
  if (absent.isolation === undefined) return null;
  try {
    checkIsolation(records, absent, analysis);
    return null;
  } catch (error) {
    return error.message;
  }
}

/**
 * Whether an exit status is the tool failing. Exit 0 and 1 depend on the findings, which a report
 * classifies; any other status means its absences would certify nothing, unless the case declares it.
 * @param {number|null} status Exit status of the run, `null` when it never reached one.
 * @param {object} testCase Recall manifest case.
 * @returns {boolean}
 */
export function isOperationalExit(status, testCase) {
  return status === null || (status !== 0 && status !== 1 && status !== (testCase.expectedExit || 0));
}

/**
 * The result for a case that could not be observed: no counts, only the cause.
 * @param {string} name Case name.
 * @param {string} cause Why the run gave nothing to classify.
 * @returns {object}
 */
export function unobservableCase(name, cause) {
  return { name, observable: false, runFailure: cause, rules: {}, entries: [], ambiguous: 0 };
}

/**
 * Observes and classifies one case. A failure while observing (a report that is not valid JSON, a
 * copy that fails) makes that case unobservable instead of ending the whole evaluation.
 * @param {object} testCase Recall manifest case.
 * @param {() => Promise<object>} observe Produces the case's observation.
 * @returns {Promise<object>} The classification, or an unobservable result with the error message.
 */
export async function evaluateCase(testCase, observe) {
  try {
    return classifyCase(testCase, await observe());
  } catch (error) {
    return unobservableCase(testCase.name, `observation failed: ${error.message}`);
  }
}

/**
 * The part of a finding a reader needs to recognise it and judge its scores.
 * @param {object} finding Active finding from the JSON report.
 * @returns {object}
 */
function describeFinding(finding) {
  const comparison = finding.evidence?.comparison;
  return {
    rule: finding.rule,
    primary: locationLabel(finding.primary),
    related: finding.related.map(locationLabel),
    message: finding.message,
    matchers: comparison ? [comparison.leftMatcher, comparison.rightMatcher] : null,
    matcherSimilarity: finding.evidence?.matcherSimilarity ?? null,
    handlerSimilarity: finding.evidence?.handlerSimilarity ?? null,
  };
}

/**
 * The fingerprint digests of the definitions an expectation names by matcher text.
 * @param {object} expected Manifest expectation.
 * @param {object[]} records Parsed `--emit-fingerprints` records of the run.
 * @returns {object[]}
 */
function participantsOf(expected, records) {
  const wanted = new Set(expected.matchers ?? []);
  return records
    .filter((record) => wanted.has(record.matcher))
    .map((record) => ({ at: `${record.path}:${record.line}`, matcher: record.matcher, handler: record.handler }));
}

/**
 * Classifies a case's observation. Findings and expectations each count once: a finding with no
 * owner is a false positive, a finding beyond its owner's `count` is an excess, and an expectation
 * short of its `count` is a false negative. Findings the case declares ambiguous are set aside, and
 * a finding two expectations both claim is reported as an anomaly rather than counted.
 * @param {object} testCase Recall manifest case.
 * @param {{status: number|null, stderr: string, spawnFailure: string|null, report: object|null,
 *   fingerprintsText: string|null}} observation Result of `observeRecallCase`.
 * @returns {{name: string, observable: boolean, runFailure: string|null, rules: object,
 *   entries: object[], ambiguous: number}} Per-rule counts and the entries worth reviewing.
 */
export function classifyCase(testCase, observation) {
  const { report, status, spawnFailure } = observation;
  // Exit 0 and 1 depend on the findings, which this report classifies. Any other status is the tool
  // failing, so its absences would certify nothing; a case that declares exit 2 expects it.
  if (report === null || isOperationalExit(status, testCase)) {
    const cause = spawnFailure ?? `exit ${status}${report === null ? " wrote no JSON report" : ""}: ${observation.stderr}`.trim();
    return unobservableCase(testCase.name, cause);
  }
  // A run that analyzed other inputs than the case declares proves nothing about its absences, so
  // its findings are not scored; the gate fails the same mismatch.
  const mismatches = extractionMismatches(testCase, report);
  if (mismatches.length > 0) return unobservableCase(testCase.name, `extraction mismatch: ${mismatches.join("; ")}`);
  const rules = {};
  /** The counts for `rule`, created on first use. */
  const counts = (rule) => (rules[rule] ??= emptyCounts());
  const entries = [];
  const records = parseRecords(observation.fingerprintsText ?? "");
  /** Records an entry to review for this case. */
  const entry = (kind, rule, extra) => entries.push({
    case: testCase.name, rule, kind, candidateSources: report.analysis?.candidateSources ?? null, ...extra,
  });

  const active = report.findings.filter((finding) => finding.suppression === null);
  const { owned, ambiguous } = partitionAmbiguous(active, testCase.expectedAmbiguous || []);
  // Positive expectations and known misses both claim the findings they stand for, so ownership is
  // decided once across all of them: a finding with one claimant is attributed to it, with several
  // it is an anomaly counted nowhere, and with none it is unowned.
  const claimants = [
    ...(testCase.expectedFindings || []).map((expected) => ({ expected, miss: false })),
    ...(testCase.knownMisses || []).map((expected) => ({ expected, miss: true })),
  ];
  const attributed = new Map(claimants.map((claimant) => [claimant, []]));
  const unowned = new Set();
  for (const finding of owned) {
    const owners = claimants.filter((claimant) => countMatches([finding], claimant.expected) > 0);
    if (owners.length === 1) {
      attributed.get(owners[0]).push(finding);
    } else if (owners.length === 0) {
      counts(finding.rule).fp += 1;
      unowned.add(finding);
      const declaredAbsent = (testCase.expectedAbsent || []).some((absent) => countMatches([finding], absent) > 0);
      entry("unowned", finding.rule, { declaredAbsent, observed: describeFinding(finding) });
    } else {
      entry("overlapping-expectations", finding.rule, { observed: describeFinding(finding), owners: owners.length });
    }
  }
  for (const [{ expected, miss }, findings] of attributed) {
    // A known miss stands for one desired finding. Found, it is a true positive and no longer a
    // known miss; found again, the extra is an excess like any other.
    const wanted = miss ? 1 : expected.count ?? 1;
    const found = Math.min(findings.length, wanted);
    counts(expected.rule).tp += found;
    if (miss && found === 0) {
      counts(expected.rule).knownMisses += 1;
      entry("known-miss", expected.rule, { expected, participants: participantsOf(expected, records) });
    } else if (miss) {
      entry("fixed-known-miss", expected.rule, { expected });
    } else if (found < wanted) {
      counts(expected.rule).fn += wanted - found;
      entry("missing", expected.rule, { expected, missing: wanted - found, participants: participantsOf(expected, records) });
    }
    for (const finding of findings.slice(wanted)) {
      counts(expected.rule).fp += 1;
      entry("excess", expected.rule, { expected, observed: describeFinding(finding) });
    }
  }
  for (const absent of testCase.expectedAbsent || []) {
    const matched = active.filter((finding) => countMatches([finding], absent) > 0);
    if (matched.length === 0) {
      // A declared isolation says why the finding is absent; when the run does not show that, the
      // absence is not credited, as the gate would reject it.
      const failure = isolationFailure(records, absent, report.analysis);
      if (failure === null) counts(absent.rule).tn += 1;
      else entry("isolation-failed", absent.rule, { expected: absent, error: failure });
    }
    // A violating finding nothing owns is already listed as unowned; list the absence only when the
    // finding is also owned or ambiguous, where the two declarations contradict.
    else if (!matched.every((finding) => unowned.has(finding))) entry("violated-absence", absent.rule, { expected: absent });
  }
  return { name: testCase.name, observable: true, runFailure: null, rules, entries, ambiguous: ambiguous.length };
}
