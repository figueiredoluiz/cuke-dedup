import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { cp, mkdir, mkdtemp, readFile, readdir, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { spawnSync } from "node:child_process";
import { cargoTestArgs, completionPassed, corpusCoverageDeficits, evidenceRunPassed, evidenceTargets, executesEvidence, parityDeficits, validateOracle, validateUnitEntry } from "./lib/ruby-parity.mjs";
import { normalizeOutcome } from "./lib/behavior-spec.mjs";
import { regressionFailures, snapshot } from "./lib/parity-regression.mjs";

const args = process.argv.slice(2);
const regression = args.includes("--regression");
assert.ok(args.filter((arg) => !arg.startsWith("--")).length <= 1, "expected at most one binary");
assert.ok(args.filter((arg) => arg.startsWith("--")).every((arg) =>
  ["--regression", "--inventory-only", "--corpus-coverage-only"].includes(arg)), "unknown option");
const coverageOnly = args.includes("--corpus-coverage-only");
const inventoryOnly = args.includes("--inventory-only") || coverageOnly;
assert.ok(!regression || !inventoryOnly, "regression requires execution");
const binary = resolve(args.find((arg) => !arg.startsWith("--")) ?? "target/debug/cuke-dedup");
const root = resolve("fixtures/ruby-parity");
const readJson = async (path) => JSON.parse(await readFile(path, "utf8"));
const manifest = await readJson(join(root, "manifest.json"));
assert.equal(manifest.schemaVersion, 1);

function within(parent, child) {
  const path = resolve(parent, child);
  const rel = relative(parent, path);
  assert.ok(!isAbsolute(rel) && rel !== ".." && !rel.startsWith(`..${sep}`), `fixture escapes root: ${child}`);
  return path;
}

const sources = new Map();
for (const file of manifest.sourceManifests) {
  for (const item of (await readJson(file)).cases) sources.set(`${file}:${item.name}`, item);
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
// The unit census is completion bookkeeping pinned to every Rust test file. Regression mode guards
// observable outcomes only, so unrelated test edits elsewhere in the repository cannot fail it.
let unitContracts = 0;
const units = regression ? { schemaVersion: 1, files: {} } : await readJson(join(root, "unit-cases.json"));
assert.equal(units.schemaVersion, 1);
async function testFiles(directory) {
  const files = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = `${directory}/${entry.name}`;
    if (path === "src/ruby" || path === "tests/ruby.rs") continue;
    if (entry.isDirectory()) files.push(...await testFiles(path));
    else if (path.endsWith(".rs") && /#\[test\]/.test(await readFile(path, "utf8"))) files.push(path);
  }
  return files;
}
if (!regression) {
  assert.deepEqual(Object.keys(units.files).sort(), [...await testFiles("src"), ...await testFiles("tests")].sort(),
    "test files changed; extend the Ruby completion census");
}
const census = new Map();
for (const [file, inventory] of Object.entries(units.files)) {
  for (const item of inventory.tests) census.set(`${file}::${item.name}`, item);
}
for (const [file, inventory] of Object.entries(units.files)) {
  const content = (await readFile(file, "utf8")).replaceAll("\r\n", "\n");
  assert.equal(createHash("sha256").update(content).digest("hex"), inventory.sourceSha256,
    `${file}: source tests changed; reassess unit-test mappings`);
  const names = [...content.matchAll(/#\[test\]\s*fn (\w+)/g)].map((match) => match[1]);
  assert.deepEqual(inventory.tests.map((item) => item.name).sort(), names.sort(),
    `${file}: unit-test census is incomplete or duplicated`);
  for (const item of inventory.tests) {
    unitContracts++;
    const blocker = validateUnitEntry(file, item, groups, census);
    if (blocker) blockers.push(blocker);
  }
}
// Rust evidence is executed, not merely named: an ignored, filtered-out or failing test blocks.
// Only completion mode executes; inventory and coverage validation never run the analyzer.
if (executesEvidence({ regression, inventoryOnly })) {
  for (const target of evidenceTargets(units)) {
    const args = cargoTestArgs(target);
    const run = spawnSync("cargo", args, { cwd: resolve("."), encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
    if (run.status !== 0 || !evidenceRunPassed(run.stdout)) {
      blockers.push(`${target.referencedBy}: rust-evidence ${target.file}:${target.name} did not run and pass`);
    }
  }
}
for (const [name, group] of Object.entries(groups)) {
  validateOracle(group);
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
      const project = within(scratch, name);
      await cp(within(root, oracle.path), project, { recursive: true });
      for (const item of oracle.materialize ?? []) {
        const destination = within(project, item.destination);
        await mkdir(dirname(destination), { recursive: true });
        await cp(within(project, item.source), destination);
      }
      const output = join(scratch, `${name}-output`);
      const run = spawnSync(binary, [".",
        ...(oracle.arguments ?? ["--definitions", oracle.definitionsPattern ?? "*.rb"]),
        "--reporters", "json", "--output", output, "--no-metrics", "--fail-on-incomplete"],
      { cwd: project, encoding: "utf8", timeout: 30_000, maxBuffer: 1024 * 1024 });
      let deficits, measured, digest, failure;
      try {
        if (run.error) throw run.error;
        assert.ok([0, 1, 2].includes(run.status), `analyzer did not complete: ${run.signal ?? run.status}`);
        const report = await readJson(join(output, "cuke-dedup.json"));
        measured = { ...normalizeOutcome(report, run.status, { "steps.ts": "steps", "steps.rb": "steps" }), duplication: report.summary.duplication,
          candidateSources: report.analysis.candidateSources };
        deficits = parityDeficits(oracle, report, run.status);
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
console.log(`${mappings.length} corpus cases mapped; `
  + (regression ? "unit census not checked in regression mode; " : `${unitContracts} Rust test functions inventoried; `)
  + `${Object.keys(groups).length} executable Ruby groups; `
  + (inventoryOnly ? "not executed; " : `${observations.length - failed} passing, ${failed} failing; `)
  + `${blockers.length} pending mappings/design items (not distinct defects).`);
if (process.env.CUKE_DEDUP_RUBY_PARITY_REPORT) {
  const binarySha256 = inventoryOnly ? null : createHash("sha256").update(await readFile(binary)).digest("hex");
  await writeFile(process.env.CUKE_DEDUP_RUBY_PARITY_REPORT,
    `${JSON.stringify({ binarySha256, mappings: mappings.length, unitContracts, observations, blockers }, null, 2)}\n`);
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
