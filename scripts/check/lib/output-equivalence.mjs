// Comparison rules for the output-equivalence check: what in two runs of the analyzer may differ
// (timing, the scratch output directory, the version string) and what must not (everything else).
import { createHash, randomUUID } from "node:crypto";
import { closeSync, constants, fstatSync, lstatSync, openSync, readdirSync, readFileSync, readlinkSync, statSync, writeFileSync } from "node:fs";
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

// `ignoreBOM` keeps a byte-order mark as a character, so output that gains or loses one differs.
const UTF8 = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });

/** Output as compared: text when valid UTF-8, otherwise `{ binary }`, a digest of its exact bytes. */
export function comparable(value) {
  if (!Buffer.isBuffer(value)) return value;
  try {
    return UTF8.decode(value);
  } catch {
    // Lossy decoding would map different invalid bytes to the same replacement characters.
    return { binary: createHash("sha256").update(value).digest("hex") };
  }
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
  /** Adds the difference between two outputs of `part`: text normalized and diffed, binary by digest. */
  const compare = (part, before, after) => {
    const [text, other] = [comparable(before), comparable(after)];
    if (typeof text === "string" && typeof other === "string") {
      const difference = firstDifference(normalize(text, left), normalize(other, right));
      if (difference) differences.push({ part, ...difference });
      return;
    }
    /** A label that differs whenever the bytes or the text/binary kind differ. */
    const label = (value) => (typeof value === "string" ? "text" : `binary sha256:${value.binary}`);
    if (label(text) !== label(other)) differences.push({ part, baseline: label(text), candidate: label(other) });
  };
  for (const part of ["stdout", "stderr"]) compare(part, baseline[part], candidate[part]);
  const baselineFiles = baseline.files ?? new Map();
  const candidateFiles = candidate.files ?? new Map();
  const names = [...new Set([...baselineFiles.keys(), ...candidateFiles.keys()])].sort();
  for (const name of names) {
    /** How one side left `name`: untouched, deleted, a link to a target, or written. */
    const state = (files) => {
      if (!files.has(name)) return "missing";
      const value = files.get(name);
      if (value === DELETED) return "deleted";
      // Only a plain object is a link: a string has a `link` method of its own.
      const link = typeof value === "object" && !Buffer.isBuffer(value);
      return link ? `link -> ${value.link}` : "present";
    };
    const [before, after] = [state(baselineFiles), state(candidateFiles)];
    if (before !== "present" || after !== "present") {
      if (before !== after) differences.push({ part: `file:${name}`, baseline: before, candidate: after });
      continue;
    }
    compare(`file:${name}`, baselineFiles.get(name), candidateFiles.get(name));
  }
  return differences;
}

// Opening never follows a link or waits on a FIFO; both flags are absent (0) on Windows.
const NO_FOLLOW = constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0) | (constants.O_NONBLOCK ?? 0);

/**
 * The non-directory entry at `path`, read through one descriptor so its time and bytes describe the
 * same file: a link by its target (never followed), a regular file by time and bytes.
 */
function leafEntry(path) {
  let descriptor;
  try {
    descriptor = openSync(path, NO_FOLLOW);
  } catch (error) {
    // `O_NOFOLLOW` refuses a link with ELOOP, so a link swapped in after classification stays a leaf.
    if (error.code === "ELOOP") return { kind: "link", target: readlinkSync(path) };
    throw error;
  }
  try {
    const stats = fstatSync(descriptor);
    // fail-closed: a FIFO, socket or device in a write location would block or not compare.
    if (!stats.isFile()) throw new Error(`unsupported file type in a write location: ${path}`);
    return { kind: "file", time: stats.mtimeMs, bytes: readFileSync(descriptor) };
  } finally {
    closeSync(descriptor);
  }
}

/**
 * Every directory, file and symbolic link at or below `location`, keyed by path; empty when
 * `location` does not exist. Links, the location itself included, carry their target and are never
 * followed, so a link loop or a link out of the location cannot hang the walk or pull in outside paths.
 */
export function snapshot(location) {
  const entries = new Map();
  /** Records the directory `path`, once listed, and everything below it. */
  const visit = (path) => {
    const children = readdirSync(path, { withFileTypes: true });
    entries.set(path, { kind: "directory" });
    for (const entry of children) {
      const child = join(path, entry.name);
      if (entry.isSymbolicLink()) entries.set(child, { kind: "link", target: readlinkSync(child) });
      else if (entry.isDirectory()) visit(child);
      else entries.set(child, leafEntry(child));
    }
  };
  const root = lstatSync(location, { throwIfNoEntry: false });
  if (root?.isSymbolicLink()) entries.set(location, { kind: "link", target: readlinkSync(location) });
  else if (root?.isDirectory()) visit(location);
  else if (root) entries.set(location, leafEntry(location));
  return entries;
}

/** Content recorded for a file a run deleted; a symbol, so no file content can equal it. */
export const DELETED = Symbol("deleted");

/** Whether `entry` differs from `prior`: another kind, a moved link, or a file rewritten by time or bytes. */
function changed(prior, entry) {
  if (!prior || prior.kind !== entry.kind) return true;
  if (entry.kind === "link") return prior.target !== entry.target;
  // Time catches a rewrite with identical bytes; bytes catch one within a coarse timestamp tick.
  return entry.kind === "file" && (prior.time !== entry.time || !prior.bytes.equals(entry.bytes));
}

/**
 * Files and links at or below `location` created, changed or deleted since the `before` snapshot:
 * a file as its bytes, a link as `{ link: target }`, a deletion as `DELETED`.
 */
export function changedFiles(location, before) {
  const now = snapshot(location);
  const written = [...now]
    .filter(([path, entry]) => entry.kind !== "directory" && changed(before.get(path), entry))
    .map(([path, entry]) => [path, entry.kind === "link" ? { link: entry.target } : entry.bytes]);
  const deleted = [...before]
    // A leaf whose path now holds a directory is gone too; the directory itself is not output.
    .filter(([path, entry]) => entry.kind !== "directory" && (!now.has(path) || now.get(path).kind === "directory"))
    .map(([path]) => [path, DELETED]);
  return [...written, ...deleted];
}

/** Modification time of every file below `root`, keyed by relative path. */
export function fileTimes(root) {
  return new Map(listFiles(root).map((name) => [name, statSync(join(root, name)).mtimeMs]));
}

/**
 * Files below `root` created or rewritten since the `before` snapshot, as their bytes.
 *
 * A checker may reuse an output directory, so files a previous run left behind are not this run's.
 */
export function readFiles(root, before = new Map()) {
  return new Map([...fileTimes(root)]
    .filter(([name, time]) => before.get(name) !== time)
    .map(([name]) => [name, readFileSync(join(root, name))]));
}

/**
 * Writes one shim record as its own file in `directory`. Parallel test threads can run many shims at
 * once; separate files, created exclusively, cannot interleave the way appends to one log could.
 */
export function writeRecord(directory, record) {
  writeFileSync(join(directory, `${process.pid}-${randomUUID()}.json`), JSON.stringify(record), { flag: "wx" });
}

/** Every record `writeRecord` left in `directory`. */
export function readRecords(directory) {
  return readdirSync(directory).sort().map((name) => JSON.parse(readFileSync(join(directory, name), "utf8")));
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
