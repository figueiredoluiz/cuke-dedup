// Compares two analyzer binaries on the committed corpus for the release notes: findings per rule,
// definitions and feature steps analyzed, and analysis time. Counts come from the JSON reports of
// the same inputs run by both binaries, so a difference is the binaries'. Timing is the report's own
// `analysisMs`, summarized by median, and is advisory: shared CI runners are noisy.

import { spawnSync } from "node:child_process";
import { cp, mkdir, readFile } from "node:fs/promises";
import { join } from "node:path";

/**
 * The committed corpus cases both binaries are run on: the behavior corpus and the recall corpus.
 * @param {string} root Repository root.
 * @returns {Promise<{name: string, source: string, dir: string, arguments: string[],
 *   materialize: object[]}[]>}
 */
export async function loadCases(root) {
  const cases = [];
  for (const [source, directory] of [["corpus", "fixtures/corpus"], ["recall", "fixtures/recall"]]) {
    const manifest = JSON.parse(await readFile(join(root, directory, "manifest.json"), "utf8"));
    for (const testCase of manifest.cases) {
      cases.push({
        name: `${source}/${testCase.name}`,
        source,
        dir: join(root, directory, testCase.path),
        arguments: testCase.arguments || [],
        materialize: testCase.materialize || [],
      });
    }
  }
  return cases;
}

/**
 * Runs one binary on one case and returns what its JSON report says.
 * @param {string} binary Path to the analyzer binary.
 * @param {object} testCase A case from {@link loadCases}.
 * @param {string} scratch Directory the case's copy and report are written under.
 * @param {number} [timeoutMs] Longest a run may take before it is killed and counts as failed.
 * @returns {Promise<{status: number|null, spawnFailure: string|null, report: object|null}>}
 */
export async function runCase(binary, testCase, scratch, timeoutMs = 120_000) {
  const caseRoot = join(scratch, "case");
  const output = join(scratch, "report");
  await cp(testCase.dir, caseRoot, { recursive: true });
  for (const item of testCase.materialize) {
    const destination = join(caseRoot, item.destination);
    await mkdir(join(destination, ".."), { recursive: true });
    await cp(join(caseRoot, item.source), destination);
  }
  const run = spawnSync(binary, [".", ...testCase.arguments, "--reporters", "json", "--output", output], { cwd: caseRoot, encoding: "utf8", timeout: timeoutMs });
  const text = await readFile(join(output, "cuke-dedup.json"), "utf8").catch((error) => {
    if (error.code === "ENOENT") return null;
    throw error;
  });
  // A release that crashed mid-write leaves a report that is not JSON; that case has no report, and
  // the comparison goes on without it instead of losing every other case.
  let report = null;
  let invalid = null;
  try {
    report = text === null ? null : JSON.parse(text);
  } catch (error) {
    invalid = `report is not valid JSON: ${error.message}`;
  }
  // Valid JSON without a summary has nothing to compare, so it is as unusable as a corrupt report.
  if (report !== null && (typeof report.summary !== "object" || report.summary === null || typeof report.summary.byRule !== "object" || report.summary.byRule === null)) {
    report = null;
    invalid = "report is missing a usable summary";
  }
  return {
    status: run.status,
    spawnFailure: run.error?.message ?? (run.signal ? `killed by ${run.signal}` : invalid),
    report,
  };
}

/** The median of numbers, or `null` for none. */
export function median(values) {
  if (values.length === 0) return null;
  const sorted = [...values].sort((left, right) => left - right);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 1 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
}

/**
 * Reduces the repeated runs of one binary on one case to what is compared. Counts must agree across
 * repeats, since the analyzer is deterministic; a case whose repeats disagree is `unstable`. The
 * time is the median of the repeats' `analysisMs`.
 * @param {{status: number|null, report: object|null}[]} runs Repeated runs.
 * @returns {{observable: boolean, unstable: boolean, failure: string|null, status: number|null,
 *   byRule: object, findings: number, definitions: number, featureSteps: number,
 *   analysisMs: number|null}} `failure` is the first reason a run gave no report.
 */
export function summarizeRuns(runs) {
  const reported = runs.filter((run) => run.report !== null && run.status !== null);
  if (reported.length !== runs.length || runs.length === 0) {
    const failed = runs.find((candidate) => candidate.report === null || candidate.status === null);
    const failure = failed?.spawnFailure ?? (failed ? `exit ${failed.status} wrote no usable report` : "no runs");
    return { observable: false, unstable: false, failure, status: null, byRule: {}, findings: 0, definitions: 0, featureSteps: 0, analysisMs: null };
  }
  /** What a run contributes to the comparison, apart from its time. */
  const counts = (run) => ({
    status: run.status,
    byRule: Object.fromEntries(Object.entries(run.report.summary.byRule).sort(([left], [right]) => left.localeCompare(right))),
    findings: run.report.summary.findings,
    definitions: run.report.summary.definitionsAnalyzed,
    featureSteps: run.report.summary.featureStepsAnalyzed,
  });
  const first = counts(reported[0]);
  const unstable = reported.some((run) => JSON.stringify(counts(run)) !== JSON.stringify(first));
  const times = reported.map((run) => run.report.metrics?.analysisMs).filter((value) => typeof value === "number");
  return { observable: true, unstable, failure: null, ...first, analysisMs: median(times) };
}

