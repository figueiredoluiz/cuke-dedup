import assert from "node:assert/strict";
import test from "node:test";

import { releaseMatrix } from "../../scripts/release/release-matrix.mjs";

test("release matrix contains only the eight validated GitHub-hosted targets", () => {
  const matrix = releaseMatrix();
  assert.equal(matrix.include.length, 8);
  assert.deepEqual(
    new Set(matrix.include.map((target) => target.runner)),
    new Set([
      "macos-15",
      "macos-15-intel",
      "ubuntu-22.04-arm",
      "ubuntu-22.04",
      "windows-11-arm",
      "windows-latest",
    ]),
  );
  assert.ok(matrix.include.every((target) => target.target && target.binary));
});
