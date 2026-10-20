// Real-project corpus: runs the analyzer over pinned checkouts of open-source BDD suites and reduces
// each run to a sanitized snapshot, so recall and precision claims are not limited to synthetic
// fixtures. A snapshot holds counts and digests only: no source, matcher text, handler text or
// path, so it can be committed without redistributing third-party code. Everything that touches the
// filesystem or the binary is kept apart from the functions that judge, summarize and compare.

import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { existsSync, realpathSync, statSync } from "node:fs";
import { mkdir, mkdtemp, readFile, rename, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { parseRecords } from "../../check/lib/negative-isolation.mjs";
import { PAIR_RULES, definitionKey } from "./candidate-recall.mjs";

/** Framework labels a suite may declare; they are the labels the analyzer writes per definition. The lane starts with Playwright-BDD and Cucumber.js; another suite kind is added here and in the docs. */
export const FRAMEWORKS = Object.freeze(["playwright-bdd", "cucumber-js"]);

/** Exit statuses accepted by default: no findings, or findings. Anything else is a tool failure. */
const DEFAULT_EXITS = Object.freeze([0, 1]);

/** Snapshot schema version, bumped when a snapshot field changes meaning. */
export const SNAPSHOT_SCHEMA = 1;

const FILE_OPERATIONS = Object.freeze({ mkdir, mkdtemp, rename, rm, writeFile });

/** A suite name: a lowercase checkout directory name. */
const NAME = /^[a-z0-9][a-z0-9._-]*$/;

/** A repository URL: https, a host and a path, with no credentials, port, query or fragment. */
const REPO = /^https:\/\/[A-Za-z0-9.-]+\/[A-Za-z0-9._~-]+(?:\/[A-Za-z0-9._~-]+)*$/;

/** A full revision. */
const REVISION = /^[0-9a-f]{40}$/;

/**
 * Replaces a set of snapshots as one transaction. Every new file is prepared before any target
 * changes; an interrupted replacement restores old targets and removes newly created ones.
 * @param {string} directory Snapshot directory.
 * @param {{suite: string, text: string}[]} pending Snapshots to write.
 * @param {Partial<typeof FILE_OPERATIONS>} overrides Filesystem operations for failure tests.
 * @returns {Promise<string|null>} A non-fatal warning when committed transaction cleanup fails.
 */
export async function replaceSnapshots(directory, pending, overrides = {}) {
  const operations = { ...FILE_OPERATIONS, ...overrides };
  await operations.mkdir(directory, { recursive: true });
  const transaction = await operations.mkdtemp(join(dirname(directory), `.${basename(directory)}-`));
  const entries = pending.map(({ suite, text }) => ({
    text,
    target: join(directory, `${suite}.json`),
    fresh: join(transaction, `${suite}.new`),
    backup: join(transaction, `${suite}.old`),
    original: false,
    installed: false,
  }));
  try {
    for (const entry of entries) await operations.writeFile(entry.fresh, entry.text, { flag: "wx" });
    for (const entry of entries) {
      try {
        await operations.rename(entry.target, entry.backup);
        entry.original = true;
      } catch (error) {
        if (error.code !== "ENOENT") throw error;
      }
      await operations.rename(entry.fresh, entry.target);
      entry.installed = true;
    }
  } catch (error) {
    const rollbackErrors = [];
    const strandedBackups = [];
    for (const entry of [...entries].reverse()) {
      if (entry.installed) {
        try {
          await operations.rm(entry.target, { force: true });
        } catch (rollbackError) {
          rollbackErrors.push(rollbackError);
        }
      }
      if (entry.original) {
        try {
          await operations.rename(entry.backup, entry.target);
        } catch (rollbackError) {
          rollbackErrors.push(rollbackError);
          strandedBackups.push(entry.backup);
        }
      }
    }
    if (strandedBackups.length === 0) {
      try {
        await operations.rm(transaction, { recursive: true, force: true });
      } catch (cleanupError) {
        rollbackErrors.push(cleanupError);
      }
    }
    const recovery = strandedBackups.length > 0 ? `; original snapshots kept in ${transaction}: ${strandedBackups.join(", ")}` : "";
    if (rollbackErrors.length > 0) throw new AggregateError([error, ...rollbackErrors], `${error.message}; rollback failed: ${rollbackErrors.map((failure) => failure.message).join("; ")}${recovery}`);
    throw error;
  }
  try {
    await operations.rm(transaction, { recursive: true, force: true });
    return null;
  } catch (error) {
    return `snapshot update succeeded but could not remove ${transaction}: ${error.message}`;
  }
}

/** Whether `root` is a nonempty relative path with no `..` segment, in either separator style. */
function insideCheckout(root) {
  return typeof root === "string" && root !== "" && !isAbsolute(root) && !/^[A-Za-z]:/.test(root) && !root.split(/[\\/]/).includes("..");
}

/** Three-way string comparison. */
export function compare(a, b) {
  return a < b ? -1 : a > b ? 1 : 0;
}

/** The first 16 hex digits of a SHA-256 over `parts`, encoded so a part containing a separator cannot alias two parts. */
export function digest(parts) {
  return createHash("sha256").update(JSON.stringify(parts)).digest("hex").slice(0, 16);
}

/**
 * Validates a manifest and fills defaults. Fails closed: an unpinned revision, an unknown framework,
 * a root that leaves the checkout or a duplicate name rejects the whole manifest.
 * @param {object} manifest Parsed `manifest.json`.
 * @returns {object[]} Suites with `expectedExits` defaulted.
 */
export function validateManifest(manifest) {
  const problems = [];
  const suites = Array.isArray(manifest?.suites) ? manifest.suites : [];
  if (manifest?.schemaVersion !== 1) problems.push("schemaVersion must be 1");
  if (suites.length === 0) problems.push("suites must be a nonempty array");
  const names = new Set();
  suites.forEach((suite, index) => {
    const label = `suite ${suite?.name ?? index}`;
    if (!NAME.test(suite?.name ?? "")) problems.push(`${label}: name must be a lowercase directory name`);
    if (names.has(suite?.name)) problems.push(`${label}: duplicate name`);
    names.add(suite?.name);
    if (!REPO.test(suite?.repo ?? "")) problems.push(`${label}: repo must be a plain https URL without credentials, query or fragment`);
    if (!REVISION.test(suite?.commit ?? "")) problems.push(`${label}: commit must be a full 40-hex revision, not a branch, tag or abbreviation`);
    if (!FRAMEWORKS.includes(suite?.framework)) problems.push(`${label}: framework must be one of ${FRAMEWORKS.join(", ")}`);
    if (!insideCheckout(suite?.root)) problems.push(`${label}: root must be a relative path inside the checkout`);
    const exits = suite?.expectedExits ?? DEFAULT_EXITS;
    if (!Array.isArray(exits) || exits.length === 0 || !exits.every((status) => Number.isInteger(status) && status >= 0)) {
      problems.push(`${label}: expectedExits must be a nonempty array of exit statuses`);
    }
  });
  if (problems.length > 0) throw new Error(`invalid real-corpus manifest:\n- ${problems.join("\n- ")}`);
  return suites.map((suite) => ({ ...suite, expectedExits: suite.expectedExits ?? [...DEFAULT_EXITS] }));
}

/**
 * Whether `target` is `parent` or lies below it; both must already be resolved.
 * @param {string} parent
 * @param {string} target
 * @returns {boolean}
 */
export function isWithin(parent, target) {
  const rel = relative(parent, target);
  return rel !== ".." && !rel.startsWith(`..${sep}`) && !isAbsolute(rel);
}

/**
 * Why a checkout cannot back its suite, or `null` when it can.
 * @param {object} suite Manifest suite.
 * @param {{exists: boolean, repository: boolean, head: string|null, changes: string[], rootExists: boolean}} state
 * @returns {string|null}
 */
export function checkoutProblem(suite, { exists, repository, head, changes, rootExists }) {
  if (!exists) return `checkout missing; clone ${suite.repo} and check out ${suite.commit}`;
  if (!repository) return `directory is not the top of a git checkout; clone ${suite.repo} and check out ${suite.commit}`;
  if (head !== suite.commit) return `checkout is at ${head ?? "an unreadable revision"}, pinned ${suite.commit}`;
  if (changes.length > 0) return `checkout has local changes (${changes.slice(0, 5).join("; ")}); digests would not describe the pinned revision`;
  if (!rootExists) return `root ${suite.root} is not a directory in the checkout`;
  return null;
}

/** Output of a `git` run in `dir`, or `null` when git failed. */
function git(dir, args) {
  // Variables that select a repository would make `-C` describe the calling repository instead.
  const selecting = ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_OBJECT_DIRECTORY", "GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_COMMON_DIR", "GIT_NAMESPACE"];
  const env = Object.fromEntries(Object.entries(process.env).filter(([name]) => !selecting.includes(name.toUpperCase())));
  const run = spawnSync("git", ["-C", dir, ...args], { encoding: "utf8", env });
  return run.status === 0 ? run.stdout : null;
}

/**
 * The entries of `git status --porcelain --ignored` that the analyzer could read beyond the pinned
 * tree: modified, untracked or ignored files. A `node_modules` directory is excluded by default and
 * `.DS_Store` is not source, so neither counts. Git quotes paths with spaces in this format.
 * @param {string} status Output of the status command.
 * @returns {string[]} The offending entries.
 */
export function localChanges(status) {
  return status.split("\n").filter((line) => line !== "" && !/^(!!|\?\?) "?(.*\/)?(node_modules(\/|"|$)|\.DS_Store"?$)/.test(line));
}

/**
 * Reads what {@link checkoutProblem} judges from the filesystem and git.
 * @param {object} suite Manifest suite.
 * @param {string} directory The suite's checkout directory.
 * @returns {{exists: boolean, repository: boolean, head: string|null, changes: string[], rootExists: boolean}}
 */
export function readCheckoutState(suite, directory) {
  const exists = existsSync(directory);
  const status = exists ? git(directory, ["status", "--porcelain", "--ignored", "--untracked-files=normal", "--no-renames"]) : null;
  const rootPath = join(directory, suite.root);
  const top = exists ? (git(directory, ["rev-parse", "--show-toplevel"]) ?? "").trim() : "";
  return {
    exists,
    // An empty directory inside another repository would otherwise describe that repository.
    repository: top !== "" && realpathSync(top) === realpathSync(directory),
    head: exists ? (git(directory, ["rev-parse", "HEAD"]) ?? "").trim() || null : null,
    changes: status === null ? ["git status failed"] : localChanges(status),
    // A symlinked root could lead the analyzer outside the pinned tree.
    rootExists: existsSync(rootPath) && statSync(rootPath).isDirectory() && isWithin(realpathSync(directory), realpathSync(rootPath)),
  };
}

/**
 * Runs the binary over a suite's root and reads its outputs. Nothing is judged here.
 * @param {object} options
 * @param {string} options.binary Analyzer binary.
 * @param {string} options.root Directory analyzed.
 * @returns {Promise<{status: number|null, spawnFailure: string|null, stderr: string, reportText: string|null,
 *   fingerprintsText: string|null, pairsText: string|null}>}
 */
export async function runSuite({ binary, root }) {
  const scratch = await mkdtemp(join(tmpdir(), "cuke-dedup-real-"));
  try {
    const fingerprints = join(scratch, "fingerprints.jsonl");
    const pairs = join(scratch, "pairs.jsonl");
    const run = spawnSync(
      binary,
      [".", "--reporters", "json", "--output", scratch, "--emit-fingerprints", fingerprints, "--emit-pairs", pairs],
      { cwd: root, encoding: "utf8", maxBuffer: 1 << 26, timeout: 600_000 },
    );
    /** Reads a file the run may not have written. */
    const optional = (path) => readFile(path, "utf8").catch((error) => {
      if (error.code === "ENOENT") return null;
      throw error;
    });
    return {
      status: run.status,
      spawnFailure: run.error?.message ?? (run.signal ? `killed by ${run.signal}` : null),
      stderr: run.stderr ?? "",
      reportText: await optional(join(scratch, "cuke-dedup.json")),
      fingerprintsText: await optional(fingerprints),
      pairsText: await optional(pairs),
    };
  } finally {
    await rm(scratch, { recursive: true, force: true });
  }
}

/**
 * Why a parsed report lacks a field the snapshot reads, or `null`. A missing field would read as
 * zero or false, so it fails closed instead.
 * @param {object} report Parsed JSON report.
 * @returns {string|null}
 */
export function reportShapeProblem(report) {
  /** Whether a finding carries the fields the snapshot reads: rule, suppression, locations. */
  const wellFormedFinding = (finding) => typeof finding?.rule === "string" && finding.suppression !== undefined
    && Array.isArray(finding.related) && [finding.primary, ...finding.related].every((place) => typeof place?.path === "string" && Number.isInteger(place.line) && Number.isInteger(place.column));
  /** Whether every named field of `section` is a nonnegative integer. */
  const counts = (section, names) => names.every((name) => Number.isInteger(section?.[name]) && section[name] >= 0);
  const analysis = report?.analysis;
  const sources = analysis?.candidateSources;
  const problems = [
    [Array.isArray(report?.findings) && report.findings.every(wellFormedFinding), "findings"],
    [Number.isInteger(report?.findingsTruncated) && report.findingsTruncated >= 0, "findingsTruncated"],
    [counts(report?.summary, ["definitionsAnalyzed", "featureStepsAnalyzed"]), "summary counts"],
    [counts(report?.corpus, ["definitionFiles", "featureFiles"]) && typeof report?.corpus?.incomplete === "boolean", "corpus section"],
    [counts(analysis, ["candidateComparisonsEvaluated", "skippedCandidateComparisons", "truncatedStructuralClasses"]) && typeof analysis?.truncated === "boolean", "analysis section"],
    [sources !== null && typeof sources === "object" && Object.values(sources).every((source) => counts(source, ["evaluated", "skipped"])), "candidateSources"],
  ].filter(([ok]) => !ok);
  return problems.length === 0 ? null : `the JSON report lacks or malforms: ${problems.map(([, name]) => name).join(", ")}`;
}

/**
 * Why an observation cannot be snapshotted, or `null`. A run that failed, wrote no output or cut its
 * findings short certifies nothing, so no snapshot is built from it.
 * @param {object} suite Manifest suite.
 * @param {object} observation Result of {@link runSuite}.
 * @returns {string|null}
 */
export function observationProblem(suite, { status, spawnFailure, stderr = "", reportText, fingerprintsText, pairsText }) {
  if (spawnFailure) return `the run did not complete: ${spawnFailure}`;
  if (!suite.expectedExits.includes(status)) return `exit ${status}, expected one of ${suite.expectedExits.join(", ")}${stderr.trim() === "" ? "" : `:\n${stderr.trim().split("\n").slice(-5).join("\n")}`}`;
  if (reportText === null) return "the run wrote no JSON report";
  if (fingerprintsText === null) return "the run wrote no fingerprint file";
  if (pairsText === null) return "the run wrote no pair file";
  let report;
  try {
    report = JSON.parse(reportText);
  } catch {
    return "the JSON report is not valid JSON";
  }
  const shape = reportShapeProblem(report);
  if (shape) return shape;
  if (report.findingsTruncated > 0) return `the report retains ${report.findingsTruncated} fewer findings than found; digests would be incomplete`;
  return null;
}

/** Occurrences of each string in `list`. */
function multiset(list) {
  const counts = new Map();
  for (const item of list) counts.set(item, (counts.get(item) ?? 0) + 1);
  return counts;
}

/** Counts of `values`, keyed by value and sorted by key. */
function tally(values) {
  return Object.fromEntries([...multiset(values)].sort(([a], [b]) => compare(a, b)));
}

/**
 * Reduces an observation to a sanitized snapshot, plus the local-only locations of each finding for
 * the verdict sampler. The snapshot carries counts and digests; locations never enter it.
 * @param {object} suite Manifest suite.
 * @param {object} observation A run that passed {@link observationProblem}.
 * @returns {{snapshot: object, locations: Map<string, string[]>}} The snapshot and each finding's
 *   `path:line:column` locations by finding id.
 */
export function buildSnapshot(suite, observation) {
  const report = JSON.parse(observation.reportText);
  const definitions = parseRecords(observation.fingerprintsText);
  const pairs = parseRecords(observation.pairsText);
  const frameworks = tally(definitions.map((record) => record.framework));
  // A suite whose declared framework matched no definition was not analyzed as that framework.
  if (!(frameworks[suite.framework] > 0)) throw new Error(`no definition was recognised as ${suite.framework}`);
  // Registrations can share a source position, so a key maps to every fingerprint recorded there.
  const fingerprints = new Map();
  for (const record of definitions) {
    const key = definitionKey(record);
    if (!fingerprints.has(key)) fingerprints.set(key, []);
    fingerprints.get(key).push(record.definitionFingerprint);
  }
  const locations = new Map();
  const findings = {};
  let truncatedFindings = 0;
  for (const finding of report.findings.filter((candidate) => candidate.suppression === null)) {
    const places = [finding.primary, ...finding.related].map(definitionKey);
    const matched = places.map((place) => fingerprints.get(place));
    // A pair-rule finding is decided between definitions, so a location with no definition record
    // means the fingerprint file and the report describe different runs.
    if (PAIR_RULES.includes(finding.rule) && matched.includes(undefined)) {
      throw new Error(`${finding.rule} finding at ${places[0]} names a location with no fingerprint record`);
    }
    const truncated = finding.relatedLocationsTruncated ? [`+${finding.evidence?.cluster?.memberCount ?? "?"}`] : [];
    // The primary location stays first, so swapping it with a related one changes the id.
    const id = digest([finding.rule, places[0], ...places.slice(1).sort(), ...truncated]);
    // A cluster lists its members' fingerprints up to the reporter's cap; the census counts the findings it cut short.
    const fp = digest([finding.rule, ...(finding.evidence?.cluster?.definitionFingerprints ?? matched.flat().filter((print) => print !== undefined)).sort()]);
    if (finding.relatedLocationsTruncated || finding.evidence?.cluster?.membersTruncated) truncatedFindings += 1;
    (findings[finding.rule] ??= []).push({ id, fp });
    locations.set(id, places);
  }
  for (const list of Object.values(findings)) list.sort((a, b) => compare(a.id + a.fp, b.id + b.fp));
  const pairRules = pairs.flatMap((pair) => pair.rules);
  const candidateSources = Object.fromEntries(
    Object.entries(report.analysis.candidateSources).sort(([a], [b]) => compare(a, b)).map(([source, { evaluated, skipped }]) => [source, { evaluated, skipped }]),
  );
  const snapshot = {
    schemaVersion: SNAPSHOT_SCHEMA,
    suite: suite.name,
    repo: suite.repo,
    commit: suite.commit,
    root: suite.root,
    framework: suite.framework,
    exit: observation.status,
    census: {
      definitions: report.summary.definitionsAnalyzed,
      featureSteps: report.summary.featureStepsAnalyzed,
      definitionFiles: report.corpus.definitionFiles,
      featureFiles: report.corpus.featureFiles,
      corpusIncomplete: report.corpus.incomplete,
      frameworks,
      findings: Object.fromEntries(Object.keys(findings).sort().map((rule) => [rule, findings[rule].length])),
      truncatedFindings,
      suppressed: report.findings.filter((finding) => finding.suppression !== null).length,
      pairs: { evaluated: pairs.length, byRule: tally(pairRules), bySource: tally(pairs.flatMap((pair) => pair.candidateSources)) },
      candidates: {
        truncated: report.analysis.truncated,
        evaluated: report.analysis.candidateComparisonsEvaluated,
        skipped: report.analysis.skippedCandidateComparisons,
        truncatedStructuralClasses: report.analysis.truncatedStructuralClasses,
        sources: candidateSources,
      },
    },
    findings: Object.fromEntries(Object.keys(findings).sort().map((rule) => [rule, findings[rule]])),
  };
  return { snapshot, locations };
}

/**
 * Throws unless a snapshot has the committed shape and nothing else. Every string is a known
 * identifier, a URL, a name or a hex digest, so source text, matcher text and paths cannot be
 * stored: an unknown key or a free-form value is rejected rather than inspected for secrets.
 * @param {object} snapshot A parsed snapshot.
 */
export function assertSanitized(snapshot) {
  const problems = [];
  const keys = ["schemaVersion", "suite", "repo", "commit", "root", "framework", "exit", "census", "findings"];
  if (JSON.stringify(Object.keys(snapshot)) !== JSON.stringify(keys)) problems.push(`top-level keys must be ${keys.join(", ")}`);
  if (snapshot.schemaVersion !== SNAPSHOT_SCHEMA) problems.push(`schemaVersion must be ${SNAPSHOT_SCHEMA}`);
  if (!NAME.test(snapshot.suite ?? "")) problems.push("suite is not a directory name");
  if (!REPO.test(snapshot.repo ?? "")) problems.push("repo is not a plain https URL");
  if (!REVISION.test(snapshot.commit ?? "")) problems.push("commit is not a full revision");
  if (!insideCheckout(snapshot.root)) problems.push("root is not a relative path");
  if (!FRAMEWORKS.includes(snapshot.framework)) problems.push("framework is not a known label");
  if (!Number.isInteger(snapshot.exit) || snapshot.exit < 0) problems.push("exit is not an exit status");
  /** Whether a census key or value is an identifier-like name (rule, framework, candidate source). */
  const name = (value) => /^[A-Za-z][A-Za-z0-9-]*$/.test(value);
  /** Collects the problems in one census subtree. */
  const walk = (value, path) => {
    if (value !== null && typeof value === "object" && !Array.isArray(value)) {
      for (const [key, child] of Object.entries(value)) {
        if (!name(key)) problems.push(`${path}.${key}: key is not an identifier`);
        walk(child, `${path}.${key}`);
      }
    } else if (!(typeof value === "number" && Number.isInteger(value) && value >= 0) && typeof value !== "boolean") {
      problems.push(`${path}: only counts and flags are allowed`);
    }
  };
  walk(snapshot.census ?? null, "census");
  for (const [rule, list] of Object.entries(snapshot.findings ?? {})) {
    if (!name(rule)) problems.push(`findings.${rule}: key is not an identifier`);
    for (const entry of Array.isArray(list) ? list : [null]) {
      if (JSON.stringify(Object.keys(entry ?? {})) !== '["id","fp"]' || !/^[0-9a-f]{16}$/.test(entry.id) || !/^[0-9a-f]{16}$/.test(entry.fp)) {
        problems.push(`findings.${rule}: entries must be {id, fp} 16-hex digests`);
        break;
      }
    }
  }
  if (problems.length > 0) throw new Error(`snapshot is not sanitized:\n- ${problems.join("\n- ")}`);
}

/**
 * The stable text of a snapshot: two-space JSON with one finding per line, so a drift diff reads
 * finding by finding.
 * @param {object} snapshot
 * @returns {string}
 */
export function formatSnapshot(snapshot) {
  const { findings, ...rest } = snapshot;
  const head = JSON.stringify(rest, null, 2).slice(0, -2);
  const rules = Object.keys(findings).map((rule) => `    ${JSON.stringify(rule)}: [\n${findings[rule].map((entry) => `      ${JSON.stringify(entry)}`).join(",\n")}\n    ]`);
  return `${head},\n  "findings": ${rules.length === 0 ? "{}" : `{\n${rules.join(",\n")}\n  }`}\n}\n`;
}

/** Census leaves as `dotted.path` to value. */
function flatten(value, prefix = "") {
  return value !== null && typeof value === "object"
    ? Object.entries(value).flatMap(([key, child]) => flatten(child, prefix === "" ? key : `${prefix}.${key}`))
    : [[prefix, value]];
}

/** Each finding id mapped to its sorted, joined fingerprints. */
function fingerprintsById(list) {
  const groups = new Map();
  for (const { id, fp } of list) groups.set(id, [...(groups.get(id) ?? []), fp]);
  return new Map([...groups].map(([id, prints]) => [id, prints.sort().join()]));
}

/**
 * Compares a fresh snapshot with the committed one, per rule and per census field.
 * @param {object} committed
 * @param {object} fresh
 * @returns {{clean: boolean, census: {field: string, committed: *, fresh: *}[],
 *   rules: {rule: string, committed: number, fresh: number, added: number, removed: number,
 *   changed: number}[]}} Census fields that differ, and every rule whose findings differ: `added`
 *   and `removed` count finding ids on one side only, `changed` ids on both sides whose
 *   fingerprints differ.
 */
export function compareSnapshots(committed, fresh) {
  /** Census leaves other than per-rule finding counts, which the rule rows already carry. */
  const leaves = ({ census: { findings: _counts, ...rest }, exit, commit }) => new Map(flatten({ ...rest, exit, commit }));
  const before = leaves(committed);
  const after = leaves(fresh);
  const census = [...new Set([...before.keys(), ...after.keys()])].sort()
    .filter((field) => before.get(field) !== after.get(field))
    .map((field) => ({ field, committed: before.get(field) ?? null, fresh: after.get(field) ?? null }));
  const rules = [];
  for (const rule of [...new Set([...Object.keys(committed.findings), ...Object.keys(fresh.findings)])].sort()) {
    const old = committed.findings[rule] ?? [];
    const now = fresh.findings[rule] ?? [];
    const oldIds = multiset(old.map((entry) => entry.id));
    const nowIds = multiset(now.map((entry) => entry.id));
    /** Occurrences of ids in `a` beyond those in `b`. */
    const surplus = (a, b) => [...a].reduce((sum, [id, count]) => sum + Math.max(0, count - (b.get(id) ?? 0)), 0);
    const oldPrints = fingerprintsById(old);
    const nowPrints = fingerprintsById(now);
    // Ids whose multiplicity differs are already `added` or `removed`, so `changed` counts the rest.
    const changed = [...oldPrints].filter(([id, prints]) => oldIds.get(id) === nowIds.get(id) && nowPrints.get(id) !== prints).length;
    const row = { rule, committed: old.length, fresh: now.length, added: surplus(nowIds, oldIds), removed: surplus(oldIds, nowIds), changed };
    if (row.added + row.removed + row.changed > 0) rules.push(row);
  }
  return { clean: census.length === 0 && rules.length === 0, census, rules };
}

/**
 * Renders drift for one suite as Markdown.
 * @param {string} suite Suite name.
 * @param {object} drift Result of {@link compareSnapshots}.
 * @returns {string}
 */
export function renderDrift(suite, drift) {
  if (drift.clean) return `### ${suite}\n\nno drift\n`;
  const lines = [`### ${suite}`, ""];
  if (drift.rules.length > 0) {
    lines.push("| rule | committed | fresh | added | removed | changed |", "| --- | ---: | ---: | ---: | ---: | ---: |");
    for (const row of drift.rules) lines.push(`| ${row.rule} | ${row.committed} | ${row.fresh} | ${row.added} | ${row.removed} | ${row.changed} |`);
    lines.push("");
  }
  for (const { field, committed, fresh } of drift.census) lines.push(`- ${field}: ${committed} -> ${fresh}`);
  return `${lines.join("\n")}\n`;
}

/**
 * Observes one suite end to end: verifies the pin, runs the binary and builds the snapshot.
 * @param {object} suite Manifest suite.
 * @param {{binary: string, references: string}} options
 * @returns {Promise<{snapshot: object, locations: Map<string, string[]>}>}
 */
export async function analyzeSuite(suite, { binary, references }) {
  const directory = resolve(references, suite.name);
  const problem = checkoutProblem(suite, readCheckoutState(suite, directory));
  if (problem) throw new Error(problem);
  const observation = await runSuite({ binary: resolve(binary), root: join(directory, suite.root) });
  const failure = observationProblem(suite, observation);
  if (failure) throw new Error(failure);
  return buildSnapshot(suite, observation);
}
