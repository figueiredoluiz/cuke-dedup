import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmod, mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { compare, loadCases, median, renderTable, runCase, summarizeRuns } from "./release-comparison.mjs";

/** A run of a binary that produced `byRule` findings, `time` ms of analysis, and exit `status`. */
const run = (byRule, time = 1, status = 0, over = {}) => ({
  status,
  spawnFailure: null,
  report: {
    summary: { byRule, findings: Object.values(byRule).reduce((sum, count) => sum + count, 0), definitionsAnalyzed: 4, featureStepsAnalyzed: 2 },
    metrics: { analysisMs: time },
    ...over,
  },
});

/** Checks the median of odd, even and empty lists. */
test("median handles odd, even and empty lists", () => {
  assert.equal(median([3, 1, 2]), 2);
  assert.equal(median([4, 1, 3, 2]), 2.5);
  assert.equal(median([]), null);
});

/** Checks repeats agree on counts, take the median time, and a disagreement or missing report is flagged. */
test("repeated runs reduce to counts and a median time, or are flagged", () => {
  const stable = summarizeRuns([run({ a: 1 }, 5), run({ a: 1 }, 1), run({ a: 1 }, 3)]);
  assert.deepEqual([stable.observable, stable.unstable, stable.findings, stable.analysisMs], [true, false, 1, 3]);
  assert.equal(summarizeRuns([run({ a: 1 }), run({ a: 2 })]).unstable, true);
  // The order of rules in a report is not a difference.
  assert.equal(summarizeRuns([run({ a: 1, b: 2 }), run({ b: 2, a: 1 })]).unstable, false);
  assert.equal(summarizeRuns([run({ a: 1 }, 1, 0), run({ a: 1 }, 1, 1)]).unstable, true);
  assert.equal(summarizeRuns([run({ a: 1 }), { status: 0, report: null }]).observable, false);
  assert.equal(summarizeRuns([{ status: null, spawnFailure: "x", report: null }]).observable, false);
  assert.equal(summarizeRuns([]).observable, false);
  // A report without metrics still counts, with no time.
  assert.equal(summarizeRuns([run({ a: 1 }, 1, 0, { metrics: undefined })]).analysisMs, null);
});

/** Checks totals are sums over cases both releases produced a stable report for, with the rest listed. */
test("comparison sums the shared cases and lists the cases left out", () => {
  /** The summary of one run of a release on a case. */
  const side = (byRule, time = 1, status = 0) => summarizeRuns([run(byRule, time, status)]);
  const comparison = compare([
    { name: "one", previous: side({ a: 2, b: 1 }, 2), candidate: side({ a: 3 }, 4) },
    { name: "two", previous: side({ b: 1 }, 6), candidate: side({ b: 1, c: 1 }, 8, 1) },
    { name: "gone", previous: summarizeRuns([{ status: null, report: null }]), candidate: side({ a: 9 }) },
    { name: "flaky", previous: summarizeRuns([run({ a: 1 }), run({ a: 2 })]), candidate: side({ a: 1 }) },
  ]);
  assert.deepEqual(comparison.rules, [
    { rule: "a", previous: 2, candidate: 3 },
    { rule: "b", previous: 2, candidate: 1 },
    { rule: "c", previous: 0, candidate: 1 },
  ]);
  assert.deepEqual(comparison.findings, { previous: 4, candidate: 5 });
  assert.deepEqual(comparison.definitions, { previous: 8, candidate: 8 });
  assert.deepEqual(comparison.exitStatusChanged, ["two"]);
  assert.deepEqual(comparison.medianAnalysisMs, { previous: 4, candidate: 6 });
  assert.deepEqual(comparison.excluded, [{ name: "gone", reason: "no report from the previous release: exit null wrote no usable report" }, { name: "flaky", reason: "unstable counts" }]);
  assert.deepEqual([comparison.cases, comparison.compared], [4, 2]);
  // Control: the excluded cases' findings do not leak into the totals.
  assert.equal(comparison.rules.find((entry) => entry.rule === "a").candidate, 3);
});

/** Checks the timing medians use only the cases both releases measured. */
test("timing is compared over the cases both releases measured", () => {
  /** A summary of one run with the given analysis time, or none. */
  const side = (time) => summarizeRuns([run({ a: 1 }, time, 0, time === null ? { metrics: undefined } : {})]);
  // Previous [1, unmeasured] and candidate [1, 100]: the only paired case is unchanged.
  const comparison = compare([
    { name: "paired", previous: side(1), candidate: side(1) },
    { name: "half", previous: side(null), candidate: side(100) },
  ]);
  assert.deepEqual(comparison.medianAnalysisMs, { previous: 1, candidate: 1 });
  assert.deepEqual(comparison.totalAnalysisMs, { previous: 1, candidate: 1 });
  // The counts still cover both cases.
  assert.equal(comparison.findings.candidate, 2);
});

