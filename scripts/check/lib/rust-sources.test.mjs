import { test } from "node:test";
import assert from "node:assert/strict";
import { filterExisting, parseGitSources } from "./rust-sources.mjs";

/** Checks repeated merge-stage paths collapse to one source inventory entry. */
test("PR #43: a conflicted path listed once per merge stage collapses to one entry", () => {
  // `git ls-files --cached` prints a conflicted path once per stage — three times — which without
  // de-duplication trips the scope-overlap assertion on an overlap that does not exist.
  const stdout = "src/a.rs\nsrc/conflict.rs\nsrc/conflict.rs\nsrc/conflict.rs\nsrc/b.rs\n";
  assert.deepEqual(parseGitSources(stdout), ["src/a.rs", "src/conflict.rs", "src/b.rs"]);
});

/** Checks blank inventory lines disappear while first-seen path order is preserved. */
test("blank lines are dropped and first-seen order is preserved", () => {
  assert.deepEqual(parseGitSources("\nb.rs\n\na.rs\nb.rs\n"), ["b.rs", "a.rs"]);
  assert.deepEqual(parseGitSources(""), []);
});

/** Checks indexed paths missing from disk are excluded from the source inventory. */
test("a path deleted-but-still-in-index is filtered by what is on disk", () => {
  const paths = ["kept.rs", "deleted.rs"];
  const onDisk = new Set(["kept.rs"]);
  assert.deepEqual(filterExisting(paths, (p) => onDisk.has(p)), ["kept.rs"]);
});
