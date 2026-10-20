// Classifies one observed run against a `requiredFindings` / `requiredAbsent` oracle, without
// asserting. The parity corpora share this shape: a paired language-parity case runs once per
// language against the same oracle (`ownership` off), a Ruby-parity group once against its own
// (`ownership` on).
//
//   tp  required findings the run reported (each expectation up to its `count`, default 1)
//   fn  required findings the run did not report
//   fp  findings beyond an expectation's count; with `ownership`, also findings no expectation owns
//   tn  `requiredAbsent` entries no active finding matches
//
// Unowned findings are scored only for a corpus whose gate requires every finding to have exactly
// one owner (Ruby parity). The language-parity gate checks required counts and forbidden findings
// only, so there an unowned finding is not a disagreement unless a `requiredAbsent` entry matches it.
//
// A run that cannot be scored is reported unscored (unobservable) with the cause, never clean: it
// did not complete, truncated its findings, analyzed other inputs than the oracle declares, or the
// oracle itself has a shape this classifier does not model.

import { countMatches, findingOwners } from "../../check/lib/recall-oracle.mjs";
import { unobservableCase } from "./classify.mjs";

/** Whether a value is a nonnegative safe integer. */
const count = (value) => Number.isSafeInteger(value) && value >= 0;

/** Whether every entry of a list is an object naming a rule, with a positive `count` when present. */
const wellFormed = (list) =>
  Array.isArray(list) && list.every((item) => typeof item?.rule === "string" && item.rule !== "" && (item.count === undefined || (count(item.count) && item.count > 0)));

/**
 * Why an oracle cannot be scored, or `null` when it can.
 * @param {object} oracle Parity oracle.
 * @returns {string|null}
 */
function oracleProblem(oracle) {
  if (typeof oracle !== "object" || oracle === null) return "no oracle";
  if (!wellFormed(oracle.requiredFindings)) return "requiredFindings is not a list of rule expectations";
  if (!wellFormed(oracle.requiredAbsent)) return "requiredAbsent is not a list of rule expectations";
  if (!count(oracle.definitions) || !count(oracle.featureSteps)) return "definitions or featureSteps are not nonnegative integers";
  const sources = oracle.expectedCandidateSources;
  if (sources !== undefined && (typeof sources !== "object" || sources === null || Array.isArray(sources) || !Object.values(sources).every(count))) {
    return "expectedCandidateSources is not a map of nonnegative integers";
  }
  if (oracle.complete !== undefined && typeof oracle.complete !== "boolean") return "complete is not a boolean";
  if (oracle.expectedExit !== undefined && !Number.isInteger(oracle.expectedExit)) return "expectedExit is not an integer";
  return null;
}

/**
 * Why the run's inputs or findings cannot be trusted, or `null` when they can.
 * @param {object} oracle Parity oracle.
 * @param {object} report The run's JSON report.
 * @returns {string|null}
 */
function runProblem(oracle, report) {
  if (typeof report.summary !== "object" || report.summary === null || !Array.isArray(report.findings)) {
    return "unmodeled report: no summary or findings list";
  }
  if (report.findingsTruncated !== 0) return `findings truncated or untracked (${report.findingsTruncated})`;
  if (typeof report.corpus?.incomplete !== "boolean") return "unmodeled report: no corpus.incomplete flag";
  const mismatches = [
    ["definitions", report.summary.definitionsAnalyzed, oracle.definitions],
    ["feature steps", report.summary.featureStepsAnalyzed, oracle.featureSteps],
    ...Object.entries(oracle.expectedCandidateSources ?? {}).map(([source, wanted]) => [`${source} candidates`, report.analysis?.candidateSources?.[source]?.evaluated, wanted]),
  ].filter(([, observed, wanted]) => observed !== wanted).map(([label, observed, wanted]) => `${label} ${observed}, expected ${wanted}`);
  return mismatches.length > 0 ? `extraction mismatch: ${mismatches.join("; ")}` : null;
}

/**
 * Classifies a parity run.
 * @param {object} target
 * @param {string} target.corpus Corpus the run belongs to.
 * @param {string} target.name Case or group name.
 * @param {string|null} target.lane Language of a paired run, `null` for a corpus without lanes.
 * @param {object} target.oracle The oracle: a paired case's `expected`, or a Ruby group itself.
 * @param {boolean} target.ownership Whether a finding no `requiredFindings` entry owns is a false positive.
 * @param {{status: number|null, stderr: string, spawnFailure: string|null, report: object|null}} observation
 *   Result of a parity observer.
 * @returns {object} Per-rule counts and the entries worth reviewing, or an unobservable result.
 */
