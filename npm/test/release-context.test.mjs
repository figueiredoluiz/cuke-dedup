import assert from "node:assert/strict";
import test from "node:test";

import { validateReleaseContext } from "../../scripts/release/validate-release-context.mjs";

const tagSha = "a".repeat(40);
const valid = {
  publishRelease: true,
  releaseTag: "v1.2.3",
  githubRef: "refs/tags/v1.2.3",
  githubSha: tagSha,
  releaseSha: tagSha,
};

/** Verify publishing accepts matching dispatched tags and immutable checkout commits. */
test("release context accepts an immutable checkout of the dispatched tag", () => {
  assert.doesNotThrow(() => validateReleaseContext(valid));
});

/** Verify missing tags, branch dispatches, mismatched commits, and malformed commit IDs reject publication. */
test("release context rejects every mismatch that can split provenance", () => {
  for (const [override, message] of [
    [{ releaseTag: "" }, /release_tag is required/],
    [{ githubRef: "refs/heads/main" }, /must be dispatched from the release tag/],
    [{ releaseSha: "b".repeat(40) }, /resolved commit does not match/],
    [{ releaseSha: "not-a-sha" }, /failed to resolve an immutable release commit/],
  ]) {
    assert.throws(() => validateReleaseContext({ ...valid, ...override }), message);
  }
});

/** Verify non-publishing builds accept an immutable commit without a release tag or tag ref. */
test("non-publishing builds require only an immutable resolved commit", () => {
  assert.doesNotThrow(() => validateReleaseContext({
    ...valid,
    publishRelease: false,
    releaseTag: "",
    githubRef: "refs/heads/main",
  }));
});
