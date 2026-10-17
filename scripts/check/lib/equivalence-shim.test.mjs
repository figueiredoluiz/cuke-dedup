import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { readRecords } from "./output-equivalence.mjs";

const shim = fileURLToPath(new URL("./equivalence-shim.mjs", import.meta.url));

/**
 * A fake analyzer: `--print-config` prints a configuration whose report directory is
 * `reports/cuke-dedup`; any other run writes `report` there. `probe` runs first on `--print-config`.
 */
function fakeAnalyzer(directory, name, report, probe = "") {
  const path = join(directory, name);
  writeFileSync(path, `#!/bin/sh
if [ "$1" = "--print-config" ] || [ "$2" = "--print-config" ]; then
  ${probe}
  printf '{"root":"%s","output":"reports/cuke-dedup"}' "$PWD"
  exit 0
fi
mkdir -p reports/cuke-dedup
printf '%s' '${report}' > reports/cuke-dedup/cuke-dedup.json
echo done
`);
  chmodSync(path, 0o755);
  return path;
}

/** Runs the shim in `work` with `baseline` and `candidate`, returning the process and the records it left. */
function runShim(work, baseline, candidate) {
  const log = join(work, "records");
  mkdirSync(log, { recursive: true });
  const result = spawnSync(process.execPath, [shim, "."], {
    cwd: join(work, "project"),
    encoding: "utf8",
    env: {
      ...process.env,
      CUKE_DEDUP_EQUIVALENCE_BASELINE: baseline,
      CUKE_DEDUP_EQUIVALENCE_CANDIDATE: candidate,
      CUKE_DEDUP_EQUIVALENCE_LOG: log,
      CUKE_DEDUP_EQUIVALENCE_SUITE: "test",
    },
  });
  return { result, records: readRecords(log) };
}

/** Runs `check` with a fresh work directory holding an empty `project` and a stale report. */
function withWork(check) {
  const work = mkdtempSync(join(tmpdir(), "equivalence-shim-test-"));
  try {
    mkdirSync(join(work, "project", "reports", "cuke-dedup"), { recursive: true });
    writeFileSync(join(work, "project", "reports", "cuke-dedup", "cuke-dedup.json"), "stale");
    check(work);
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

const posix = { skip: process.platform === "win32" };

/** Reports written through the configured directory are compared, and the candidate starts from the original state. */
test("the shim compares what each binary writes in its configured locations", posix, () => withWork((work) => {
  const same = runShim(work, fakeAnalyzer(work, "a", "{}"), fakeAnalyzer(work, "b", "{}"));
  assert.equal(same.result.status, 0, same.result.stderr);
  assert.equal(same.result.stdout, "done\n");
  assert.deepEqual(same.records.map(({ differences }) => differences), [[]]);
  rmSync(join(work, "records"), { recursive: true });
  writeFileSync(join(work, "project", "reports", "cuke-dedup", "cuke-dedup.json"), "stale");
  const differing = runShim(work, fakeAnalyzer(work, "c", '{"a":1}'), fakeAnalyzer(work, "d", '{"a":2}'));
  assert.deepEqual(differing.records.flatMap(({ differences }) => differences.map(({ part }) => part)),
    // Keys come from the configuration the binary prints, which resolves `$PWD` (macOS `/private/var`).
    [`file:${join(realpathSync(work), "project", "reports", "cuke-dedup", "cuke-dedup.json")}`]);
  // The checker sees the candidate's output.
  assert.equal(readFileSync(join(work, "project", "reports", "cuke-dedup", "cuke-dedup.json"), "utf8"), '{"a":2}');
}));

/** A binary that cannot start, or a configuration probe killed by a signal, fails the shim and records nothing. */
test("the shim fails closed when a binary or its configuration probe fails", posix, () => withWork((work) => {
  const candidate = fakeAnalyzer(work, "ok", "{}");
  const missing = runShim(work, join(work, "absent"), candidate);
  assert.notEqual(missing.result.status, 0);
  assert.match(missing.result.stderr, /ENOENT/);
  assert.deepEqual(missing.records, []);
  rmSync(join(work, "records"), { recursive: true });
  const killed = runShim(work, fakeAnalyzer(work, "killed", "{}", "kill -9 $$"), candidate);
  assert.notEqual(killed.result.status, 0);
  assert.match(killed.result.stderr, /--print-config terminated by SIGKILL/);
  assert.deepEqual(killed.records, []);
  assert.deepEqual(readdirSync(join(work, "project", "reports", "cuke-dedup")), ["cuke-dedup.json"]);
}));