export function classifyOracle({ corpus, name, lane, oracle, ownership }, { status, signal, stderr, spawnFailure, report, reportError }) {
  /** An unscored result with the cause. */
  const unscored = (cause) => unobservableCase(name, cause, corpus, lane);
  const problem = oracleProblem(oracle);
  if (problem !== null) return unscored(`unmodeled label: ${problem}`);
  // Exit 2 is the strict-incompleteness exit these runs ask for; anything else is the tool failing.
  // A process error (output limit, killed child) taints the run even with a report and an allowed exit.
  if (report === null || spawnFailure !== null || ![0, 1, 2].includes(status)) {
    return unscored(spawnFailure ?? (signal ? `killed by ${signal}` : null) ?? `exit ${status}${report === null ? (reportError ? ` wrote an invalid JSON report (${reportError})` : " wrote no JSON report") : ""}: ${stderr}`.trim());
  }
  const cause = runProblem(oracle, report);
  if (cause !== null) return unscored(cause);

  const rules = {};
  /** The counts for `rule`, created on first use. */
  const counts = (rule) => (rules[rule] ??= { tp: 0, fp: 0, fn: 0, tn: 0, knownMisses: 0 });
  const entries = [];
  /** Records an entry to review for this case. */
  const entry = (kind, rule, detail) => entries.push({ corpus, lane, case: name, rule, kind, detail });
  const active = report.findings.filter((finding) => finding.suppression === null);
  /** Findings already counted as false positives, so no finding is counted twice. */
  const scored = new Set();
  const attributed = new Map(oracle.requiredFindings.map((expected) => [expected, []]));
  for (const finding of active) {
    const owners = findingOwners(finding, oracle.requiredFindings);
    const where = `${finding.primary.path}:${finding.primary.line}`;
    if (owners.length > 1 && ownership) {
      entry("overlapping-expectations", finding.rule, `${owners.length} expectations own the finding at ${where}`);
    } else if (owners.length > 0) {
      // Without ownership the gate counts each expectation independently, so an overlap counts for each.
      for (const owner of owners) attributed.get(owner).push(finding);
    } else if (ownership) {
      counts(finding.rule).fp += 1;
      scored.add(finding);
      entry("unowned", finding.rule, `finding at ${where} matches no required finding`);
    }
  }
  for (const [expected, findings] of attributed) {
    const wanted = expected.count ?? 1;
    const found = Math.min(findings.length, wanted);
    counts(expected.rule).tp += found;
    if (found < wanted) {
      counts(expected.rule).fn += wanted - found;
      entry("missing", expected.rule, `expected ${wanted}, observed ${findings.length}: ${JSON.stringify(expected)}`);
    }
    if (findings.length > wanted) {
      counts(expected.rule).fp += findings.length - wanted;
      for (const extra of findings.slice(wanted)) scored.add(extra);
      entry("excess", expected.rule, `expected ${wanted}, observed ${findings.length}: ${JSON.stringify(expected)}`);
    }
  }
  for (const absent of oracle.requiredAbsent) {
    const matched = countMatches(active, absent);
    if (matched === 0) {
      counts(absent.rule).tn += 1;
    } else {
      // A forbidden finding is a false positive unless it was already counted as unowned or excess.
      for (const finding of active.filter((candidate) => countMatches([candidate], absent) > 0 && !scored.has(candidate))) {
        counts(absent.rule).fp += 1;
        scored.add(finding);
      }
      entry("forbidden", absent.rule, `${matched} finding(s) match the absence ${JSON.stringify(absent)}`);
    }
  }
  // Completeness and exit are not per-rule findings; a disagreement is listed, not scored.
  if (typeof oracle.complete === "boolean" && !report.corpus?.incomplete !== oracle.complete) {
    entry("completeness", "-", `expected complete ${oracle.complete}, got ${!report.corpus?.incomplete}`);
  }
  // A Ruby group declares its exit; a paired case implies it as its gate does: strict exit 2 when
  // incomplete, else 1 exactly when an active finding is an error.
  const complete = !report.corpus?.incomplete;
  const wantedExit = oracle.expectedExit ?? (complete ? Number(active.some((finding) => finding.severity === "error")) : 2);
  if (status !== wantedExit) entry("exit", "-", `expected exit ${wantedExit}, got ${status}`);
  return { corpus, lane, name, observable: true, runFailure: null, rules, entries, ambiguous: 0 };
}
