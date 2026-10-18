import { test } from "node:test";
import assert from "node:assert/strict";
import { chmod, mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { assertCaseWellFormed, judgeRecallCase, observeRecallCase } from "./recall-runner.mjs";

const TODAY = "2026-10-19";

/** A valid recall case: one duplicate-matcher pair and one deliberate non-finding of another rule. */
const baseCase = (over = {}) => ({
  name: "case",
  path: "case",
  role: "positive",
  origin: "synthetic",
  language: "ecmascript",
  frameworks: ["cucumber-js"],
  verdict: "reviewed",
  classifiedAt: "2026-10-18",
  expectedDefinitions: 2,
  expectedFeatureSteps: 0,
  expectedFindings: [{ rule: "duplicate-matcher", matchers: ["a", "a"] }],
  expectedAbsent: [{ rule: "unused-definition" }],
  ...over,
});

/** A duplicate-matcher finding over matchers `a` and `a`, with the shape the oracle reads. */
const duplicate = (over = {}) => ({
  rule: "duplicate-matcher",
  suppression: null,
  primary: { path: "steps.ts", line: 1 },
  related: [{ path: "steps.ts", line: 2 }],
  message: "",
  evidence: { comparison: { leftMatcher: "a", rightMatcher: "a" } },
  ...over,
});

/** An observation of a clean run holding `findings`. */
const observed = (findings = [duplicate()], over = {}) => ({
  status: 0,
  stderr: "",
  spawnFailure: null,
  report: {
    summary: { definitionsAnalyzed: 2, featureStepsAnalyzed: 0 },
    analysis: { candidateSources: {} },
    findings,
  },
  fingerprintsText: ["a", "b"].map((matcher) => JSON.stringify({ domain: "ecmascript", framework: "cucumber-js", matcher })).join("\n"),
  ...over,
});

/** Judges with fresh totals and ids; returns the totals. */
async function judge(testCase, observation, extra = {}) {
  const totals = { detected: 0, knownMisses: 0, unisolated: 0 };
  await judgeRecallCase(testCase, observation, {
    totals,
    knownIds: new Set(),
    attemptCheck: (_label, check) => check(),
    ...extra,
  });
  return totals;
}

/** Checks a matching run passes and adds its findings, known misses and unisolated absences to the totals. */
test("a matching observation passes and updates the totals", async () => {
  const totals = await judge(
    baseCase({
      expectedAbsent: [{ rule: "near-duplicate-step", matchers: ["x", "y"] }],
      knownMisses: [{ id: "m", reason: "r", rule: "duplicate-handler", matchers: ["p", "q"] }],
    }),
    observed(),
  );
  assert.deepEqual(totals, { detected: 1, knownMisses: 1, unisolated: 1 });
});

/** Checks each static or run-level mismatch is rejected, each with its own message. */
test("exit status, report, definition and feature-step mismatches are rejected", async () => {
  const bad = [
    [observed(undefined, { status: 1 }), /exit 1, expected 0/],
    [observed(undefined, { status: 2, report: null }), /exit 2, expected 0/],
    // The cause of a run that never reached an exit status is not lost behind `exit null`.
    [observed(undefined, { status: null, stderr: null, spawnFailure: "spawn x ENOENT", report: null }), /exit null, expected 0: spawn x ENOENT/],
    [observed(undefined, { status: null, stderr: "", spawnFailure: "killed by SIGKILL", report: null }), /killed by SIGKILL/],
    [observed(undefined, { report: null }), /wrote no JSON report/],
    [{ ...observed(), report: { ...observed().report, summary: { definitionsAnalyzed: 3, featureStepsAnalyzed: 0 } } }, /definitions/],
    [{ ...observed(), report: { ...observed().report, summary: { definitionsAnalyzed: 2, featureStepsAnalyzed: 1 } } }, /extracted feature steps/],
  ];
  for (const [observation, message] of bad) await assert.rejects(judge(baseCase(), observation), message);
  await assert.rejects(judge(baseCase({ expectedFeatureSteps: -1 }), observed()), /nonnegative integer/);
  // Control: the unmodified pair passes, so each rejection above is caused by its own change.
  await judge(baseCase(), observed());
});

/** Checks missing, extra and doubly owned findings are rejected, and a suppressed finding is ignored. */
test("findings must match their expectations one to one", async () => {
  await assert.rejects(judge(baseCase(), observed([])), /expected finding/);
  await assert.rejects(judge(baseCase(), observed([duplicate(), duplicate({ primary: { path: "steps.ts", line: 9 } })])), /expected finding/);
  await assert.rejects(
    judge(baseCase(), observed([duplicate(), duplicate({ rule: "near-duplicate-step" })])),
    /each finding needs exactly one expectation, none matched/,
  );
  await assert.rejects(
    judge(baseCase({ expectedFindings: [{ rule: "duplicate-matcher" }, { rule: "duplicate-matcher", matchers: ["a", "a"] }] }), observed()),
    /2 matched/,
  );
  await judge(baseCase(), observed([duplicate(), duplicate({ suppression: { reason: "x" } })]));
});

/** Checks a declared ambiguous finding needs no owner and is not counted, whether or not it is reported. */
test("ambiguous findings are exempt from ownership and the detected count", async () => {
  const ambiguous = baseCase({
    role: "ambiguous",
    expectedFindings: [],
    expectedAmbiguous: [{ rule: "duplicate-matcher", matchers: ["a", "a"], reason: "undecided" }],
  });
  // Emitted and absent are both accepted, and neither counts as detected.
  assert.equal((await judge(ambiguous, observed([duplicate()]))).detected, 0);
  assert.equal((await judge(ambiguous, observed([]))).detected, 0);
  // Control: without the declaration the same finding has no owner.
  await assert.rejects(judge({ ...ambiguous, expectedAmbiguous: [] }, observed([duplicate()])), /none matched/);
});

/** Checks a non-finding that appears fails, and a known miss needs a reason, a unique id and an absent finding. */
test("absences and known misses are held to their contracts", async () => {
  await assert.rejects(judge(baseCase({ expectedAbsent: [{ rule: "duplicate-matcher" }] }), observed()), /deliberate non-finding/);
  /** A case holding one known miss, overridden by `over`. */
  const miss = (over) => baseCase({ knownMisses: [{ id: "m", reason: "r", rule: "near-duplicate-step", ...over }] });
  await assert.rejects(judge(miss({ reason: "" }), observed()), /needs a reason/);
  await assert.rejects(judge(miss({ rule: "duplicate-matcher" }), observed()), /was fixed/);
  await assert.rejects(judge(miss({}), observed(), { knownIds: new Set(["m"]) }), /duplicate known-miss id m/);
  await judge(miss({}), observed());
});

/** Checks an isolation check runs through attemptCheck, so its failure does not stop the case. */
test("isolation checks are delegated and their failures do not stop the case", async () => {
  const labels = [];
  const isolated = baseCase({
    expectedAbsent: [{ rule: "duplicate-handler", matchers: ["a", "b"], isolation: { participants: [] } }],
  });
  const totals = await judge(isolated, observed(), {
    attemptCheck: async (label, check) => {
      labels.push(label);
      try {
        await check();
      } catch {
        // A failed isolation is recorded by the real attempt and does not stop the case.
      }
    },
  });
  assert.equal(labels.length, 1);
  assert.match(labels[0], /^recall case: isolation /);
  // An isolated absence is not counted as unisolated, and the case still completes.
  assert.deepEqual(totals, { detected: 1, knownMisses: 0, unisolated: 0 });
});

/** Checks the labels are held against the fingerprint records the run wrote. */
test("language and framework labels are checked against the records", async () => {
  await assert.rejects(judge(baseCase({ language: "ruby" }), observed()), /language label/);
  await assert.rejects(judge(baseCase({ frameworks: ["playwright-bdd"] }), observed()), /frameworks label/);
  await assert.rejects(judge(baseCase(), observed(undefined, { fingerprintsText: null })), /no fingerprint records/);
});

/** Checks an expectation without a rule, a case that pins nothing and invalid metadata are rejected before a run. */
test("a malformed case is rejected before it is run", () => {
  assertCaseWellFormed(baseCase(), TODAY);
  assert.throws(() => assertCaseWellFormed(baseCase({ expectedAbsent: [{ matchers: ["a", "b"] }] }), TODAY), /names no rule/);
  assert.throws(
    () => assertCaseWellFormed(baseCase({ role: "negative", expectedFindings: [], expectedAbsent: [], knownMisses: [] }), TODAY),
    /pinning anything/,
  );
  assert.throws(() => assertCaseWellFormed(baseCase({ origin: "scraped" }), TODAY), /origin/);
});

/** Checks observing a run that wrote nothing returns its status and null outputs instead of throwing. */
test("observing a failed run returns its status and no outputs", { skip: process.platform === "win32" }, async () => {
  const directory = await mkdtemp(join(tmpdir(), "recall-runner-"));
  try {
    await mkdir(join(directory, "cases", "case"), { recursive: true });
    const binary = join(directory, "fake-binary");
    await writeFile(binary, "#!/bin/sh\necho broken >&2\nexit 2\n");
    await chmod(binary, 0o755);
    const observation = await observeRecallCase({
      binary,
      recallRoot: join(directory, "cases"),
      scratch: join(directory, "scratch"),
      testCase: { path: "case" },
    });
    assert.equal(observation.status, 2);
    assert.match(observation.stderr, /broken/);
    assert.equal(observation.spawnFailure, null);
    assert.equal(observation.report, null);
    assert.equal(observation.fingerprintsText, null);
    // A binary that cannot be started reports why instead of a bare null status.
    const missing = await observeRecallCase({
      binary: join(directory, "no-such-binary"),
      recallRoot: join(directory, "cases"),
      scratch: join(directory, "scratch-missing"),
      testCase: { path: "case" },
    });
    assert.equal(missing.status, null);
    assert.match(missing.spawnFailure, /ENOENT/);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
