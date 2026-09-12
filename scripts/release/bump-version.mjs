// Single entry point for a version bump. Rewrites every manifest, workflow input, documentation
// pin, and changelog heading that must agree with the Cargo package version, then refreshes both
// Cargo lockfiles. `scripts/check/check-release-version.mjs` verifies the same set of files, so a
// bump performed here always satisfies the release gate.
//
// Every file is read and its replacement validated before anything is written, so a repository
// that has drifted out of the shapes below fails the bump with the tree untouched.
import { execFile } from "node:child_process";
import { readFile, readdir, writeFile } from "node:fs/promises";
import { promisify } from "node:util";

import { ACTION_PIN, findActionPins } from "./version-sites.mjs";

const run = promisify(execFile);

const SEMVER = /^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?$/;
const REPOSITORY = "https://github.com/figueiredoluiz/cuke-dedup";
const CARGO_VERSION = /^\[package\]\n(?:.*\n)*?version = "([^"]+)"$/m;

const [version, ...rest] = process.argv.slice(2);
if (!version || rest.length > 0) {
  throw new Error("usage: node scripts/release/bump-version.mjs VERSION");
}
if (!SEMVER.test(version)) {
  throw new Error(`not a release version: ${version}`);
}

const planned = new Map();

// Rewrites the single site matching `pattern`. More or fewer matches means the file no longer has
// the shape this script understands, which must fail loudly rather than be guessed at.
async function planEdit(path, pattern, replacement) {
  const before = await readFile(path, "utf8");
  const found = before.match(new RegExp(pattern.source, `${pattern.flags}g`))?.length ?? 0;
  if (found !== 1) {
    throw new Error(`expected exactly one version site in ${path}, found ${found}`);
  }
  planned.set(path, before.replace(pattern, replacement));
  return before;
}

// Only for files npm itself writes: `JSON.stringify(…, 2)` reproduces their formatting exactly.
async function planJson(path, transform) {
  const document = JSON.parse(await readFile(path, "utf8"));
  transform(document);
  planned.set(path, `${JSON.stringify(document, null, 2)}\n`);
}

// Cargo.toml is the source of truth every other site is derived from.
const cargo = await planEdit(
  "Cargo.toml",
  /(^\[package\]\n(?:.*\n)*?version = ")[^"]+(")/m,
  `$1${version}$2`,
);
const previous = cargo.match(CARGO_VERSION)?.[1];
if (!previous) {
  throw new Error("Cargo.toml package version was not found");
}
if (previous === version) {
  throw new Error(`Cargo.toml is already at ${version}`);
}

// The fuzz crate pins the analyzer exactly so a fuzz run can never test a stale library.
await planEdit(
  "fuzz/Cargo.toml",
  /(^cuke-dedup = \{ path = "\.\.", version = "=)[^"]+(" \}$)/m,
  `$1${version}$2`,
);

const prebuilt = JSON.parse(await readFile("npm/prebuilt-targets.json", "utf8"));
const platformDirectories = Object.values(prebuilt.targets).map((target) => target.packageDirectory);
const discovered = (await readdir("npm/platforms")).sort();
if (discovered.join() !== [...platformDirectories].sort().join()) {
  throw new Error(
    `npm/platforms holds ${discovered.join(", ")} but prebuilt-targets.json declares ${platformDirectories.join(", ")}`,
  );
}

await planJson("package.json", (manifest) => {
  manifest.version = version;
  for (const name of Object.keys(manifest.optionalDependencies)) {
    manifest.optionalDependencies[name] = version;
  }
});

// Edited as text, not re-serialized: these manifests keep short arrays such as `"os": ["darwin"]`
// on one line, and a JSON round-trip would expand every one of them into unrelated diff noise.
for (const directory of platformDirectories) {
  await planEdit(
    `npm/platforms/${directory}/package.json`,
    /(^ {2}"version": ")[^"]+(",$)/m,
    `$1${version}$2`,
  );
}

// package-lock.json is edited rather than regenerated: the platform packages are workspace links
// whose `os`/`cpu` never match the machine running the bump, and the `npm install --force` needed
// to get past that check also drops the `libc` filters that let a consumer's npm choose between
// the glibc and musl binaries.
await planJson("package-lock.json", (lock) => {
  lock.version = version;
  const root = lock.packages[""];
  root.version = version;
  for (const name of Object.keys(root.optionalDependencies)) {
    root.optionalDependencies[name] = version;
  }
  for (const directory of platformDirectories) {
    lock.packages[`npm/platforms/${directory}`].version = version;
  }
});

// Both workflow inputs default to the version being released so a dispatch needs no manual entry.
await planEdit(
  "action.yml",
  /(^ {2}version:\n(?: {4}.*\n)*? {4}default: ")[^"]+(")/m,
  `$1${version}$2`,
);
await planEdit(
  ".github/workflows/release.yml",
  /(^ {6}version:\n(?: {8}.*\n)*? {8}default: )[^\s#]+$/m,
  `$1${version}`,
);

// Discovered rather than listed, so a pin introduced by a new guide is bumped without anyone
// remembering to register the file here.
const pins = await findActionPins();
if (pins.length === 0) {
  throw new Error("no documented Action version pins were found");
}
for (const { path } of pins) {
  const before = await readFile(path, "utf8");
  planned.set(path, before.replace(ACTION_PIN, `figueiredoluiz/cuke-dedup@v${version}`));
}

// Promote the accumulated Unreleased notes into a dated section and open a fresh Unreleased.
const changelogBefore = await readFile("CHANGELOG.md", "utf8");
const notes = changelogBefore.match(/^## \[Unreleased\]\n([\s\S]*?)(?=^## \[)/m);
if (!notes) {
  throw new Error("CHANGELOG.md Unreleased section was not found");
}
if (notes[1].trim() === "") {
  throw new Error("CHANGELOG.md Unreleased section is empty; describe the release first");
}
const released = process.env.RELEASE_DATE ?? new Date().toISOString().slice(0, 10);
if (!/^\d{4}-\d{2}-\d{2}$/.test(released)) {
  throw new Error(`RELEASE_DATE must be YYYY-MM-DD, got ${released}`);
}
// Anchored to the first version link definition so the new entry joins that block rather than
// landing above an unrelated reference-style link elsewhere in the document.
const links = /^\[\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?\]: \S+$/m;
if (!links.test(changelogBefore)) {
  throw new Error("CHANGELOG.md version link definitions were not found");
}
planned.set(
  "CHANGELOG.md",
  changelogBefore
    .replace(/^## \[Unreleased\]\n/m, `## [Unreleased]\n\n## [${version}] - ${released}\n`)
    .replace(links, `[${version}]: ${REPOSITORY}/compare/v${previous}...v${version}\n$&`),
);

const written = [];
for (const [path, content] of planned) {
  const before = await readFile(path, "utf8");
  if (content !== before) {
    await writeFile(path, content);
    written.push(path);
  }
}

// Cargo rewrites only the workspace member entry, and needs the new Cargo.toml already on disk.
await run("cargo", ["update", "--workspace", "--offline"]);
await run("cargo", ["update", "--workspace", "--offline", "--manifest-path", "fuzz/Cargo.toml"]);
written.push("Cargo.lock", "fuzz/Cargo.lock");

console.log(`bumped ${previous} -> ${version} across ${written.length} files`);
for (const path of written) {
  console.log(`  ${path}`);
}
console.log("\nnext: npm run check:versions && git diff");
