import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { distribution, renderDistribution } from "./distribution.mjs";

/** A case with the metadata the report reads, overridable. */
const testCase = (name, over = {}) => ({
  name,
  path: name,
  role: "positive",
  origin: "synthetic",
  language: "ecmascript",
  frameworks: ["cucumber-js"],
  verdict: "reviewed",
  axes: ["import-form"],
  expectedFindings: [{ rule: "duplicate-matcher" }],
  ...over,
});

/** Checks totals per dimension count each case once per value, and multi-valued fields count each value. */
test("dimension totals count cases, and multi-valued fields count every value", () => {
  const report = distribution({
    cases: [
      testCase("a", { frameworks: ["cucumber-js", "jest-cucumber"], axes: ["import-form", "scope"] }),
      testCase("b", { role: "negative", origin: "regression", axes: ["scope"], expectedFindings: [], expectedAbsent: [{ rule: "duplicate-matcher" }] }),
    ],
  });
  assert.equal(report.cases, 2);
  assert.deepEqual(report.dimensions.role, [{ value: "negative", cases: 1 }, { value: "positive", cases: 1 }]);
  assert.deepEqual(report.dimensions.framework, [{ value: "cucumber-js", cases: 2 }, { value: "jest-cucumber", cases: 1 }]);
  assert.deepEqual(report.axes.filter((entry) => entry.cases > 0).map((entry) => [entry.axis, entry.cases]), [["import-form", 1], ["scope", 2]]);
  assert.deepEqual(report.dimensions.origin.map((entry) => entry.value).sort(), ["regression", "synthetic"]);
});

/** Checks expectations per rule: counts honour `count`, and only isolatable rules report isolation. */
test("rule totals sum expectations by outcome and isolation", () => {
  const report = distribution({
    cases: [
      testCase("a", {
        expectedFindings: [{ rule: "duplicate-handler", count: 3 }, { rule: "unused-definition" }],
        expectedAbsent: [{ rule: "duplicate-handler", isolation: { differs: [] } }, { rule: "duplicate-handler" }, { rule: "unused-definition" }],
        knownMisses: [{ rule: "duplicate-handler", id: "m" }],
        expectedAmbiguous: [{ rule: "unused-definition" }],
      }),
    ],
  });
  const handler = report.rules.find((entry) => entry.rule === "duplicate-handler");
  assert.deepEqual([handler.positives, handler.absent, handler.isolated, handler.knownMisses, handler.isolatable], [3, 2, 1, 1, true]);
  const unused = report.rules.find((entry) => entry.rule === "unused-definition");
  assert.deepEqual([unused.positives, unused.absent, unused.ambiguous, unused.isolatable], [1, 1, 1, false]);
  // Only the isolatable rule's undeclared negative counts as unisolated.
  assert.equal(report.unisolatedNegatives, 1);
});

/** Checks the thin cells name axes and frameworks at or below the floor, rules missing an outcome, and a single language. */
test("thin cells are named by the floor", () => {
  const manifest = { cases: [testCase("a", { axes: ["import-form", "scope"] }), testCase("b", { axes: ["import-form"], frameworks: ["cypress-cucumber"] })] };
  const one = distribution(manifest, { floor: 1 });
  assert.ok(one.thin.axes.some((entry) => entry.axis === "scope" && entry.cases === 1));
  assert.ok(!one.thin.axes.some((entry) => entry.axis === "import-form"));
  assert.deepEqual(one.thin.frameworks.map((entry) => entry.value).sort(), ["cucumber-js", "cypress-cucumber"]);
  // Control: a higher floor reaches the axis and framework that a lower one does not.
  assert.ok(distribution(manifest, { floor: 2 }).thin.axes.some((entry) => entry.axis === "import-form"));
  assert.equal(distribution(manifest, { floor: 0 }).thin.frameworks.length, 0);
  assert.deepEqual(one.thin.rulesWithoutNegatives, ["duplicate-matcher"]);
  assert.deepEqual(one.thin.rulesWithoutPositives, []);
  assert.deepEqual(one.thin.languagesMissing, ["ruby"]);
  assert.deepEqual(one.thin.originsMissing, ["real", "regression"]);
  const mixed = distribution({ cases: [testCase("a", { language: "ruby" }), testCase("b")] });
  assert.deepEqual(mixed.thin.languagesMissing, []);
});

