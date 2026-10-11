import assert from "node:assert/strict";
import { chmod, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { fileURLToPath } from "node:url";

const projectRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const preflight = path.join(projectRoot, "scripts/check/preflight.sh");
const steps = [
  "diff-worktree",
  "diff-index",
  "fmt-main",
  "fmt-fuzz",
  "ruby-keywords",
  "checker-tests",
  "language-boundary",
  "docs-parity",
];

const wrapper = [
  "#!/usr/bin/env bash",
  "set -euo pipefail",
  'command_name="${0##*/}"',
  'if [[ "$command_name" == "git" && "${1:-}" == "rev-parse" ]]; then',
  "  printf '%s\\n' \"$PREFLIGHT_PROJECT_ROOT\"",
  "  exit 0",
  "fi",
  'case "$command_name:$*" in',
  '  "git:diff --check") step="diff-worktree" ;;',
  '  "git:diff --cached --check") step="diff-index" ;;',
  '  "cargo:+stable fmt --all -- --check") step="fmt-main" ;;',
  '  "cargo:+stable fmt --manifest-path fuzz/Cargo.toml -- --check") step="fmt-fuzz" ;;',
  '  "node:scripts/check/check-ruby-step-keywords.mjs") step="ruby-keywords" ;;',
  '  "node:--test scripts/check/lib/"*) step="checker-tests" ;;',
  '  "node:scripts/check/check-language-boundary.mjs") step="language-boundary" ;;',
  '  "node:scripts/check/check-docs-parity.mjs") step="docs-parity" ;;',
  "  *) printf 'unexpected command: %s\\n' \"$command_name:$*\" >&2; exit 97 ;;",
  "esac",
  "printf '%s\\n' \"$step\" >> \"$PREFLIGHT_TRACE\"",
  'if [[ "$step" == "${PREFLIGHT_FAIL_STEP:-}" ]]; then exit 23; fi',
  "",
].join("\n");

async function fixture() {
  const directory = await mkdtemp(path.join(tmpdir(), "cuke-dedup-preflight-"));
  const bin = path.join(directory, "bin");
  const trace = path.join(directory, "trace");
  await mkdir(bin);
  for (const command of ["git", "cargo", "node"]) {
    const target = path.join(bin, command);
    await writeFile(target, wrapper);
    await chmod(target, 0o755);
  }
  return { bin, directory, trace };
}

async function readTrace(trace) {
  try {
    return (await readFile(trace, "utf8")).trim().split("\n").filter(Boolean);
  } catch (error) {
    if (error.code === "ENOENT") return [];
    throw error;
  }
}

async function run(failStep = "") {
  const { bin, directory, trace } = await fixture();
  try {
    const result = spawnSync("bash", [preflight], {
      cwd: path.join(projectRoot, "scripts"),
      encoding: "utf8",
      env: {
        ...process.env,
        PATH: `${bin}:/usr/bin:/bin`,
        PREFLIGHT_FAIL_STEP: failStep,
        PREFLIGHT_PROJECT_ROOT: projectRoot,
        PREFLIGHT_TRACE: trace,
      },
    });
    const observed = await readTrace(trace);
    return { observed, result };
  } finally {
    await rm(directory, { recursive: true });
  }
}

test("a failure before the first command produces an empty trace", async () => {
  const directory = await mkdtemp(path.join(tmpdir(), "cuke-dedup-missing-trace-"));
  try {
    assert.deepEqual(await readTrace(path.join(directory, "missing")), []);
  } finally {
    await rm(directory, { recursive: true });
  }
});

test("preflight executes every inexpensive check in order", async () => {
  const { observed, result } = await run();
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(observed, steps);
});

for (const [index, step] of steps.entries()) {
  test(`preflight stops after ${step} fails`, async () => {
    const { observed, result } = await run(step);
    assert.equal(result.status, 23, result.stderr);
    assert.deepEqual(observed, steps.slice(0, index + 1));
  });
}
