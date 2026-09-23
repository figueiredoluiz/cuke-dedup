import { test } from "node:test";
import assert from "node:assert/strict";
import {
  camel,
  cliFlags,
  configKeys,
  flagDocumented,
  kebab,
  structBody,
} from "./docs-parity.mjs";

test("structBody returns the brace-matched body of the named struct", () => {
  const src = "struct Other { a: u8 }\nstruct Target {\n  x: u8,\n  y: Vec<u8>,\n}\n";
  assert.match(structBody(src, "Target"), /x: u8/);
  assert.doesNotMatch(structBody(src, "Target"), /struct Other/);
  assert.throws(() => structBody(src, "Missing"), /struct Missing not found/);
});

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

test("cliFlags scans linearly and returns quickly on an adversarial attribute string", () => {
  // Guards the ReDoS CodeQL flagged in the old regex: the previous `(?:(?:#\[…\]|///…)\s*)*` could
  // backtrack exponentially on many repetitions of `///` or `#[` after `#[arg()]`.
  const body = `#[arg()]${"///\n".repeat(4000)}    x: bool,\n`;
  const start = process.hrtime.bigint();
  cliFlags(body);
  const ms = Number(process.hrtime.bigint() - start) / 1e6;
  assert.ok(ms < 500, `cliFlags took ${ms}ms on a degenerate input; expected linear time`);
});

test("flagDocumented matches a whole flag token, not a prefix of a longer flag", () => {
  const docs = "Configure `--exclude-defaults` and `--baseline-from-ref` here.";
  assert.equal(flagDocumented(docs, "--exclude-defaults"), true);
  assert.equal(flagDocumented(docs, "--exclude"), false, "a prefix must not count as documented");
  assert.equal(flagDocumented(docs, "--baseline"), false);
  assert.equal(flagDocumented("Pass --baseline to reuse.", "--baseline"), true);
});

test("configKeys camelCases every field of the struct body", () => {
  const body = `
    #[serde(default)]
    fail_on_unparseable: Option<bool>,
    #[serde(default)]
    max_candidate_comparisons: Option<usize>,
  `;
  assert.deepEqual(configKeys(body), ["failOnUnparseable", "maxCandidateComparisons"]);
});

test("kebab and camel are inverse spellings of a snake_case field", () => {
  assert.equal(kebab("fail_on_new"), "fail-on-new");
  assert.equal(camel("fail_on_new"), "failOnNew");
});
