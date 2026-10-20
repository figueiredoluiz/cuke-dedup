import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

// A Windows checkout may convert the workflow to CRLF; the patterns below match LF.
const workflow = (
  await readFile(new URL("../../.github/workflows/release.yml", import.meta.url), "utf8")
).replaceAll("\r\n", "\n");

/** Verify release validation and all five checkouts bind to the dispatched immutable commit. */
test("release publishing stays bound to one immutable tagged commit", () => {
  assert.doesNotMatch(workflow, /inputs\.ref|BUILD_REF|outputs\.release_sha/);
  assert.match(
    workflow,
    /RELEASE_SHA="\$release_sha" node scripts\/release\/validate-release-context\.mjs/,
  );

  const immutableCheckouts = workflow.match(
    /ref: \$\{\{ github\.sha \}\}/g,
  );
  assert.equal(immutableCheckouts?.length, 5);
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

/** Verify the release comparison is read-only, non-blocking and kept out of the release assets. */
test("release comparison cannot block or widen the release", () => {
  // Controls: an empty slice would otherwise fail or pass the assertions below for the wrong reason.
  const job = workflow.match(/\n  release-comparison:\n([\s\S]*?)\n  [a-z][a-z-]*:\n/)?.[1];
  assert.ok(job?.includes("steps:"), "release-comparison job slice must be found");
  assert.match(job, /continue-on-error: true/);
  const permissions = job.match(/\n    permissions:\n((?: {6}\S.*\n)+)/)?.[1];
  assert.equal(
    permissions?.replace(/ #.*$/gm, ""),
    "      contents: read\n      attestations: read\n",
  );
  assert.doesNotMatch(job, /contents: write|id-token|attestations: write/);
  assert.match(job, /name: comparison-table/);
  assert.match(job, /binary-\$\{\{ env\.COMPARISON_TARGET \}\}/);
  assert.match(workflow, /needs: \[build-native, release-comparison\]/);
  // The previous binary is executed, so its provenance is checked first and pinned to this workflow.
  assert.match(job, /gh attestation verify[^\n]*\\\n\s*--signer-workflow "\$\{GH_REPO\}\/\.github\/workflows\/release\.yml"\n\s*tar -xzf/);
  assert.match(workflow, /if: steps\.comparison\.outcome == 'success'/);
  assert.match(workflow, /if: steps\.comparison\.outcome == 'failure' \|\| steps\.append\.outcome == 'failure'/);
  // `release-*` is downloaded into the release assets and the npm archive check.
  assert.doesNotMatch(job, /name: release-/);
  const append = workflow.match(/name: Append the comparison[\s\S]*?gh release edit/)?.[0];
  assert.ok(append, "append step slice must be found");
  assert.match(append, /continue-on-error: true/);
});
