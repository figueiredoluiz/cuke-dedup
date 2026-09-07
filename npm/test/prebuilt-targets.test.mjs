import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const rootPackage = JSON.parse(
  await readFile(new URL("../../package.json", import.meta.url), "utf8"),
);
const prebuilt = JSON.parse(
  await readFile(new URL("../prebuilt-targets.json", import.meta.url), "utf8"),
);

test("prebuilt manifest, platform packages, and optional dependencies do not drift", async () => {
  assert.equal(prebuilt.schemaVersion, 1);
  const targets = Object.values(prebuilt.targets);
  assert.equal(targets.length, 8);
  assert.deepEqual(
    Object.keys(rootPackage.optionalDependencies).sort(),
    targets.map((target) => target.packageName).sort(),
  );

  for (const target of targets) {
    const platformPackage = JSON.parse(
      await readFile(
        new URL(`../platforms/${target.packageDirectory}/package.json`, import.meta.url),
        "utf8",
      ),
    );
    assert.equal(platformPackage.name, target.packageName);
    assert.deepEqual(platformPackage.os, [target.platform]);
    assert.deepEqual(platformPackage.cpu, [target.arch]);
    if (target.libc) {
      assert.deepEqual(platformPackage.libc, [target.libc === "gnu" ? "glibc" : "musl"]);
    } else {
      assert.equal(platformPackage.libc, undefined);
    }
    assert.deepEqual(
      platformPackage.files,
      [`bin/${target.binaryName}`, "LICENSE", "THIRD-PARTY-LICENSES.md"],
    );
  }
});
