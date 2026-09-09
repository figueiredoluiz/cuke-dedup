import { chmod, copyFile, mkdir, readFile, writeFile } from "node:fs/promises";
import assert from "node:assert/strict";
import { join } from "node:path";

import { TARGETS } from "../../npm/lib/targets.mjs";
import { renderPlatformReadme } from "./npm-platform-readme.mjs";

const artifacts = process.argv[2];
if (!artifacts) {
  throw new Error("usage: node scripts/release/prepare-npm-packages.mjs ARTIFACTS_DIR|--check");
}

for (const target of Object.values(TARGETS)) {
  const { rustTarget, packageDirectory, binaryName } = target;
  const packageRoot = join("npm", "platforms", packageDirectory);
  const packageManifest = JSON.parse(await readFile(join(packageRoot, "package.json"), "utf8"));
  assert.deepEqual(
    packageManifest.files,
    [`bin/${binaryName}`, "README.md", "LICENSE", "THIRD-PARTY-LICENSES.md"],
    `${packageManifest.name} must publish its binary, README, and legal notices`,
  );
  if (artifacts === "--check") {
    continue;
  }
  const destinationDirectory = join(
    packageRoot,
    "bin",
  );
  const destination = join(destinationDirectory, binaryName);
  await mkdir(destinationDirectory, { recursive: true });
  await copyFile(join(artifacts, `binary-${rustTarget}`, binaryName), destination);
  if (binaryName !== "cuke-dedup.exe") {
    await chmod(destination, 0o755);
  }
  await copyFile("LICENSE", join(packageRoot, "LICENSE"));
  await copyFile("THIRD-PARTY-LICENSES.md", join(packageRoot, "THIRD-PARTY-LICENSES.md"));
  await writeFile(join(packageRoot, "README.md"), renderPlatformReadme(packageManifest));
}
