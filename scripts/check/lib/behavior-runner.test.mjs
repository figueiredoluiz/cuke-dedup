import { test } from "node:test";
import assert from "node:assert/strict";
import { chmod, mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { observeBehaviorCase } from "./behavior-runner.mjs";

/** A behavior case expecting two definitions and a non-passing duplication summary. */
const testCase = {
  name: "case",
  path: "case",
  arguments: [],
  expected: { definitions: 2, featureSteps: 0, duplicatedDefinitions: 2, percentage: 100, threshold: 0, passed: false },
};

/** Checks a report that is not JSON names the case, the exit status and stderr; a missing report is null. */
test("observeBehaviorCase names the run when its report is not JSON", { skip: process.platform === "win32" }, async () => {
  const directory = await mkdtemp(join(tmpdir(), "behavior-runner-"));
  try {
    await mkdir(join(directory, "cases", "case"), { recursive: true });
    /** A fake binary that writes `body` as its report, then prints to stderr and exits 1. */
    const fake = async (name, body) => {
      const binary = join(directory, name);
      const script = body === null ? "" : `mkdir -p "$out"\nprintf '%s' '${body}' > "$out/cuke-dedup.json"\n`;
      await writeFile(binary, `#!/bin/sh\nwhile [ $# -gt 0 ]; do [ "$1" = --output ] && out=$2; shift; done\n${script}echo crashed >&2\nexit 1\n`);
      await chmod(binary, 0o755);
      return binary;
    };
    /** Observe options for a fake binary and a scratch directory name. */
    const options = (binary, scratch) => ({ binary, corpusRoot: join(directory, "cases"), scratch: join(directory, scratch), testCase });
    await assert.rejects(observeBehaviorCase(options(await fake("truncated", "{\"summary\":"), "a")), /case: report is not valid JSON.*exit 1: crashed/s);
    const good = await observeBehaviorCase(options(await fake("good", "{\"summary\":{}}"), "b"));
    assert.deepEqual([good.status, good.report, good.fingerprintsText], [1, { summary: {} }, null]);
    const none = await observeBehaviorCase(options(await fake("none", null), "c"));
    assert.deepEqual([none.status, none.report], [1, null]);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
