import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { attempt, reportFailures } from "./keep-going.mjs";

const checkCorpus = resolve(dirname(fileURLToPath(import.meta.url)), "..", "check-corpus.mjs");

/** A throwing check is recorded and the next one still runs; reporting fails the run only then. */
test("attempt records failures and continues", async (t) => {
  const printed = [];
  t.mock.method(console, "error", (value) => printed.push(value instanceof Error ? value.message : value));
  const failures = [];
  const ran = [];
  await attempt(failures, "case-1", () => { ran.push(1); throw new Error("first"); });
  await attempt(failures, "case-2", async () => { ran.push(2); });
  await attempt(failures, "case-3", async () => { ran.push(3); throw new Error("third"); });
  assert.deepEqual([ran, failures.map(({ name, error }) => [name, error.message])], [[1, 2, 3], [["case-1", "first"], ["case-3", "third"]]]);
  try {
    assert.equal(reportFailures([], "check"), true);
    assert.equal(process.exitCode, undefined);
    assert.equal(reportFailures(failures, "check"), false);
    assert.equal(process.exitCode, 1);
    // Each error follows its case name, so a bare assertion still names the failing case.
    assert.deepEqual(printed, ["case-1:", "first", "case-3:", "third", "check: 2 failing checks"]);
  } finally {
    process.exitCode = undefined;
  }
});

/** Writes `files` (relative path to content) below `root`. */
function writeTree(root, files) {
  for (const [path, content] of Object.entries(files)) {
    mkdirSync(dirname(join(root, path)), { recursive: true });
    writeFileSync(join(root, path), content);
  }
}

/** The corpus checker reports every failing behavior case, not the first, and never a passing one. */
test("corpus check reports every failing case", () => {
  const root = mkdtempSync(join(tmpdir(), "keep-going-test-"));
  try {
    const expected = { definitions: 0, featureSteps: 0, duplicatedDefinitions: 0, percentage: 0, threshold: 0, passed: true, byRule: {} };
    const summary = { definitionsAnalyzed: 0, featureStepsAnalyzed: 0, byRule: {},
      duplication: { duplicatedDefinitions: 0, percentage: 0, threshold: 0, passed: true } };
    const cases = { "fail-one": 9, "pass-one": 0, "fail-two": 9 };
    // The labels every behavior case carries; a case with no required finding is negative.
    const labels = { role: "negative", origin: "synthetic", language: "ecmascript", frameworks: ["cucumber-js"],
      verdict: "reviewed", classifiedAt: "2026-10-18", axes: ["discovery"] };
    // The "binary" is Node itself: `node . ARGS` runs the case directory's index.js, on every platform.
    const caseFiles = Object.entries(cases).flatMap(([name, exit]) => [
      [`fixtures/corpus/${name}/package.json`, '{"type":"commonjs"}'],
      [`fixtures/corpus/${name}/index.js`, `const { mkdirSync, writeFileSync } = require("node:fs");
const output = process.argv[process.argv.indexOf("--output") + 1];
mkdirSync(output, { recursive: true });
writeFileSync(process.argv[process.argv.indexOf("--emit-fingerprints") + 1], "");
writeFileSync(require("node:path").join(output, "cuke-dedup.json"), ${JSON.stringify(JSON.stringify({ summary }))});
process.exitCode = ${exit};
`],
    ]);
    writeTree(root, {
      "src/.keep": "",
      "Cargo.toml": "",
      "Cargo.lock": "",
      "fixtures/corpus/manifest.json": JSON.stringify({ schemaVersion: 1,
        cases: Object.keys(cases).map((name) => ({ name, path: name, arguments: [], expectedExit: 0, expected, ...labels })) }),
      "fixtures/recall/manifest.json": JSON.stringify({ schemaVersion: 1, cases: [] }),
      ...Object.fromEntries(caseFiles),
    });
    const run = spawnSync(process.execPath, [checkCorpus, process.execPath, "fixtures/corpus"], { cwd: root, encoding: "utf8" });
    assert.equal(run.status, 1, run.stderr);
    const reported = [...run.stderr.matchAll(/^AssertionError \[ERR_ASSERTION\]: ((?:fail|pass)-\w+):/gm)].map((match) => match[1]);
    assert.deepEqual(reported, ["fail-one", "fail-two"], run.stderr);
    assert.ok(!run.stdout.includes("Validated"), run.stdout);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

/** A malformed recall entry fails under its own name; the entries after it still run. */
test("corpus check reports every malformed recall case", () => {
  const root = mkdtempSync(join(tmpdir(), "keep-going-test-"));
  try {
    writeTree(root, {
      "src/.keep": "",
      "Cargo.toml": "",
      "Cargo.lock": "",
      "fixtures/corpus/manifest.json": JSON.stringify({ schemaVersion: 1, cases: [] }),
      "fixtures/recall/manifest.json": JSON.stringify({ schemaVersion: 1, cases: [
        { name: "no-rule", path: "no-rule", expectedFindings: [{ matcher: "x" }] },
        { name: "no-assertion", path: "no-assertion" },
      ] }),
    });
    const run = spawnSync(process.execPath, [checkCorpus, process.execPath, "fixtures/corpus"], { cwd: root, encoding: "utf8" });
    assert.equal(run.status, 1, run.stderr);
    assert.match(run.stderr, /^recall no-rule:$/m);
    assert.match(run.stderr, /^recall no-assertion:$/m);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