/**
 * Compares two binaries' summaries case by case. A case counts only when both binaries produced a
 * stable report for it, so a total is a sum over the same inputs.
 * @param {{name: string, previous: object, candidate: object}[]} cases Per-case summaries from
 *   {@link summarizeRuns}.
 * @returns {object} Per-rule, definition and step totals for each binary, exit-status and time
 *   figures, and the cases left out.
 */
export function compare(cases) {
  const usable = cases.filter(({ previous, candidate }) => previous.observable && candidate.observable && !previous.unstable && !candidate.unstable);
  const excluded = cases.filter((entry) => !usable.includes(entry)).map(({ name, previous, candidate }) => ({
    name,
    reason: !previous.observable ? `no report from the previous release: ${previous.failure}` : !candidate.observable ? `no report from the new release: ${candidate.failure}` : "unstable counts",
  }));
  /** The sum of one count over the usable cases for a side. */
  const total = (side, key) => usable.reduce((sum, entry) => sum + entry[side][key], 0);
  const rules = [...new Set(usable.flatMap((entry) => [...Object.keys(entry.previous.byRule), ...Object.keys(entry.candidate.byRule)]))].sort();
  /** The findings of one rule summed over the usable cases for a side. */
  const ruleTotal = (side, rule) => usable.reduce((sum, entry) => sum + (entry[side].byRule[rule] ?? 0), 0);
  /** The measured per-case analysis times of the usable cases for a side. */
  const times = (side) => usable.map((entry) => entry[side].analysisMs).filter((value) => value !== null);
  return {
    cases: cases.length,
    compared: usable.length,
    excluded,
    rules: rules.map((rule) => ({ rule, previous: ruleTotal("previous", rule), candidate: ruleTotal("candidate", rule) })),
    findings: { previous: total("previous", "findings"), candidate: total("candidate", "findings") },
    definitions: { previous: total("previous", "definitions"), candidate: total("candidate", "definitions") },
    featureSteps: { previous: total("previous", "featureSteps"), candidate: total("candidate", "featureSteps") },
    exitStatusChanged: usable.filter((entry) => entry.previous.status !== entry.candidate.status).map((entry) => entry.name),
    medianAnalysisMs: { previous: median(times("previous")), candidate: median(times("candidate")) },
    totalAnalysisMs: {
      previous: times("previous").reduce((sum, value) => sum + value, 0),
      candidate: times("candidate").reduce((sum, value) => sum + value, 0),
    },
  };
}

/** A signed change in `digits` decimals: `+3`, `-2` or `0`. */
function delta(previous, candidate, digits = 0) {
  const change = Number((candidate - previous).toFixed(digits));
  return change > 0 ? `+${change.toFixed(digits)}` : change.toFixed(digits);
}

/** Milliseconds to two decimals, or a dash when unmeasured. */
function milliseconds(value) {
  return value === null ? "-" : value.toFixed(2);
}

/** Most left-out cases listed by name in the table; the rest are counted. */
const MAX_LISTED_EXCLUSIONS = 10;

/**
 * Renders the comparison as markdown for the release notes.
 * @param {object} comparison Result of {@link compare}.
 * @param {{previous: string, candidate: string}} labels Names of the two releases.
 * @returns {string}
 */
export function renderTable(comparison, labels) {
  const lines = [
    `## Comparison with ${labels.previous}`,
    "",
    `The committed corpus (${comparison.compared} of ${comparison.cases} cases) analyzed by both releases.`,
    "",
    `| | ${labels.previous} | ${labels.candidate} | Change |`,
    "| --- | ---: | ---: | ---: |",
  ];
  /** Appends a table row for a pair of counts. */
  const row = (label, { previous, candidate }) => lines.push(`| ${label} | ${previous} | ${candidate} | ${delta(previous, candidate)} |`);
  for (const rule of comparison.rules) row(`\`${rule.rule}\``, rule);
  row("**findings**", comparison.findings);
  row("definitions analyzed", comparison.definitions);
  row("feature steps analyzed", comparison.featureSteps);
  const { previous, candidate } = comparison.medianAnalysisMs;
  lines.push(`| median analysis time per case (ms, advisory) | ${milliseconds(previous)} | ${milliseconds(candidate)} | ${previous === null || candidate === null ? "-" : delta(previous, candidate, 2)} |`);
  if (comparison.exitStatusChanged.length > 0) {
    lines.push("", `Exit status changed on ${comparison.exitStatusChanged.length} cases: ${comparison.exitStatusChanged.map((name) => `\`${name}\``).join(", ")}.`);
  }
  if (comparison.excluded.length > 0) {
    lines.push("", `${comparison.excluded.length} cases are left out:`, "");
    for (const entry of comparison.excluded.slice(0, MAX_LISTED_EXCLUSIONS)) lines.push(`- \`${entry.name}\`: ${entry.reason}`);
    if (comparison.excluded.length > MAX_LISTED_EXCLUSIONS) lines.push(`- and ${comparison.excluded.length - MAX_LISTED_EXCLUSIONS} more (all are in the \`--json\` output)`);
  }
  return `${lines.join("\n")}\n`;
}
