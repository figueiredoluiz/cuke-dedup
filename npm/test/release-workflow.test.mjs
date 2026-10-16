import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const workflow = await readFile(
  new URL("../../.github/workflows/release.yml", import.meta.url),
  "utf8",
);

/** Verify release validation and all four checkouts bind to the dispatched immutable commit. */
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

/** Verify manually dispatched release builds do not configure reusable build caches. */
test("manually selected release refs cannot write reusable build caches", () => {
  assert.doesNotMatch(workflow, /(?:actions\/cache|Swatinem\/rust-cache)@/);
});

/** Verify recovery jobs respect cancellation instead of using unconditional execution. */
test("npm recovery does not continue after workflow cancellation", () => {
  assert.match(workflow, /!cancelled\(\)/);
  assert.doesNotMatch(workflow, /\balways\(\)/);
});

/** Verify npm binary validation consumes native binaries and the collected release archives. */
test("npm packages are checked against checksummed release archives", () => {
  assert.match(workflow, /pattern: release-\*/);
  assert.match(workflow, /verify_npm_binaries\.py --binaries native --archives release-assets/);
});

/** Verify npm publication requests OIDC permission, checks npm support, and avoids persistent token variables. */
test("npm publishing uses short-lived OIDC credentials only", () => {
  assert.match(
    workflow,
    /publish-npm:[\s\S]*?permissions:\s*\n\s*contents: read\s*\n\s*id-token: write/,
  );
  assert.match(workflow, /check-npm-trusted-publishing\.mjs/);
  assert.doesNotMatch(workflow, /NPM_TOKEN|NODE_AUTH_TOKEN/);
});
