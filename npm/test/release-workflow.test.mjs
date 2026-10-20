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

/** Slice one top-level job out of a workflow text, up to the next job or the end of the file. */
function jobSlice(text, name) {
  return text.match(new RegExp(`\\n  ${name}:\\n([\\s\\S]*?)(?:\\n  [a-z][a-z-]*:\\n|$)`))?.[1];
}

/** Assert every contract property of the notify-site job; throws on the first violation. */
function assertSiteJobContract(text) {
  const job = jobSlice(text, "notify-site");
  assert.ok(job?.includes("steps:"), "notify-site job slice must be found");

  const needs = job.match(/\n    needs: \[([^\]]*)\]/)?.[1].split(",").map((n) => n.trim());
  assert.deepEqual(needs, ["github-release", "publish-crate", "publish-npm"]);
  assert.doesNotMatch(job, /release-comparison/);

  // A real publish, a successful release or npm-only recovery, and no failed registry job.
  const condition = job.match(/\n    if: >-\n((?: {6}.*\n)+)/)?.[1].replace(/\s+/g, " ");
  // The whole expression is compared so regrouping `&&`/`||` or flipping an operator fails too.
  assert.equal(
    condition?.trim(),
    "${{ !cancelled() && inputs.publish_release && " +
      "(needs.github-release.result == 'success' || " +
      "(inputs.publish_npm_only && needs.publish-npm.result == 'success')) && " +
      "(needs.publish-crate.result == 'success' || needs.publish-crate.result == 'skipped') && " +
      "(needs.publish-npm.result == 'success' || needs.publish-npm.result == 'skipped') }}",
  );
  assert.doesNotMatch(job, /continue-on-error/);
  assert.match(job, /\n    timeout-minutes: 5\n/);

  // No repository token; the dispatch token and the tag come through env, never into the script.
  assert.match(job, /\n    permissions: \{\}\n/);
  assert.match(job, /\n {10}SITE_DISPATCH_TOKEN: \$\{\{ secrets\.SITE_DISPATCH_TOKEN \}\}\n/);
  assert.match(text, /\nenv:\n(?: {2}.*\n)*? {2}RELEASE_TAG: \$\{\{ inputs\.release_tag \}\}\n/);
  const script = job.match(/\n {8}run: \|\n([\s\S]*)$/)?.[1];
  assert.ok(script?.includes("curl"), "run script slice must be found");
  assert.doesNotMatch(script, /\$\{\{/);
  assert.match(script, /Authorization: Bearer \$SITE_DISPATCH_TOKEN/);

  // Tag validated before the JSON is built; JSON built by jq from that variable.
  const check = script.indexOf('[[ "$RELEASE_TAG" =~ ^v[0-9]+\\.[0-9]+\\.[0-9]+$ ]]');
  const build = script.indexOf('jq -nc --arg tag "$RELEASE_TAG"');
  assert.ok(check >= 0 && build > check, "tag validation must precede payload construction");
  assert.ok(script.includes('{event_type: "release-published", client_payload: {tag: $tag}}'));
  // The endpoint is matched as a whole line, so no other host can precede or follow it.
  assert.match(
    script,
    /^ +https:\/\/api\.github\.com\/repos\/figueiredoluiz\/cuke-dedup-site\/dispatches \\$/m,
  );
  assert.ok(script.includes('-d "$payload"'));
  // The POST is not idempotent (no --retry) and anything but 204 fails the job with the body shown.
  assert.doesNotMatch(script, /--retry/);
  assert.ok(script.includes('if [ "$status" != "204" ]; then'));
}

/** Verify the website trigger is gated on a real publish, isolated from the release, and carries only the tag. */
test("website rebuild trigger follows the registries and cannot block the release", () => {
  assertSiteJobContract(workflow);
  // Controls: the slicer finds other jobs with their own content and does not leak across jobs.
  assert.ok(jobSlice(workflow, "publish-npm")?.includes("id-token: write"));
  assert.ok(!jobSlice(workflow, "notify-site").includes("id-token"));
});

/** Verify each contract assertion fails when its behavior is broken (opposite-answer controls). */
test("website rebuild contract rejects each broken variant", () => {
  const variants = [
    ["needs the comparison", "needs: [github-release, publish-crate, publish-npm]", "needs: [github-release, publish-crate, publish-npm, release-comparison]"],
    ["drops the npm guard", "(needs.publish-npm.result == 'success' || needs.publish-npm.result == 'skipped')", "true"],
    ["drops the crate guard", "(needs.publish-crate.result == 'success' || needs.publish-crate.result == 'skipped')", "true"],
    ["regroups the guards", "needs.publish-crate.result == 'skipped') &&", "needs.publish-crate.result == 'skipped') ||"],
    ["treats cancelled npm as ok", "needs.publish-npm.result == 'skipped')", "needs.publish-npm.result != 'failure')"],
    ["drops the npm-only path", "inputs.publish_npm_only && needs.publish-npm.result == 'success'", "false"],
    ["runs on rehearsals", "inputs.publish_release &&\n        (needs.github-release", "(needs.github-release"],
    ["gains permissions", "permissions: {}", "permissions:\n      contents: write"],
    ["interpolates the token", 'Bearer $SITE_DISPATCH_TOKEN', "Bearer ${{ secrets.SITE_DISPATCH_TOKEN }}"],
    ["interpolates the tag", 'jq -nc --arg tag "$RELEASE_TAG"', 'jq -nc --arg tag "${{ inputs.release_tag }}"'],
    ["wrong event type", '"release-published"', '"other"'],
    ["wrong repository", "cuke-dedup-site/dispatches", "other-site/dispatches"],
    ["another host first", "https://api.github.com/repos", "https://example.invalid/?u=https://api.github.com/repos"],
    ["tag not in payload", "client_payload: {tag: $tag}", "client_payload: {}"],
    ["tag unvalidated", "=~ ^v[0-9]+", "=~ ^[0-9v]+"],
    ["retries the POST", "curl -sS --max-time 30", "curl -sS --max-time 30 --retry 3"],
    ["accepts any status", 'if [ "$status" != "204" ]; then', 'if [ "$status" = "000" ]; then'],
    ["swallows failure", "    timeout-minutes: 5\n    permissions: {}", "    timeout-minutes: 5\n    continue-on-error: true\n    permissions: {}"],
  ];
  for (const [label, from, to] of variants) {
    const start = workflow.indexOf("\n  notify-site:\n");
    assert.ok(start >= 0 && workflow.indexOf(from, start) >= 0, `${label}: pattern present`);
    const broken = workflow.slice(0, start) + workflow.slice(start).replace(from, to);
    assert.throws(() => assertSiteJobContract(broken), `${label}: contract must fail`);
  }
});
