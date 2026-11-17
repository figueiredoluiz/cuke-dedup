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
const actionPin = "cuke-dedup@0123456789abcdef0123456789abcdef01234567 # v0.2.0";
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
  await write("README.md", `# CukeDedup\n\n  - uses: figueiredoluiz/${actionPin}\n`);
  await write("docs/ci-and-baselines.md", `  - uses: figueiredoluiz/${actionPin}\n`);

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
function checkVersions(root, environment = {}, timeout) {
  return spawnSync(process.execPath, [checkScript], {
    cwd: root, encoding: "utf8", env: { ...childEnv, ...environment }, timeout,
  });
}

/** Read a fixture file with CRLF normalized to LF for line-ending-independent assertions. */
async function text(root, relative) {
  return (await readFile(join(root, relative), "utf8")).replaceAll("\r\n", "\n");
}

/** Group version-bump integration cases, skipping them when Cargo is unavailable. */
describe("bump-version", { skip: cargoAvailable ? false : "cargo is unavailable" }, () => {
  /** Verify all version sites are bumped while release examples keep their published commit. */
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
      ["README.md", actionPin],
      ["docs/ci-and-baselines.md", actionPin],
      ["Cargo.lock", 'version = "0.3.0"'],
      ["fuzz/Cargo.lock", 'version = "0.3.0"'],
    ]) {
      assert.ok((await text(root, relative)).includes(expected), `${relative} missing ${expected}`);
    }

    await assert.rejects(readFile(join(root, "CHANGELOG.md")), { code: "ENOENT" });
  });

  /** Verify release preparation preserves published tag pins and rejects a mismatched commit. */
  test("keeps published Action pins through a version bump and verifies their Git tags", async () => {
    const root = await fixture();
    const env = Object.fromEntries(Object.entries(childEnv).filter(([key]) => !/^GIT_/i.test(key)));
    /** Runs fixture Git with an isolated hook path and an explicit identity for the test commit. */
    const git = (args) => {
      const result = spawnSync("git", ["-c", `core.hooksPath=${join(root, "no-hooks")}`, ...args], { cwd: root, env, encoding: "utf8" });
      assert.equal(result.status, 0, result.stderr);
      return result.stdout.trim();
    };
    git(["init", "--quiet"]);
    git(["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "-c", "commit.gpgsign=false", "commit", "--allow-empty", "--quiet", "-m", "published release"]);
    const sha = git(["rev-parse", "HEAD"]);
    const examples = ["README.md", "docs/ci-and-baselines.md"];
    /** Creates a complete Action example pinned to the supplied commit at the published version. */
    const example = (commit) => `- uses: figueiredoluiz/cuke-dedup@${commit} # v0.2.0\n`;
    for (const path of examples) await writeFile(join(root, path), example(sha));
    const strict = { REQUIRE_ACTION_PIN_TAGS: "true" };
    const local = checkVersions(root);
    assert.equal(local.status, 0, local.stderr);
    const missingTag = checkVersions(root, strict);
    assert.equal(missingTag.status, 1, missingTag.stderr);
    assert.match(missingTag.stderr, /Action pin tag v0\.2\.0 is required/);
    git(["-c", "tag.gpgSign=false", "tag", "v0.2.0"]);
    const prepared = bump(root, "0.3.0");
    assert.equal(prepared.status, 0, prepared.stderr);
    const valid = checkVersions(root, strict);
    assert.equal(valid.status, 0, valid.stderr);
    assert.match(valid.stdout, /release versions agree at 0\.3\.0/);
    for (const path of examples) {
      assert.equal(await text(root, path), example(sha));
      await writeFile(join(root, path), example("0".repeat(40)));
    }
    for (const invalid of [checkVersions(root, strict), bump(root, "0.4.0")]) {
      assert.equal(invalid.status, 1, invalid.stderr);
      assert.match(invalid.stderr, /Action commit pin does not match v0\.2\.0/);
    }
  });

  /** Verify pin checks respect project file selection and surface Git discovery errors. */
  test("checks project pins independently of ignored references and Git environment overrides", async () => {
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
      assert.ok((await text(root, path)).includes(actionPin), path);
    }
    for (const path of ignored) assert.equal(await text(root, path), note);
    const index = join(root, ".git/index");
    await writeFile(index, "invalid index");
    const failed = bump(root, "0.4.0");
    assert.equal(failed.status, 1);
    assert.match(failed.stderr, /Git version-site discovery failed/);
    assert.ok((await text(root, "Cargo.toml")).includes('version = "0.3.0"'));
  });

  /** Verify an invalid Action use in a newly added guide prevents version edits. */
  test("rejects Action examples without a full release commit and version comment", async () => {
    const root = await fixture();
    const example = join(root, "docs/example.md");
    const invalidPins = [
      ...["v0.2.0", "main", "0123456", actionPin.split(" # ")[0].split("@")[1]].map((ref) => ["uses", ref]),
      ['"uses"', "main"],
      ["'uses'", "main"],
    ];
    for (const [key, ref] of invalidPins) {
      await writeFile(example, `  - ${key}: figueiredoluiz/cuke-dedup@${ref}\n`);
      const result = bump(root, "0.3.0");
      assert.equal(result.status, 1, `${key}: ${ref}`);
      assert.match(result.stderr, /example\.md.*full commit SHA.*version comment/);
      const checked = checkVersions(root);
      assert.equal(checked.status, 1, checked.stderr);
      assert.match(checked.stderr, /example\.md.*full commit SHA.*version comment/);
      assert.match(await text(root, "Cargo.toml"), /version = "0\.2\.0"/);
    }
    await writeFile(example, ["uses", '"uses"', "'uses'"].map((key) =>
      `  - ${key}: figueiredoluiz/${actionPin}\n`,
    ).join(""));
    const checked = checkVersions(root);
    assert.equal(checked.status, 0, checked.stderr);
    assert.match(checked.stdout, /^release versions agree at 0\.2\.0$/m);
    assert.equal(bump(root, "0.3.0").status, 0);
  });

  /** Verify Action reference validation follows the literal value across YAML scalar forms. */
  test("validates Action pins in block, flow, and anchored YAML values", async () => {
    const root = await fixture();
    const example = join(root, "docs/example.md");
    const [pin, version] = actionPin.split(" # ");
    const layouts = [
      (ref, tag) => `- uses: >-${tag}\r\n    ${ref}\r\n`,
      (ref, tag) => `- 'uses': |2-${tag}\n    ${ref}\n`,
      (ref, tag) => `- "uses": &pin !!str >-2${tag}\n    ${ref}\n`,
      (ref, tag) => `- { uses: "${ref}" }${tag}\n`,
      (ref, tag) => `- { id: scan, uses: "${ref}", name: Tom's scan }${tag}\n`,
      (ref, tag) => `- { uses: "${ref}", name: "A \\"scan\\" # v9.9.9 example" }${tag}\n`,
      (ref, tag) => `- { uses: '${ref}', name: 'A ''scan'' # v9.9.9 example', id: scan }${tag}\n`,
      (ref, tag) => `- uses: &pin '${ref}'${tag}\n`,
    ];
    for (const exampleFor of layouts) {
      for (const [ref, tag] of [["cuke-dedup@main", ` # ${version}`], [pin, ""]]) {
        const invalid = exampleFor(`figueiredoluiz/${ref}`, tag);
        await writeFile(example, invalid);
        for (const result of [bump(root, "0.3.0"), checkVersions(root)]) {
          assert.equal(result.status, 1, invalid);
          assert.match(result.stderr, /example\.md.*full commit SHA.*version comment/);
        }
      }
      const valid = exampleFor(`figueiredoluiz/${pin}`, ` # ${version}`);
      await writeFile(example, valid);
      const checked = checkVersions(root);
      assert.equal(checked.status, 0, checked.stderr);
    }
    await writeFile(example, `- uses: another-owner/cuke-dedup@main\n- uses: otherfigueiredoluiz/cuke-dedup@main\nSee https://example.test/figueiredoluiz/cuke-dedup@main.\n${layouts[0](`figueiredoluiz/${pin}`, ` # ${version}`)}`);
    assert.equal(bump(root, "0.3.0").status, 0);
  });

  /** Verify a Markdown inline snippet carries its own Action reference and annotation. */
  test("validates Action pins in Markdown inline code spans", async () => {
    const [pin, version] = actionPin.split(" # ");
    for (const delimiter of ["`", "``"]) {
      /** Wraps an Action reference and its optional version comment in the current delimiter. */
      const snippet = (ref, tag) => `${delimiter}uses: figueiredoluiz/${ref}${tag}${delimiter}`;
      const adjacent = `${delimiter}# ${version}${delimiter}`;
      for (const [text, code] of [
        [`See ${adjacent} and ${snippet(pin, ` # ${version}`)}.`, 0],
        [`See ${snippet(pin, "")} # ${version}`, 0],
        [`See ${snippet(pin, "")} and ${adjacent}.`, 1],
        [`See ${snippet("cuke-dedup@main", ` # ${version}`)}.`, 1],
        [`Example: ${snippet(pin, ` # ${version}`)}`, 0],
        [`- "Example": ${snippet(pin, "")} # ${version}`, 0],
        [`Example: ${snippet(pin, "")} and ${adjacent}.`, 1],
        [`Example: ${snippet("cuke-dedup@main", ` # ${version}`)}`, 1],
        [`\`\`\`yaml\nExample: ${snippet(pin, ` # ${version}`)}\n\`\`\``, 1],
        [`    Example: ${snippet(pin, ` # ${version}`)}`, 1],
        [`uses: figueiredoluiz/${pin} extra # ${version} ${adjacent}`, 1],
      ]) {
        const root = await fixture();
        await writeFile(join(root, "docs/example.md"), `${text}\n`);
        for (const result of [checkVersions(root), bump(root, "0.3.0")]) {
          assert.equal(result.status, code, `${text}\n${result.stderr}`);
        }
      }
    }
  });

  /** Verify equivalent quoted coordinates pass and encoded invalid pins fail both version commands. */
  test("checks escaped Action coordinates in double-quoted YAML values", async () => {
    const root = await fixture();
    const workflow = join(root, ".github/workflows/example.yml");
    const guide = join(root, "docs/example.md");
    const [coordinate, version] = `figueiredoluiz/${actionPin}`.split(" # ");
    const sha = coordinate.split("@")[1];
    const encoded = String.raw`\x66igueiredoluiz\/cuke-dedup\u0040`;
    const invalid = [
      `- uses: "figueiredoluiz/cuke-dedup\\u0040main" # ${version}`,
      `- uses: "${encoded}main" # ${version}`,
      `- {id: scan, "uses": "figueiredoluiz/cuke-dedup\\U00000040${sha}\\x23extra"} # ${version}`,
      `- uses: "figueiredoluiz/cuke-\\\n    dedup@main" # ${version}`,
      `- {uses: "${encoded}${sha}", name: "example # ${version}"}`,
    ];
    for (const line of invalid) {
      await writeFile(workflow, `${line}\n`);
      for (const result of [checkVersions(root), bump(root, "0.3.0")]) {
        assert.equal(result.status, 1, `${line}\n${result.stderr}`);
        assert.match(result.stderr, /example\.yml.*full commit SHA.*version comment/);
      }
    }
    await writeFile(workflow, `- uses: "${encoded}${"f".repeat(40)}" # ${version}\n`);
    for (const result of [checkVersions(root), bump(root, "0.3.0")]) {
      assert.equal(result.status, 1, result.stderr);
      assert.match(result.stderr, /conflicting Action commit pins for v0\.2\.0/);
    }
    await writeFile(workflow, [
      `- uses: "${encoded}${sha}" # ${version}`,
      `- uses: "figueiredoluiz/cuke-\\\r\n    dedup@${sha}" # ${version}`,
      `- name: 'figueiredoluiz/cuke-dedup\\u0040main'`,
      `- name: "figueiredoluiz/cuke-dedup\\\\u0040main"`,
      `- uses: "another-owner/cuke-dedup\\u0040main"`,
      `# Example text "${encoded}main"`,
      "",
    ].join("\n"));
    await writeFile(guide, `See \`# comment\` and \`uses: "${encoded}main" # ${version}\`.\n`);
    for (const result of [checkVersions(root), bump(root, "0.3.0")]) {
      assert.equal(result.status, 1, result.stderr);
      assert.match(result.stderr, /example\.md.*full commit SHA.*version comment/);
    }
    await writeFile(guide, `See \`uses: "figueiredoluiz/cuke-dedup\\U00000040${sha}" # ${version}\`.\n`);
    const checked = checkVersions(root);
    assert.equal(checked.status, 0, checked.stderr);
    assert.equal(bump(root, "0.3.0").status, 0);
  });

  /** Verify unfinished quoted prose still allows the version check to reach subsequent examples. */
  test("checks Action pins after unfinished quoted prose", async () => {
    const root = await fixture();
    const guide = join(root, "docs/example.md");
    const continuation = `\\\n${" ".repeat(20)}`;
    const prefix = `Example: "${continuation.repeat(12)}\n\n`;
    for (const [reference, status] of [[actionPin, 0], ["cuke-dedup@main # v0.2.0", 1]]) {
      await writeFile(guide, `${prefix}- uses: figueiredoluiz/${reference}\n`);
      const result = checkVersions(root, {}, 10_000);
      assert.ifError(result.error);
      assert.equal(result.status, status, result.stderr);
      if (status === 0) {
        assert.match(result.stdout, /^release versions agree at 0\.2\.0$/m);
      } else {
        assert.match(result.stderr, /example\.md.*full commit SHA.*version comment/);
      }
    }
  });

  /** Verify a scalar's final newline and indentation are part of its decoded Action value. */
  test("checks line endings in block scalar Action pins", async () => {
    const root = await fixture();
    const workflow = join(root, ".github/workflows/example.yml");
    const [pin, version] = `figueiredoluiz/${actionPin}`.split(" # ");
    for (const style of ["|", ">"]) {
      for (const [indicator, tail, valid] of [
        ["", "", true],
        ["+", "", true],
        ["", "\n", false],
        ["+", "\r\n\r\n", false],
        ["-", "\r\n\r\n", true],
        ["2-", "\n  \n", true],
        ["-2", "\n    \n", false],
        ["-", "\n  \t\n", false],
        ["-", "\n  # scalar text\n", false],
        ["-", "\n# outside the scalar\n", true],
      ]) {
        const source = `uses: ${style}${indicator} # ${version}\n  ${pin}${tail}`;
        await writeFile(workflow, source);
        const result = checkVersions(root);
        assert.equal(result.status, valid ? 0 : 1, JSON.stringify(source));
        if (valid) {
          assert.match(result.stdout, /release versions agree at 0\.2\.0/);
        } else {
          assert.match(result.stderr, /example\.yml.*full commit SHA.*version comment/);
        }
      }
    }
  });

  /** Verify a full Action value can contain only the pinned reference across YAML scalar styles. */
  test("validates the complete Action scalar before accepting its pin", async () => {
    const root = await fixture();
    const workflow = join(root, ".github/workflows/example.yml");
    const guide = join(root, "docs/example.md");
    const [pin, version] = `figueiredoluiz/${actionPin}`.split(" # ");
    const annotation = ` # ${version}`;
    for (const [path, source, valid] of [
      [workflow, `- uses: '${pin}'${annotation}`, true],
      [workflow, `- { uses: "${pin}", id: scan }${annotation}`, true],
      [workflow, `- { uses: ${pin}, id: scan }${annotation}`, true],
      [workflow, `- {\n    uses: ${pin},${annotation}\n    id: scan\n  }`, true],
      [workflow, `- { uses:\n    ${pin},${annotation}\n    id: scan }`, true],
      [workflow, `- { uses:\n    ${pin} extra,${annotation}\n    id: scan }`, false],
      [workflow, `- { uses: # on the next line\n    ${pin}#extra,${annotation}\n    id: scan }`, false],
      [guide, `\`\`\`yaml\n- { uses:\n    ${pin} extra,${annotation}\n    id: scan }\n\`\`\``, false],
      [guide, `- { uses:\n  ${pin},${annotation}\n  id: scan }`, true],
      [guide, `- { uses:\n  ${pin} extra,${annotation}\n  id: scan }`, false],
      [workflow, `- uses: ${pin}${annotation}\n  id: scan`, true],
      [workflow, `- uses:\n    ${pin}${annotation}\n  id: scan`, true],
      [workflow, `- uses: >-${annotation}\n    ${pin}\n  id: scan`, true],
      [workflow, `- 'uses': &pin !!str |2-${annotation}\n    ${pin}\n  id: scan`, true],
      [workflow, `- "uses" : >-${annotation}\n    ${pin}`, true],
      [guide, `See \`uses: "${pin}"${annotation}\`.`, true],
      [workflow, `- uses: "${pin}#extra"${annotation}`, false],
      [workflow, `- { uses: '${pin},extra', id: scan }${annotation}`, false],
      [workflow, `- uses: '${pin} '${annotation}`, false],
      [workflow, `- uses: "${pin}\\x2cextra"${annotation}`, false],
      [guide, `See \`uses: "${pin}]"${annotation}\`.`, false],
      [guide, `\`\`\`yaml\n- uses: '${pin}#extra'${annotation}\n\`\`\``, false],
      [workflow, `- uses: ${pin} extra${annotation}`, false],
      [workflow, `- uses : ${pin} extra${annotation}`, false],
      [workflow, `- "uses" : >-${annotation}\n    ${pin}\n    extra`, false],
      [workflow, `- uses: ${pin}#extra${annotation}`, false],
      [workflow, `- uses: ${pin},extra${annotation}`, false],
      [workflow, `- { uses: ${pin} extra, id: scan }${annotation}`, false],
      [workflow, `- {\n    uses: ${pin} extra,${annotation}\n    id: scan\n  }`, false],
      [workflow, `- uses:\n    ${pin} extra${annotation}`, false],
      [workflow, `- uses: |2-${annotation}\n    ${pin}\n    # scalar content`, false],
      [workflow, `- uses: >-${annotation}\n    ${pin}\n\n    extra`, false],
      [workflow, `- uses: >-\n    extra\n    ${pin}${annotation}`, false],
      [guide, `See \`uses: ${pin} extra${annotation}\`.`, false],
      [guide, `\`\`\`yaml\n- uses: >-${annotation}\n    ${pin}\n    extra\n\`\`\``, false],
    ]) {
      await writeFile(path, `${source}\n`);
      const result = checkVersions(root);
      if (valid) {
        assert.equal(result.status, 0, result.stderr);
        assert.match(result.stdout, /release versions agree at 0\.2\.0/);
      } else {
        for (const invalid of [result, bump(root, "0.3.0")]) {
          assert.equal(invalid.status, 1, source);
          assert.match(invalid.stderr, /example\.(?:md|yml).*full commit SHA.*version comment/);
        }
      }
      await writeFile(path, "");
    }
    assert.match(await text(root, "Cargo.toml"), /version = "0\.2\.0"/);
  });

  /** Verify punctuation is accepted around prose pins while code retains its exact spelling. */
  test("distinguishes Markdown prose punctuation from Action pin contents", async () => {
    const root = await fixture();
    const guide = join(root, "docs/example.md");
    const workflow = join(root, ".github/workflows/example.yml");
    const [pin, version] = `figueiredoluiz/${actionPin}`.split(" # ");
    for (const [path, source] of [
      [guide, `See (figueiredoluiz/cuke-dedup@main) # ${version}.`],
      [guide, `See \`uses: ${pin}) # ${version}\`.`],
      [guide, `\`\`\`yaml\n- uses: ${pin} # ${version}.\n\`\`\``],
      [guide, `~~~yaml\n- uses: ${pin}. # ${version}\n~~~`],
      [guide, `    uses: ${pin}; # ${version}`],
      [guide, `- { uses: ${pin}) } # ${version}`],
      [workflow, `- uses: ${pin}) # ${version}`],
    ]) {
      await writeFile(path, `${source}\n`);
      for (const result of [checkVersions(root), bump(root, "0.3.0")]) {
        assert.equal(result.status, 1, source);
        assert.match(result.stderr, /example\.(?:md|yml).*full commit SHA.*version comment/);
      }
      await writeFile(path, "");
    }
    await writeFile(workflow, `- uses: ${pin} # ${version}\n`);
    await writeFile(guide, [
      `\`\`\`\`yaml\n- uses: ${pin} # ${version}\n\`\`\`\``,
      `See (${pin}) # ${version}; continue here.`,
      `See ${pin}. # ${version}.`,
      `See (\`uses: ${pin}\` # ${version}).`,
      "",
    ].join("\n"));
    const checked = checkVersions(root);
    assert.equal(checked.status, 0, checked.stderr);
    assert.equal(bump(root, "0.3.0").status, 0);
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
      assert.ok(raw.includes(relative === "README.md" ? actionPin : "0.3.0"), relative);
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
