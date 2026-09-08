import assert from "node:assert/strict";
import test from "node:test";

import {
  npmPublishArguments,
  npmInvocation,
  registryVersionState,
  runNpmCommand,
} from "../../scripts/release/publish-npm-packages.mjs";

test("release publishing has a working default npm runner", () => {
  const result = runNpmCommand(["--version"], {
    capture: true,
    root: process.cwd(),
  });

  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /^\d+\.\d+\.\d+/);
});

test("release publishing invokes npm through Node on Windows", () => {
  assert.deepEqual(
    npmInvocation({
      platform: "win32",
      nodeExecutable: "C:\\node\\node.exe",
      npmExecutablePath: "C:\\node\\node_modules\\npm\\bin\\npm-cli.js",
    }),
    {
      executable: "C:\\node\\node.exe",
      prefixArguments: ["C:\\node\\node_modules\\npm\\bin\\npm-cli.js"],
    },
  );
});

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
