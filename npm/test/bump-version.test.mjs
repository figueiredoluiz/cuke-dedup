import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { after, describe, test } from "node:test";

const script = resolve("scripts/release/bump-version.mjs");
const cargoAvailable =
  spawnSync("cargo", ["--version"], { encoding: "utf8" }).status === 0;

const temporary = [];
after(async () => {
  await Promise.all(temporary.map((path) => rm(path, { force: true, recursive: true })));
});

/// Builds the smallest tree the bumper understands: a dependency-free Cargo workspace so
/// `cargo update --offline` needs no registry, plus every npm, workflow, and documentation site.
async function fixture({ eol = "\n", unreleased = "\n### Fixed\n\n- Something.\n" } = {}) {
  const root = await mkdtemp(join(tmpdir(), "cuke-dedup-bump-"));
  temporary.push(root);
  const write = async (relative, text) => {
    const path = join(root, relative);
    await mkdir(join(path, ".."), { recursive: true });
    await writeFile(path, eol === "\n" ? text : text.replaceAll("\n", eol));
  };

  await write(
    "Cargo.toml",
    '[package]\nname = "cuke-dedup"\nversion = "0.2.0"\nedition = "2021"\n\n[lib]\npath = "src/lib.rs"\n',
  );
  await write("src/lib.rs", "\n");
  await write(
    "fuzz/Cargo.toml",
    '[package]\nname = "cuke-dedup-fuzz"\nversion = "0.0.0"\npublish = false\nedition = "2021"\n\n' +
      '[lib]\npath = "src/lib.rs"\n\n[dependencies]\n' +
      'cuke-dedup = { path = "..", version = "=0.2.0" }\n\n[workspace]\nmembers = ["."]\n',
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
          "npm/platforms/darwin-arm64": { version: "0.2.0" },
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
  await write(
    "CHANGELOG.md",
    `# Changelog\n\n## [Unreleased]\n${unreleased}\n## [0.2.0] - 2026-09-10\n\n### Added\n\n` +
      "- Initial.\n\n[0.2.0]: https://github.com/figueiredoluiz/cuke-dedup/compare/v0.1.0...v0.2.0\n",
  );

  for (const manifest of [".", "fuzz"]) {
    const locked = spawnSync("cargo", ["generate-lockfile", "--offline"], {
      cwd: join(root, manifest),
      encoding: "utf8",
    });
    assert.equal(locked.status, 0, `generate-lockfile in ${manifest}: ${locked.stderr}`);
  }
  return root;
}

function bump(root, version) {
  return spawnSync(process.execPath, [script, version], {
    cwd: root,
    encoding: "utf8",
    env: { ...process.env, RELEASE_DATE: "2026-09-13" },
  });
}

/// Reads a file back as LF so assertions are independent of the fixture's line endings.
async function text(root, relative) {
  return (await readFile(join(root, relative), "utf8")).replaceAll("\r\n", "\n");
}

describe("bump-version", { skip: cargoAvailable ? false : "cargo is unavailable" }, () => {
  test("rewrites every version site and promotes the changelog", async () => {
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

    const changelog = await text(root, "CHANGELOG.md");
    assert.match(changelog, /## \[Unreleased\]\n\n## \[0\.3\.0\] - 2026-09-13\n/);
    assert.ok(changelog.includes("[0.3.0]: https://github.com/figueiredoluiz/cuke-dedup/compare/v0.2.0...v0.3.0"));
  });

  test("keeps CRLF files on CRLF", async () => {
    const root = await fixture({ eol: "\r\n" });
    const result = bump(root, "0.3.0");
    assert.equal(result.status, 0, result.stderr);

    for (const relative of ["Cargo.toml", "package.json", "action.yml", "CHANGELOG.md", "README.md"]) {
      const raw = await readFile(join(root, relative), "utf8");
      assert.ok(raw.includes("\r\n"), `${relative} lost its CRLF endings`);
      assert.ok(!/[^\r]\n/.test(raw), `${relative} gained mixed line endings`);
      assert.ok(raw.replaceAll("\r\n", "\n").includes("0.3.0"), `${relative} was not bumped`);
    }
  });

  test("refuses an empty Unreleased section and writes nothing", async () => {
    const root = await fixture({ unreleased: "\n" });
    const before = await text(root, "Cargo.toml");
    const result = bump(root, "0.3.0");
    assert.equal(result.status, 1);
    assert.match(result.stderr, /Unreleased section is empty/);
    assert.equal(await text(root, "Cargo.toml"), before);
  });

  test("refuses a drifted file and writes nothing", async () => {
    const root = await fixture();
    // A manifest that no longer carries the shape the script understands.
    await writeFile(join(root, "action.yml"), "inputs:\n  version:\n    default: 0.2.0\n");
    const before = {
      cargo: await text(root, "Cargo.toml"),
      changelog: await text(root, "CHANGELOG.md"),
      manifest: await text(root, "package.json"),
    };
    const result = bump(root, "0.3.0");
    assert.equal(result.status, 1);
    assert.match(result.stderr, /expected exactly one version site in action\.yml/);
    assert.equal(await text(root, "Cargo.toml"), before.cargo);
    assert.equal(await text(root, "CHANGELOG.md"), before.changelog);
    assert.equal(await text(root, "package.json"), before.manifest);
  });

  test("restores the tree when a lockfile refresh fails", async () => {
    const root = await fixture();
    const before = {
      cargo: await text(root, "Cargo.toml"),
      changelog: await text(root, "CHANGELOG.md"),
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
    assert.equal(await text(root, "CHANGELOG.md"), before.changelog);
    assert.equal(await text(root, "package.json"), before.manifest);
    assert.equal(await text(root, "Cargo.lock"), before.lock);
  });
});
