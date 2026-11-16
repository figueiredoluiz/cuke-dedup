import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { after, describe, test } from "node:test";

const script = resolve("scripts/release/bump-version.mjs");
const checkScript = resolve("scripts/check/check-release-version.mjs");
// Child node drops NODE_* (NODE_OPTIONS preloads code); the TOML reader's Python needs PATH and HOME.
const childEnv = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("NODE_")));
const cargoAvailable =
  spawnSync("cargo", ["--version"], { encoding: "utf8" }).status === 0;

const temporary = [];
after(async () => {
  await Promise.all(temporary.map((path) => rm(path, { force: true, recursive: true })));
});

/**
 * Build an offline Cargo fixture with every supported npm, workflow, and documentation version site and record it for cleanup.
 * `header`, `cargoVersion` and `fuzzPin` replace the `[package]` header, its version line and the fuzz requirement line;
 * `bom` prefixes Cargo.toml.
 */
async function fixture({
  eol = "\n",
  bom = "",
  header = "[package]",
  cargoVersion = 'version = "0.2.0"',
  fuzzPin = 'cuke-dedup = { path = "..", version = "=0.2.0" }',
} = {}) {
  const root = await mkdtemp(join(tmpdir(), "cuke-dedup-bump-"));
  temporary.push(root);
  /** Write a fixture file after creating its parent directories and applying the requested line endings. */
  const write = async (relative, text) => {
    const path = join(root, relative);
    await mkdir(join(path, ".."), { recursive: true });
    await writeFile(path, eol === "\n" ? text : text.replaceAll("\n", eol));
  };

  await write(
    "Cargo.toml",
    `${bom}${header}\nname = "cuke-dedup"\n${cargoVersion}\nedition = "2021"\n\n[lib]\npath = "src/lib.rs"\n`,
  );
  await write("src/lib.rs", "\n");
  await write(
    "fuzz/Cargo.toml",
    '[package]\nname = "cuke-dedup-fuzz"\nversion = "0.0.0"\npublish = false\nedition = "2021"\n\n' +
      '[lib]\npath = "src/lib.rs"\n\n[dependencies]\n' +
      `${fuzzPin}\n\n[workspace]\nmembers = ["."]\n`,
  );
  await write("fuzz/src/lib.rs", "\n");
  await write(
    "npm/prebuilt-targets.json",
    `${JSON.stringify(
      { schemaVersion: 1, targets: { "darwin-arm64": { packageDirectory: "darwin-arm64" } } },
      null,
      2,
    )}\n`,
  );
  await write(
    "npm/platforms/darwin-arm64/package.json",
    '{\n  "name": "cuke-dedup-darwin-arm64",\n  "version": "0.2.0",\n  "os": ["darwin"]\n}\n',
  );
  await write(
    "package.json",
    `${JSON.stringify(
      { name: "cuke-dedup", version: "0.2.0", optionalDependencies: { "cuke-dedup-darwin-arm64": "0.2.0" } },
      null,
      2,
    )}\n`,
  );
  await write(
    "package-lock.json",
    `${JSON.stringify(
      {
        name: "cuke-dedup",
        version: "0.2.0",
        packages: {
          "": { version: "0.2.0", optionalDependencies: { "cuke-dedup-darwin-arm64": "0.2.0" } },
          "npm/platforms/darwin-arm64": { name: "cuke-dedup-darwin-arm64", version: "0.2.0" },
        },
      },
      null,
      2,
    )}\n`,
  );
  await write(
    "action.yml",
    'inputs:\n  version:\n    description: Release version\n    default: "0.2.0"\n',
  );
  await write(
    ".github/workflows/release.yml",
    "on:\n  workflow_dispatch:\n    inputs:\n      version:\n" +
      "        description: Package version\n        required: true\n        default: 0.2.0\n",
  );
  await write("README.md", "# CukeDedup\n\n  - uses: figueiredoluiz/cuke-dedup@v0.2.0\n");
  await write("docs/ci-and-baselines.md", "  - uses: figueiredoluiz/cuke-dedup@v0.2.0\n");

  for (const manifest of [".", "fuzz"]) {
    const locked = spawnSync("cargo", ["generate-lockfile", "--offline"], {
      cwd: join(root, manifest),
      encoding: "utf8",
    });
    assert.equal(locked.status, 0, `generate-lockfile in ${manifest}: ${locked.stderr}`);
  }
  return root;
}

/** Run the version bumper in the fixture root with optional environment overrides and capture its result. */
function bump(root, version, environment = {}) {
  return spawnSync(process.execPath, [script, version], {
    cwd: root,
    encoding: "utf8",
    env: { ...childEnv, ...environment },
  });
}

