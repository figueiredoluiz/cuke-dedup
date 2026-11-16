// Cargo version sites for the release scripts: `[package] version`, the fuzz crate's exact pin on
// it and each lockfile's `cuke-dedup` entry. The toolchain check's TOML reader parses them, so
// comments, literal strings and CRLF follow the TOML spec instead of a regex.
import { join } from "node:path";

import { describe, readTomlFiles, tableOf } from "./rust-toolchain.mjs";

const CRATE = "cuke-dedup";
/** The root and fuzz manifests, relative to the repository root. */
export const MANIFESTS = ["Cargo.toml", "fuzz/Cargo.toml"];
/** Both Cargo lockfiles, relative to the repository root. */
export const LOCKFILES = ["Cargo.lock", "fuzz/Cargo.lock"];

/** `[package] version` of a parsed manifest; throws unless it is a string. */
export function packageVersion(manifest, path) {
  const version = tableOf(manifest, "package", path)?.version;
  if (version === undefined) throw new Error(`${path} has no [package] version`);
  // A table here is `version.workspace = true`, which names no version of its own.
  if (typeof version !== "string") throw new Error(`${path} [package] version must be a string, got ${describe(version)}`);
  return version;
}

/** The version in the fuzz manifest's `cuke-dedup = { path = "..", version = "=x.y.z" }` requirement. */
export function fuzzPin(manifest, path) {
  const dependency = tableOf(manifest, "dependencies", path)?.[CRATE];
  const requirement = dependency?.version;
  // Without `path` Cargo resolves the published crate, so fuzz runs would not test this tree.
  if (dependency?.path !== ".." || typeof requirement !== "string" || !requirement.startsWith("=")) {
    throw new Error(`${path} [dependencies] ${CRATE} must be path = ".." with version = "=x.y.z", got ${describe(dependency)}`);
  }
  // Cargo reads `= 1.2.3` as `=1.2.3`.
  return requirement.slice(1).trim();
}

/** The version of the one local (no `source`) `cuke-dedup` `[[package]]` entry in a parsed Cargo.lock. */
export function lockedVersion(lock, path) {
  const entries = Array.isArray(lock?.package) ? lock.package.filter((entry) => entry?.name === CRATE) : [];
  if (entries.length !== 1) throw new Error(`${path} must hold one ${CRATE} [[package]] entry, found ${entries.length}`);
  const { version, source } = entries[0];
  if (source !== undefined) throw new Error(`${path} ${CRATE} entry must be the local crate, got source ${describe(source)}`);
  if (typeof version !== "string") throw new Error(`${path} ${CRATE} entry version must be a string, got ${describe(version)}`);
  return version;
}

/**
 * Read the version sites under `root` in one parser launch: `{version, fuzzPin, locked}`, where
 * `locked` lists `[file, version]` per lockfile and is empty when `lockfiles` is false.
 */
export function readCargoVersions(root = ".", { lockfiles = true, readFiles = readTomlFiles } = {}) {
  // Messages name the repository-relative file, the same on every OS.
  const files = [...MANIFESTS, ...(lockfiles ? LOCKFILES : [])];
  const [manifest, fuzz, ...locks] = readFiles(files.map((file) => join(root, file)));
  return {
    version: packageVersion(manifest, files[0]),
    fuzzPin: fuzzPin(fuzz, files[1]),
    locked: locks.map((lock, index) => [files[index + 2], lockedVersion(lock, files[index + 2])]),
  };
}
