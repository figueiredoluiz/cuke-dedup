// Real-project corpus: pinned open-source BDD suites reduced to sanitized snapshots (counts and
// digests, no third-party source), plus a local-only verdict sampler and precision report.
//
//   node scripts/evaluate/real-corpus.mjs [run] [--gate | --update] [--suite NAME] [--json FILE]
//   node scripts/evaluate/real-corpus.mjs sample --suite NAME [--per-rule N] [--seed S] [--labels FILE]
//   node scripts/evaluate/real-corpus.mjs verdicts [--suite NAME] [--labels DIR]
//
// Common options: --binary PATH, --references DIR, --manifest FILE, --snapshots DIR.
// Exit 0: done (`run` only reports drift). Exit 1: `--gate` found drift. Exit 2: usage error or a
// suite that could not be observed (checkout missing, not at the pin, run failed).

import { spawnSync } from "node:child_process";
import { existsSync, realpathSync, statSync } from "node:fs";
import { mkdir, readFile, rename, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { parseArgs } from "node:util";
import {
  analyzeSuite,
  assertSanitized,
  compareSnapshots,
  formatSnapshot,
  renderDrift,
  isWithin,
  validateManifest,
} from "./lib/real-corpus.mjs";
import { buildLabels, labelsPathProblem, precisionReport, renderPrecision, validateLabels } from "./lib/real-verdicts.mjs";

/** Stops with exit 2 and a message. */
function fail(message) {
  console.error(`real-corpus: ${message}`);
  process.exit(2);
}

const OPTIONS = {
  binary: { type: "string", default: `target/release/cuke-dedup${process.platform === "win32" ? ".exe" : ""}` },
  references: { type: "string", default: "references.local/repos" },
  manifest: { type: "string", default: "evaluation/real/manifest.json" },
  snapshots: { type: "string", default: "evaluation/real/snapshots" },
  suite: { type: "string" },
  gate: { type: "boolean", default: false },
  update: { type: "boolean", default: false },
  json: { type: "string" },
  "per-rule": { type: "string" },
  seed: { type: "string" },
  labels: { type: "string" },
};

/** Parsed options and positionals; an unknown or malformed option is a usage error, not a drift status. */
function parseOptions() {
  try {
    return parseArgs({ allowPositionals: true, options: OPTIONS });
  } catch (error) {
    return fail(error.message);
  }
}

const { values, positionals } = parseOptions();

/** Reads a committed snapshot of `suite`, or `null` when none exists; rejects one that is unsanitized or pins another revision. */
async function readSnapshot(suite) {
  const path = join(values.snapshots, `${suite.name}.json`);
  if (!existsSync(path)) return null;
  const snapshot = JSON.parse(await readFile(path, "utf8"));
  assertSanitized(snapshot);
  const stale = ["commit", "repo", "root", "framework"].filter((field) => snapshot[field] !== suite[field]);
  if (snapshot.suite !== suite.name || stale.length > 0) {
    throw new Error(`${path} was taken with another ${stale.join(", ") || "suite"} than the manifest declares; run with --update`);
  }
  return snapshot;
}

/** Whether `path`, or its nearest existing ancestor, lies inside this checkout's work tree; unknown counts as inside. */
function insideWorkTree(path) {
  const run = spawnSync("git", ["rev-parse", "--show-toplevel"], { encoding: "utf8" });
  if (run.status !== 0) return true;
  let probe = resolve(path);
  while (!existsSync(probe)) probe = dirname(probe);
  return isWithin(realpathSync(run.stdout.trim()), realpathSync(probe));
}

/** Whether `path` cannot enter a commit of this checkout: it lies outside the work tree, or git ignores it. */
function keptOutOfCommits(path) {
  return !insideWorkTree(path) || spawnSync("git", ["check-ignore", "-q", "--no-index", "--", path]).status === 0;
}

/** The validated suites of the manifest; a missing, unreadable or invalid manifest is a usage error. */
async function manifestSuites() {
  try {
    return validateManifest(JSON.parse(await readFile(values.manifest, "utf8")));
  } catch (error) {
    return fail(error.message);
  }
}

const command = positionals[0] ?? "run";
if (!["run", "sample", "verdicts"].includes(command) || positionals.length > 1) fail(`unknown command ${positionals.join(" ")}`);
if (values.gate && values.update) fail("--gate and --update are exclusive");
if (command === "verdicts" && values.labels !== undefined && existsSync(values.labels) && statSync(values.labels).isFile()) fail("verdicts takes the labels directory, not a file");
if (command === "sample" && values.labels !== undefined && existsSync(values.labels) && statSync(values.labels).isDirectory()) fail("sample takes a labels file path, not a directory");
if (values.update && values.json !== undefined) fail("--json reports drift, which --update does not compute");
const perRuleText = values["per-rule"] ?? "10";
if (command === "sample" && !/^[1-9][0-9]*$/.test(perRuleText)) fail("--per-rule must be a positive integer");
// A flag a command ignores would let a CI step pass while checking nothing. `--binary` and `--references` are shared and have defaults.
const ignoredBy = { run: ["labels", "per-rule", "seed"], sample: ["gate", "update", "json"], verdicts: ["gate", "update", "json", "per-rule", "seed"] };
for (const option of ignoredBy[command] ?? []) {
  if (values[option] !== undefined && values[option] !== false) fail(`--${option} does not apply to ${command}`);
}
const suites = (await manifestSuites()).filter((suite) => values.suite === undefined || suite.name === values.suite);
if (suites.length === 0) fail(`no suite named ${values.suite}`);
const labelsDir = join(dirname(values.references), "real-labels");

const errors = [];
let drifted = false;
let complete = true;
let output = "";

if (command === "run") {
  const report = [];
  const pending = [];
  for (const suite of suites) {
    try {
      // The committed snapshot is read first, so a missing or stale one fails before a long run.
      const committed = values.update ? null : await readSnapshot(suite);
      if (committed === null && !values.update) {
        if (values.gate) throw new Error("no committed snapshot; run with --update");
        output += `### ${suite.name}\n\nno committed snapshot; nothing to compare (create one with --update)\n\n`;
        complete = false;
        continue;
      }
      const { snapshot } = await analyzeSuite(suite, values);
      assertSanitized(snapshot);
      if (values.update) {
        pending.push({ suite: suite.name, text: formatSnapshot(snapshot) });
        continue;
      }
      const drift = compareSnapshots(committed, snapshot);
      drifted ||= !drift.clean;
      report.push({ suite: suite.name, drift });
      output += `${renderDrift(suite.name, drift)}\n`;
    } catch (error) {
      errors.push(`${suite.name}: ${error.message}`);
    }
  }
  // Snapshots are replaced together or not at all, so one failing suite cannot leave a mix of analyzer builds.
  if (errors.length > 0 && pending.length > 0) output += "no snapshot written: a suite could not be observed\n";
  else if (pending.length > 0) {
    await mkdir(values.snapshots, { recursive: true });
    // Written beside the targets and renamed, so a failed write leaves the old snapshots whole.
    for (const { suite, text } of pending) await writeFile(join(values.snapshots, `${suite}.json.tmp`), text);
    for (const { suite } of pending) {
      await rename(join(values.snapshots, `${suite}.json.tmp`), join(values.snapshots, `${suite}.json`));
      output += `### ${suite}\n\nsnapshot written\n\n`;
    }
  }
  if (values.json && !complete) errors.push("--json: not written because a suite has no committed snapshot");
  if (values.json && complete && errors.length === 0) {
    await writeFile(values.json, `${JSON.stringify(report, null, 2)}\n`).catch((error) => errors.push(`--json: ${error.message}`));
  }
} else if (command === "sample") {
  if (suites.length !== 1 || values.suite === undefined) fail("sample needs --suite NAME");
  const [suite] = suites;
  const path = values.labels ?? join(labelsDir, `${suite.name}.json`);
  try {
    const problem = labelsPathProblem(path, { exists: existsSync(path), ignored: keptOutOfCommits(path) });
    if (problem) throw new Error(problem);
    const committed = await readSnapshot(suite);
    if (committed === null) throw new Error("no committed snapshot to label; run with --update first");
    const { snapshot, locations } = await analyzeSuite(suite, values);
    if (!compareSnapshots(committed, snapshot).clean) throw new Error("the run drifts from the committed snapshot, so its ids would not match; run with --update first");
    const perRule = Number(perRuleText);
    await mkdir(dirname(path), { recursive: true });
    // `wx` fails if the file appeared during the run, so reviewed verdicts are never overwritten.
    await writeFile(path, `${JSON.stringify(buildLabels(snapshot, locations, { perRule, seed: values.seed ?? "1" }), null, 2)}\n`, { flag: "wx" });
    output += `labels written to ${path}; fill in each verdict by hand\n`;
  } catch (error) {
    errors.push(`${suite.name}: ${error.message}`);
  }
} else {
  const sets = [];
  for (const suite of suites) {
    const path = values.labels ? join(values.labels, `${suite.name}.json`) : join(labelsDir, `${suite.name}.json`);
    if (!existsSync(path)) {
      console.error(`real-corpus: ${suite.name}: no labels at ${path}`);
      continue;
    }
    try {
      const committed = await readSnapshot(suite);
      if (committed === null) throw new Error("no committed snapshot to check the labels against");
      sets.push({ suite: suite.name, samples: validateLabels(JSON.parse(await readFile(path, "utf8")), committed) });
    } catch (error) {
      errors.push(`${suite.name}: ${error.message}`);
    }
  }
  if (sets.length > 0) output += renderPrecision(precisionReport(sets));
  else if (errors.length === 0) errors.push(`no labels found for ${suites.map((suite) => suite.name).join(", ")}`);
}

process.stdout.write(output);
for (const message of errors) console.error(`real-corpus: ${message}`);
process.exitCode = errors.length > 0 ? 2 : values.gate && drifted ? 1 : 0;