/** Checks a case missing its labels is a row in the report, not a crash, and a stray axis is named. */
test("missing labels and axes outside the vocabulary are reported, not thrown", () => {
  const bare = { name: "bare", path: "bare", expectedFindings: [{ rule: "duplicate-matcher" }] };
  const report = distribution({ cases: [bare, testCase("odd", { axes: ["import-form", "made-up"] })] });
  assert.deepEqual(report.dimensions.role, [{ value: "(unlabelled)", cases: 1 }, { value: "positive", cases: 1 }]);
  assert.deepEqual(report.dimensions.framework.map((entry) => entry.value).sort(), ["(unlabelled)", "cucumber-js"]);
  assert.deepEqual(report.thin.casesWithoutAxes, ["bare"]);
  assert.deepEqual(report.thin.axesOutsideVocabulary, ["made-up"]);
  assert.match(renderDistribution(report), /Cases with no axes: bare/);
  assert.match(renderDistribution(report), /Axes outside the vocabulary: made-up/);
});

/** Checks the markdown carries every section and the thin-cell list. */
test("the markdown has each section and the thin cells", () => {
  const text = renderDistribution(distribution({ cases: [testCase("a"), testCase("b", { path: "shared" }), testCase("c", { path: "shared" })] }));
  for (const heading of ["By role", "By origin", "By language", "By verdict", "By framework", "By rule", "By axis and rule", "Fixture roots", "Thin cells"]) {
    assert.ok(text.includes(`## ${heading}`), heading);
  }
  assert.match(text, /\| shared \| 2 \|/);
  assert.match(text, /\| import-form \| 3 \| 3 \|/);
  assert.match(text, /Rules with no negative: duplicate-matcher/);
  assert.match(text, /Languages with no case: ruby/);
  assert.match(text, /Behavior corpus cases, which carry no labels: 0/);
  assert.match(renderDistribution(distribution({ cases: [testCase("a")] }, { behaviorCases: 23 })), /carry no labels: 23/);
});

/** Checks the real manifest's totals agree with the baselines the corpus gate asserts exactly. */
test("the committed manifest's totals match the gate's baselines", async () => {
  const manifest = JSON.parse(await readFile("fixtures/recall/manifest.json", "utf8"));
  const report = distribution(manifest);
  assert.equal(report.cases, manifest.minimumCases);
  assert.equal(report.rules.reduce((sum, entry) => sum + entry.knownMisses, 0), manifest.knownMissBaseline);
  assert.equal(report.unisolatedNegatives, manifest.unisolatedNegativeBaseline);
  assert.equal(report.axes.reduce((sum, entry) => sum + entry.cases, 0) >= report.cases, true);
  // Every case sits in at least one axis, so the axis totals cannot be below the case count.
  assert.ok(manifest.cases.every((entry) => entry.axes.length > 0));
});

/** Checks the command exits 2 for a bad option or floor, and writes the report for a good call. */
test("the command exits 2 for a bad invocation and prints the report otherwise", () => {
  const script = fileURLToPath(new URL("../corpus-distribution.mjs", import.meta.url));
  for (const args of [["--bogus"], ["--floor"], ["--floor", "x"], ["--floor", "-1"]]) {
    assert.equal(spawnSync(process.execPath, [script, ...args], { encoding: "utf8" }).status, 2, args.join(" "));
  }
  const good = spawnSync(process.execPath, [script, "--floor", "2"], { encoding: "utf8" });
  assert.equal(good.status, 0);
  assert.match(good.stdout, /^# Recall corpus distribution/);
  assert.match(good.stdout, /A value is thin at 2 cases or fewer/);
});
