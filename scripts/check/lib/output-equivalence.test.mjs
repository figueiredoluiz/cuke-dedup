import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readdirSync, rmSync, utimesSync, writeFileSync, mkdirSync } from "node:fs";
import { spawn } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  compareRuns,
  fileTimes,
  firstDifference,
  listFiles,
  normalize,
  outputArgument,
  readFiles,
  readRecords,
  summarize,
  withOutput,
} from "./output-equivalence.mjs";

/** Analyzer run record with defaults for every field `over` does not set. */
const run = (over = {}) => ({ status: 0, signal: null, stdout: "", stderr: "", files: null, ...over });

/** Both `--output DIR` and `--output=DIR` are found and rewritten in place; other arguments stay. */
test("output argument is found and replaced in both spellings", () => {
  const spaced = [".", "--reporters", "json", "--output", "report", "--no-metrics"];
  const found = outputArgument(spaced);
  assert.deepEqual(found, { index: 3, inline: false, value: "report" });
  assert.deepEqual(withOutput(spaced, found, "/tmp/x"), [".", "--reporters", "json", "--output", "/tmp/x", "--no-metrics"]);
  const inline = [".", "--output=report"];
  const foundInline = outputArgument(inline);
  assert.deepEqual(foundInline, { index: 1, inline: true, value: "report" });
  assert.deepEqual(withOutput(inline, foundInline, "/tmp/x"), [".", "--output=/tmp/x"]);
  assert.equal(outputArgument([".", "--reporters", "terminal"]), null);
  assert.equal(outputArgument([".", "--output"]), null);
});

/** Timing values and replacements are erased; every other number survives, so counts still compare. */
test("normalization erases timing and replacements but keeps other numbers", () => {
  const json = '{"metrics":{"discoveryMs":1.25,"parsingMs":3e-2,"analysisMs":12,"definitionFiles":4}}';
  assert.equal(normalize(json), '{"metrics":{"discoveryMs":0,"parsingMs":0,"analysisMs":0,"definitionFiles":4}}');
  assert.equal(normalize("Found 2 findings.\nDetection time: 41.7 ms"), "Found 2 findings.\nDetection time: 0 ms");
  assert.equal(normalize("/tmp/a/cuke-dedup.json 0.13.0", [["/tmp/a", "<output>"], ["0.13.0", "0.12.0"]]), "<output>/cuke-dedup.json 0.12.0");
});

/** The first differing line is reported with both sides, including a missing trailing line. */
test("first difference names the line and both sides", () => {
  assert.equal(firstDifference("a\nb", "a\nb"), null);
  assert.deepEqual(firstDifference("a\nb", "a\nc"), { line: 2, baseline: "b", candidate: "c" });
  assert.deepEqual(firstDifference("a", "a\nb"), { line: 2, baseline: "<end>", candidate: "b" });
});

/** Identical runs compare equal; an exit, stream or file change is each reported as its own part. */
test("runs differing in exit, stream or file content are reported per part", () => {
  const files = new Map([["cuke-dedup.json", '{"findings":1,"analysisMs":3}']]);
  const same = new Map([["cuke-dedup.json", '{"findings":1,"analysisMs":9}']]);
  assert.deepEqual(compareRuns(run({ files }), run({ files: same })), []);
  const changed = new Map([["cuke-dedup.json", '{"findings":2,"analysisMs":3}']]);
  assert.deepEqual(compareRuns(run({ files }), run({ files: changed })).map((d) => d.part), ["file:cuke-dedup.json"]);
  assert.deepEqual(compareRuns(run(), run({ status: 1 })), [{ part: "exit", baseline: 0, candidate: 1 }]);
  assert.deepEqual(compareRuns(run(), run({ signal: "SIGKILL", status: null })), [{ part: "exit", baseline: 0, candidate: "SIGKILL" }]);
  assert.deepEqual(compareRuns(run({ stderr: "warn" }), run()).map((d) => d.part), ["stderr"]);
});

/** A report written by only one side is a difference, not a silent skip. */
test("a file present on one side only is a difference", () => {
  const files = new Map([["cuke-dedup.json", "{}"]]);
  assert.deepEqual(compareRuns(run({ files }), run({ files: new Map() })), [
    { part: "file:cuke-dedup.json", baseline: "present", candidate: "missing" },
  ]);
});

/** Each side's own replacements map its output directory to one token, so differing paths agree. */
test("side-specific replacements reconcile output directories", () => {
  const baseline = run({ stdout: "wrote /tmp/scratch/cuke-dedup.json" });
  const candidate = run({ stdout: "wrote /work/report/cuke-dedup.json" });
  assert.equal(compareRuns(baseline, candidate).length, 1);
  assert.deepEqual(compareRuns(baseline, candidate, {
    baseline: [["/tmp/scratch", "<output>"]],
    candidate: [["/work/report", "<output>"]],
  }), []);
});

