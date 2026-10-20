import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmod, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { loadCorpora } from "./corpora.mjs";

const root = fileURLToPath(new URL("../../../", import.meta.url));
const script = join(root, "scripts/evaluate/corpus-evaluate.mjs");
/** The corpora the driver evaluates. */
const evaluated = ["recall", "behavior", "language-parity", "ruby-parity"];

// Matrix for the driver. Exit codes: unknown corpus or case (2), a binary that cannot start in every
// evaluated corpus (1, with the control that each corpus lists its own case as unscored, so a skipped
// corpus would show as missing). Wiring: a fake binary's report reaches the behavior classifier and,
// per language, the paired classifier, which does not score a finding no oracle owns, and the
// Ruby-parity classifier, which does; a report that is not JSON leaves the case unscored under its
// own corpus and lane.

/** Runs the driver from the repository root and returns its exit status and stderr. */
const run = (args, env = {}) => spawnSync(process.execPath, [script, ...args], { cwd: root, encoding: "utf8", env: { ...process.env, ...env } });

/** Checks an unknown corpus or case exits 2 and names it. */
test("an unknown corpus or case exits 2", () => {
  const corpus = run(["--corpus", "nope"]);
  assert.equal(corpus.status, 2);
  assert.match(corpus.stderr, /no corpus named nope/);
  assert.doesNotMatch(corpus.stderr, /no case named/);
  const unknown = run(["--case", "no-such-case"]);
  assert.equal(unknown.status, 2);
  assert.match(unknown.stderr, /no case named no-such-case/);
  const scoped = run(["--corpus", "behavior", "--case", "no-such-case"]);
  assert.equal(scoped.status, 2);
  assert.match(scoped.stderr, /in corpus behavior/);
});

/** Checks a binary that cannot start leaves every corpus's case unscored, exits 1 and scores nothing as clean. */
test("an unobservable case in every corpus exits 1 and is listed with its corpus", async () => {
  const scratch = await mkdtemp(join(tmpdir(), "cuke-evaluate-test-"));
  try {
    for (const corpus of (await loadCorpora(join(root, "fixtures"))).filter(({ name }) => evaluated.includes(name))) {
      const name = corpus.items[0].name ?? corpus.items[0].id;
      const json = join(scratch, `${corpus.name}.json`);
      const result = run(["--binary", join(scratch, "missing-binary"), "--corpus", corpus.name, "--case", name, "--json", json]);
      assert.equal(result.status, 1, `${corpus.name}: ${result.stderr}`);
      const report = JSON.parse(await readFile(json, "utf8"));
      const lanes = corpus.name === "language-parity" ? ["typescript", "ruby"] : [null];
      assert.deepEqual(report.unobservable.map((entry) => [entry.corpus, entry.name, entry.lane]), lanes.map((lane) => [corpus.name, name, lane]));
      assert.deepEqual([report.cases, report.observed, report.rules], [1, 0, []], corpus.name);
    }
  } finally {
    await rm(scratch, { recursive: true, force: true });
  }
});

/** Checks a binary's report reaches the behavior classifier; needs a shell script as the binary. */
test("the driver wires the behavior corpus to its classifier", { skip: process.platform === "win32" }, async () => {
  const scratch = await mkdtemp(join(tmpdir(), "cuke-evaluate-fake-"));
  try {
    const binary = join(scratch, "fake-binary");
    await writeFile(binary, '#!/bin/sh\nwhile [ $# -gt 0 ]; do [ "$1" = --output ] && out=$2; shift; done\nmkdir -p "$out"\ncp "$FAKE_REPORT" "$out/cuke-dedup.json"\nexit "$FAKE_EXIT"\n');
    await chmod(binary, 0o755);
    const item = (await loadCorpora(join(root, "fixtures"))).find(({ name }) => name === "behavior").items[0];
    /** Evaluates the first behavior case against a fake report text and returns the exit status and JSON report. */
    const evaluate = async (reportText, exit) => {
      const file = join(scratch, "fake-report.json");
      await writeFile(file, reportText);
      const json = join(scratch, "behavior.json");
      const result = run(["--binary", binary, "--corpus", "behavior", "--case", item.name, "--json", json], { FAKE_REPORT: file, FAKE_EXIT: String(exit) });
      return { status: result.status, report: JSON.parse(await readFile(json, "utf8")) };
    };
    /** A report with the case's census and duplication summary and the given per-rule counts. */
    const fakeReport = (byRule) => JSON.stringify({
      summary: {
        definitionsAnalyzed: item.expected.definitions,
        featureStepsAnalyzed: item.expected.featureSteps,
        byRule,
        duplication: { duplicatedDefinitions: item.expected.duplicatedDefinitions, percentage: item.expected.percentage, threshold: item.expected.threshold, passed: item.expected.passed },
      },
    });
    const counted = await evaluate(fakeReport({ ...item.expected.byRule, "stray-rule": 1 }), item.expectedExit);
    assert.equal(counted.status, 0);
    assert.deepEqual(counted.report.rules.filter((row) => row.fp > 0).map((row) => [row.corpus, row.rule, row.fp]), [["behavior", "stray-rule", 1]]);
    // Control: the expected counts alone have no false positive.
    const exact = await evaluate(fakeReport(item.expected.byRule), item.expectedExit);
    assert.deepEqual(exact.report.rules.filter((row) => row.fp > 0), []);
    // A report whose duplication summary is malformed is unscored, not scored from its byRule alone.
    const malformed = await evaluate(JSON.stringify({ summary: { ...JSON.parse(fakeReport(item.expected.byRule)).summary, duplication: {} } }), item.expectedExit);
    assert.equal(malformed.status, 1);
    assert.ok(malformed.report.unobservable.some((entry) => entry.corpus === "behavior" && /unmodeled report/.test(entry.runFailure)));
    // A report that is not JSON makes the case unscored under its own corpus, not the default.
    const garbage = await evaluate("{not json", 0);
    assert.equal(garbage.status, 1);
    assert.ok(garbage.report.unobservable.some((entry) => entry.corpus === "behavior" && /observation failed/.test(entry.runFailure)));
  } finally {
    await rm(scratch, { recursive: true, force: true });
  }
});

