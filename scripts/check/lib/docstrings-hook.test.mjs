import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, copyFileSync, writeFileSync, rmSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname, resolve } from "node:path";
import { test } from "node:test";

/** Locate Git Bash when its executable is absent from the native Windows PATH. */
function bashExecutable() {
  if (process.platform !== "win32") return "bash";
  const lookup = spawnSync("where.exe", ["git"], { encoding: "utf8", timeout: 10_000 });
  for (const git of lookup.stdout?.split(/\r?\n/).filter(Boolean) ?? []) {
    const candidate = resolve(dirname(git), "..", "bin", "bash.exe");
    if (existsSync(candidate)) return candidate;
  }
  throw new Error("Git Bash was not found beside the installed Git executable");
}

/** Exercise the real hook and checker with immutable commits and a cheap full-gate stand-in. */
test("pre-push documentation contract", () => {
  const root = mkdtempSync(join(tmpdir(), "cuke-docstring-hook-"));
  /** Run bounded Git commands in the disposable repository. */
  const git = (...args) => execFileSync("git", args, { cwd: root, encoding: "utf8", timeout: 10_000 });
  /** Create synthetic fixture commits without invoking developer hooks. */
  function snapshot() {
    git("add", ".");
    const tree = git("write-tree").trim();
    const parent = spawnSync("git", ["rev-parse", "--verify", "HEAD"], { cwd: root, encoding: "utf8" });
    const commit = git("-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit-tree", tree,
      ...(parent.status === 0 ? ["-p", parent.stdout.trim()] : []), "-m", "Fixture").trim();
    git("update-ref", "HEAD", commit);
    return commit;
  }
  /** Invoke the hook with Git's ref-update protocol. */
  function push(head, remote = "0".repeat(40)) {
    return spawnSync(bashExecutable(), [".githooks/pre-push", "origin"], {
      cwd: root, encoding: "utf8", timeout: 10_000,
      input: `refs/heads/topic ${head} refs/heads/topic ${remote}\n`,
    });
  }
  try {
    git("init", "--quiet");
    for (const path of [".githooks/pre-push", "scripts/check/check-docstrings.mjs", "scripts/check/lib/docstrings.mjs", "scripts/check/lib/rust-docstring-code.mjs",
      "scripts/check/tools/node_modules/typescript/package.json", "scripts/check/tools/node_modules/typescript/lib/typescript.js"]) {
      mkdirSync(join(root, path, ".."), { recursive: true });
      copyFileSync(new URL(`../../../${path}`, import.meta.url), join(root, path));
    }
    writeFileSync(join(root, "package.json"), '{"type":"module"}');
    writeFileSync(join(root, ".gitignore"), "full-gate-ran\n");
    writeFileSync(join(root, "scripts/check/check.sh"), "#!/bin/sh\ntouch full-gate-ran\n", { mode: 0o755 });
    const base = snapshot();
    git("update-ref", "refs/remotes/origin/main", base);
    writeFileSync(join(root, "sample.rs"), "fn missing() {}\n");
    const undocumented = snapshot();
    let result = push(undocumented);
    assert.equal(result.status, 1, result.stderr);
    assert.match(result.stderr, /missing has no doc comment/);
    assert.equal(existsSync(join(root, "full-gate-ran")), false);

    // A later push cannot hide an undocumented function in an already-pushed commit.
    writeFileSync(join(root, "note.txt"), "Unrelated change.\n");
    const later = snapshot();
    result = push(later, undocumented);
    assert.equal(result.status, 1, result.stderr);
    assert.match(result.stderr, /missing has no doc comment/);

    writeFileSync(join(root, "sample.rs"), "/// Run the sample.\nfn missing() {}\n");
    const documented = snapshot();
    result = push(documented, later);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(existsSync(join(root, "full-gate-ran")), true);
    rmSync(join(root, "full-gate-ran"));

    git("update-ref", "-d", "refs/remotes/origin/main");
    assert.equal(push(documented).status, 1);
    assert.equal(existsSync(join(root, "full-gate-ran")), false);
    git("update-ref", "refs/remotes/origin/main", base);
    assert.match(push(base).stderr, /checked-out HEAD/);
    writeFileSync(join(root, "note.txt"), "Dirty.\n");
    assert.match(push(documented).stderr, /worktree must be clean/);
    // Ref deletion remains allowed without checking documentation or requiring a clean tree.
    assert.equal(push("0".repeat(40), documented).status, 0);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
