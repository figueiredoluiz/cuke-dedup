import { pathToFileURL } from "node:url";

export function validateReleaseContext({
  publishRelease,
  buildRef,
  releaseTag,
  githubRef,
  githubSha,
  releaseSha,
}) {
  if (!/^[0-9a-f]{40}$/.test(releaseSha)) {
    throw new Error("failed to resolve an immutable release commit");
  }
  if (!publishRelease) {
    return;
  }
  if (!releaseTag) {
    throw new Error("release_tag is required when publish_release is true");
  }
  if (buildRef !== releaseTag) {
    throw new Error("ref must equal release_tag when publishing");
  }
  if (githubRef !== `refs/tags/${releaseTag}`) {
    throw new Error("publishing must be dispatched from the release tag");
  }
  if (releaseSha !== githubSha) {
    throw new Error("resolved commit does not match the dispatched release tag");
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    validateReleaseContext({
      publishRelease: process.env.PUBLISH_RELEASE === "true",
      buildRef: process.env.BUILD_REF,
      releaseTag: process.env.RELEASE_TAG,
      githubRef: process.env.GITHUB_REF,
      githubSha: process.env.GITHUB_SHA,
      releaseSha: process.env.RELEASE_SHA,
    });
  } catch (error) {
    console.error(error.message);
    process.exitCode = 2;
  }
}
