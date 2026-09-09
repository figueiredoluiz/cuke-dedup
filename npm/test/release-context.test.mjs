import assert from "node:assert/strict";
import test from "node:test";

import { validateReleaseContext } from "../../scripts/release/validate-release-context.mjs";

const tagSha = "a".repeat(40);
const valid = {
  publishRelease: true,
  buildRef: "v1.2.3",
  releaseTag: "v1.2.3",
  githubRef: "refs/tags/v1.2.3",
  githubSha: tagSha,
  releaseSha: tagSha,
};

test("release context accepts an immutable checkout of the dispatched tag", () => {
  assert.doesNotThrow(() => validateReleaseContext(valid));
});

test("release context rejects every mismatch that can split provenance", () => {
  for (const [override, message] of [
    [{ releaseTag: "" }, /release_tag is required/],
    [{ buildRef: "main" }, /ref must equal release_tag/],
    [{ githubRef: "refs/heads/main" }, /must be dispatched from the release tag/],
    [{ releaseSha: "b".repeat(40) }, /resolved commit does not match/],
    [{ releaseSha: "not-a-sha" }, /failed to resolve an immutable release commit/],
  ]) {
    assert.throws(() => validateReleaseContext({ ...valid, ...override }), message);
  }
});

test("non-publishing builds require only an immutable resolved commit", () => {
  assert.doesNotThrow(() => validateReleaseContext({
    ...valid,
    publishRelease: false,
    buildRef: "feature/test",
    releaseTag: "",
    githubRef: "refs/heads/main",
  }));
});
