import assert from "node:assert/strict";
import { copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { renderSeed } from "./corpus-seed.mjs";
import { replayProject, sealProject } from "./corpus-replay.mjs";

/** Verify that the reviewed seed regenerates the owned fixture exactly. */
test("base bytes and labels match the reviewed mixed-module fixture", async () => {
  const { project } = renderSeed([]);
  assert.equal(project.metadata.verdict, "reviewed");
  assert.equal(project.applicability, "valid");
  assert.deepEqual(Object.keys(project.files).sort(), ["common.cjs", "esm.mjs", "example.feature"]);
  for (const [path, bytes] of Object.entries(project.files)) {
    assert.equal(bytes, await readFile(new URL(`../../../fixtures/corpus/mixed-esm-cjs/${path}`, import.meta.url), "utf8"));
  }
});

/** Derivatives retain the declared ownership and order across a JSON-only round trip. */
test("composed transforms preserve owners and replay their exact files", async () => {
  const ops = ["move-module", "extract-helper", "value=café bleu", "outline", "disable-ambiguity", "restore-ambiguity"];
  const bundle = renderSeed(ops);
  assert.deepEqual(renderSeed(ops), bundle);
  assert.deepEqual(bundle.project.operations, ops);
  assert.equal(bundle.project.metadata.verdict, "generated");
  assert.deepEqual(bundle.project.locations.prepare, { path: "steps/registration.mjs", line: 4 });
  assert.deepEqual(bundle.project.locations.restore, { path: "common.cjs", line: 3 });
  assert.match(bundle.project.files["steps/registration.mjs"], /from "\.\.\/support\/workspace\.mjs"/);
  const root = await replayProject(JSON.parse(JSON.stringify(bundle)));
  try {
    for (const [path, text] of Object.entries(bundle.project.files)) assert.equal(await readFile(join(root, path), "utf8"), text);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

/** Invalid and unsupported cases cannot acquire successful semantic expectations. */
test("operations have explicit applicability and enforce composition preconditions", () => {
  const invalid = renderSeed(["malformed-config"]).project;
  assert.equal(invalid.applicability, "invalid");
  assert.deepEqual(invalid.expected, { refusal: "configuration", exit: 2 });
  const unsupported = renderSeed(["unknown-framework"]).project;
  assert.equal(unsupported.applicability, "unsupported");
  assert.equal(unsupported.expected, null);
  for (const ops of [["restore-ambiguity"], ["outline", "outline"], ["malformed-config", "outline"], ["unknown-framework", "move-module"], ["value=a|b"], ["value=  "], Array(17).fill("outline")]) {
    assert.throws(() => renderSeed(ops), undefined, JSON.stringify(ops));
  }
});

/** A modified expectation or file fails checksum verification; unsafe or oversized input fails even when re-sealed. */
test("replay refuses corrupt, out-of-domain and over-budget bundles", async () => {
  const bundle = renderSeed();
  await assert.rejects(replayProject({ ...bundle, schemaVersion: 2 }), /version/);
  const changed = structuredClone(bundle);
  changed.project.expected.groups = [];
  await assert.rejects(replayProject(changed), /digest/);
  for (const files of [{ "../escape.mjs": "" }, { "steps/CON.mjs": "" }, { "nul.cjs": "" }, { "x.mjs": "é".repeat(32769) }, {}, Object.fromEntries(Array.from({ length: 65 }, (_, i) => [`f${i}.mjs`, ""]))]) {
    await assert.rejects(replayProject(sealProject({ ...bundle.project, files })), /path|filename|budget/);
  }
});

/** An installed replay entry point needs only its reader and saved artifact, even for a retired recipe. */
test("CLI replays saved bytes with the renderer and manifest absent", async (t) => {
  const scratch = await mkdtemp(join(tmpdir(), "seed-reader-"));
  t.after(() => rm(scratch, { recursive: true, force: true }));
  await mkdir(join(scratch, "lib"));
  const cli = join(scratch, "corpus-seed.mjs");
  await copyFile(new URL("../corpus-seed.mjs", import.meta.url), cli);
  await copyFile(new URL("./corpus-replay.mjs", import.meta.url), join(scratch, "lib", "corpus-replay.mjs"));
  const project = renderSeed(["outline"]).project;
  project.operations = ["retired-operation"];
  project.files["example.feature"] += "# preserved concrete input\n";
  const file = join(scratch, "case.json");
  await writeFile(file, JSON.stringify(sealProject(project)));
  const run = spawnSync(process.execPath, [cli, "replay", file], { encoding: "utf8", timeout: 5000 });
  assert.equal(run.status, 0, run.stderr);
  const root = run.stdout.trim();
  t.after(() => rm(root, { recursive: true, force: true }));
  assert.equal(await readFile(join(root, "example.feature"), "utf8"), project.files["example.feature"]);
});
