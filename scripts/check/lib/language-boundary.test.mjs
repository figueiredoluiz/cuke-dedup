import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmod, mkdir, mkdtemp, readFile, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { classify, diffRows, inventorySources, render, rows, totals, validateManifest } from "../check-language-boundary.mjs";

// Matrix: edge kind (module, framework, language, domain, marker, name, key, grammar) x location
// (code, line/doc/block comment, string, raw string, char, inline cfg(test) module, nested module,
// alias, glob) -> counted or ignored, each ignored row paired with a counted control of the same
// text. File classes (core, registry, adapter, exempt, test, asset, unclassified), baseline
// behaviours (growth, stale, file removed, new file, malformed) and --update run through the CLI.

/** Inventories every edge kind in one synthetic Rust source. */
const full = (path, source) => inventorySources({ [path]: source })[path];
/** Inventories module, framework, language, domain and grammar edges, dropping the overlapping name and key kinds. */
const inventory = (path, source) => Object.fromEntries(Object.entries(full(path, source)).filter(([key]) => !/^(name|key):/.test(key)));

/** Checks qualified adapter and framework references produce canonical dependency counts. */
test("qualified references record canonical adapter and framework edges", () => {
  assert.deepEqual(inventory("src/analysis.rs", `
    fn analyze() {
      crate::ruby::extract();
      crate::typescript::extract();
      let _framework = Framework::CucumberRuby;
    }
  `), {
    "Framework::CucumberRuby": 1,
    "crate::ruby": 1,
    "crate::typescript": 1,
  });
});

/** Checks raw identifiers and aliases retain canonical adapter and framework edges. */
test("raw identifiers in tracked path segments and aliases retain canonical edges", () => {
  const cases = [
    ["crate::r#ruby::extract();", { "crate::ruby": 1 }],
    ["crate::model::r#Framework::CucumberRuby;", { "Framework::CucumberRuby": 1 }],
    ["use crate::r#ruby as r#adapter; r#adapter::extract();", { "crate::ruby": 2 }],
  ];
  for (const [source, expected] of cases) {
    assert.deepEqual(inventory("src/analysis.rs", source), expected, source);
  }
});

/** Checks raw keyword identifiers remain import components and aliases rather than syntax keywords. */
test("raw keyword import components remain identifiers", () => {
  for (const [source, expected] of [
    ["use crate::r#as::extract;", {}],
    ["use crate::ruby::r#as; r#as();", { "crate::ruby": 2 }],
    ["use crate::ruby::r#as as r#type; r#type();", { "crate::ruby": 2 }],
    ["use crate::ruby::{r#as as renamed, r#use}; renamed(); r#use();", { "crate::ruby": 4 }],
    ["use crate::ruby::{self as r#as}; r#as::extract();", { "crate::ruby": 2 }],
    ["use crate::ruby as r#as; r#as::extract();", { "crate::ruby": 2 }],
  ]) assert.deepEqual(inventory("src/analysis.rs", source), expected, source);
});

/** Checks relative paths and aliases cannot hide adapter or framework dependencies at different source depths. */
test("relative paths cannot hide adapter or framework dependencies", () => {
  const cases = [
    ["super::ruby::extract();", { "crate::ruby": 1 }],
    ["self::typescript::extract();", { "crate::typescript": 1 }],
    ["use super::ruby as rb; rb::extract();", { "crate::ruby": 2 }],
    ["use super as parent; parent::ruby::extract();", { "crate::ruby": 1 }],
    ["mod nested { super::super::ruby::extract(); }", { "crate::ruby": 1 }],
    ["use super::model::Framework as F; F::CucumberRuby;", { "Framework": 1, "Framework::CucumberRuby": 1 }],
    ["self::Framework::JestCucumber;", { "Framework::JestCucumber": 1 }],
    ["super::utilities::helper();", {}],
  ];
  for (const path of ["src/cli.rs", "src/analysis/mod.rs", "src/analysis/pairs.rs"]) {
    for (const [source, expected] of cases) assert.deepEqual(inventory(path, source), expected, `${path}: ${source}`);
  }
});