/** Only files created or rewritten after the snapshot belong to the run; stale ones are ignored. */
test("readFiles keeps only files written since the snapshot", () => {
  const root = mkdtempSync(join(tmpdir(), "equivalence-test-"));
  try {
    mkdirSync(join(root, "nested"));
    writeFileSync(join(root, "stale.json"), "old");
    writeFileSync(join(root, "nested", "rewritten.json"), "old");
    utimesSync(join(root, "stale.json"), 1, 1);
    utimesSync(join(root, "nested", "rewritten.json"), 1, 1);
    const before = fileTimes(root);
    writeFileSync(join(root, "nested", "rewritten.json"), "new");
    writeFileSync(join(root, "created.json"), "new");
    assert.deepEqual(listFiles(root), ["created.json", "nested/rewritten.json", "stale.json"]);
    assert.deepEqual([...readFiles(root, before)].map(([name, bytes]) => [name, bytes.toString()]).sort(), [["created.json", "new"], ["nested/rewritten.json", "new"]]);
    assert.deepEqual(listFiles(join(root, "absent")), []);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

/** A suite with no invocation fails the verdict even when nothing mismatched. */
test("summary fails on mismatches and on suites that made no invocation", () => {
  const clean = { suite: "corpus", differences: [] };
  const dirty = { suite: "ruby", differences: [{ part: "exit" }] };
  assert.equal(summarize([clean], ["corpus"]).ok, true);
  const empty = summarize([clean], ["corpus", "ruby"]);
  assert.deepEqual([empty.ok, empty.empty], [false, ["ruby"]]);
  const mismatch = summarize([clean, dirty], ["corpus", "ruby"]);
  assert.deepEqual([mismatch.ok, mismatch.mismatches.length, mismatch.invocations], [false, 1, { corpus: 1, ruby: 1 }]);
});

/** Output that is not valid UTF-8 compares by its exact bytes; valid text is still normalized. */
test("binary output compares by bytes", () => {
  /** A run whose stdout and report are `bytes`. */
  const out = (bytes) => run({ stdout: Buffer.from(bytes), files: new Map([["report.bin", Buffer.from(bytes)]]) });
  assert.deepEqual(compareRuns(out([0xff]), out([0xfe])).map(({ part }) => part), ["stdout", "file:report.bin"]);
  assert.deepEqual(compareRuns(out([0xff]), out([0xff])), []);
  assert.deepEqual(compareRuns(out([0xff]), out([0x78])).map(({ part }) => part), ["stdout", "file:report.bin"]);
  assert.deepEqual(compareRuns(run({ stdout: Buffer.from("Detection time: 3 ms") }), run({ stdout: Buffer.from("Detection time: 9 ms") })), []);
});

/** A byte-order mark is content: output that gains or loses one differs. */
test("a byte-order mark is not dropped", () => {
  /** A run whose report is `bytes`. */
  const report = (bytes) => run({ files: new Map([["cuke-dedup.json", Buffer.from(bytes)]]) });
  const bom = [0xef, 0xbb, 0xbf, 0x7b, 0x7d];
  assert.deepEqual(compareRuns(report(bom), report([0x7b, 0x7d])).map(({ part }) => part), ["file:cuke-dedup.json"]);
  assert.deepEqual(compareRuns(report(bom), report(bom)), []);
});

/** Parallel shims each write their own record file; every record is read back whole, however large. */
test("shim records are separate files read back whole", async () => {
  const root = mkdtempSync(join(tmpdir(), "equivalence-test-"));
  try {
    const large = "x".repeat(256 * 1024);
    const script = join(root, "writer.mjs");
    writeFileSync(script, `import { writeRecord } from ${JSON.stringify(new URL("./output-equivalence.mjs", import.meta.url).href)};
for (let index = 0; index < 5; index += 1) writeRecord(process.argv[2], { suite: "cli", index, payload: "x".repeat(${large.length}) });`);
    const records = join(root, "records");
    mkdirSync(records);
    /** Runs one writer process to completion and resolves with its exit code. */
    const writer = () => new Promise((done) => spawn(process.execPath, [script, records], { stdio: "ignore" }).on("exit", done));
    assert.deepEqual(await Promise.all(Array.from({ length: 6 }, writer)), Array(6).fill(0));
    const read = readRecords(records);
    assert.equal(readdirSync(records).length, 30);
    assert.equal(read.length, 30);
    assert.ok(read.every((record) => record.payload === large));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