/** Run the release version check in the fixture root and capture its result. */
function checkVersions(root) {
  return spawnSync(process.execPath, [checkScript], { cwd: root, encoding: "utf8", env: childEnv });
}

/** Read a fixture file with CRLF normalized to LF for line-ending-independent assertions. */
async function text(root, relative) {
  return (await readFile(join(root, relative), "utf8")).replaceAll("\r\n", "\n");
}

/** Group version-bump integration cases, skipping them when Cargo is unavailable. */
describe("bump-version", { skip: cargoAvailable ? false : "cargo is unavailable" }, () => {
  /** Verify all supported manifests, lockfiles, workflow defaults, and documentation pins are bumped without a changelog. */
  test("rewrites every version site without creating a changelog", async () => {
    const root = await fixture();
    const result = bump(root, "0.3.0");
    assert.equal(result.status, 0, result.stderr);

    for (const [relative, expected] of [
      ["Cargo.toml", 'version = "0.3.0"'],
      ["fuzz/Cargo.toml", 'version = "=0.3.0"'],
      ["package.json", '"version": "0.3.0"'],
      ["package.json", '"cuke-dedup-darwin-arm64": "0.3.0"'],
      ["package-lock.json", '"version": "0.3.0"'],
      ["npm/platforms/darwin-arm64/package.json", '"version": "0.3.0"'],
      ["action.yml", 'default: "0.3.0"'],
      [".github/workflows/release.yml", "default: 0.3.0"],
      ["README.md", "cuke-dedup@v0.3.0"],
      ["docs/ci-and-baselines.md", "cuke-dedup@v0.3.0"],
      ["Cargo.lock", 'version = "0.3.0"'],
      ["fuzz/Cargo.lock", 'version = "0.3.0"'],
    ]) {
      assert.ok((await text(root, relative)).includes(expected), `${relative} missing ${expected}`);
    }

    await assert.rejects(readFile(join(root, "CHANGELOG.md")), { code: "ENOENT" });
  });

  /** Verify ignored files stay untouched, inherited Git overrides are isolated, and discovery errors prevent writes. */
  test("updates project pins without rewriting ignored reference or history files", async () => {
    const root = await fixture();
    const gitEnvironment = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith("GIT_")));
    assert.equal(spawnSync("git", ["init", "--quiet", root], { env: gitEnvironment }).status, 0);
    assert.equal(spawnSync("git", ["add", "README.md"], { cwd: root, env: gitEnvironment }).status, 0);
    await writeFile(join(root, ".git/info/exclude"), "references.local/\n*.local.md\n");
    await mkdir(join(root, "references.local"));
    const ignored = ["references.local/README.md", "journal.local.md"];
    const note = "uses: figueiredoluiz/cuke-dedup@v0.1.0\n";
    for (const path of ignored) await writeFile(join(root, path), note);
    const foreignIndex = join(root, "foreign-index");
    await writeFile(foreignIndex, "invalid index");
    const result = bump(root, "0.3.0", { GIT_DIR: join(root, "foreign.git"), GIT_WORK_TREE: join(root, "foreign-tree"), GIT_INDEX_FILE: foreignIndex });
    assert.equal(result.status, 0, result.stderr);
    for (const path of ["README.md", "docs/ci-and-baselines.md"]) {
      assert.ok((await text(root, path)).includes("@v0.3.0"), path);
    }
    for (const path of ignored) assert.equal(await text(root, path), note);
    const index = join(root, ".git/index");
    await writeFile(index, "invalid index");
    const failed = bump(root, "0.4.0");
    assert.equal(failed.status, 1);
    assert.match(failed.stderr, /Git version-site discovery failed/);
    assert.ok((await text(root, "Cargo.toml")).includes('version = "0.3.0"'));
  });

  /** Verify bumping CRLF fixtures preserves their line endings without introducing mixed endings. */
  test("keeps CRLF files on CRLF", async () => {
    const root = await fixture({ eol: "\r\n" });
    const result = bump(root, "0.3.0");
    assert.equal(result.status, 0, result.stderr);

    for (const relative of ["Cargo.toml", "package.json", "action.yml", "README.md"]) {
      const raw = await readFile(join(root, relative), "utf8");
      assert.ok(raw.includes("\r\n"), `${relative} lost its CRLF endings`);
      assert.ok(!/[^\r]\n/.test(raw), `${relative} gained mixed line endings`);
      assert.ok(raw.replaceAll("\r\n", "\n").includes("0.3.0"), `${relative} was not bumped`);
    }
  });

  /** Verify an unsupported version-site shape aborts planning before existing files change. */
  test("refuses a drifted file and writes nothing", async () => {
    const root = await fixture();
    // A manifest that no longer carries the shape the script understands.
    await writeFile(join(root, "action.yml"), "inputs:\n  version:\n    default: 0.2.0\n");
    const before = {
      cargo: await text(root, "Cargo.toml"),
      manifest: await text(root, "package.json"),
    };
    const result = bump(root, "0.3.0");
    assert.equal(result.status, 1);
    assert.match(result.stderr, /expected exactly one version site in action\.yml/);
    assert.equal(await text(root, "Cargo.toml"), before.cargo);
    assert.equal(await text(root, "package.json"), before.manifest);
  });

  /** Verify TOML forms the parser reads: the bump keeps each form and the release check agrees before and after. */
  test("bumps every TOML form the release check reads", async () => {
    const forms = [
      [{ cargoVersion: 'version = "0.2.0" # released' }, "Cargo.toml", 'version = "0.3.0" # released'],
      [{ cargoVersion: "version = '0.2.0'" }, "Cargo.toml", "version = '0.3.0'"],
      [{ cargoVersion: 'version="0.2.0"' }, "Cargo.toml", 'version="0.3.0"'],
      [{ cargoVersion: "version = '0.2.0' # released", eol: "\r\n" }, "Cargo.toml", "version = '0.3.0' # released"],
      [{ fuzzPin: "cuke-dedup = { path = '..', version = '=0.2.0' } # exact" }, "fuzz/Cargo.toml", "version = '=0.3.0' } # exact"],
      [{ fuzzPin: 'cuke-dedup = { path = "..", version = "= 0.2.0" }' }, "fuzz/Cargo.toml", 'version = "= 0.3.0" }'],
      [{ header: "[package] # crate" }, "Cargo.toml", '[package] # crate\nname = "cuke-dedup"\nversion = "0.3.0"'],
      [{ header: "[ package ]" }, "Cargo.toml", '[ package ]\nname = "cuke-dedup"\nversion = "0.3.0"'],
      [{ bom: "\ufeff" }, "Cargo.toml", '\ufeff[package]\nname = "cuke-dedup"\nversion = "0.3.0"'],
      [{ cargoVersion: 'description = """\n[not a table]\n"""\nversion = "0.2.0"' }, "Cargo.toml", '"""\nversion = "0.3.0"'],
    ];
    for (const [options, file, expected] of forms) {
      const root = await fixture(options);
      const before = checkVersions(root);
      assert.equal(before.status, 0, before.stderr);
      const result = bump(root, "0.3.0");
      assert.equal(result.status, 0, result.stderr);
      assert.match(result.stdout, /^bumped 0\.2\.0 -> 0\.3\.0 across/m);
      assert.ok((await text(root, file)).includes(expected), `${file} lacks ${expected}`);
      const after = checkVersions(root);
      assert.equal(after.status, 0, after.stderr);
      assert.match(after.stdout, /^release versions agree at 0\.3\.0$/m);
    }
  });

  /** Verify an edit the parser does not read as the version is refused before anything is written. */
  test("refuses a version site the parser does not read", async () => {
    // The regex finds the line inside the description; the quoted key is the real version.
    const root = await fixture({ cargoVersion: '"version" = "0.2.0"\ndescription = """\nversion = "0.2.0"\n"""' });
    const before = { cargo: await text(root, "Cargo.toml"), fuzz: await text(root, "fuzz/Cargo.toml") };
    const result = bump(root, "0.3.0");
    assert.equal(result.status, 1);
    assert.match(result.stderr, /planned manifests parse as version 0\.2\.0 and fuzz pin 0\.3\.0, not 0\.3\.0/);
    assert.equal(await text(root, "Cargo.toml"), before.cargo);
    assert.equal(await text(root, "fuzz/Cargo.toml"), before.fuzz);
  });

  /** Verify the release check names a fuzz pin or lockfile entry that differs from Cargo.toml. */
  test("release check fails on a differing Cargo site", async () => {
    const root = await fixture();
    const rows = [
      ["fuzz/Cargo.toml", '"=0.2.0"', '"=0.1.0"', /fuzz\/Cargo\.toml pin differs from Cargo\.toml/],
      ["Cargo.lock", 'name = "cuke-dedup"\nversion = "0.2.0"', 'name = "cuke-dedup"\nversion = "0.1.0"', /: Cargo\.lock version differs from Cargo\.toml/],
      ["fuzz/Cargo.lock", 'name = "cuke-dedup"\nversion = "0.2.0"', 'name = "cuke-dedup"\nversion = "0.1.0"', /fuzz.Cargo\.lock version differs from Cargo\.toml/],
    ];
    for (const [file, from, to, message] of rows) {
      const original = await text(root, file);
      assert.ok(original.includes(from), `${file} lacks ${from}`);
      await writeFile(join(root, file), original.replace(from, to));
      const result = checkVersions(root);
      assert.equal(result.status, 1, file);
      assert.match(result.stderr, message);
      await writeFile(join(root, file), original);
    }
    assert.equal(checkVersions(root).status, 0);
  });

  /** Verify a later table's version, which the site search can reach, is refused before anything is written. */
  test("refuses a version site outside the package table", async () => {
    const root = await fixture({ cargoVersion: '"version" = "0.2.0"\n\n[package.metadata.release]\nversion = "0.2.0"' });
    const before = await text(root, "Cargo.toml");
    const result = bump(root, "0.3.0");
    assert.equal(result.status, 1);
    assert.match(result.stderr, /planned manifests parse as version 0\.2\.0 and fuzz pin 0\.3\.0, not 0\.3\.0/);
    assert.equal(await text(root, "Cargo.toml"), before);
  });

  /** Verify an edit that breaks the manifest is refused before anything is written, naming the repository file. */
  test("refuses an edit that makes a manifest unreadable", async () => {
    // The site search takes the backslash of `\"` inside the description; the edit ends the string early.
    const root = await fixture({ cargoVersion: 'description = """\nversion = "\\"""\n"""\nversion = "0.2.0"' });
    assert.equal(checkVersions(root).status, 0);
    const before = await text(root, "Cargo.toml");
    const result = bump(root, "0.3.0");
    assert.equal(result.status, 1);
    assert.match(result.stderr, /planned manifests fail to read, nothing written: python3? failed reading Cargo\.toml, fuzz.Cargo\.toml \(exit 1\): Cargo\.toml: TOMLDecodeError: /);
    assert.doesNotMatch(result.stderr, /cuke-dedup-bump-/);
    assert.equal(await text(root, "Cargo.toml"), before);
  });

  /** Verify a JSON file with a BOM, which the release check refuses, is refused before anything is written. */
  test("refuses a BOM in package.json", async () => {
    const root = await fixture();
    await writeFile(join(root, "package.json"), `\ufeff${await text(root, "package.json")}`);
    const before = { cargo: await text(root, "Cargo.toml"), manifest: await readFile(join(root, "package.json"), "utf8") };
    const result = bump(root, "0.3.0");
    assert.equal(result.status, 1);
    assert.match(result.stderr, /SyntaxError/);
    assert.equal(await text(root, "Cargo.toml"), before.cargo);
    assert.equal(await readFile(join(root, "package.json"), "utf8"), before.manifest);
  });

  /** Verify a fuzz pin the parser does not read, or a second pin, is refused before anything is written. */
  test("refuses a fuzz pin site the parser does not read", async () => {
    const pin = 'cuke-dedup = { path = "..", version = "=0.2.0" }';
    const rows = [
      // The regex finds the line inside the string; the quoted key is the real requirement.
      [`"cuke-dedup" = { path = "..", version = "=0.2.0" }\n\n[package.metadata]\nnote = """\n${pin}\n"""`, /planned manifests parse as version 0\.3\.0 and fuzz pin 0\.2\.0, not 0\.3\.0/],
      [`${pin}\n\n[target.'cfg(fuzzing)'.dependencies]\n${pin}`, /expected exactly one version site in fuzz\/Cargo\.toml, found 2/],
    ];
    for (const [fuzzPin, message] of rows) {
      const root = await fixture({ fuzzPin });
      const before = { cargo: await text(root, "Cargo.toml"), fuzz: await text(root, "fuzz/Cargo.toml") };
      const result = bump(root, "0.3.0");
      assert.equal(result.status, 1);
      assert.match(result.stderr, message);
      assert.equal(await text(root, "Cargo.toml"), before.cargo);
      assert.equal(await text(root, "fuzz/Cargo.toml"), before.fuzz);
    }
  });

  /** Verify a failed fuzz lockfile refresh restores the already-written manifests and root lockfile. */
  test("restores the tree when a lockfile refresh fails", async () => {
    const root = await fixture();
    const before = {
      cargo: await text(root, "Cargo.toml"),
      manifest: await text(root, "package.json"),
      lock: await text(root, "Cargo.lock"),
    };
    // Corrupt the fuzz lockfile. Planning never reads lockfile contents, so every file still has
    // the shape it expects: planning succeeds, the root refresh succeeds, and the failure lands
    // on the fuzz refresh with the tree already written — the only path that rolls back.
    await writeFile(join(root, "fuzz", "Cargo.lock"), "this is not toml [[[\n");
    const result = bump(root, "0.3.0");
    assert.equal(result.status, 1);
    assert.match(result.stderr, /bump failed and the tree was restored/);
    assert.equal(await text(root, "Cargo.toml"), before.cargo);
    assert.equal(await text(root, "package.json"), before.manifest);
    assert.equal(await text(root, "Cargo.lock"), before.lock);
  });
});
