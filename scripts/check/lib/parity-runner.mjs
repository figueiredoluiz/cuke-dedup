// Observers for the two parity corpora, shared by their checks and by the evaluation report. An
// observer runs the binary and returns what it produced without judging; the checks derive deficits
// from the observation and the report classifies it. Both runs use strict incompleteness.

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { cp, mkdir, writeFile } from "node:fs/promises";
import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { readOptional } from "./read-optional.mjs";

/**
 * Runs the analyzer with strict incompleteness and JSON output, the one invocation both parity
 * corpora use.
 * @param {string} binary Path to the analyzer binary.
 * @param {string} cwd Directory holding the project.
 * @param {string[]} selection Definition and feature selection arguments.
 * @param {string} output Output directory, absolute or relative to `cwd`.
 * @returns {Promise<object>} See {@link observation}.
 */
function runStrict(binary, cwd, selection, output) {
  const run = spawnSync(binary, [".", ...selection, "--reporters", "json", "--output", output, "--no-metrics", "--fail-on-incomplete"],
    { cwd, encoding: "utf8", timeout: 30_000, maxBuffer: 1024 * 1024 });
  return observation(run, resolve(cwd, output, "cuke-dedup.json"));
}

/**
 * Resolves a fixture path inside its parent and rejects paths escaping that root.
 * @param {string} parent Directory the path must stay within.
 * @param {string} child Path relative to `parent`.
 * @returns {string} The resolved absolute path.
 */
export function within(parent, child) {
  const path = resolve(parent, child);
  const rel = relative(parent, path);
  assert.ok(!isAbsolute(rel) && rel !== ".." && !rel.startsWith(`..${sep}`), `fixture escapes root: ${child}`);
  return path;
}

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
 * Runs one Ruby-parity group through the binary.
 * @param {object} options
 * @param {string} options.binary Path to the analyzer binary.
 * @param {string} options.root The Ruby-parity fixture directory.
 * @param {string} options.scratch Directory the group's copy and output are written under.
 * @param {object} options.oracle The group's manifest entry.
 * @returns {Promise<object>} See {@link observation}. Rejects when the fixture cannot be copied or
 *   materialized, or a path escapes the fixture root; a run that fails is returned, not thrown.
 */
export async function observeRubyGroup({ binary, root, scratch, oracle }) {
  const project = join(scratch, "project");
  const output = join(scratch, "output");
  await cp(within(root, oracle.path), project, { recursive: true });
  for (const item of oracle.materialize ?? []) {
    const destination = within(project, item.destination);
    await mkdir(dirname(destination), { recursive: true });
    await cp(within(project, item.source), destination);
  }
  return runStrict(binary, project, oracle.arguments ?? ["--definitions", oracle.definitionsPattern ?? "*.rb"], output);
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
  return runStrict(binary, directory, ["--definitions", `*.${extension}`, "--features", "*.feature"], "report");
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
