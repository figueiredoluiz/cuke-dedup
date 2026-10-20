import { after, before, test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const CLI = fileURLToPath(new URL("../real-corpus.mjs", import.meta.url));
const REPOSITORY = fileURLToPath(new URL("../../../", import.meta.url));
const POSIX = process.platform !== "win32";
let work;
let commit;

/** Runs a git command in `cwd` and returns its trimmed output. */
function git(cwd, ...args) {
  // Hook variables such as GIT_DIR would override `cwd` and point the commands at the caller's repository.
  const selecting = ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_OBJECT_DIRECTORY", "GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_COMMON_DIR", "GIT_NAMESPACE"];
  const env = Object.fromEntries(Object.entries(process.env).filter(([name]) => !selecting.includes(name.toUpperCase())));
  const run = spawnSync("git", ["-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null", ...args], { cwd, encoding: "utf8", env });
  assert.equal(run.status, 0, run.stderr);
  return run.stdout.trim();
}

/** The fake analyzer: writes the report, definitions and pairs a spec file describes, then exits with its status. */
const FAKE_BINARY = `#!/usr/bin/env node
import { readFileSync, writeFileSync } from "node:fs";
/** The value following a flag on the command line. */
const at = (flag) => process.argv[process.argv.indexOf(flag) + 1];
const spec = JSON.parse(readFileSync(process.env.FAKE_RUN, "utf8"));
writeFileSync(at("--output") + "/cuke-dedup.json", JSON.stringify(spec.report));
writeFileSync(at("--emit-fingerprints"), spec.definitions.map((record) => JSON.stringify(record)).join("\\n"));
writeFileSync(at("--emit-pairs"), "");
process.exit(spec.status);
`;

/** A definition record. */
const definition = (line) => ({ path: "s.ts", line, column: 1, framework: "cucumber-js", definitionFingerprint: `fp${line}` });

/** A run spec with a duplicate-handler finding between each given pair of lines. */
const spec = (pairs, status = 1) => ({
  status,
  definitions: [1, 2, 3, 4].map(definition),
  report: {
    summary: { definitionsAnalyzed: 4, featureStepsAnalyzed: 0 },
    corpus: { definitionFiles: 1, featureFiles: 0, incomplete: false },
    analysis: { truncated: false, candidateComparisonsEvaluated: 0, skippedCandidateComparisons: 0, truncatedStructuralClasses: 0, candidateSources: {} },
    findings: pairs.map(([a, b]) => ({
      rule: "duplicate-handler",
      suppression: null,
      primary: { path: "s.ts", line: a, column: 1 },
      related: [{ path: "s.ts", line: b, column: 1 }],
      relatedLocationsTruncated: false,
      evidence: {},
    })),
    findingsTruncated: 0,
  },
});

before(() => {
  work = mkdtempSync(join(tmpdir(), "real-corpus-cli-"));
  const checkout = join(work, "refs", "fake-suite");
  mkdirSync(checkout, { recursive: true });
  git(checkout, "init", "-q");
  writeFileSync(join(checkout, "a.txt"), "x");
  // A tracked symlink out of the checkout: a root that names it must be refused.
  symlinkSync(work, join(checkout, "escape"));
  git(checkout, "add", ".");
  git(checkout, "commit", "-q", "-m", "init");
  commit = git(checkout, "rev-parse", "HEAD");
  writeFileSync(join(work, "binary.mjs"), FAKE_BINARY);
  chmodSync(join(work, "binary.mjs"), 0o755);
  writeManifest(commit);
});

after(() => rmSync(work, { recursive: true, force: true }));

/** Writes the manifest with the given revision. */
function writeManifest(revision, over = {}) {
  writeFileSync(join(work, "manifest.json"), JSON.stringify({
    schemaVersion: 1,
    suites: [{ name: "fake-suite", repo: "https://example.com/o/fake-suite", commit: revision, root: ".", framework: "cucumber-js", ...over }],
  }));
}

/** Runs the CLI against the fake suite with `run` as the analyzer's behavior. */
function cli(args, run = spec([[1, 2]]), extra = []) {
  writeFileSync(join(work, "run.json"), JSON.stringify(run));
  const result = spawnSync(process.execPath, [CLI, ...args, "--binary", join(work, "binary.mjs"), "--references", join(work, "refs"), "--manifest", join(work, "manifest.json"), "--snapshots", join(work, "snapshots"), ...extra], {
    encoding: "utf8",
    cwd: REPOSITORY,
    env: { ...process.env, FAKE_RUN: join(work, "run.json") },
  });
  return { status: result.status, out: result.stdout, err: result.stderr };
}

/** Checks report mode never fails on drift, gate mode does, and a run that cannot be observed always does. */
test("run reports drift, gate fails on it, and unobservable runs fail closed", { skip: !POSIX }, () => {
  const missing = cli(["run"]);
  assert.deepEqual([missing.status, /no committed snapshot/.test(missing.out)], [0, true]);
  assert.equal(cli(["run", "--gate"]).status, 2);
  assert.equal(cli(["run", "--update"]).status, 0);
  assert.ok(existsSync(join(work, "snapshots", "fake-suite.json")));
  const clean = cli(["run", "--gate"]);
  assert.deepEqual([clean.status, /no drift/.test(clean.out)], [0, true]);
  // A snapshot taken under another framework or repo is stale even at the same revision.
  writeManifest(commit, { framework: "playwright-bdd" });
  const stale = cli(["run", "--gate"]);
  assert.equal(stale.status, 2);
  assert.match(stale.err, /another framework than the manifest declares/);
  writeManifest(commit);
  const grown = spec([[1, 2], [3, 4]]);
  const reported = cli(["run"], grown);
  assert.equal(reported.status, 0);
  assert.match(reported.out, /\| duplicate-handler \| 1 \| 2 \| 1 \| 0 \| 0 \|/);
  const gated = cli(["run", "--gate"], grown);
  assert.equal(gated.status, 1);
  assert.match(gated.out, /\| duplicate-handler \| 1 \| 2 \| 1 \| 0 \| 0 \|/);
  const shrunk = cli(["run", "--gate"], spec([]));
  assert.equal(shrunk.status, 1);
  assert.match(shrunk.out, /\| duplicate-handler \| 1 \| 0 \| 0 \| 1 \| 0 \|/);
  for (const mode of [["run"], ["run", "--gate"]]) {
    const failed = cli(mode, spec([[1, 2]], 2));
    assert.equal(failed.status, 2, mode.join(" "));
    assert.match(failed.err, /exit 2, expected one of 0, 1/);
  }
  assert.equal(cli(["run", "--gate", "--update"]).status, 2);
  // An update where one suite fails writes nothing.
  const before = readFileSync(join(work, "snapshots", "fake-suite.json"), "utf8");
  rmSync(join(work, "snapshots", "fake-suite.json"));
  const refused = cli(["run", "--update"], spec([[1, 2]], 2));
  assert.deepEqual([refused.status, existsSync(join(work, "snapshots", "fake-suite.json"))], [2, false]);
  assert.equal(cli(["run", "--update"]).status, 0);
  assert.equal(readFileSync(join(work, "snapshots", "fake-suite.json"), "utf8"), before);
  assert.equal(cli(["run", "--suite", "absent"]).status, 2);
  assert.equal(cli(["bogus"]).status, 2);
  // Usage and setup errors must not share the status a gate uses for drift.
  assert.equal(cli(["run", "--gat"]).status, 2);
  assert.equal(cli(["run", "--update", "--json", join(work, "drift.json")]).status, 2);
  for (const bad of ["0", "abc", "1.5", "-1"]) assert.equal(cli(["sample", "--suite", "fake-suite", "--per-rule", bad]).status, 2, bad);
  assert.equal(cli(["verdicts", "--labels", join(work, "no-labels-here")]).status, 2);
  // `verdicts` takes the directory `sample` wrote into, not a labels file.
  writeFileSync(join(work, "a-file.json"), "{}");
  const filePath = cli(["verdicts", "--labels", join(work, "a-file.json")]);
  assert.equal(filePath.status, 2);
  assert.match(filePath.err, /directory/);
  // An option the command ignores would let a gate pass while checking nothing.
  for (const args of [["verdicts", "--gate"], ["verdicts", "--update"], ["sample", "--gate"], ["sample", "--json", "x"], ["run", "--seed", "3"], ["run", "--per-rule", "3"], ["run", "--labels", "x"], ["verdicts", "--per-rule", "3"]]) {
    assert.equal(cli(args).status, 2, args.join(" "));
  }
  assert.equal(cli(["run"], spec([[1, 2]]), ["--json", join(work, "missing-dir", "drift.json")]).status, 2);
  assert.equal(cli(["run", "--suite"]).status, 2);
  assert.equal(cli(["run"], spec([[1, 2]]), ["--manifest", join(work, "absent.json")]).status, 2);
  writeFileSync(join(work, "broken.json"), "{");
  assert.equal(cli(["run"], spec([[1, 2]]), ["--manifest", join(work, "broken.json")]).status, 2);
});

/** Checks a checkout that is missing, unpinned, at another revision or modified is never analyzed. */
test("run refuses checkouts that do not match the pin", { skip: !POSIX }, () => {
  writeManifest("HEAD");
  const unpinned = cli(["run"]);
  assert.equal(unpinned.status, 2);
  assert.match(unpinned.err, /invalid real-corpus manifest/);
  writeManifest(commit, { root: "escape" });
  const escaped = cli(["run", "--update"]);
  assert.equal(escaped.status, 2);
  assert.match(escaped.err, /root escape is not a directory in the checkout/);
  writeManifest("b".repeat(40));
  const moved = cli(["run", "--update"]);
  assert.equal(moved.status, 2);
  assert.match(moved.err, /pinned b{40}/);
  writeManifest(commit);
  const absent = cli(["run", "--update"], spec([[1, 2]]), ["--references", join(work, "nowhere")]);
  assert.equal(absent.status, 2);
  assert.match(absent.err, /checkout missing/);
  writeFileSync(join(work, "refs", "fake-suite", "stray.txt"), "local edit");
  const dirty = cli(["run", "--update"]);
  assert.equal(dirty.status, 2);
  assert.match(dirty.err, /local changes/);
  rmSync(join(work, "refs", "fake-suite", "stray.txt"));
  assert.equal(cli(["run"]).status, 0);
});

/** Checks the sampler writes unreviewed labels once, outside the commit tree, and the report counts only reviewed verdicts. */
test("sample and verdicts keep labels local and count reviewed verdicts only", { skip: !POSIX }, () => {
  const labels = join(work, "labels");
  mkdirSync(labels, { recursive: true });
  const path = join(labels, "fake-suite.json");
  cli(["run", "--update"], spec([[1, 2], [3, 4]]));
  assert.equal(cli(["sample", "--suite", "fake-suite", "--labels", path], spec([[1, 2], [3, 4]])).status, 0);
  const drifted = cli(["sample", "--suite", "fake-suite", "--labels", join(labels, "other.json")], spec([[1, 2]]));
  assert.equal(drifted.status, 2);
  assert.match(drifted.err, /drifts from the committed snapshot/);
  assert.match(cli(["sample", "--suite", "fake-suite", "--labels", path], spec([[1, 2], [3, 4]])).err, /may hold reviewed verdicts/);
  assert.match(cli(["sample", "--suite", "fake-suite", "--labels", "evaluation/real/labels-untracked-check.json"], spec([[1, 2], [3, 4]])).err, /not ignored by git/);
  assert.equal(cli(["sample"]).status, 2);
  assert.match(cli(["sample", "--suite", "fake-suite", "--labels", labels], spec([[1, 2], [3, 4]])).err, /not a directory/);
  const written = JSON.parse(readFileSync(path, "utf8"));
  assert.equal(written.samples.length, 2);
  assert.ok(written.samples.every((sample) => sample.verdict === null));
  const unreviewed = cli(["verdicts", "--labels", labels]);
  assert.match(unreviewed.out, /\| duplicate-handler \| 2 \| 0 \| 0 \| 0 \| 2 \| n\/a \(n=0\) \|/);
  written.samples[0].verdict = "true-positive";
  written.samples[1].verdict = "false-positive";
  writeFileSync(path, JSON.stringify(written));
  assert.match(cli(["verdicts", "--labels", labels]).out, /\| duplicate-handler \| 2 \| 1 \| 1 \| 0 \| 0 \| 50\.0% \(n=2\) \|/);
  written.samples[0].verdict = "tp";
  writeFileSync(path, JSON.stringify(written));
  assert.equal(cli(["verdicts", "--labels", labels]).status, 2);
});
