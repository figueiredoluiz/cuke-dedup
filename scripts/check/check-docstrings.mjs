#!/usr/bin/env node
import { execFileSync } from "node:child_process";
import { coverage } from "./lib/docstrings.mjs";

try {
  if (process.argv.length !== 4 || process.argv[2] !== "--base") {
    throw new Error("usage: check-docstrings.mjs --base <ref>");
  }
  /** Run bounded Git reads against the immutable checked-out commit. */
  const git = (...args) => execFileSync("git", args, {
    encoding: "utf8", timeout: 30_000, maxBuffer: 32 * 1024 * 1024,
  });
  const base = git("rev-parse", "--verify", "--end-of-options", `${process.argv[3]}^{commit}`).trim();
  const head = git("rev-parse", "HEAD").trim();
  const mergeBase = git("merge-base", base, head).trim();
  const diff = git("-c", "core.quotePath=true", "diff", "--no-ext-diff", "--no-textconv", "--no-renames", "--no-color", "-U3", mergeBase, head, "--");
  const result = coverage(diff, path => git("show", `${head}:${path}`));
  for (const fn of result.functions.filter(fn => !fn.documented)) {
    console.error(`${fn.path}:${fn.line}: ${fn.name} has no doc comment`);
  }
  console.log(`Local docstrings: ${result.covered}/${result.functions.length} touched functions; required 100% (base ${mergeBase.slice(0, 12)})`);
  if (!result.passed) process.exitCode = 1;
} catch (error) {
  console.error(`Docstring validation failed: ${error.message}`);
  process.exitCode = 1;
}
