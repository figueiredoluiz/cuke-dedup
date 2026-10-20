// Observer for the paired language-parity corpus, shared by its check and by the evaluation report.
// An observer runs the binary and returns what it produced without judging; the check derives
// deficits from the observation and the report classifies it. The run uses strict incompleteness.

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { readOptional } from "./read-optional.mjs";

/** The definition file extension of each language of a paired case. */
export const LANGUAGE_EXTENSIONS = { typescript: "ts", ruby: "rb" };

/**
 * Turns a finished spawn into an observation. A run that did not finish has no report (a partial
 * one proves nothing), and a report that is not valid JSON is `null` with the parse error kept, so
 * the exit status and stderr always survive.
 * @param {object} run Result of `spawnSync`.
 * @param {string} reportPath Where the run would have written its JSON report.
 * @returns {Promise<{status: number|null, signal: string|null, stderr: string,
 *   spawnFailure: string|null, report: object|null, reportError: string|null}>}
 */
async function observation(run, reportPath) {
  const finished = run.status !== null;
  const text = finished ? await readOptional(reportPath) : null;
  let report = null;
  let reportError = null;
  try {
    report = text === null ? null : JSON.parse(text);
  } catch (error) {
    reportError = error.message;
  }
  return {
    status: run.status,
    signal: run.signal ?? null,
    stderr: run.stderr ?? "",
    spawnFailure: run.error?.message ?? null,
    report,
    reportError,
  };
}

/**
 * Runs one paired language-parity case in one language.
 * @param {object} options
 * @param {string} options.binary Path to the analyzer binary.
 * @param {string} options.directory Empty directory the case's sources are written into.
 * @param {object} options.item The paired manifest case.
 * @param {"typescript"|"ruby"} options.language Which source of the pair to analyze.
 * @param {string} options.extension File extension of that language's definitions.
 * @returns {Promise<object>} See {@link observation}.
 */
export async function observeParityLane({ binary, directory, item, language, extension }) {
  await mkdir(directory, { recursive: true });
  await writeFile(join(directory, `steps.${extension}`), item.sources[language]);
  await writeFile(join(directory, "suite.feature"), item.feature);
  const run = spawnSync(binary, [".", "--definitions", `*.${extension}`,
    "--features", "*.feature", "--reporters", "json", "--output", "report", "--no-metrics", "--fail-on-incomplete"],
  { cwd: directory, encoding: "utf8", timeout: 30_000, maxBuffer: 1024 * 1024 });
  return observation(run, join(directory, "report/cuke-dedup.json"));
}

/**
 * The report of a parity run that completed, or an error saying why the run is not usable: it did
 * not start, did not reach an exit status of 0, 1 or 2, or wrote no report.
 * @param {object} observed Result of an observer in this module.
 * @returns {object} The JSON report.
 */
export function completedReport(observed) {
  if (observed.spawnFailure) throw new Error(observed.spawnFailure);
  assert.ok([0, 1, 2].includes(observed.status), `analyzer did not complete: ${observed.signal ?? observed.status}`);
  assert.ok(observed.report, observed.reportError ? `analyzer wrote an invalid JSON report: ${observed.reportError}` : "analyzer wrote no JSON report");
  return observed.report;
}
