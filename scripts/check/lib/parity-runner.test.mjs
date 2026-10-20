import { test } from "node:test";
import assert from "node:assert/strict";
import { chmod, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { completedReport, observeParityLane, observeRubyGroup, within } from "./parity-runner.mjs";

/** An observation with the given overrides, otherwise a completed run with a report. */
const observed = (over = {}) => ({ status: 1, signal: null, stderr: "", spawnFailure: null, report: { findings: [] }, ...over });

/** Checks a completed run returns its report and every operational failure throws with its cause. */
test("completedReport returns the report and rejects operational failures", () => {
  const report = { findings: [] };
  for (const status of [0, 1, 2]) assert.equal(completedReport(observed({ status, report })), report);
  assert.throws(() => completedReport(observed({ spawnFailure: "spawn ENOENT" })), /spawn ENOENT/);
  assert.throws(() => completedReport(observed({ status: null, signal: "SIGTERM" })), /did not complete: SIGTERM/);
  assert.throws(() => completedReport(observed({ status: 101 })), /did not complete: 101/);
  assert.throws(() => completedReport(observed({ report: null })), /wrote no JSON report/);
  assert.throws(() => completedReport(observed({ report: null, reportError: "Unexpected end" })), /invalid JSON report: Unexpected end/);
});

/** Checks a report that is not JSON keeps the exit status and stderr, and a binary that cannot start has no report. */
test("observeParityLane keeps the run when the report is unusable", { skip: process.platform === "win32" }, async () => {
  const directory = await mkdtemp(join(tmpdir(), "parity-runner-"));
  try {
    const binary = join(directory, "fake-binary");
    await writeFile(binary, "#!/bin/sh\nmkdir -p report\nprintf \"{broken\" > report/cuke-dedup.json\necho crashed >&2\nexit 1\n");
    await chmod(binary, 0o755);
    const item = { sources: { typescript: "x" }, feature: "Feature: f" };
    const broken = await observeParityLane({ binary, directory: join(directory, "a"), item, language: "typescript", extension: "ts" });
    assert.deepEqual([broken.status, broken.report, broken.spawnFailure, broken.stderr.trim()], [1, null, null, "crashed"]);
    assert.match(broken.reportError, /JSON/);
    assert.throws(() => completedReport(broken), /invalid JSON report/);
    // A run killed after writing a partial report has no report: the partial one proves nothing.
    const killed = join(directory, "killed-binary");
    await writeFile(killed, "#!/bin/sh\nmkdir -p report\nprintf \"{}\" > report/cuke-dedup.json\nkill -9 $$\n");
    await chmod(killed, 0o755);
    const partial = await observeParityLane({ binary: killed, directory: join(directory, "c"), item, language: "typescript", extension: "ts" });
    assert.deepEqual([partial.status, partial.signal, partial.report], [null, "SIGKILL", null]);
    const missing = await observeParityLane({ binary: join(directory, "none"), directory: join(directory, "b"), item, language: "typescript", extension: "ts" });
    assert.deepEqual([missing.status, missing.report, missing.reportError], [null, null, null]);
    assert.match(missing.spawnFailure, /ENOENT/);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

/** Checks a path inside the parent resolves and one escaping it is rejected. */
test("within keeps fixture paths under their parent", () => {
  assert.equal(within(resolve("/a/b"), "c/d"), resolve("/a/b/c/d"));
  assert.throws(() => within(resolve("/a/b"), "../c"), /escapes root/);
  assert.throws(() => within(resolve("/a/b"), resolve("/etc")), /escapes root/);
});

/** Checks a group runs from a copy with its materialized files, its own arguments, and an escaping path is refused. */
test("observeRubyGroup copies the group, materializes files and runs with its arguments", { skip: process.platform === "win32" }, async () => {
  const directory = await mkdtemp(join(tmpdir(), "parity-runner-ruby-"));
  try {
    await mkdir(join(directory, "root", "group"), { recursive: true });
    await writeFile(join(directory, "root", "group", "source.rb"), "x");
    const binary = join(directory, "fake-binary");
    await writeFile(binary, "#!/bin/sh\nprintf \"%s \" \"$@\" > args.txt\nls >> args.txt\ncat nested/copy.rb >> args.txt 2>/dev/null\nwhile [ $# -gt 0 ]; do [ \"$1\" = --output ] && out=$2; shift; done\nmkdir -p \"$out\"\nprintf \"{}\" > \"$out/cuke-dedup.json\"\nexit 1\n");
    await chmod(binary, 0o755);
    const base = { binary, root: join(directory, "root"), scratch: join(directory, "scratch") };
    const oracle = { path: "group", materialize: [{ source: "source.rb", destination: "nested/copy.rb" }] };
    const observed = await observeRubyGroup({ ...base, oracle });
    assert.deepEqual([observed.status, observed.report], [1, {}]);
    const args = await readFile(join(directory, "scratch", "project", "args.txt"), "utf8");
    assert.match(args, /^\. --definitions \*\.rb --reporters json --output .* --no-metrics --fail-on-incomplete/);
    assert.match(args, /nested\nsource\.rb\nx$/);
    // The committed fixture is copied, not written into.
    assert.equal(await readFile(join(directory, "root", "group", "source.rb"), "utf8"), "x");
    await assert.rejects(readFile(join(directory, "root", "group", "nested", "copy.rb")), /ENOENT/);
    const custom = await observeRubyGroup({ ...base, scratch: join(directory, "scratch2"), oracle: { path: "group", arguments: ["--definitions", "*.x"] } });
    assert.match(await readFile(join(directory, "scratch2", "project", "args.txt"), "utf8"), /^\. --definitions \*\.x --reporters/);
    const pattern = await observeRubyGroup({ ...base, scratch: join(directory, "scratch4"), oracle: { path: "group", definitionsPattern: "*.y" } });
    assert.equal(pattern.status, 1);
    assert.match(await readFile(join(directory, "scratch4", "project", "args.txt"), "utf8"), /^\. --definitions \*\.y --reporters/);
    await assert.rejects(observeRubyGroup({ ...base, scratch: join(directory, "scratch3"), oracle: { path: "../outside" } }), /escapes root/);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
