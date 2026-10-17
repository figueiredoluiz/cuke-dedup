// Stands in for the analyzer binary under the existing checkers. Each invocation runs the baseline
// binary and then the candidate with the original arguments, records any difference, and
// reproduces the candidate's result so the checker sees only it. Every location a run writes (the
// report directory, explicit or configured, and an updated `--baseline`) is snapshotted before each
// run and restored before the candidate runs, so both start from one state and each run is compared
// on what it created, rewrote or deleted there.
import { spawnSync } from "node:child_process";
import { compareRuns, pairRuns, probeArgs, writeLocations, writeRecord } from "./output-equivalence.mjs";

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
// Streams stay bytes: they are compared exactly when not valid UTF-8, and forwarded unchanged.
const options = { maxBuffer: 512 * 1024 * 1024 };

/** One run of `binary` with the original arguments; `pairRuns` adds the files it wrote. */
function execute(binary) {
  const run = spawnSync(binary, args, options);
  if (run.error) throw run.error;
  return { status: run.status, signal: run.signal, stdout: run.stdout, stderr: run.stderr, files: null };
}

// Both binaries' configurations, so a location either one resolves differently is still watched.
const locations = [baseline, candidate].flatMap((binary) => {
  const probe = spawnSync(binary, probeArgs(args), options);
  // A probe that could not run, overflowed or was killed would silently leave its locations untracked.
  if (probe.error) throw probe.error;
  if (probe.signal) throw new Error(`${binary} --print-config terminated by ${probe.signal}`);
  return writeLocations(probe.stdout.toString("utf8"), args);
});
const [baselineRun, candidateRun] = pairRuns(locations, () => execute(baseline), () => execute(candidate));
const differences = compareRuns(baselineRun, candidateRun, { candidate: JSON.parse(versions || "[]") });
writeRecord(log, { suite, cwd: process.cwd(), args, differences });
process.stdout.write(candidateRun.stdout);
process.stderr.write(candidateRun.stderr);
process.exitCode = candidateRun.status;
if (candidateRun.signal) process.kill(process.pid, candidateRun.signal);