/** Checks a binary's report reaches the paired classifier once per language; needs a shell script as the binary. */
test("the driver scores a paired case once per language", { skip: process.platform === "win32" }, async () => {
  const scratch = await mkdtemp(join(tmpdir(), "cuke-evaluate-fake-"));
  try {
    const binary = join(scratch, "fake-binary");
    await writeFile(binary, '#!/bin/sh\nwhile [ $# -gt 0 ]; do [ "$1" = --output ] && out=$2; shift; done\nmkdir -p "$out"\ncp "$FAKE_REPORT" "$out/cuke-dedup.json"\nexit 1\n');
    await chmod(binary, 0o755);
    const item = (await loadCorpora(join(root, "fixtures"))).find(({ name }) => name === "language-parity").items[0];
    /** Evaluates the first paired case against a fake report text and returns the exit status and JSON report. */
    const evaluate = async (reportText) => {
      const file = join(scratch, "fake-report.json");
      await writeFile(file, reportText);
      const json = join(scratch, "language-parity.json");
      const result = run(["--binary", binary, "--corpus", "language-parity", "--case", item.id, "--json", json], { FAKE_REPORT: file });
      return { status: result.status, report: JSON.parse(await readFile(json, "utf8")) };
    };
    const stray = { rule: "stray-rule", suppression: null, primary: { path: "steps.rb", line: 1 }, related: [], message: "m", evidence: {} };
    /** A completed report with the oracle's census and the given findings. */
    const fakeReport = (findings) => JSON.stringify({
      summary: { definitionsAnalyzed: item.expected.definitions, featureStepsAnalyzed: item.expected.featureSteps },
      corpus: { incomplete: false },
      findingsTruncated: 0,
      analysis: { candidateSources: {} },
      findings,
    });
    // The paired gate does not require ownership, so an unowned finding is not a false positive here.
    const scored = await evaluate(fakeReport([stray]));
    assert.equal(scored.status, 0);
    assert.deepEqual(scored.report.rules.filter((row) => row.fp > 0), []);
    assert.deepEqual([...new Set(scored.report.rules.map((row) => row.lane))].sort(), ["ruby", "typescript"]);
    // A report that is not JSON leaves each language unscored under its own lane.
    const garbage = await evaluate("{not json");
    assert.equal(garbage.status, 1);
    assert.deepEqual(garbage.report.unobservable.map((entry) => [entry.corpus, entry.lane]).sort(), [["language-parity", "ruby"], ["language-parity", "typescript"]]);
  } finally {
    await rm(scratch, { recursive: true, force: true });
  }
});

/** Checks a binary's report reaches the Ruby-parity classifier, which scores a finding no required finding owns. */
test("the driver scores a Ruby-parity group with ownership", { skip: process.platform === "win32" }, async () => {
  const scratch = await mkdtemp(join(tmpdir(), "cuke-evaluate-fake-"));
  try {
    const binary = join(scratch, "fake-binary");
    await writeFile(binary, '#!/bin/sh\nwhile [ $# -gt 0 ]; do [ "$1" = --output ] && out=$2; shift; done\nmkdir -p "$out"\ncp "$FAKE_REPORT" "$out/cuke-dedup.json"\nexit "$FAKE_EXIT"\n');
    await chmod(binary, 0o755);
    const item = (await loadCorpora(join(root, "fixtures"))).find(({ name }) => name === "ruby-parity").items[0];
    const stray = { rule: "stray-rule", suppression: null, primary: { path: "steps.rb", line: 1 }, related: [], message: "m", evidence: {} };
    /** Evaluates the first group against a fake report text and returns the exit status and JSON report. */
    const evaluate = async (reportText) => {
      const file = join(scratch, "fake-report.json");
      await writeFile(file, reportText);
      const json = join(scratch, "ruby-parity.json");
      const result = run(["--binary", binary, "--corpus", "ruby-parity", "--case", item.name, "--json", json], { FAKE_REPORT: file, FAKE_EXIT: String(item.expectedExit) });
      return { status: result.status, report: JSON.parse(await readFile(json, "utf8")) };
    };
    const scored = await evaluate(JSON.stringify({
      summary: { definitionsAnalyzed: item.definitions, featureStepsAnalyzed: item.featureSteps },
      corpus: { incomplete: !item.complete },
      findingsTruncated: 0,
      analysis: { candidateSources: Object.fromEntries(Object.entries(item.expectedCandidateSources ?? {}).map(([source, evaluated]) => [source, { evaluated }])) },
      findings: [stray],
    }));
    assert.equal(scored.status, 0);
    assert.deepEqual(scored.report.rules.filter((row) => row.fp > 0).map((row) => [row.corpus, row.rule, row.fp]), [["ruby-parity", "stray-rule", 1]]);
    const garbage = await evaluate("{not json");
    assert.equal(garbage.status, 1);
    assert.deepEqual(garbage.report.unobservable.map((entry) => [entry.corpus, entry.name, entry.lane]), [["ruby-parity", item.name, null]]);
  } finally {
    await rm(scratch, { recursive: true, force: true });
  }
});
