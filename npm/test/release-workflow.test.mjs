import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const workflow = await readFile(
  new URL("../../.github/workflows/release.yml", import.meta.url),
  "utf8",
);

test("release publishing stays bound to one immutable tagged commit", () => {
  assert.doesNotMatch(workflow, /inputs\.ref|BUILD_REF|outputs\.release_sha/);
  assert.match(
    workflow,
    /RELEASE_SHA="\$release_sha" node scripts\/release\/validate-release-context\.mjs/,
  );

  const immutableCheckouts = workflow.match(
    /ref: \$\{\{ github\.sha \}\}/g,
  );
  assert.equal(immutableCheckouts?.length, 4);
});

test("manually selected release refs cannot write reusable build caches", () => {
  assert.doesNotMatch(workflow, /(?:actions\/cache|Swatinem\/rust-cache)@/);
});

test("npm recovery does not continue after workflow cancellation", () => {
  assert.match(workflow, /!cancelled\(\)/);
  assert.doesNotMatch(workflow, /\balways\(\)/);
});

test("npm packages are checked against checksummed release archives", () => {
  assert.match(workflow, /pattern: release-\*/);
  assert.match(workflow, /verify_npm_binaries\.py --binaries native --archives release-assets/);
});
