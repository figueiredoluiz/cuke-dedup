// Comparison rules for the output-equivalence check: what in two runs of the analyzer may differ
// (timing, the scratch output directory, the version string) and what must not (everything else).
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, sep } from "node:path";

const TIMING_KEYS = /("(?:discoveryMs|parsingMs|analysisMs)"\s*:\s*)-?\d+(?:\.\d+)?(?:[eE][-+]?\d+)?/g;
const TERMINAL_TIMING = /Detection time: \d+(?:\.\d+)? ms/g;

/** Index and value of the `--output` directory argument, or null when the run writes no directory. */
export function outputArgument(args) {
  for (let index = 0; index < args.length; index += 1) {
    if (args[index] === "--output" && index + 1 < args.length) {
      return { index, inline: false, value: args[index + 1] };
    }
    if (args[index].startsWith("--output=")) {
      return { index, inline: true, value: args[index].slice("--output=".length) };
    }
  }
  return null;
}

/** Copy of `args` whose `--output` directory is `directory`; other arguments are untouched. */
export function withOutput(args, output, directory) {
  const copy = [...args];
  if (output.inline) copy[output.index] = `--output=${directory}`;
  else copy[output.index + 1] = directory;
  return copy;
}

/** Text with run-dependent values replaced: timing metrics plus each `[from, to]` replacement. */
export function normalize(text, replacements = []) {
  let normalized = text.replace(TIMING_KEYS, "$10").replace(TERMINAL_TIMING, "Detection time: 0 ms");
  for (const [from, to] of replacements) {
    if (from) normalized = normalized.split(from).join(to);
  }
  return normalized;
}

/** Sorted `/`-separated relative paths of every file below `root`, or an empty list when it does not exist. */
export function listFiles(root) {
  const files = [];
  /** Appends every file below `directory` to `files`. */
  const walk = (directory) => {
    let entries;
    try {
      entries = readdirSync(directory, { withFileTypes: true });
    } catch (error) {
      if (error.code === "ENOENT") return;
      throw error;
    }
    for (const entry of entries) {
      const full = join(directory, entry.name);
      if (entry.isDirectory()) walk(full);
      // One spelling on every platform, so report names compare equal across runs and hosts.
      else files.push(relative(root, full).split(sep).join("/"));
    }
  };
  walk(root);
  return files.sort();
}

/** First line at which two texts differ, with both sides, or null when they are equal. */
export function firstDifference(left, right) {
  if (left === right) return null;
  const a = left.split("\n");
  const b = right.split("\n");
  for (let line = 0; line < Math.max(a.length, b.length); line += 1) {
    if (a[line] !== b[line]) {
      return { line: line + 1, baseline: a[line] ?? "<end>", candidate: b[line] ?? "<end>" };
    }
  }
  return { line: a.length, baseline: "<trailing>", candidate: "<trailing>" };
}

/**
 * Differences between a baseline and a candidate run: exit status, streams and written files.
 *
 * Each side is normalized with its own replacements, so both can map their own output directory
 * and version string to one shared spelling.
 */
export function compareRuns(baseline, candidate, { baseline: left = [], candidate: right = [] } = {}) {
  const differences = [];
  if (baseline.status !== candidate.status || baseline.signal !== candidate.signal) {
    differences.push({
      part: "exit",
      baseline: baseline.signal ?? baseline.status,
      candidate: candidate.signal ?? candidate.status,
    });
  }
  for (const part of ["stdout", "stderr"]) {
    const difference = firstDifference(
      normalize(baseline[part], left),
      normalize(candidate[part], right),
    );
    if (difference) differences.push({ part, ...difference });
  }
  const baselineFiles = baseline.files ?? new Map();
  const candidateFiles = candidate.files ?? new Map();
  const names = [...new Set([...baselineFiles.keys(), ...candidateFiles.keys()])].sort();
  for (const name of names) {
    if (!baselineFiles.has(name) || !candidateFiles.has(name)) {
      differences.push({
        part: `file:${name}`,
        baseline: baselineFiles.has(name) ? "present" : "missing",
        candidate: candidateFiles.has(name) ? "present" : "missing",
      });
      continue;
    }
    const difference = firstDifference(
      normalize(baselineFiles.get(name), left),
      normalize(candidateFiles.get(name), right),
    );
    if (difference) differences.push({ part: `file:${name}`, ...difference });
  }
  return differences;
}

/** Modification time of every file below `root`, keyed by relative path. */
export function fileTimes(root) {
  return new Map(listFiles(root).map((name) => [name, statSync(join(root, name)).mtimeMs]));
}

/**
 * Files below `root` created or rewritten since the `before` snapshot, read as UTF-8.
 *
 * A checker may reuse an output directory, so files a previous run left behind are not this run's.
 */
export function readFiles(root, before = new Map()) {
  return new Map([...fileTimes(root)]
    .filter(([name, time]) => before.get(name) !== time)
    .map(([name]) => [name, readFileSync(join(root, name), "utf8")]));
}

/** Verdict over the shim log: invocations per suite, mismatches, and suites that ran nothing. */
export function summarize(records, suites) {
  const invocations = Object.fromEntries(suites.map((suite) => [suite, 0]));
  const mismatches = [];
  for (const record of records) {
    invocations[record.suite] = (invocations[record.suite] ?? 0) + 1;
    if (record.differences.length > 0) mismatches.push(record);
  }
  const empty = suites.filter((suite) => invocations[suite] === 0);
  return { invocations, mismatches, empty, ok: mismatches.length === 0 && empty.length === 0 };
}
