// Classifies one observed behavior case without asserting. A behavior case pins finding counts per
// rule (`expected.byRule`), not individual findings, so scoring is per rule on counts: the expected
// count is what the rule should report, the observed count is `summary.byRule` of the run.
//
//   tp  min(expected, observed)         findings the case requires and the run reported
//   fn  max(0, expected - observed)     required findings the run did not report
//   fp  max(0, observed - expected)     findings beyond the required count, or of an unlisted rule
//   tn  always 0                        the case declares no absences, only counts
//
// A case that cannot be scored is reported unscored (unobservable) with the cause, never counted as
// clean: a run that failed operationally, analyzed other inputs than declared, or a label or report
// shape the classifier does not model.

import { summaryComparisons } from "../../check/lib/behavior-summary.mjs";
import { isOperationalExit, unobservableCase } from "./classify.mjs";

/** The corpus this classifier scores. */
export const BEHAVIOR_CORPUS = "behavior";

/** Whether a value is a nonnegative safe integer. */
const count = (value) => Number.isSafeInteger(value) && value >= 0;

/** Whether a value is a plain object whose values are all nonnegative integers. */
const countMap = (value) =>
  typeof value === "object" && value !== null && !Array.isArray(value) && Object.values(value).every(count);

/**
 * Why a case's labels cannot be scored, or `null` when they can.
 * @param {object} testCase Behavior manifest case.
 * @returns {string|null}
 */
function labelProblem(testCase) {
  const expected = testCase.expected;
  if (typeof expected !== "object" || expected === null) return "no expected block";
  if (!countMap(expected.byRule)) return "expected.byRule is not a map of nonnegative integer counts";
  if (!count(expected.definitions) || !count(expected.featureSteps)) return "expected definitions or feature steps are not nonnegative integers";
  if (!count(expected.duplicatedDefinitions) || typeof expected.percentage !== "number" || typeof expected.threshold !== "number" || typeof expected.passed !== "boolean") {
    return "expected duplication summary (duplicatedDefinitions, percentage, threshold, passed) is incomplete";
  }
  if (!Number.isInteger(testCase.expectedExit)) return "expectedExit is not an integer";
  return null;
}

/**
 * Classifies a behavior case's observation.
 * @param {object} testCase Behavior manifest case.
 * @param {{status: number|null, stderr: string, spawnFailure: string|null, report: object|null}} observation
 *   Result of `observeBehaviorCase`.
 * @returns {object} Per-rule counts and the entries worth reviewing, or an unobservable result.
 */
export function classifyBehaviorCase(testCase, { status, stderr, spawnFailure, report }) {
  /** An unscored result with the cause. */
  const unscored = (cause) => unobservableCase(testCase.name, cause, BEHAVIOR_CORPUS);
  const problem = labelProblem(testCase);
  if (problem !== null) return unscored(`unmodeled label: ${problem}`);
  if (report === null || isOperationalExit(status, testCase)) {
    return unscored(spawnFailure ?? `exit ${status}${report === null ? " wrote no JSON report" : ""}: ${stderr}`.trim());
  }
  const expected = testCase.expected;
  const observedByRule = report.summary?.byRule;
  if (!countMap(observedByRule)) return unscored("unmodeled report: summary.byRule is not a map of counts");
  // A run that analyzed other inputs than the case declares proves nothing about its counts.
  const comparisons = summaryComparisons(testCase, report);
  const census = comparisons.filter(({ census: inputs, actual, expected: wanted }) => inputs && actual !== wanted)
    .map(({ label, actual, expected: wanted }) => `${label} ${actual}, expected ${wanted}`);
  if (census.length > 0) return unscored(`extraction mismatch: ${census.join("; ")}`);

  const entries = [];
  /** Records an entry to review for this case. */
  const entry = (kind, rule, detail) => entries.push({ corpus: BEHAVIOR_CORPUS, case: testCase.name, rule, kind, detail });
  const rules = {};
  for (const rule of new Set([...Object.keys(expected.byRule), ...Object.keys(observedByRule)])) {
    const wanted = expected.byRule[rule] ?? 0;
    const found = observedByRule[rule] ?? 0;
    if (wanted === 0 && found === 0) continue;
    rules[rule] = { tp: Math.min(wanted, found), fp: Math.max(0, found - wanted), fn: Math.max(0, wanted - found), tn: 0, knownMisses: 0 };
    if (found < wanted) entry("missing", rule, `expected ${wanted}, observed ${found}`);
    if (found > wanted) entry("unexpected", rule, `expected ${wanted}, observed ${found}`);
  }
  // Exit and duplication summary are not per-rule findings; a disagreement is listed, not scored.
  if (status !== testCase.expectedExit) entry("exit", "-", `expected exit ${testCase.expectedExit}, got ${status}`);
  for (const { label, census: inputs, actual, expected: wanted } of comparisons) {
    if (!inputs && actual !== wanted) entry("summary", "-", `${label} expected ${wanted}, got ${actual}`);
  }
  for (const message of testCase.stderrIncludes || []) {
    if (!stderr.includes(message)) entry("stderr", "-", `missing message ${message}`);
  }
  return { corpus: BEHAVIOR_CORPUS, lane: null, name: testCase.name, observable: true, runFailure: null, rules, entries, ambiguous: 0 };
}
