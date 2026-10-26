import { test } from "node:test";
import assert from "node:assert/strict";
import {
  camel,
  cliFlags,
  configFields,
  configSurface,
  findConfigStruct,
  flagDocumented,
  kebab,
  structBody,
} from "./docs-parity.mjs";

/** Checks named struct extraction returns its matched body and rejects missing structs. */
test("structBody returns the brace-matched body of the named struct", () => {
  const src = "struct Other { a: u8 }\nstruct Target {\n  x: u8,\n  y: Vec<u8>,\n}\n";
  assert.match(structBody(src, "Target"), /x: u8/);
  assert.doesNotMatch(structBody(src, "Target"), /struct Other/);
  assert.throws(() => structBody(src, "Missing"), /struct Missing not found/);
});

/** Checks CLI flag extraction handles derived names, explicit long names, and multiline attributes while excluding positional arguments. */
test("cliFlags derives kebab names, honours long overrides, and spans multi-line attributes", () => {
  // A single-line bare `long`, a `long = "..."` override, and a multi-line `#[arg(...)]` block whose
  // continuation lines do not start with `#[` — the case an earlier line-by-line parser dropped.
  const body = `
    #[arg(long, global = true)]
    require_features: bool,

    #[arg(long = "rule", global = true)]
    rules: Vec<(Rule, Severity)>,

    #[arg(
        long,
        global = true,
        value_name = "PERCENT",
    )]
    threshold: Option<f64>,

    #[arg(value_name = "PATH")]
    path: PathBuf,
  `;
  const flags = cliFlags(body);
  assert.ok(flags.includes("--require-features"));
  assert.ok(flags.includes("--rule"));
  assert.ok(flags.includes("--threshold"), "multi-line #[arg(...)] must not be skipped");
  assert.ok(!flags.includes("--path"), "a positional arg without `long` is not a flag");
});

/** Checks CLI flag extraction retains fields with public visibility modifiers. */
test("cliFlags parses a field behind a `pub`/`pub(crate)` visibility modifier", () => {
  const body = `
    #[arg(long, global = true)]
    pub require_features: bool,

    #[arg(long)]
    pub(crate) fail_on_new: Option<usize>,
  `;
  const flags = cliFlags(body);
  assert.ok(flags.includes("--require-features"), "a pub field must not be dropped");
  assert.ok(flags.includes("--fail-on-new"), "a pub(crate) field must not be dropped");
});

/** Checks adversarial repeated comment markers remain within a bounded flag-extraction runtime. */
test("cliFlags scans linearly and returns quickly on an adversarial attribute string", () => {
  // Guards the ReDoS CodeQL flagged in the old regex: the previous `(?:(?:#\[…\]|///…)\s*)*` could
  // backtrack exponentially on many repetitions of `///` or `#[` after `#[arg()]`.
  const body = `#[arg()]${"///\n".repeat(4000)}    x: bool,\n`;
  const start = process.hrtime.bigint();
  cliFlags(body);
  const ms = Number(process.hrtime.bigint() - start) / 1e6;
  assert.ok(ms < 500, `cliFlags took ${ms}ms on a degenerate input; expected linear time`);
});

/** Checks documentation matching requires a whole flag token rather than a longer flag's prefix. */
test("flagDocumented matches a whole flag token, not a prefix of a longer flag", () => {
  const docs = "Configure `--exclude-defaults` and `--baseline-from-ref` here.";
  assert.equal(flagDocumented(docs, "--exclude-defaults"), true);
  assert.equal(flagDocumented(docs, "--exclude"), false, "a prefix must not count as documented");
  assert.equal(flagDocumented(docs, "--baseline"), false);
  assert.equal(flagDocumented("Pass --baseline to reuse.", "--baseline"), true);
});

/** Checks configuration field names convert from snake case to camel case with their types and flatten flag. */
test("configFields camelCases every field of the struct body", () => {
  const body = `
    #[serde(default)]
    fail_on_unparseable: Option<bool>,
    #[serde(flatten)]
    frontend: crate::source_adapter::RawFrontendConfig,
    #[serde(default)]
    max_candidate_comparisons: Option<usize>,
  `;
  assert.deepEqual(configFields(body), [
    { key: "failOnUnparseable", type: "Option<bool>", flatten: false },
    { key: "frontend", type: "crate::source_adapter::RawFrontendConfig", flatten: true },
    { key: "maxCandidateComparisons", type: "Option<usize>", flatten: false },
  ]);
});

/** Checks snake-case field names convert to their expected kebab and camel spellings. */
test("kebab and camel are inverse spellings of a snake_case field", () => {
  assert.equal(kebab("fail_on_new"), "fail-on-new");
  assert.equal(camel("fail_on_new"), "failOnNew");
});

/** Checks flattened frontend structs, aliases and nested sections join the config surface. */
test("configSurface follows flatten, aliases and sections, and fails closed on a lost struct", () => {
  const sources = [
    `struct RawConfig {
      #[serde(default)]
      threshold: Option<f64>,
      #[serde(flatten)]
      frontend: crate::source_adapter::RawFrontendConfig,
      #[serde(default)]
      output: Option<PathBuf>,
    }`,
    "pub(crate) type RawFrontendConfig = crate::ruby::config::RawRubyConfig;",
    `struct RawRubyConfig {
      #[serde(default)]
      ruby: Option<RawRubySection>,
      #[serde(default)]
      ruby_load_paths: Option<Vec<PathBuf>>,
    }
    struct RawRubySection {
      #[serde(default)]
      load_paths: Option<Vec<PathBuf>>,
    }`,
  ];
  /** Looks a config struct up in the sample sources. */
  const find = (name) => findConfigStruct(sources, name);
  assert.deepEqual(configSurface(find, "RawConfig"), [
    "threshold",
    "ruby",
    "ruby.loadPaths",
    "rubyLoadPaths",
    "output",
  ]);
  assert.equal(find("Missing"), null);
  assert.throws(
    () => configSurface((name) => (name === "RawConfig" ? find(name) : null), "RawConfig"),
    /config struct RawFrontendConfig not found/,
  );
  // A lost section type throws too; `Option<PathBuf>` above is a scalar, not a section.
  const lostSection = sources.map((text) => text.replace("struct RawRubySection {", "struct Renamed {"));
  assert.throws(
    () => configSurface((name) => findConfigStruct(lostSection, name), "RawConfig"),
    /config struct RawRubySection not found/,
  );
});

/** Checks a `use … as Name;` rename resolves while `as Name` in prose or a cast does not. */
test("findConfigStruct follows use renames, not every `as Name` text", () => {
  const target = "struct Target {\n  load_paths: Option<Vec<PathBuf>>,\n}";
  assert.match(findConfigStruct(["pub(crate) use crate::a::Target as Renamed;", target], "Renamed"), /load_paths/);
  // Prose or a cast saying `as Target` in an earlier file must not divert the lookup of a struct
  // that exists: misread as a rename, each would resolve to a missing name and return null.
  for (const decoy of ["// stored as Target in the registry", "let x = y as Target;"]) {
    assert.match(findConfigStruct([decoy, target], "Target"), /load_paths/, decoy);
  }
});
