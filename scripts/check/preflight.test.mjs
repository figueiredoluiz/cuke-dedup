import assert from "node:assert/strict";
import { existsSync } from "node:fs";
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

/** Locate Bash directly on Windows, where Git Bash is installed but is not necessarily on PATH. */
function bashExecutable() {
  if (process.platform !== "win32") return "bash";
  const lookup = spawnSync("where.exe", ["git"], { encoding: "utf8" });
  for (const git of lookup.stdout?.split(/\r?\n/).filter(Boolean) ?? []) {
    const candidate = path.resolve(path.dirname(git), "..", "bin", "bash.exe");
    if (existsSync(candidate)) return candidate;
  }
  throw new Error(`Git Bash was not found from the installed Git paths: ${lookup.stderr}`);
}

/** Convert a native path to the drive-prefixed form understood inside Git Bash. */
function bashPath(value, platform = process.platform) {
  if (platform !== "win32") return value;
  const normalized = value.replaceAll("\\", "/");
  if (normalized.length < 2 || normalized[1] !== ":") return normalized;
  return `/${normalized[0].toLowerCase()}${normalized.slice(2)}`;
}

/** Create isolated command stubs and a trace location for one preflight execution. */
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

/** Read the executed steps, treating an absent trace as no executed command. */
async function readTrace(trace) {
  try {
    return (await readFile(trace, "utf8")).trim().split("\n").filter(Boolean);
  } catch (error) {
    if (error.code === "ENOENT") return [];
    throw error;
  }
}

/** Execute preflight with command stubs, optionally failing one named step. */
async function run(failStep = "") {
  const { bin, directory, trace } = await fixture();
  try {
    const result = spawnSync(bashExecutable(), ["-c", 'export PATH="$PREFLIGHT_TEST_BIN:/usr/bin:/bin"; exec bash "$PREFLIGHT_SCRIPT"'], {
      cwd: path.join(projectRoot, "scripts"),
      encoding: "utf8",
      env: {
        ...process.env,
        PREFLIGHT_FAIL_STEP: failStep,
        PREFLIGHT_PROJECT_ROOT: bashPath(projectRoot),
        PREFLIGHT_SCRIPT: bashPath(preflight),
        PREFLIGHT_TEST_BIN: bashPath(bin),
        PREFLIGHT_TRACE: bashPath(trace),
      },
    });
    const observed = await readTrace(trace);
    return { observed, result };
  } finally {
    await rm(directory, { recursive: true });
  }
}

/** Checks native Windows paths convert to the drive-prefixed path format expected by Git Bash. */
test("native Windows paths are converted for Git Bash", () => {
  assert.equal(bashPath("D:\\a\\cuke-dedup\\scripts", "win32"), "/d/a/cuke-dedup/scripts");
});

/** Checks an absent preflight trace returns an empty sequence of executed commands. */
test("a failure before the first command produces an empty trace", async () => {
  const directory = await mkdtemp(path.join(tmpdir(), "cuke-dedup-missing-trace-"));
  try {
    assert.deepEqual(await readTrace(path.join(directory, "missing")), []);
  } finally {
    await rm(directory, { recursive: true });
  }
});

/** Checks a successful preflight executes every configured inexpensive check in order. */
test("preflight executes every inexpensive check in order", async () => {
  const { observed, result } = await run();
  assert.equal(result.status, 0, result.error?.stack ?? result.stderr);
  assert.deepEqual(observed, steps);
});

for (const [index, step] of steps.entries()) {
  /** Checks failure at each preflight step preserves its exit status and prevents later commands. */
  test(`preflight stops after ${step} fails`, async () => {
    const { observed, result } = await run(step);
    assert.equal(result.status, 23, result.error?.stack ?? result.stderr);
    assert.deepEqual(observed, steps.slice(0, index + 1));
  });
}
