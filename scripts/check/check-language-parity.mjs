import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { classify, deficits, outcome, validateManifest } from "./lib/language-parity.mjs";
import { regressionFailures, snapshot } from "./lib/parity-regression.mjs";

const args = process.argv.slice(2);
const regression = args[0] === "--regression";
assert.equal(args.length, 3, "Usage: node scripts/check/check-language-parity.mjs CURRENT_BINARY V010_BINARY REPORT_JSON | --regression CURRENT_BINARY REPORT_JSON");
const [current, baseline, destination] = (regression ? [args[1], null, args[2]] : args)
  .map((arg) => arg === null ? null : resolve(arg));
const manifestBytes = await readFile("fixtures/language-parity/manifest.json");
const manifest = JSON.parse(manifestBytes);
validateManifest(manifest);
const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");
const lanes = {
  ...(regression ? {} : { releasedTypescript: { binary: baseline, language: "typescript", extension: "ts" } }),
  currentTypescript: { binary: current, language: "typescript", extension: "ts" },
  ruby: { binary: current, language: "ruby", extension: "rb" },
};
const observations = [];
const scratch = await mkdtemp(join(tmpdir(), "cuke-language-parity-"));
try {
  for (const item of manifest.cases) {
    const observation = { id: item.id, rule: item.rule };
    for (const [name, lane] of Object.entries(lanes)) {
      const directory = join(scratch, item.id, name);
      await mkdir(directory, { recursive: true });
      await writeFile(join(directory, `steps.${lane.extension}`), item.sources[lane.language]);
      await writeFile(join(directory, "suite.feature"), item.feature);
      const run = spawnSync(lane.binary, [".", "--definitions", `*.${lane.extension}`,
        "--features", "*.feature", "--reporters", "json", "--output", "report", "--no-metrics", "--fail-on-incomplete"],
      { cwd: directory, encoding: "utf8", timeout: 30_000, maxBuffer: 1024 * 1024 });
      try {
        if (run.error) throw run.error;
        assert.ok([0, 1, 2].includes(run.status), `analyzer did not complete: ${run.signal ?? run.status}`);
        const report = JSON.parse(await readFile(join(directory, "report/cuke-dedup.json"), "utf8"));
        observation[name] = { outcome: outcome(report, run.status), deficits: deficits(item.expected, report, run.status) };
      } catch (error) {
        observation[name] = { error: error.message, stderr: run.stderr?.slice(-2000) };
      }
    }
    observation.classification = classify(observation.currentTypescript, observation.ruby);
    observation.typescriptDrift = regression || observation.releasedTypescript.error || observation.currentTypescript.error
      ? null : JSON.stringify(observation.releasedTypescript.outcome) !== JSON.stringify(observation.currentTypescript.outcome);
    observations.push(observation);
    console.log(`${observation.classification}: ${item.id}${observation.typescriptDrift ? " (TypeScript drift)" : ""}`);
  }
} finally {
  await rm(scratch, { recursive: true, force: true });
}
const summary = {};
for (const item of observations) summary[item.classification] = (summary[item.classification] ?? 0) + 1;
const regressionObservations = regression ? observations.map((item) => ({
    id: item.id,
    outcome: { typescript: item.currentTypescript.outcome, ruby: item.ruby.outcome },
    deficits: [...(item.currentTypescript.deficits ?? []).map((gap) => `TypeScript: ${gap}`),
      ...(item.ruby.deficits ?? []).map((gap) => `Ruby: ${gap}`)],
    difference: item.classification === "outcome-difference",
    ...(item.currentTypescript.error === undefined && item.ruby.error === undefined ? {}
      : { error: item.currentTypescript.error ?? item.ruby.error }),
  })) : [];
for (const [index, item] of regressionObservations.entries()) {
  if (!Object.hasOwn(item, "error")) observations[index].digest = snapshot({ outcome: item.outcome, deficits: item.deficits });
}
const report = {
  mode: regression ? "regression" : "historical",
  baselineRef: regression ? null : manifest.baselineRef,
  // The caller supplies a binary built from this pinned commit; hashes identify the actual artifacts.
  expectedBaselineCommit: regression ? null : manifest.baselineCommit,
  manifestSha256: sha256(manifestBytes),
  binaries: { current: { path: current, sha256: sha256(await readFile(current)) },
    baseline: regression ? null : { path: baseline, sha256: sha256(await readFile(baseline)) } },
  scope: manifest.scope,
  summary,
  typescriptDrift: observations.filter((item) => item.typescriptDrift).map((item) => item.id),
  observations,
};
await writeFile(destination, `${JSON.stringify(report, null, 2)}\n`);
console.log(JSON.stringify({ cases: observations.length, ...summary, typescriptDrift: report.typescriptDrift.length }));
// Successful measurement is not successful parity. Missing baselines and shared failures also fail.
if (regression) {
  const failures = regressionFailures(regressionObservations, JSON.parse(await readFile("fixtures/language-parity/regressions.json", "utf8")));
  for (const failure of failures) console.error(failure);
  console.log(`Paired regression gate: ${failures.length} failures.`);
  if (failures.length) process.exitCode = 1;
} else if (observations.some((item) => item.classification !== "equivalent" || item.typescriptDrift !== false
  || item.releasedTypescript.deficits?.length)) process.exitCode = 1;
