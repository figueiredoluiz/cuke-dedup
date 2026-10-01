import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdir, mkdtemp, rm, unlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { compareInventory, inventorySources } from "../check-language-boundary.mjs";

const inventory = (path, source) => inventorySources({ [path]: source })[path];

test("qualified references record canonical adapter and framework edges", () => {
  assert.deepEqual(inventory("src/analysis.rs", `
    fn analyze() {
      crate::ruby::extract();
      crate::typescript::extract();
      let _framework = crate::model::Framework::CucumberRuby;
    }
  `), {
    "crate::model::Framework::CucumberRuby": 1,
    "crate::ruby": 1,
    "crate::typescript": 1,
  });
});

test("raw identifiers in tracked path segments and aliases retain canonical edges", () => {
  const cases = [
    ["crate::r#ruby::extract();", { "crate::ruby": 1 }],
    ["crate::model::r#Framework::CucumberRuby;", { "crate::model::Framework::CucumberRuby": 1 }],
    ["use crate::r#ruby as r#adapter; r#adapter::extract();", { "crate::ruby": 2 }],
  ];
  for (const [source, expected] of cases) {
    assert.deepEqual(inventory("src/analysis.rs", source), expected, source);
  }
});

test("relative paths cannot hide adapter or framework dependencies", () => {
  const cases = [
    ["super::ruby::extract();", { "crate::ruby": 1 }],
    ["self::typescript::extract();", { "crate::typescript": 1 }],
    ["use super::ruby as rb; rb::extract();", { "crate::ruby": 2 }],
    ["use super as parent; parent::ruby::extract();", { "crate::ruby": 1 }],
    ["mod nested { super::super::ruby::extract(); }", { "crate::ruby": 1 }],
    ["use super::model::Framework as F; F::CucumberRuby;", { "crate::model::Framework": 1, "crate::model::Framework::CucumberRuby": 1 }],
    ["self::Framework::JestCucumber;", { "crate::model::Framework::JestCucumber": 1 }],
    ["super::utilities::helper();", {}],
  ];
  for (const path of ["src/cli.rs", "src/analysis/mod.rs", "src/analysis/pairs.rs"]) {
    for (const [source, expected] of cases) assert.deepEqual(inventory(path, source), expected, `${path}: ${source}`);
  }
});

test("grouped imports and aliases count each imported leaf and each alias use", () => {
  const edges = inventory("src/analysis.rs", `
    use crate::ruby::{self as ruby, assertions::trust as trust_assertions};
    use crate::typescript::{self as ts, semantic::extract as extract_ts};
    use crate::model::Framework as F;
    use crate::model::Framework::{CucumberRuby as Ruby, JestCucumber};

    fn analyze(framework: F) {
      ruby::extract();
      trust_assertions();
      ts::extract();
      extract_ts();
      match framework {
        F::CucumberRuby | Ruby | JestCucumber => {}
        _ => {}
      }
    }
  `);
  assert.deepEqual(edges, {
    "crate::model::Framework": 2,
    "crate::model::Framework::CucumberRuby": 3,
    "crate::model::Framework::JestCucumber": 2,
    "crate::ruby": 4,
    "crate::typescript": 4,
  });
});

test("ancestor aliases resolve through crate and model namespaces", () => {
  assert.deepEqual(inventory("src/analysis.rs", `
    use crate as root;
    use crate::model as m;
    root::ruby::extract();
    let _framework = m::Framework::CucumberRuby;
  `), {
    "crate::model::Framework::CucumberRuby": 1,
    "crate::ruby": 1,
  });
});

test("aliases resolve when their namespace import appears later", () => {
  assert.deepEqual(inventory("src/analysis.rs", `
    use rb::extract as f;
    use crate::ruby as rb;
    f();
  `), { "crate::ruby": 3 });
});

test("only import items treat use as a keyword", () => {
  for (const source of [
    "fn borrow<'a>(x: &'a str) -> impl Sized + use<'a> { x } use crate::ruby as rb; rb::extract();",
    "fn r#use() {} r#use(); crate::ruby::one(); crate::ruby::two();",
    "use crate::ruby as r#use; r#use::extract();",
    "#[allow(unused)] pub use crate::ruby as rb; rb::extract();",
    "pub(crate) use crate::ruby as rb; rb::extract();",
    "pub(in crate::analysis) use crate::ruby as rb; rb::extract();",
  ]) assert.deepEqual(inventory("src/analysis.rs", source), { "crate::ruby": 2 }, source);
});

test("comments, ordinary strings, raw strings and chars do not create edges", () => {
  const edges = inventory("src/analysis.rs", String.raw`
    // crate::ruby::commented(); crate::typescript::commented();
    /* crate::model::Framework::JestCucumber */
    const TEXT: &str = "crate::ruby::string crate::typescript::string";
    const RAW: &str = r###"crate::model::Framework::CucumberRuby"###;
    const CHAR: char = 'x';
    fn accepts<'a>(value: &'a str) { let _ = value; }
  `);
  assert.deepEqual(edges, {});
});