/** Checks the table shows each rule with a signed change, the time row with decimals, and the notes only when needed. */
test("the table shows signed changes, timing and the exclusion notes", () => {
  /** The summary of one run with a given analysis time. */
  const side = (byRule, time) => summarizeRuns([run(byRule, time)]);
  const table = renderTable(compare([{ name: "one", previous: side({ a: 2 }, 0.2), candidate: side({ a: 1 }, 0.24) }]), { previous: "v1", candidate: "v2" });
  assert.match(table, /## Comparison with v1/);
  assert.match(table, /\| `a` \| 2 \| 1 \| -1 \|/);
  assert.match(table, /\| \*\*findings\*\* \| 2 \| 1 \| -1 \|/);
  assert.match(table, /\| definitions analyzed \| 4 \| 4 \| 0 \|/);
  assert.match(table, /\| median analysis time per case \(ms, advisory\) \| 0\.20 \| 0\.24 \| \+0\.04 \|/);
  // A change below the printed precision is "0.00", not "-0.00".
  const same = renderTable(compare([{ name: "one", previous: side({ a: 1 }, 1), candidate: side({ a: 1 }, 0.999) }]), { previous: "v1", candidate: "v2" });
  assert.match(same, /\| 1\.00 \| 1\.00 \| 0\.00 \|/);
  assert.doesNotMatch(table, /Exit status changed|left out/);
  const noisy = renderTable(compare([
    { name: "x", previous: side({ a: 1 }, 1), candidate: summarizeRuns([run({ a: 1 }, 1, 1)]) },
    { name: "y", previous: summarizeRuns([{ status: 2, report: null }]), candidate: side({ a: 1 }, 1) },
  ]), { previous: "v1", candidate: "v2" });
  assert.match(noisy, /Exit status changed on 1 cases: `x`/);
  assert.match(noisy, /1 cases are left out:\n\n- `y`: no report from the previous release: exit 2 wrote no usable report/);
  // Many exclusions are capped and counted.
  const many = renderTable({ ...compare([{ name: "x", previous: side({ a: 1 }, 1), candidate: side({ a: 1 }, 1) }]), excluded: Array.from({ length: 12 }, (_, index) => ({ name: `c${index}`, reason: "r" })) }, { previous: "v1", candidate: "v2" });
  assert.match(many, /12 cases are left out/);
  assert.match(many, /- `c9`: r/);
  assert.doesNotMatch(many, /`c10`/);
  assert.match(many, /and 2 more/);
  const noTime = renderTable(compare([{ name: "z", previous: summarizeRuns([run({ a: 1 }, 1, 0, { metrics: undefined })]), candidate: side({ a: 1 }, 1) }]), { previous: "v1", candidate: "v2" });
  // With no measurement on one side the case is not timed on either, so both columns are a dash.
  assert.match(noTime, /\| - \| - \| - \|/);
});

/** Checks the committed manifests produce one case per entry, from both corpora. */
test("loadCases reads the behavior and recall corpora", async () => {
  const cases = await loadCases(process.cwd());
  assert.ok(cases.some((entry) => entry.name.startsWith("corpus/")));
  assert.ok(cases.some((entry) => entry.name.startsWith("recall/")));
  assert.equal(new Set(cases.map((entry) => entry.name)).size, cases.length);
});

/** Checks a bad option or a missing value exits 2 like the other invocation errors, and a good call does not. */
test("the command exits 2 for an unknown option or a missing value", { skip: process.platform === "win32" }, () => {
  const script = fileURLToPath(new URL("../release-comparison.mjs", import.meta.url));
  for (const args of [["--bogus"], ["--previous"], ["--previous", "a"], ["--previous", "a", "--candidate", "b", "--repeats", "x"], ["--previous", "a", "--candidate", "b", "--timeout-ms", "0"]]) {
    const result = spawnSync(process.execPath, [script, ...args], { encoding: "utf8" });
    assert.equal(result.status, 2, `${args.join(" ")}: ${result.stderr}`);
  }
});

/** Checks a run reports a binary's status and report, and a binary that writes none gives null. */
test("runCase returns the report a binary wrote, or none", { skip: process.platform === "win32" }, async () => {
  const directory = await mkdtemp(join(tmpdir(), "release-comparison-"));
  try {
    await mkdir(join(directory, "fixture"));
    await writeFile(join(directory, "fixture", "steps.ts"), "x\n");
    const testCase = { dir: join(directory, "fixture"), arguments: ["--flag"], materialize: [] };
    const writer = join(directory, "writer");
    await writeFile(writer, '#!/bin/sh\nwhile [ $# -gt 0 ]; do [ "$1" = "--output" ] && out="$2"; shift; done\nmkdir -p "$out"\necho \'{"summary":{"byRule":{},"findings":0,"definitionsAnalyzed":1,"featureStepsAnalyzed":0}}\' > "$out/cuke-dedup.json"\nexit 1\n');
    await chmod(writer, 0o755);
    const written = await runCase(writer, testCase, join(directory, "scratch-a"));
    assert.equal(written.status, 1);
    assert.deepEqual(written.report, { summary: { byRule: {}, findings: 0, definitionsAnalyzed: 1, featureStepsAnalyzed: 0 } });
    const silent = join(directory, "silent");
    await writeFile(silent, "#!/bin/sh\nexit 2\n");
    await chmod(silent, 0o755);
    const none = await runCase(silent, testCase, join(directory, "scratch-b"));
    assert.deepEqual([none.status, none.report], [2, null]);
    const broken = join(directory, "broken");
    await writeFile(broken, '#!/bin/sh\nwhile [ $# -gt 0 ]; do [ "$1" = "--output" ] && out="$2"; shift; done\nmkdir -p "$out"\necho \'{"summary":\' > "$out/cuke-dedup.json"\nexit 0\n');
    await chmod(broken, 0o755);
    const truncated = await runCase(broken, testCase, join(directory, "scratch-d"));
    assert.equal(truncated.report, null);
    assert.match(truncated.spawnFailure, /not valid JSON/);
    // A report with a summary but not every compared count is unusable, not a source of NaN totals.
    const partial = join(directory, "partial");
    await writeFile(partial, '#!/bin/sh\nwhile [ $# -gt 0 ]; do [ "$1" = "--output" ] && out="$2"; shift; done\nmkdir -p "$out"\necho \'{"summary":{"byRule":{"a":"x"},"findings":1,"definitionsAnalyzed":1,"featureStepsAnalyzed":0}}\' > "$out/cuke-dedup.json"\nexit 0\n');
    await chmod(partial, 0o755);
    assert.match((await runCase(partial, testCase, join(directory, "scratch-g"))).spawnFailure, /missing a usable summary/);
    // Each top-level count is required too.
    for (const [index, summary] of [
      '{"byRule":{},"definitionsAnalyzed":1,"featureStepsAnalyzed":0}',
      '{"byRule":{},"findings":0,"featureStepsAnalyzed":0}',
      '{"byRule":{},"findings":0,"definitionsAnalyzed":1}',
      '{"byRule":{},"findings":null,"definitionsAnalyzed":1,"featureStepsAnalyzed":0}',
    ].entries()) {
      const missingCount = join(directory, `missing-${index}`);
      await writeFile(missingCount, `#!/bin/sh\nwhile [ $# -gt 0 ]; do [ "$1" = "--output" ] && out="$2"; shift; done\nmkdir -p "$out"\necho '{"summary":${summary}}' > "$out/cuke-dedup.json"\nexit 0\n`);
      await chmod(missingCount, 0o755);
      assert.match((await runCase(missingCount, testCase, join(directory, `scratch-m${index}`))).spawnFailure, /missing a usable summary/, summary);
    }
    const bare = join(directory, "bare");
    await writeFile(bare, '#!/bin/sh\nwhile [ $# -gt 0 ]; do [ "$1" = "--output" ] && out="$2"; shift; done\nmkdir -p "$out"\necho \'{"findings":[]}\' > "$out/cuke-dedup.json"\nexit 0\n');
    await chmod(bare, 0o755);
    const noSummary = await runCase(bare, testCase, join(directory, "scratch-f"));
    assert.equal(noSummary.report, null);
    assert.match(noSummary.spawnFailure, /missing a usable summary/);
    const missing = await runCase(join(directory, "absent"), testCase, join(directory, "scratch-c"));
    assert.equal(missing.status, null);
    assert.match(missing.spawnFailure, /ENOENT/);
    // A hung binary is killed at the timeout and counts as failed, so the other cases still run.
    const hung = join(directory, "hung");
    await writeFile(hung, "#!/bin/sh\nsleep 5\n");
    await chmod(hung, 0o755);
    const timedOut = await runCase(hung, testCase, join(directory, "scratch-e"), 200);
    assert.equal(timedOut.report, null);
    assert.match(timedOut.spawnFailure, /ETIMEDOUT/);
    // A binary that ignores SIGTERM and leaves a child behind is still ended, without waiting on it.
    const stubborn = join(directory, "stubborn");
    await writeFile(stubborn, "#!/bin/sh\ntrap '' TERM\nsleep 30 &\nsleep 30\n");
    await chmod(stubborn, 0o755);
    const started = Date.now();
    const stuck = await runCase(stubborn, testCase, join(directory, "scratch-h"), 200);
    assert.ok(Date.now() - started < 10_000, "a stubborn binary must not outlast the timeout");
    assert.match(stuck.spawnFailure, /ETIMEDOUT/);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
