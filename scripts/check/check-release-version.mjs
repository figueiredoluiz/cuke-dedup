import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const prebuilt = JSON.parse(await readFile("npm/prebuilt-targets.json", "utf8"));
const manifests = [
  "package.json",
  ...Object.values(prebuilt.targets).map(
    (target) => `npm/platforms/${target.packageDirectory}/package.json`,
  ),
];
const packages = await Promise.all(
  manifests.map(async (path) => [path, JSON.parse(await readFile(path, "utf8"))]),
);
const cargo = await readFile("Cargo.toml", "utf8");
let inPackageSection = false;
let cargoVersion;
for (const line of cargo.split(/\r?\n/)) {
  if (line.trim() === "[package]") {
    inPackageSection = true;
    continue;
  }
  if (line.trim().startsWith("[")) {
    inPackageSection = false;
  }
  if (inPackageSection) {
    cargoVersion = line.match(/^version\s*=\s*"([^"]+)"\s*$/)?.[1] ?? cargoVersion;
  }
}
assert(cargoVersion, "Cargo.toml package version was not found");

for (const [path, manifest] of packages) {
  assert.equal(manifest.version, cargoVersion, `${path} version differs from Cargo.toml`);
}
const root = packages[0][1];
for (const [name, version] of Object.entries(root.optionalDependencies)) {
  assert.equal(version, cargoVersion, `${name} dependency version differs from Cargo.toml`);
}

const expectedVersion = process.env.EXPECTED_VERSION;
if (expectedVersion) {
  assert.equal(cargoVersion, expectedVersion, "requested version does not match package versions");
}

const releaseTag = process.env.RELEASE_TAG;
if (releaseTag) {
  assert.equal(releaseTag, `v${cargoVersion}`, "release tag does not match package versions");
}

console.log(`release versions agree at ${cargoVersion}`);
