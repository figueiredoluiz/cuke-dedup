import assert from "node:assert/strict";
import test from "node:test";

import {
  npmPublishArguments,
  registryVersionState,
} from "../../scripts/release/publish-npm-packages.mjs";

test("release publishing skips an exact version already in the registry", () => {
  assert.equal(
    registryVersionState({ status: 0, stdout: '"0.1.0"', stderr: "" }, "0.1.0"),
    "published",
  );
});

test("release publishing treats only an npm E404 as a missing version", () => {
  assert.equal(
    registryVersionState({ status: 1, stdout: "", stderr: "npm error code E404" }, "0.1.0"),
    "missing",
  );
  assert.throws(
    () => registryVersionState(
      { status: 1, stdout: "", stderr: "npm error code E401" },
      "0.1.0",
    ),
    /refusing to publish without a reliable registry check/,
  );
});

test("release publishing preserves launcher and platform arguments", () => {
  assert.deepEqual(
    npmPublishArguments("npm/platforms/darwin-arm64"),
    [
      "publish",
      "npm/platforms/darwin-arm64",
      "--access",
      "public",
      "--provenance",
    ],
  );
  assert.deepEqual(
    npmPublishArguments("."),
    ["publish", "--workspaces=false", "--access", "public", "--provenance"],
  );
});