test("adding an edge to a file already coupled to an adapter is still a regression", () => {
  const baseline = { "src/analysis.rs": { "crate::ruby": 1 } };
  const actual = inventorySources({ "src/analysis.rs": `
    crate::ruby::extract();
    crate::typescript::extract();
  ` });
  const differences = compareInventory(actual, baseline);
  assert.ok(differences.some((item) => item.includes("src/analysis.rs") && item.includes("crate::typescript")));
});

test("removing a recorded edge requires tightening the baseline", () => {
  const baseline = { "src/analysis.rs": { "crate::ruby": 2 } };
  const actual = inventorySources({ "src/analysis.rs": "crate::ruby::extract();" });
  const differences = compareInventory(actual, baseline);
  assert.ok(differences.some((item) => item.includes("src/analysis.rs") && item.includes("crate::ruby")));
});

test("new and missing scoped files are both reported", () => {
  const baseline = {
    "src/analysis/old.rs": { "crate::ruby": 1 },
    "src/analysis/steady.rs": { "crate::typescript": 1 },
  };
  const actual = {
    "src/analysis/new.rs": { "crate::ruby": 1 },
    "src/analysis/steady.rs": { "crate::typescript": 1 },
  };
  const differences = compareInventory(actual, baseline);
  assert.ok(differences.some((item) => item.includes("src/analysis/new.rs")));
  assert.ok(differences.some((item) => item.includes("src/analysis/old.rs")));
});

test("unsupported imports under a tracked framework path fail closed", () => {
  assert.throws(() => inventory("src/analysis.rs", `
    use crate::model::Framework::*;
  `));
});

test("CLI uses the selected root from another cwd and fails on missing inputs", async () => {
  const workspace = await mkdtemp(join(tmpdir(), "language-boundary-test-"));
  const root = join(workspace, "repository");
  const checker = fileURLToPath(new URL("../check-language-boundary.mjs", import.meta.url));
  const modules = ["analysis", "cli", "discovery", "config", "source_filter", "model"];
  const exclusions = {
    "src/lib.rs": "composition root", "src/main.rs": "composition root",
    "src/source_adapter.rs": "adapter routing/session composition",
    "src/source_adapter/": "adapter routing tests/support",
    "src/typescript.rs": "language frontend", "src/typescript/": "language frontend",
    "src/ruby.rs": "language frontend", "src/ruby/": "language frontend",
  };
  const manifestPath = join(root, "scripts/check/language-boundary.json");
  const run = () => spawnSync(process.execPath, [checker, "--root", root], {
    cwd: workspace, encoding: "utf8", timeout: 10_000, maxBuffer: 1024 * 1024,
  });
  try {
    for (const module of modules) {
      const sourcePath = join(root, `src/${module}.rs`);
      await mkdir(join(root, "src"), { recursive: true });
      await writeFile(sourcePath, "");
    }
    await mkdir(join(root, "scripts/check"), { recursive: true });
    await writeFile(manifestPath, `${JSON.stringify({
      schemaVersion: 1,
      files: Object.fromEntries(modules.map((module) => [`src/${module}.rs`, {}])),
      exclusions,
    })}\n`);

    const passing = run();
    assert.equal(passing.status, 0, passing.stderr);
    assert.match(passing.stdout, /6 files match the reviewed inventory/);

    const nested = join(root, "src/analysis/new.rs");
    await mkdir(join(root, "src/analysis"), { recursive: true });
    await writeFile(nested, "crate::ruby::extract();");
    const addedSource = run();
    assert.equal(addedSource.status, 1);
    assert.match(addedSource.stderr, /src\/analysis\/new\.rs: new scoped file/);
    await unlink(nested);
    await writeFile(join(root, "src/analysis/tests.rs"), "crate::ruby::extract();");
    assert.equal(run().status, 0, "dedicated test files are outside the boundary");

    await writeFile(join(root, "src/analysis.rs"), "crate::ruby::extract();");
    assert.equal(run().status, 1, "the CLI must reject a new dependency");
    await writeFile(join(root, "src/analysis.rs"), "");

    await unlink(join(root, "src/config.rs"));
    const missingSource = run();
    assert.equal(missingSource.status, 1);
    assert.match(missingSource.stderr, /src[\\/]config\.rs/);

    await unlink(manifestPath);
    const missingManifest = run();
    assert.equal(missingManifest.status, 1);
    assert.match(missingManifest.stderr, /language-boundary\.json/);
  } finally {
    await rm(workspace, { recursive: true, force: true });
  }
});
