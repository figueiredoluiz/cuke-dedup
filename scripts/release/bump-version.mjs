// Synchronize release version sites and Cargo lockfiles atomically.
//
// The bump is atomic. Every file is read and its replacement validated before anything is
// written, and if a lockfile refresh fails afterwards the written files are restored, so a failed
// bump always leaves the tree as it was found.
import { execFile } from "node:child_process";
import { mkdir, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, sep } from "node:path";
import { promisify } from "node:util";

import { LOCKFILES, MANIFESTS, readCargoVersions } from "../check/lib/cargo-versions.mjs";
import { ACTION_PIN, findActionPins } from "./version-sites.mjs";

const run = promisify(execFile);

const SEMVER = /^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?$/;
// The first `version` line after the `[package]` header: basic or literal string, any spacing, a
// trailing comment. A `[` line can sit inside a multi-line string, so the match may run past the
// table; a site the parser does not read as `[package] version` fails the check of the planned
// manifests below. Other forms find no site and fail.
const PACKAGE_VERSION = /(^\[[ \t]*package[ \t]*\][ \t]*(?:#.*)?\n(?:.*\n)*?[ \t]*version[ \t]*=[ \t]*(["']))[^"'\n]+(?=\2)/m;
// The fuzz crate's inline `cuke-dedup = { ..., version = "=x" }` requirement. Any table counts: a
// second pin (`[target.*.dependencies]`) is a second site, which fails instead of bumping one.
const FUZZ_PIN = /(^cuke-dedup[ \t]*=[ \t]*\{.*?\bversion[ \t]*=[ \t]*(["'])=[ \t]*)[^"'\n]+(?=\2)/m;

const [version, ...rest] = process.argv.slice(2);
if (!version || rest.length > 0) {
  throw new Error("usage: node scripts/release/bump-version.mjs VERSION");
}
if (!SEMVER.test(version)) {
  throw new Error(`not a release version: ${version}`);
}

/// Final bytes to write, keyed by path.
const planned = new Map();
/// Bytes as found, keyed by path, so a failed bump can put them back.
const original = new Map();

/**
 * Reads `path` as LF text and records the original bytes and that format.
 *
 * A Windows checkout has no `.gitattributes` forcing LF, so a file can arrive with CRLF. The
 * patterns below are written against LF text: CRLF is normalized before matching, and a file that
 * contained any CRLF is written back with CRLF on every line. A uniformly LF or CRLF file keeps
 * its endings; mixed endings become all CRLF. A TOML file's leading BOM, which Cargo accepts, is
 * set aside and kept; the release check refuses one in JSON, so bump does too.
 */
async function read(path) {
  const raw = await readFile(path, "utf8");
  original.set(path, raw);
  const bom = path.endsWith(".toml") && raw.startsWith("\ufeff");
  const crlf = raw.includes("\r\n");
  const text = bom ? raw.slice(1) : raw;
  return { text: crlf ? text.replaceAll("\r\n", "\n") : text, format: { bom, crlf } };
}

/** Queues `text` for `path` in the `format` `read` found: CRLF line endings, a BOM. */
function plan(path, text, { bom, crlf }) {
  const lines = crlf ? text.replaceAll("\n", "\r\n") : text;
  planned.set(path, bom ? `\ufeff${lines}` : lines);
}

/**
 * Rewrites the single site matching `pattern`.
 *
 * More or fewer matches means the file no longer has the shape this script understands, which must
 * fail loudly rather than be guessed at.
 */
async function planEdit(path, pattern, replacement) {
  const { text, format } = await read(path);
  const found = text.match(new RegExp(pattern.source, `${pattern.flags}g`))?.length ?? 0;
  if (found !== 1) {
    throw new Error(`expected exactly one version site in ${path}, found ${found}`);
  }
  plan(path, text.replace(pattern, replacement), format);
}

/**
 * Applies `transform` to a JSON file and queues the result.
 *
 * Only for files npm itself writes: `JSON.stringify(…, 2)` reproduces their formatting exactly.
 */
async function planJson(path, transform) {
  const { text, format } = await read(path);
  const document = JSON.parse(text);
  transform(document);
  plan(path, `${JSON.stringify(document, null, 2)}\n`, format);
}

/**
 * Parse the planned manifests from a scratch copy before anything is written: the text edits must
 * have set exactly the parsed `[package] version` and fuzz pin, not a lookalike line.
 */
async function verifyPlannedManifests() {
  const scratch = await mkdtemp(join(tmpdir(), "cuke-dedup-bump-"));
  try {
    await mkdir(join(scratch, "fuzz"));
    for (const path of MANIFESTS) await writeFile(join(scratch, path), planned.get(path));
    let after;
    try {
      after = readCargoVersions(scratch, { lockfiles: false });
    } catch (error) {
      // An edit can break the file, e.g. one that ends a multi-line string early. Name repository files.
      throw new Error(`planned manifests fail to read, nothing written: ${error.message.replaceAll(`${scratch}${sep}`, "")}`);
    }
    if (after.version !== version || after.fuzzPin !== version) {
      throw new Error(`planned manifests parse as version ${after.version} and fuzz pin ${after.fuzzPin}, not ${version}`);
    }
  } finally {
    await rm(scratch, { recursive: true, force: true });
  }
}

/** Put `version` after a site's captured prefix; the closing quote is outside the match. */
const setVersion = (_match, head) => `${head}${version}`;

// Cargo.toml is the source of truth every other site is derived from.
const { version: previous } = readCargoVersions(".", { lockfiles: false });
if (previous === version) {
  throw new Error(`Cargo.toml is already at ${version}`);
}
await planEdit("Cargo.toml", PACKAGE_VERSION, setVersion);
// The fuzz crate pins the analyzer exactly so a fuzz run can never test a stale library.
await planEdit("fuzz/Cargo.toml", FUZZ_PIN, setVersion);
await verifyPlannedManifests();

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
  const { text, format } = await read(path);
  plan(path, text.replace(ACTION_PIN, `figueiredoluiz/cuke-dedup@v${version}`), format);
}

// Cargo rewrites the lockfiles from the manifests, so they can only be refreshed once the new
// versions are on disk. Snapshot them too: if either refresh fails, everything written above is
// put back rather than leaving manifests bumped against stale locks.
for (const lockfile of LOCKFILES) {
  original.set(lockfile, await readFile(lockfile, "utf8"));
}

const written = [];
try {
  for (const [path, content] of planned) {
    if (content !== original.get(path)) {
      await writeFile(path, content);
      written.push(path);
    }
  }
  await run("cargo", ["update", "--workspace", "--offline"]);
  await run("cargo", ["update", "--workspace", "--offline", "--manifest-path", "fuzz/Cargo.toml"]);
} catch (error) {
  await Promise.all(
    [...written, ...LOCKFILES].map((path) => writeFile(path, original.get(path))),
  );
  throw new Error(
    `bump failed and the tree was restored: ${error instanceof Error ? error.message : error}`,
  );
}
written.push(...LOCKFILES);

console.log(`bumped ${previous} -> ${version} across ${written.length} files`);
for (const path of written) {
  console.log(`  ${path}`);
}
console.log("\nnext: npm run check:versions && git diff");