/** Checks grouped imports count each tracked leaf and subsequent alias use. */
test("grouped imports and aliases count each imported leaf and each alias use", () => {
  const edges = inventory("src/analysis.rs", `
    use crate::ruby::{self as ruby, assertions::trust as trust_assertions};
    use crate::typescript::{self as ts, semantic::extract as extract_ts};
    use Framework as F;
    use Framework::{CucumberRuby as Ruby, JestCucumber};

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
    "Framework": 2,
    "Framework::CucumberRuby": 3,
    "Framework::JestCucumber": 2,
    "crate::ruby": 4,
    "crate::typescript": 4,
  });
});

/** Checks crate and model namespace aliases resolve to canonical tracked edges. */
test("ancestor aliases resolve through crate and model namespaces", () => {
  assert.deepEqual(inventory("src/analysis.rs", `
    use crate as root;
    use crate::model as m;
    root::ruby::extract();
    let _framework = m::Framework::CucumberRuby;
  `), {
    "Framework::CucumberRuby": 1,
    "crate::ruby": 1,
  });
});

/** Checks alias resolution handles namespace imports declared later in the source. */
test("aliases resolve when their namespace import appears later", () => {
  assert.deepEqual(inventory("src/analysis.rs", `
    use rb::extract as f;
    use crate::ruby as rb;
    f();
  `), { "crate::ruby": 3 });
});

/** Checks use keywords are recognized only in import items, preserving raw identifiers and precise-capture syntax. */
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

/** Checks comments and literal contents do not create dependency edges. */
test("comments, ordinary strings, raw strings and chars do not create edges", () => {
  const edges = inventory("src/analysis.rs", String.raw`
    // crate::ruby::commented(); crate::typescript::commented();
    /* Framework::JestCucumber */
    const TEXT: &str = "crate::ruby::string crate::typescript::string";
    const RAW: &str = r###"Framework::CucumberRuby"###;
    const CHAR: char = 'x';
    fn accepts<'a>(value: &'a str) { let _ = value; }
  `);
  assert.deepEqual(edges, {});
});

/** Checks each edge kind is counted in code and ignored in comments and prose strings, with a counted control for every ignored row. */
test("edge kinds x locations: counted in code, ignored in comments and prose strings", () => {
  const ruby = { "crate::ruby": 1, "name:ruby": 1 };
  const table = [
    ["module: code", "crate::ruby::extract();", ruby],
    ["module: line comment", "// crate::ruby::extract();", {}],
    ["module: doc comment", "/// crate::ruby::extract();\nfn f() {}", {}],
    ["module: nested block comment", "/* /* crate::ruby::extract(); */ */", {}],
    ["module: prose string", 'let _ = "call crate::ruby::extract();";', {}],
    ["module: raw string", 'let _ = r#"call crate::ruby::extract();"#;', {}],
    ["module: nested module path", "mod a { mod b { super::super::ruby::extract(); } }", ruby],
    ["module: inline cfg(test) module is counted", "#[cfg(test)] mod tests { use super::*; fn t() { crate::ruby::extract(); } }", ruby],
    ["framework: code", "let _ = Framework::CucumberRuby;", { "Framework::CucumberRuby": 1, "name:ruby": 1 }],
    ["framework: prose string", 'let _ = "Framework::CucumberRuby is gone";', {}],
    ["language: code", "let _ = SourceLanguage::Ruby;", { "SourceLanguage::Ruby": 1, "name:ruby": 1 }],
    ["language: alias", "use SourceLanguage as L; let _ = L::Ruby;", { SourceLanguage: 1, "SourceLanguage::Ruby": 1, "name:ruby": 1 }],
    ["language: grouped alias", "use crate::a::{SourceLanguage as L}; match x { L::Tsx => {} }", { SourceLanguage: 1, "SourceLanguage::Tsx": 1 }],
    ["language: Self variants inside impl", "impl super::SourceLanguage { fn d(self) { match self { Self::Ruby => 1, Self::Tsx => 2 } } }", { SourceLanguage: 1, "SourceLanguage::Ruby": 1, "SourceLanguage::Tsx": 1, "name:ruby": 1 }],
    ["language: Self inside trait impl for tracked type", "impl Display for SourceLanguage { fn f() { Self::Ruby; } } fn g() { Self::Tsx; }", { SourceLanguage: 1, "SourceLanguage::Ruby": 1, "name:ruby": 1 }],
    ["language: Self in impl of an adapter module type", "impl crate::ruby::Handler { fn f() { Self::Ruby; } }", { "crate::ruby": 1, "name:ruby": 2 }],
    ["language: raw impl identifier is not an impl block", "fn f() {} r#impl SourceLanguage { Self::Ruby; }", { SourceLanguage: 1, "name:ruby": 1 }],
    ["language: Self in impl naming a tracked generic argument is counted (upper bound)", "impl From<SourceLanguage> for Wrapper { fn f() { Self::Ruby; } }", { SourceLanguage: 1, "SourceLanguage::Ruby": 1, "name:ruby": 1 }],
    ["language: Self in impl naming two tracked types counts under each", "impl From<SourceLanguage> for Framework { fn f() { Self::Jest; } }", { SourceLanguage: 1, Framework: 1, "SourceLanguage::Jest": 1, "Framework::Jest": 1, "name:jest": 1 }],
    ["language: Self in argument-position impl", "fn f(x: impl SourceLanguage) { Self::Ruby; }", { SourceLanguage: 1, "name:ruby": 1 }],
    ["language: function-local type does not hide a variant", "impl SourceLanguage { fn f() -> Self { type Tsx = u8; Self::Tsx } }", { SourceLanguage: 1, "SourceLanguage::Tsx": 1 }],
    ["language: nested impl resolves against its own target", "impl Framework { fn f() { impl SourceLanguage { fn g() -> Self { Self::Tsx } } } }", { Framework: 1, SourceLanguage: 1, "SourceLanguage::Tsx": 1 }],
    ["language: Self in a nested impl is counted once", "impl Display for Framework { fn f() { impl Z for Framework { fn g() { Self::Q; } } Self::P; } }", { Framework: 2, "Framework::Q": 1, "Framework::P": 1 }],
    ["language: Self in an impl after a tracked impl is counted", "impl A for Framework { fn f() { Self::P; } } impl B for Framework { fn g() { Self::Q; } }", { Framework: 2, "Framework::P": 1, "Framework::Q": 1 }],
    ["language: Self in impl for a reference to a tracked type", "impl Tr for &SourceLanguage { fn f() { Self::Tsx; } }", { SourceLanguage: 1, "SourceLanguage::Tsx": 1 }],
    ["language: Self in impl with where clause and fn bound", "impl<F: Fn() -> u8> Tr for SourceLanguage where F: for<'a> B<'a> { fn f() { Self::Tsx; } }", { SourceLanguage: 1, "SourceLanguage::Tsx": 1 }],
    ["language: Self in generic inherent impl", "impl<T> SourceLanguage { fn f() { Self::Tsx; } }", { SourceLanguage: 1, "SourceLanguage::Tsx": 1 }],
    ["language: Self in generic impl for tracked type", "impl<T> Tr<T> for SourceLanguage { fn f() { Self::Tsx; } }", { SourceLanguage: 1, "SourceLanguage::Tsx": 1 }],
    ["language: Self associated types and consts are not variants", "impl FromStr for SourceLanguage { type Err = E; const MAX: u8 = 1; fn f() -> Self::Err { Self::MAX; Self::new(); Self::Tsx } }", { SourceLanguage: 1, "SourceLanguage::Tsx": 1 }],
    ["language: Self in impl of an untracked type", "impl Other { fn f() { Self::Ruby; } }", { "name:ruby": 1 }],
    ["language: type mention", "fn f(language: SourceLanguage) {}", { SourceLanguage: 1 }],
    ["language: line comment", "// SourceLanguage::Ruby", {}],
    ["domain: code", "let _ = ComparisonDomain::EcmaScript;", { "ComparisonDomain::EcmaScript": 1 }],
    ["domain: block comment", "/* ComparisonDomain::EcmaScript */", {}],
    ["marker: string", 'let _ = "method:ruby:complete-events";', { "marker:method:ruby": 1 }],
    ["marker: raw string", 'let _ = r"method:ruby:lexical-file";', { "marker:method:ruby": 1 }],
    ["marker: two languages in one literal", 'let _ = "method:ruby:a method:typescript:b";', { "marker:method:ruby": 1, "marker:method:typescript": 1 }],
    ["marker: line comment", '// let _ = "method:ruby:complete-events";', {}],
    ["marker: non-marker method prefix", 'let _ = "method:instance sync";', {}],
    ["marker: embedded in a word", 'let _ = "dispatch_method:get:";', {}],
    ["marker: built at runtime is not seen", 'let _ = format!("method:{}:x", lang); let _ = concat!("method:", "ruby", ":x");', { "key:ruby": 1 }],
    ["marker: uppercase name", 'let _ = "method:Ruby:x";', { "marker:method:Ruby": 1 }],
    ["marker: after a separator", 'let _ = "events=method:ruby:x";', { "marker:method:ruby": 1 }],
    ["name: function", "fn run_vitest() {}", { "name:vitest": 1 }],
    ["name: type and raw identifier", "struct CucumberJs; fn r#playwright() {}", { "name:cucumberjs": 1, "name:playwright": 1 }],
    ["name: two words in one identifier", "fn jest_to_cypress() {}", { "name:cypress": 1, "name:jest": 1 }],
    ["name: language-free identifier", "fn run_test() {}", {}],
    ["name: camelCase words joined across segments", "struct TypeScriptFrame; fn load_java_script() {}", { "name:typescript": 1, "name:javascript": 1 }],
    ["name: word only as a substring", "fn majestic() {} fn cucumber_json() {} fn r_uby() {} fn rubygems() {}", {}],
    ["name: run-together lowercase spelling is not matched", "fn rubyist() {} fn jestrunner() {}", {}],
    ["name: digit-delimited segments", "fn ruby2_keywords() {} fn Ruby3() {}", { "name:ruby": 2 }],
    ["name: inherited Object property names", "let _ = x::constructor::y; toString(); __proto__();", {}],
    ["name: comment", "// fn run_vitest() {}", {}],
    ["name: prose string", 'let _ = "fn run_vitest() {}";', {}],
    ["key: camelCase", 'let _ = "rubyLoadPaths";', { "key:ruby": 1 }],
    ["key: hyphenated", 'let _ = "cucumber-js";', { "key:cucumberjs": 1 }],
    ["key: raw string", 'let _ = r"jest";', { "key:jest": 1 }],
    ["key: prose string", 'let _ = "load ruby paths";', {}],
    ["grammar: path", "let _ = tree_sitter_ruby::LANGUAGE;", { "grammar:tree_sitter_ruby": 1, "name:ruby": 1 }],
    ["grammar: leading ::", "let _ = ::tree_sitter_typescript::LANGUAGE_TSX;", { "grammar:tree_sitter_typescript": 1, "name:typescript": 1 }],
    ["grammar: alias", "use tree_sitter_ruby as g; let _ = g::LANGUAGE;", { "grammar:tree_sitter_ruby": 2, "name:ruby": 1 }],
    ["grammar: generic tree_sitter crate", "let _: tree_sitter::Node;", {}],
    ["grammar: comment", "// use tree_sitter_ruby;", {}],
    ["grammar: local named like a grammar crate", "fn f(tree_sitter_ruby: u8) { let _ = tree_sitter_ruby; }", { "name:ruby": 2 }],
    ["grammar: bare tree_sitter_ prefix", "let tree_sitter_ = 1;", {}],
    ["same origin through two aliases is not ambiguous", "use a::SourceLanguage as x; use b::SourceLanguage as x; let _ = x::Ruby;", { SourceLanguage: 2, "SourceLanguage::Ruby": 1, "name:ruby": 1 }],
    ["unrelated ambiguous alias is ignored", "use a::b as x; use c::d as x; x::f();", {}],
  ];
  for (const [label, source, expected] of table) assert.deepEqual(full("src/analysis.rs", source), expected, label);
});

/** Checks syntax the scanner cannot model throws, so the check fails instead of passing silently. */
test("unmodeled shapes fail closed, with modeled siblings as controls", () => {
  for (const source of [
    "use crate::model::Framework::*;", "use crate::*;", "use super::*;", "use super::super::*;", "use super as parent; use parent::*;", "use super::foo::*;", "mod tests { use super::foo::*; }", "use crate as root; mod tests { use super::root::*; }", "mod a { use super::super::*; }", "fn f() { use super::*; }", "impl X { fn f() { use super::*; } }", "use self::super::*;", "use crate::ruby::*;", "use crate::typescript::*;",
    'let _ = "unterminated;', "/* unterminated", 'let _ = r#"unterminated;', "use crate::ruby",
    "use crate::ruby as x; use other as x; x::f();", "use a as b; use b as a; a::f();",
    "use crate::a::SourceLanguage as S; type L = S;", "type L = SourceLanguage;", "type A = ([u8; 2], SourceLanguage);", "type A = [SourceLanguage; 2];", "impl X for Y { type Out = Framework; }", "type R = crate::ruby::Handler;", "pub type T<A> = Option<Framework>;", "type G = tree_sitter_ruby::Language;",
  ]) assert.throws(() => full("src/analysis.rs", source), /src\/analysis\.rs/, source);
  for (const source of ["use crate::model::*;", "use crate::analysis::*;", "mod tests { use super::*; }", "mod a { mod b { use super::super::*; } }", "use self::*;", 'let _ = "terminated";', "type Id = u32; type Out<T> = Result<T, Error>; type Arr = [u8; 2];", "use crate::a::SourceLanguage; trait T { type Item; }", "use crate::ruby; type X = other::ruby::T;", "macro_rules! m { ($type:ty = SourceLanguage) => {} }", "type X = Box<dyn Fn()>; let tree_sitter_ = 1;", "// type L = SourceLanguage;", "fn r#type() {} let _ = SourceLanguage::Ruby;"]) {
    assert.doesNotThrow(() => full("src/analysis.rs", source), source);
  }
});

/** Checks every path maps to one class and unknown file types are rejected rather than skipped. */
test("path classes: core, registry, adapter, exempt, test, asset; unknown fails closed", () => {
  for (const [path, expected] of [
    ["src/analysis/", "core"], ["src/ruby/", "adapter"], ["src/analysis/tests/", "test"], ["src/analysis/pairs.rs", "core"], ["src/ruby_helpers.rs", "core"], ["src/source_adapter/config.rs", "core"],
    ["src/source_adapter.rs", "registry"],
    ["src/ruby.rs", "adapter"], ["src/ruby/bindings.rs", "adapter"], ["src/ruby/data.bin", "adapter"],
    ["src/typescript.rs", "adapter"], ["src/typescript/handler/x.rs", "adapter"],
    ["src/lib.rs", "exempt"], ["src/main.rs", "exempt"],
    ["src/analysis/tests.rs", "test"], ["src/analysis/tests/case.rs", "test"], ["src/analysis/pairs/tests.rs", "test"],
    ["src/reporters/templates/report.html", "asset"],
  ]) assert.equal(classify(path), expected, path);
  for (const path of ["src/data.json", "src/analysis/notes.txt", "src/reporters/other.html", "vendor/x.rs", "x.rs"]) assert.throws(() => classify(path), /unclassified/, path);
});

/** Checks row differences classify growth, new edges, stale counts, removed edges, removed files and new files. */
test("diffRows reports every baseline movement and nothing for equal counts", () => {
  const baseline = [["src/a.rs", "crate::ruby", 2], ["src/gone.rs", "SourceLanguage", 1], ["src/same.rs", "name:jest", 1]];
  const actual = [["src/a.rs", "crate::ruby", 3], ["src/a.rs", "grammar:tree_sitter_ruby", 1], ["src/new.rs", "crate::ruby", 1], ["src/same.rs", "name:jest", 1]];
  assert.deepEqual(diffRows(actual, baseline).map(({ file, key, was, now }) => [file, key, was, now]), [
    ["src/a.rs", "crate::ruby", 2, 3], ["src/a.rs", "grammar:tree_sitter_ruby", 0, 1],
    ["src/gone.rs", "SourceLanguage", 1, 0], ["src/new.rs", "crate::ruby", 0, 1],
  ]);
  assert.deepEqual(diffRows(baseline, baseline), []);
  assert.deepEqual(diffRows([["src/a.rs", "crate::ruby", 1]], [["src/a.rs", "crate::ruby", 2]]).map(({ was, now }) => [was, now]), [[2, 1]]);
});

/** Checks rows sort deterministically and totals sum counts per kind. */
test("rows sort by code unit and totals sum per kind", () => {
  const list = rows({ "src/b.rs": { "crate::ruby": 1, "SourceLanguage::Ruby": 2 }, "src/a.rs": { "name:ruby": 3 } });
  assert.deepEqual(list.map(([file, key]) => `${file} ${key}`), ["src/a.rs name:ruby", "src/b.rs SourceLanguage::Ruby", "src/b.rs crate::ruby"]);
  assert.deepEqual(totals(list), { module: 1, framework: 0, language: 2, domain: 0, marker: 0, name: 3, key: 0, grammar: 0 });
});

/** Checks the baseline validator accepts a rendered baseline and rejects each malformed shape. */
test("baseline validation rejects malformed manifests, with a valid one as control", () => {
  const good = { schemaVersion: 2, core: [["src/a.rs", "crate::ruby", 1], ["src/b.rs", "name:jest", 2]], registry: [["src/source_adapter.rs", "SourceLanguage", 1]] };
  assert.doesNotThrow(() => validateManifest(good));
  assert.deepEqual(JSON.parse(render(good)), good);
  assert.match(render(good), /\n {4}\["src\/a\.rs","crate::ruby",1\],\n {4}\["src\/b\.rs","name:jest",2\]\n/);
  const bad = {
    "wrong schema": { ...good, schemaVersion: 1 },
    "extra key": { ...good, files: {} },
    "missing section": { schemaVersion: 2, core: [] },
    "unsorted rows": { ...good, core: [...good.core].reverse() },
    "duplicate row": { ...good, core: [good.core[0], good.core[0]] },
    "unknown edge": { ...good, core: [["src/a.rs", "crate::python", 1]] },
    "zero count": { ...good, core: [["src/a.rs", "crate::ruby", 0]] },
    "fractional count": { ...good, core: [["src/a.rs", "crate::ruby", 1.5]] },
    "file outside src": { ...good, core: [["vendor/x.rs", "crate::ruby", 1]] },
    "non-Rust file": { ...good, core: [["src/data.json", "crate::ruby", 1]] },
    "non-string edge": { ...good, core: [["src/a.rs", ["crate::ruby"], 1]] },
    "row arity": { ...good, core: [["src/a.rs", "crate::ruby"]] },
  };
  for (const [label, manifest] of Object.entries(bad)) assert.throws(() => validateManifest(manifest), /invalid|sorted/, label);
});

/** Builds a temporary repository with source files and a baseline, runs the body with a CLI runner, and removes the repository afterwards. */
async function withRepo(files, manifest, body) {
  const workspace = await mkdtemp(join(tmpdir(), "language-boundary-test-"));
  const root = join(workspace, "repository");
  const checker = fileURLToPath(new URL("../check-language-boundary.mjs", import.meta.url));
  const manifestPath = join(root, "scripts/check/language-boundary.json");
  /** Writes one file under the temporary repository, creating parent directories. */
  const put = async (name, text) => { await mkdir(dirname(join(root, name)), { recursive: true }); await writeFile(join(root, name), text); };
  /** Runs the checker from another working directory with bounded runtime and output. */
  const run = (...args) => spawnSync(process.execPath, [checker, "--root", root, ...args], { cwd: workspace, encoding: "utf8", timeout: 10_000, maxBuffer: 1024 * 1024 });
  try {
    for (const [name, text] of Object.entries(files)) await put(name, text);
    await put("scripts/check/language-boundary.json", typeof manifest === "string" ? manifest : render({ core: [], registry: [], ...manifest }));
    await body({ run, put, root, manifestPath });
  } finally { await rm(workspace, { recursive: true, force: true }); }
}

/** Checks the CLI passes on a matching baseline, prints per-kind totals, and fails on growth with an actionable message. */
test("CLI: matching baseline passes; added edge fails naming file, kind, counts and the remedy", async () => {
  const baseline = { core: [["src/analysis.rs", "SourceLanguage::Ruby", 1], ["src/analysis.rs", "name:ruby", 1]] };
  await withRepo({ "src/analysis.rs": "let _ = SourceLanguage::Ruby;", "src/cli.rs": "" }, baseline, async ({ run, put }) => {
    const passing = run();
    assert.equal(passing.status, 0, passing.stderr);
    assert.match(passing.stdout, /core: 1 files with edges; module 0, framework 0, language 1, domain 0, marker 0, name 1, key 0, grammar 0\./);
    assert.match(passing.stdout, /registry: 0 files with edges/);
    await put("src/cli.rs", "crate::ruby::extract();");
    const added = run();
    assert.equal(added.status, 1);
    assert.match(added.stderr, /core src\/cli\.rs: module crate::ruby: baseline 0, found 1; boundary growth: remove the edge/);
    assert.match(added.stderr, /record a reviewed increase with `--update --allow-growth`/);
    await put("src/cli.rs", "let _ = SourceLanguage::Ruby;");
    assert.match(run().stderr, /core src\/cli\.rs: language SourceLanguage::Ruby: baseline 0, found 1/);
  });
});

/** Checks the same text is ignored in adapter, exempt, test and asset paths, with a core file as the failing control. */
test("CLI: adapter, composition, test and asset paths are not scanned", async () => {
  const edges = "use crate::ruby; let _ = SourceLanguage::Ruby; fn jest() {}";
  await withRepo({ "src/analysis.rs": "" }, {}, async ({ run, put }) => {
    for (const name of ["src/ruby.rs", "src/ruby/x.rs", "src/ruby/data.bin", "src/typescript.rs", "src/typescript/x.rs", "src/lib.rs", "src/main.rs", "src/analysis/tests.rs", "src/reporters/templates/r.html"]) {
      await put(name, edges);
      assert.equal(run().status, 0, name);
    }
    await put("src/analysis/inline.rs", edges);
    assert.equal(run().status, 1, "same text in a core file fails");
  });
});

/** Checks the registry file has its own baseline and a core baseline cannot absorb its edges. */
test("CLI: source_adapter.rs is ratcheted in its own section", async () => {
  const text = "use tree_sitter_ruby; let _ = SourceLanguage::Ruby;";
  const registry = [["src/source_adapter.rs", "SourceLanguage::Ruby", 1], ["src/source_adapter.rs", "grammar:tree_sitter_ruby", 1], ["src/source_adapter.rs", "name:ruby", 2]];
  await withRepo({ "src/source_adapter.rs": text }, { registry }, async ({ run, put }) => {
    const passing = run();
    assert.equal(passing.status, 0, passing.stderr);
    assert.match(passing.stdout, /registry: 1 files with edges; module 0, framework 0, language 1, domain 0, marker 0, name 2, key 0, grammar 1\./);
    await put("src/source_adapter.rs", `${text} crate::typescript::x();`);
    assert.match(run().stderr, /registry src\/source_adapter\.rs: module crate::typescript: baseline 0, found 1/);
  });
  await withRepo({ "src/source_adapter.rs": text }, { core: registry }, async ({ run }) => {
    assert.match(run().stderr, /core src\/source_adapter\.rs: .*stale baseline/);
  });
});

/** Checks removed edges, emptied files and deleted files all fail until the baseline is lowered. */
test("CLI: stale baseline rows fail for lowered counts, removed edges and removed files", async () => {
  const baseline = { core: [["src/analysis.rs", "crate::ruby", 2], ["src/analysis.rs", "name:ruby", 2], ["src/cli.rs", "name:jest", 1]] };
  await withRepo({ "src/analysis.rs": "crate::ruby::a(); crate::ruby::b();", "src/cli.rs": "fn jest() {}" }, baseline, async ({ run, put, root }) => {
    assert.equal(run().status, 0);
    await put("src/analysis.rs", "crate::ruby::a();");
    const lowered = run();
    assert.equal(lowered.status, 1);
    assert.match(lowered.stderr, /core src\/analysis\.rs: module crate::ruby: baseline 2, found 1; stale baseline: lower it with --update/);
    await put("src/analysis.rs", "crate::ruby::a(); crate::ruby::b();");
    await put("src/cli.rs", "");
    assert.match(run().stderr, /core src\/cli\.rs: name:jest: baseline 1, found 0; stale/);
    await rm(join(root, "src/cli.rs"));
    assert.match(run().stderr, /core src\/cli\.rs: name:jest: baseline 1, found 0; stale/);
  });
});

/** Checks unclassifiable or unparsable core files fail while the same files in adapter paths pass. */
test("CLI: unmodeled inputs fail closed; the same inputs in adapter paths pass", async () => {
  await withRepo({ "src/analysis.rs": "" }, {}, async ({ run, put, root }) => {
    for (const [name, text, message] of [
      ["src/data.json", "{}", /src\/data\.json: unclassified/],
      ["src/analysis/broken.rs", 'let _ = "open;', /src\/analysis\/broken\.rs: unterminated string/],
      ["src/analysis/glob.rs", "use crate::ruby::*;", /src\/analysis\/glob\.rs: unsupported targeted wildcard/],
    ]) {
      await put(name, text);
      const result = run();
      assert.equal(result.status, 1, name);
      assert.match(result.stderr, message, name);
      await rm(join(root, name));
      assert.equal(run().status, 0, `${name} removed`);
    }
  });
  const files = { "src/analysis.rs": "", "src/ruby/data.json": "{}", "src/ruby/broken.rs": 'let _ = "open;', "src/typescript/glob.rs": "use crate::ruby::*;" };
  await withRepo(files, {}, async ({ run }) => assert.equal(run().status, 0, "adapter paths are never parsed"));
});

/** Checks the CLI rejects malformed and missing baselines. */
test("CLI: malformed or missing baseline files fail", async () => {
  const unsorted = render({ core: [], registry: [] }).replace('"core": []', '"core": [\n    ["src/b.rs","name:jest",1],\n    ["src/a.rs","name:jest",1]\n  ]');
  for (const [label, manifest] of [["not json", "{"], ["wrong schema", '{"schemaVersion":1}'], ["unsorted", unsorted]]) {
    await withRepo({ "src/analysis.rs": "" }, manifest, async ({ run }) => assert.equal(run().status, 1, label));
  }
  await withRepo({ "src/analysis.rs": "" }, {}, async ({ run, manifestPath }) => {
    assert.equal(run().status, 0, "valid empty baseline");
    await rm(manifestPath);
    assert.match(run().stderr, /language-boundary\.json/);
  });
});

/** Checks --update lowers stale rows, refuses growth, records growth only under --allow-growth, and rejects bad flags. */
test("CLI: --update lowers the baseline and refuses growth unless allowed", async () => {
  const baseline = { core: [["src/analysis.rs", "crate::ruby", 2], ["src/analysis.rs", "name:ruby", 2]] };
  await withRepo({ "src/analysis.rs": "crate::ruby::a();" }, baseline, async ({ run, put, manifestPath }) => {
    assert.equal(run().status, 1);
    const updated = run("--update");
    assert.equal(updated.status, 0, updated.stderr);
    assert.equal(await readFile(manifestPath, "utf8"), render({ core: [["src/analysis.rs", "crate::ruby", 1], ["src/analysis.rs", "name:ruby", 1]], registry: [] }));
    assert.equal(run().status, 0);
    await put("src/analysis.rs", "crate::ruby::a(); crate::ruby::b();");
    const before = await readFile(manifestPath, "utf8");
    const refused = run("--update");
    assert.equal(refused.status, 1);
    assert.match(refused.stderr, /boundary growth.*--allow-growth/s);
    assert.equal(await readFile(manifestPath, "utf8"), before, "refused update leaves the baseline untouched");
    assert.equal(run("--update", "--allow-growth").status, 0);
    assert.equal(run().status, 0);
    // A read-only baseline makes any rewrite fail, so a no-op --update must not write (not observable when running as root).
    await chmod(manifestPath, 0o444);
    const noop = run("--update");
    await chmod(manifestPath, 0o644);
    assert.equal(noop.status, 0, `a no-op --update does not rewrite the baseline: ${noop.stderr}`);
    for (const args of [["--allow-growth"], ["--bogus"], ["--root"], ["--root", "--update"]]) assert.match(run(...args).stderr, /Usage/, args.join(" "));
  });
});

/** Checks --update can regenerate an unreadable baseline only when growth is allowed. */
test("CLI: --update regenerates a corrupt baseline only with --allow-growth", async () => {
  await withRepo({ "src/analysis.rs": "crate::ruby::a();" }, "<<<<<<< conflict", async ({ run, manifestPath }) => {
    assert.equal(run().status, 1, "plain run rejects the corrupt baseline");
    const refused = run("--update");
    assert.equal(refused.status, 1);
    assert.match(refused.stderr, /Baseline unreadable.*--allow-growth/s);
    assert.equal(run("--update", "--allow-growth").status, 0);
    assert.equal(await readFile(manifestPath, "utf8"), render({ core: [["src/analysis.rs", "crate::ruby", 1], ["src/analysis.rs", "name:ruby", 1]], registry: [] }));
    assert.equal(run().status, 0);
  });
});

/** Checks a baselined file that moves into an adapter path fails as stale and --update drops its rows without allowing growth. */
test("CLI: a baselined file moved to an adapter path is stale, not malformed", async () => {
  const baseline = { core: [["src/analysis/evidence.rs", "crate::typescript", 1], ["src/analysis/evidence.rs", "name:typescript", 1]] };
  await withRepo({ "src/typescript/evidence.rs": "crate::typescript::x();" }, baseline, async ({ run, manifestPath }) => {
    const stale = run();
    assert.equal(stale.status, 1);
    assert.match(stale.stderr, /core src\/analysis\/evidence\.rs: module crate::typescript: baseline 1, found 0; stale baseline/);
    assert.doesNotMatch(stale.stderr, /Remove the added edges/);
    assert.match(stale.stderr, /run `node scripts\/check\/check-language-boundary\.mjs --update`/);
    assert.equal(run("--update").status, 0);
    assert.equal(await readFile(manifestPath, "utf8"), render({ core: [], registry: [] }));
  });
});

/** Checks hidden files in the scan scope are skipped while the same name without the dot fails closed. */
test("CLI: hidden files are skipped; the same file unhidden is unclassified", async () => {
  await withRepo({ "src/analysis.rs": "", "src/.DS_Store": "x", "src/.analysis.rs.swp": "x" }, {}, async ({ run, put, root }) => {
    assert.equal(run().status, 0);
    await put("src/DS_Store", "x");
    assert.match(run().stderr, /src\/DS_Store: unclassified/);
    await rm(join(root, "src/DS_Store"));
    await put("src/.hidden.rs", "crate::ruby::x();");
    assert.match(run().stderr, /core src\/\.hidden\.rs: module crate::ruby/);
    await rm(join(root, "src/.hidden.rs"));
    await put("src/.generated/bridge.rs", "crate::ruby::x();");
    assert.match(run().stderr, /core src\/\.generated\/bridge\.rs: module crate::ruby/);
  });
});

/** Checks a symbolic link in a scanned path fails while the same link under an adapter path is never visited. */
test("CLI: symbolic links fail in scanned paths and are ignored in adapter paths", { skip: process.platform === "win32" }, async () => {
  await withRepo({ "src/analysis.rs": "", "src/ruby/real.rs": "" }, {}, async ({ run, root }) => {
    await symlink(join(root, "src/ruby/real.rs"), join(root, "src/ruby/link.rs"));
    assert.equal(run().status, 0, "link under src/ruby is skipped");
    await symlink(join(root, "src/ruby/real.rs"), join(root, "src/ruby.rs"));
    assert.equal(run().status, 0, "link named like an adapter file is skipped");
    // A hidden link (an Emacs lock file points at a name that does not exist) is never a module; its visible twin fails below.
    await symlink("user@host.1234", join(root, "src/.#analysis.rs"));
    assert.equal(run().status, 0, "hidden dangling link is skipped");
    await symlink(join(root, "src/analysis.rs"), join(root, "src/link.rs"));
    const result = run();
    assert.equal(result.status, 1, "link in a scanned path fails");
    assert.match(result.stderr, /unsupported symbolic link/);
    await rm(join(root, "src/link.rs"));
    await mkdir(join(root, "src/analysis"), { recursive: true });
    await symlink(join(root, "src/analysis"), join(root, "src/linkdir"));
    assert.match(run().stderr, /unsupported symbolic link/, "directory link");
  });
});
