// cargo-versions: the release scripts' version sites come from real TOML parsing and refuse shapes
// they do not model.
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";

import { fuzzPin, lockedVersion, packageVersion, readCargoVersions } from "./cargo-versions.mjs";

const MANIFEST = '[package]\nname = "cuke-dedup"\nversion = "0.2.0"\n';
const FUZZ = '[dependencies]\ncuke-dedup = { path = "..", version = "=0.2.0" }\n';
const LOCK = '[[package]]\nname = "cuke-dedup"\nversion = "0.2.0"\n\n[[package]]\nname = "other"\nversion = "1.0.0"\n';

/** Write `files` ({relative path: text}) to a fresh directory, run `body(dir)`, then remove it. */
function withTree(files, body) {
  const dir = mkdtempSync(join(tmpdir(), "cargo-versions-"));
  try {
    for (const [path, text] of Object.entries(files)) {
      mkdirSync(dirname(join(dir, path)), { recursive: true });
      writeFileSync(join(dir, path), text);
    }
    return body(dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

/** Read a tree holding the given manifest and fuzz manifest texts plus valid lockfiles. */
const read = (manifest, fuzz = FUZZ, lock = LOCK) =>
  withTree(
    { "Cargo.toml": manifest, "fuzz/Cargo.toml": fuzz, "Cargo.lock": lock, "fuzz/Cargo.lock": lock },
    (dir) => readCargoVersions(dir),
  );

/** Forms the replaced regexes refused or misread: the parser decides. */
test("readCargoVersions follows the TOML spec", () => {
  const rows = [
    [MANIFEST.replace('"0.2.0"', '"0.2.0" # released'), FUZZ, "manifest trailing comment"],
    [MANIFEST.replace('"0.2.0"', "'0.2.0'"), FUZZ, "manifest literal string"],
    [MANIFEST.replaceAll("\n", "\r\n"), FUZZ.replaceAll("\n", "\r\n"), "CRLF"],
    [MANIFEST.replace("version = ", "version="), FUZZ, "no spacing"],
    [`\ufeff${MANIFEST}`, `\ufeff${FUZZ}`, "leading BOM, which Cargo accepts"],
    [MANIFEST, '[dependencies]\ncuke-dedup = { path = "..", version = "=0.2.0" } # exact\n', "fuzz trailing comment"],
    [MANIFEST, FUZZ.replace('"=0.2.0"', "'=0.2.0'"), "fuzz literal string"],
    [MANIFEST, '[dependencies.cuke-dedup]\npath = ".."\nversion = "=0.2.0"\n', "fuzz dependency table"],
    [MANIFEST, FUZZ.replace('"=0.2.0"', '"= 0.2.0"'), "fuzz space after ="],
    ['[package]\nname = "cuke-dedup"\n[package.metadata]\nversion = "9.9.9"\n', FUZZ, null],
  ];
  for (const [manifest, fuzz, label] of rows) {
    if (label === null) {
      // `[package.metadata] version` is another table's key, which a line scan took for the package's.
      assert.throws(() => read(manifest, fuzz), /Cargo\.toml has no \[package\] version$/);
      continue;
    }
    const result = read(manifest, fuzz);
    assert.equal(result.version, "0.2.0", label);
    assert.equal(result.fuzzPin, "0.2.0", label);
    assert.deepEqual(result.locked.map(([, version]) => version), ["0.2.0", "0.2.0"], label);
  }
});

/** One launch reads all four files; without lockfiles only the manifests are read. */
test("readCargoVersions reads every site in one launch", () => {
  const calls = [];
  const documents = [{ package: { version: "1.0.0" } }, { dependencies: { "cuke-dedup": { path: "..", version: "=1.0.0" } } }];
  /** Record each launch's paths and return parsed documents for them. */
  const readFiles = (paths) => {
    calls.push(paths);
    return paths.length === 2 ? documents : [...documents, { package: [{ name: "cuke-dedup", version: "1.0.0" }] }, { package: [{ name: "cuke-dedup", version: "1.0.1" }] }];
  };
  assert.deepEqual(readCargoVersions("r", { readFiles }), {
    version: "1.0.0",
    fuzzPin: "1.0.0",
    locked: [["Cargo.lock", "1.0.0"], ["fuzz/Cargo.lock", "1.0.1"]],
  });
  assert.deepEqual(readCargoVersions("r", { lockfiles: false, readFiles }).locked, []);
  assert.deepEqual(calls.map((paths) => paths.length), [4, 2]);
  // An invalid lockfile fails the read and names the file.
  assert.throws(() => read(MANIFEST, FUZZ, "[[package]\n"), /cargo-versions-\w+[\\/]Cargo\.lock: TOMLDecodeError/);
});

const FUZZ_SHAPE = 'F [dependencies] cuke-dedup must be path = ".." with version = "=x.y.z", got';

/** Shapes the release scripts do not model are refused with the value found. */
test("packageVersion, fuzzPin and lockedVersion fail closed", () => {
  const throws = [
    [() => packageVersion({}, "C"), "C has no [package] version"],
    [() => packageVersion({ package: { version: { workspace: true } } }, "C"), 'C [package] version must be a string, got {"workspace":true}'],
    [() => packageVersion({ package: [{ version: "1.0.0" }] }, "C"), 'C [package] must be a table, got array [{"version":"1.0.0"}]'],
    [() => fuzzPin({ dependencies: { "cuke-dedup": "=1.0.0" } }, "F"), `${FUZZ_SHAPE} "=1.0.0"`],
    [() => fuzzPin({ dependencies: { "cuke-dedup": { path: "..", version: "1.0.0" } } }, "F"), `${FUZZ_SHAPE} {"path":"..","version":"1.0.0"}`],
    [() => fuzzPin({ dependencies: { "cuke-dedup": { version: "=1.0.0" } } }, "F"), `${FUZZ_SHAPE} {"version":"=1.0.0"}`],
    [() => fuzzPin({ dependencies: { "cuke-dedup": { path: "../other", version: "=1.0.0" } } }, "F"), `${FUZZ_SHAPE} {"path":"../other","version":"=1.0.0"}`],
    [() => fuzzPin({ dependencies: { "cuke-dedup": { path: ".." } } }, "F"), `${FUZZ_SHAPE} {"path":".."}`],
    [() => fuzzPin({ dependencies: { "cuke-dedup": { path: "..", version: 1 } } }, "F"), `${FUZZ_SHAPE} {"path":"..","version":1}`],
    [() => fuzzPin({}, "F"), `${FUZZ_SHAPE} undefined`],
    [() => lockedVersion({ package: [] }, "L"), "L must hold one cuke-dedup [[package]] entry, found 0"],
    [() => lockedVersion({ package: [{ name: "cuke-dedup", version: "1.0.0" }, { name: "cuke-dedup", version: "0.9.0" }] }, "L"), "L must hold one cuke-dedup [[package]] entry, found 2"],
    [() => lockedVersion({ package: { name: "cuke-dedup" } }, "L"), "L must hold one cuke-dedup [[package]] entry, found 0"],
    [() => lockedVersion({ package: [{ name: "cuke-dedup" }] }, "L"), "L cuke-dedup entry version must be a string, got undefined"],
    [() => lockedVersion({ package: [{ name: "cuke-dedup", version: "1.0.0", source: "registry+https://github.com/rust-lang/crates.io-index" }] }, "L"), 'L cuke-dedup entry must be the local crate, got source "registry+https://github.com/rust-lang/crates.io-index"'],
  ];
  for (const [call, message] of throws) assert.throws(call, { message });
  assert.equal(lockedVersion({ package: [{ name: "other" }, { name: "cuke-dedup", version: "1.0.0" }] }, "L"), "1.0.0");
});
