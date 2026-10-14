// Stands in for the analyzer binary under the existing checkers. Each invocation runs the baseline
// binary (into a scratch output directory) and then the candidate with the original arguments,
// records any difference, and reproduces the candidate's result so the checker sees only it.
import { appendFileSync, existsSync, mkdtempSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { compareRuns, fileTimes, outputArgument, readFiles, withOutput } from "./output-equivalence.mjs";

const {
  CUKE_DEDUP_EQUIVALENCE_BASELINE: baseline,
  CUKE_DEDUP_EQUIVALENCE_CANDIDATE: candidate,
  CUKE_DEDUP_EQUIVALENCE_LOG: log,
  CUKE_DEDUP_EQUIVALENCE_SUITE: suite,
  CUKE_DEDUP_EQUIVALENCE_VERSIONS: versions,
} = process.env;
if (!baseline || !candidate || !log || !suite) {
  process.stderr.write("equivalence shim: run it through scripts/check/check-output-equivalence.mjs\n");
  process.exit(2);
}

const args = process.argv.slice(2);
const output = outputArgument(args);
const scratch = output ? mkdtempSync(join(tmpdir(), "cuke-equivalence-")) : null;
const options = { encoding: "utf8", maxBuffer: 512 * 1024 * 1024 };

/** Replacements mapping a run's output directory, as given and as resolved, to one token. */
function directoryTokens(directory) {
  if (!directory) return [];
  // Longest spelling first, so `/private/var/...` is not left with a `/private` prefix.
  // A failed run may never create its output directory.
  const spellings = existsSync(directory) ? [realpathSync(directory), directory] : [directory];
  return [...new Set(spellings)]
    .sort((a, b) => b.length - a.length)
    .map((spelling) => [spelling, "<output>"]);
}

/** One run of `binary`, with the files it wrote to `directory` when the run writes reports. */
function execute(binary, binaryArgs, directory) {
  const before = directory ? fileTimes(directory) : null;
  const run = spawnSync(binary, binaryArgs, options);
  if (run.error) throw run.error;
  return {
    status: run.status,
    signal: run.signal,
    stdout: run.stdout,
    stderr: run.stderr,
    files: directory ? readFiles(directory, before) : null,
  };
}

let signal = null;
try {
  const outputDirectory = output ? resolve(output.value) : null;
  const baselineRun = execute(baseline, output ? withOutput(args, output, scratch) : args, scratch);
  const candidateRun = execute(candidate, args, outputDirectory);
  const differences = compareRuns(baselineRun, candidateRun, {
    baseline: directoryTokens(scratch),
    candidate: [...directoryTokens(outputDirectory), ...JSON.parse(versions || "[]")],
  });
  appendFileSync(log, `${JSON.stringify({ suite, cwd: process.cwd(), args, differences })}\n`);
  process.stdout.write(candidateRun.stdout);
  process.stderr.write(candidateRun.stderr);
  signal = candidateRun.signal;
  process.exitCode = candidateRun.status;
} finally {
  if (scratch) rmSync(scratch, { recursive: true, force: true });
}
if (signal) process.kill(process.pid, signal);
