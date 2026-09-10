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
const lock = JSON.parse(await readFile("package-lock.json", "utf8"));
const cargo = await readFile("Cargo.toml", "utf8");
const action = await readFile("action.yml", "utf8");
const releaseWorkflow = await readFile(".github/workflows/release.yml", "utf8");
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

const actionVersion = action.match(
  /^  version:\r?\n(?: {4}.*\r?\n)*? {4}default: "([^"]+)"\s*$/m,
)?.[1];
assert(actionVersion, "action.yml version input default was not found");
assert.equal(actionVersion, cargoVersion, "action.yml version differs from Cargo.toml");

const releaseVersion = releaseWorkflow.match(
  /^      version:\r?\n(?: {8}.*\r?\n)*? {8}default: ([^\s#]+)\s*$/m,
)?.[1];
assert(releaseVersion, "release.yml version input default was not found");
assert.equal(releaseVersion, cargoVersion, "release.yml version differs from Cargo.toml");

for (const [path, manifest] of packages) {
  assert.equal(manifest.version, cargoVersion, `${path} version differs from Cargo.toml`);
}
const root = packages[0][1];
for (const [name, version] of Object.entries(root.optionalDependencies)) {
  assert.equal(version, cargoVersion, `${name} dependency version differs from Cargo.toml`);
}
assert.equal(lock.version, cargoVersion, "package-lock.json version differs from Cargo.toml");
assert.equal(lock.name, root.name, "package-lock.json name differs from package.json");
assert.equal(
  lock.packages?.[""]?.version,
  cargoVersion,
  'package-lock.json packages[""] version differs from Cargo.toml',
);
assert.deepEqual(
  lock.packages?.[""]?.optionalDependencies,
  root.optionalDependencies,
  'package-lock.json packages[""] optional dependencies differ from package.json',
);
for (const [path, manifest] of packages.slice(1)) {
  const lockPath = path.replace(/\/package\.json$/, "");
  assert.equal(
    lock.packages?.[lockPath]?.name,
    manifest.name,
    `package-lock.json ${lockPath} name differs from its manifest`,
  );
  assert.equal(
    lock.packages?.[lockPath]?.version,
    cargoVersion,
    `package-lock.json ${lockPath} version differs from Cargo.toml`,
  );
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
