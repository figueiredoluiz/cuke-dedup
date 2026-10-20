import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { mkdtemp, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { EVIDENCE_TARGET_DIR, RUBY_SUITES, TEST_FN_PATTERN, completionPassed, corpusCoverageDeficits, evidenceBatches, evidenceTargets, executesEvidence, parityDeficits, partitionEvidenceTargets, rubySuiteEvidence, testBodyMentionsRuby, evidenceFailures, testSummary, validateOracle, validateUnitEntry } from "./lib/ruby-parity.mjs";
import { normalizeOutcome } from "./lib/behavior-spec.mjs";
import { latestClassifiedAt, validateCorpusLabels } from "./lib/manifest-schema.mjs";
import { completedReport, observeRubyGroup, within } from "./lib/parity-runner.mjs";
import { regressionFailures, snapshot } from "./lib/parity-regression.mjs";

const args = process.argv.slice(2);
const regression = args.includes("--regression");
assert.ok(args.filter((arg) => !arg.startsWith("--")).length <= 1, "expected at most one binary");
assert.ok(args.filter((arg) => arg.startsWith("--")).every((arg) =>
  ["--regression", "--inventory-only", "--corpus-coverage-only", "--evidence"].includes(arg)), "unknown option");
const forceEvidence = args.includes("--evidence");
const coverageOnly = args.includes("--corpus-coverage-only");
const inventoryOnly = args.includes("--inventory-only") || coverageOnly;
assert.ok(!regression || !inventoryOnly, "regression requires execution");
assert.ok(!forceEvidence || (!regression && !inventoryOnly), "--evidence requires completion mode");
const binary = resolve(args.find((arg) => !arg.startsWith("--")) ?? "target/debug/cuke-dedup");
const root = resolve("fixtures/ruby-parity");
/** Read a UTF-8 file and parse its JSON, propagating read or parse failures. */
const readJson = async (path) => JSON.parse(await readFile(path, "utf8"));
const manifest = await readJson(join(root, "manifest.json"));
assert.equal(manifest.schemaVersion, 1);

const sources = new Map();
for (const file of manifest.sourceManifests) {
  // `isolation` checks a non-finding against fingerprint records and changes no expected outcome,
  // so declaring one does not ask for a Ruby mapping reassessment. The case metadata only
  // classifies a case for reports and likewise changes no expected outcome.
  for (const raw of (await readJson(file)).cases) {
    const { role, origin, language, frameworks, verdict, classifiedAt, axes, variantOf, ...item } = raw;
    const expectedAbsent = item.expectedAbsent?.map(({ isolation, ...expected }) => expected);
    sources.set(`${file}:${item.name}`, expectedAbsent ? { ...item, expectedAbsent } : item);
  }
}
const groups = { ...manifest.groupOracles };
const mappings = [...manifest.cases];
for (const file of manifest.groupManifests ?? []) {
  const extension = await readJson(within(root, file));
  for (const [name, group] of Object.entries(extension.groupOracles)) {
    assert.ok(!Object.hasOwn(groups, name), `duplicate group: ${name}`);
    groups[name] = group;
  }
  for (const mapping of extension.mappings) {
    const entry = mappings.find((item) => item.source === mapping.source && item.case === mapping.case);
    assert.equal(entry?.fixtureGroup, mapping.fixtureGroup, `${file}: mapping differs from the census`);
  }
}
const mapped = new Set();
const blockers = [];
for (const item of mappings) {
  const key = `${item.source}:${item.case}`;
  assert.ok(sources.has(key), `unknown source case: ${key}`);
  assert.ok(!mapped.has(key), `duplicate mapping: ${key}`);
  mapped.add(key);
  const digest = snapshot(sources.get(key));
  assert.equal(item.sourceCaseSha256, digest, `${key}: source oracle changed; reassess Ruby mapping`);
  assert.ok(["covered", "partial", "missing-fixture", "language-inapplicable", "framework-inapplicable"].includes(item.requiredStatus));
  if (["language-inapplicable", "framework-inapplicable"].includes(item.requiredStatus)) {
    assert.ok(item.rationale?.trim(), `${key}: inapplicable requires a rationale`);
  } else if (item.requiredStatus !== "covered") {
    blockers.push(`${key}: ${item.requiredStatus}`);
  }
  if (item.fixtureGroup) assert.ok(groups[item.fixtureGroup], `${key}: unknown fixture group`);
  if (item.requiredStatus === "covered") assert.ok(item.fixtureGroup, `${key}: covered without fixture`);
}
assert.deepEqual([...mapped].sort(), [...sources.keys()].sort(), "unmapped source cases");
for (const dimension of manifest.additionalRequiredDimensions ?? []) {
  if (dimension.requiredStatus !== "covered") blockers.push(`${dimension.id}: ${dimension.requiredStatus}`);
  else assert.ok(groups[dimension.fixtureGroup], `${dimension.id}: unknown fixture group`);
  for (const file of dimension.fixtureFiles ?? []) await stat(within(root, file));
}
blockers.push(...(manifest.outstandingCensus ?? []));
// The unit census gives every Rust test a Ruby decision; it pins test names, not file contents, so
// comment and non-test edits never invalidate it. Regression mode checks corpus coverage and recorded
// outcomes but skips this census; inventory-only mode enforces it without running the analyzer.
let unitContracts = 0;
const units = regression ? { schemaVersion: 1, files: {} } : await readJson(join(root, "unit-cases.json"));
assert.equal(units.schemaVersion, 1);
/** Rust files under a directory that hold tests, skipping the Ruby-only suites the census does not inventory. */
async function testFiles(directory) {
  const files = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = `${directory}/${entry.name}`;
    if (path === "src/ruby" || RUBY_SUITES.includes(path)) continue;
    if (entry.isDirectory()) files.push(...await testFiles(path));
    else if (path.endsWith(".rs") && /#\[test\]/.test(await readFile(path, "utf8"))) files.push(path);
  }
  return files;
}
if (!regression) {
  assert.deepEqual(Object.keys(units.files).sort(), [...await testFiles("src"), ...await testFiles("tests")].sort(),
    "test files changed; extend the Ruby completion census");
}
// Each census file is read once: for its hash, its test inventory and the Ruby-mention check.
// Ruby-only suites are not inventoried but may be cited as evidence.
const contents = new Map();
for (const file of Object.keys(units.files)) contents.set(file, (await readFile(file, "utf8")).replaceAll("\r\n", "\n"));
const census = new Map();
for (const [file, inventory] of Object.entries(units.files)) {
  for (const item of inventory.tests) {
    census.set(`${file}::${item.name}`, { ...item, mentionsRuby: testBodyMentionsRuby(contents.get(file), item.name, file) });
  }
}
if (!regression) {
  for (const file of RUBY_SUITES) assert.ok(existsSync(file), `${file}: Ruby-only suite is missing; update RUBY_SUITES`);
  for (const file of [...RUBY_SUITES, ...await testFiles("src/ruby")]) {
    for (const [key, entry] of rubySuiteEvidence(file, (await readFile(file, "utf8")).replaceAll("\r\n", "\n"))) census.set(key, entry);
  }
}
for (const [file, inventory] of Object.entries(units.files)) {
  const content = contents.get(file);
  // fail-closed: the census pins test names only; a leftover or new per-file field is rejected.
  assert.deepEqual(Object.keys(inventory), ["tests"], `${file}: unexpected census fields`);
  const names = [...content.matchAll(TEST_FN_PATTERN)].map((match) => match[1]);
  assert.deepEqual(inventory.tests.map((item) => item.name).sort(), names.sort(),
    `${file}: unit-test census is incomplete or duplicated`);
  for (const item of inventory.tests) {
    unitContracts++;
    const blocker = validateUnitEntry(file, item, groups, census);
    if (blocker) blockers.push(blocker);
  }
}
for (const [name, group] of Object.entries(groups)) {
  validateOracle(group);
  validateCorpusLabels(group, name, latestClassifiedAt(), group.requiredFindings.length);
  assert.ok((await stat(within(root, group.path))).isDirectory(), `${name}: missing fixture directory`);
  for (const item of group.materialize ?? []) {
    await stat(within(within(root, group.path), item.source));
    within(within(root, group.path), item.destination);
  }
}

const observations = [];
const scratch = await mkdtemp(join(tmpdir(), "cuke-ruby-parity-"));
try {
  if (!inventoryOnly) {
    for (const [name, oracle] of Object.entries(groups)) {
      // A fixture that cannot be copied or materialized aborts the run; only the analysis is per group.
      const observed = await observeRubyGroup({ binary, root, scratch: within(scratch, name), oracle });
      let deficits, measured, digest, failure;
      try {
        const report = completedReport(observed);
        measured = { ...normalizeOutcome(report, observed.status, { "steps.ts": "steps", "steps.rb": "steps" }), duplication: report.summary.duplication,
          candidateSources: report.analysis.candidateSources };
        deficits = parityDeficits(oracle, report, observed.status);
        digest = snapshot({ outcome: measured, deficits });
      } catch (error) {
        failure = error.message;
        deficits = [`analysis did not produce a usable report: ${failure}`];
      }
      observations.push({ group: name, outcome: measured, deficits, digest, ...(failure === undefined ? {} : { error: failure }) });
      console.log(`${deficits.length ? "GAP" : "PASS"} ${name}${deficits.length ? `: ${deficits.join("; ")}` : ""}`);
    }
  }
} finally {
  await rm(scratch, { recursive: true, force: true });
}
const failed = observations.filter((item) => item.deficits.length > 0).length;
// Rust evidence is executed, not merely named: an ignored, filtered-out or failing test blocks.
// Only completion mode executes; inventory and coverage validation never run the analyzer.
// Evidence runs last, after every oracle and fixture is validated, and builds into its own
// target directory so the analyzed binary is never rewritten by cargo's test profile.
let evidenceExecuted = false;
let platformDeferred = [];
if (executesEvidence({ regression, inventoryOnly, blocked: blockers.length > 0 || failed > 0, forced: forceEvidence })) {
  evidenceExecuted = true;
  const env = { ...process.env, CARGO_TARGET_DIR: EVIDENCE_TARGET_DIR };
  const partition = partitionEvidenceTargets(evidenceTargets(units), process.platform);
  platformDeferred = partition.deferred.map((target) => ({ file: target.file, name: target.name, platforms: target.platforms, referencedBy: target.referencedBy }));
  for (const target of platformDeferred) console.log(`PLATFORM-DEFERRED ${target.file}:${target.name} (requires ${target.platforms.join(", ")})`);
  for (const batch of evidenceBatches(partition.applicable)) {
    const run = spawnSync("cargo", batch.args, { cwd: resolve("."), env, encoding: "utf8", maxBuffer: 64 * 1024 * 1024, timeout: 1_800_000 });
    const failures = evidenceFailures(run, batch);
    if (failures.length === 0) continue;
    const cause = [testSummary(run.stdout), run.error?.message, ...(run.stderr ?? "").trim().split("\n").slice(-3)].filter(Boolean).join(" | ");
    for (const { target, outcome } of failures) {
      blockers.push(`${target.referencedBy.join(", ")}: rust-evidence ${target.file}:${target.name} ${outcome === "FAILED" ? "failed" : outcome} (${cause})`);
    }
  }
}
console.log(`${mappings.length} corpus cases mapped; `
  + (regression ? "unit census not checked in regression mode; " : `${unitContracts} Rust test functions inventoried; `)
  + `${Object.keys(groups).length} executable Ruby groups; `
  + (inventoryOnly ? "not executed; " : `${observations.length - failed} passing, ${failed} failing; `)
  + `${blockers.length} pending mappings/design items (not distinct defects).`);
if (process.env.CUKE_DEDUP_RUBY_PARITY_REPORT) {
  const binarySha256 = inventoryOnly ? null : createHash("sha256").update(await readFile(binary)).digest("hex");
  await writeFile(process.env.CUKE_DEDUP_RUBY_PARITY_REPORT,
    `${JSON.stringify({ binarySha256, evidenceExecuted, platformDeferred, evidenceTargetDir: EVIDENCE_TARGET_DIR, mappings: mappings.length, unitContracts, observations, blockers }, null, 2)}\n`);
}
// Inventory validation is useful during implementation; it is explicitly NOT release acceptance.
// The normal command fails until all desired outcomes and all missing-case dispositions close.
if (regression) {
  const failures = [...corpusCoverageDeficits(mappings, groups),
    ...regressionFailures(observations.map(({ group, ...item }) => ({ id: group, ...item })),
      await readJson(join(root, "regressions.json")))];
  for (const failure of failures) console.error(failure);
  console.log(`Ruby regression gate: ${failures.length} failures; completion remains a separate gate.`);
  if (failures.length) process.exitCode = 1;
} else if (coverageOnly) {
  const missing = corpusCoverageDeficits(mappings, groups);
  console.log(`Source-corpus coverage: ${mappings.length} cases; ${missing.length} missing counterpart dispositions. Unit census and implementation outcomes are separate gates.`);
  if (missing.length) {
    for (const gap of missing) console.log(`MISSING ${gap}`);
    process.exitCode = 1;
  }
} else if (!inventoryOnly && !completionPassed(observations, blockers)) process.exitCode = 1;
