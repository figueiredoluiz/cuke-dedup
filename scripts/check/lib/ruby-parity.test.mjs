import assert from "node:assert/strict";
import test from "node:test";
import { TEST_FN_PATTERN, cargoTestArgs, completionPassed, corpusCoverageDeficits, evidenceBatches, evidenceRunPassed, evidenceTargets, executesEvidence, functionBody, isRubySuite, oracleClauseResolves, parityDeficits, residueResolves, rubySuiteEvidence, testBodyMentionsRuby, testOutcomes, validateOracle, validateUnitEntry } from "./ruby-parity.mjs";

const oracle = {
  path: "example", definitions: 2, featureSteps: 0, complete: true, expectedExit: 0,
  requiredFindings: [{ rule: "duplicate-handler", primaryLine: 1 }],
  requiredAbsent: [{ rule: "near-duplicate-step" }],
};
const finding = {
  rule: "duplicate-handler", primary: { path: "steps.rb", line: 1 },
  related: [], suppression: null,
};
const report = {
  summary: { definitionsAnalyzed: 2, featureStepsAnalyzed: 0 },
  corpus: { incomplete: false }, findings: [finding], findingsTruncated: 0,
};

test("passing implemented fixtures cannot hide missing mappings or an empty run", () => {
  const passing = [{ deficits: [] }];
  assert.equal(completionPassed(passing, []), true);
  assert.equal(completionPassed(passing, ["unmapped scope test"]), false);
  assert.equal(completionPassed([{ deficits: ["missing positive"] }], []), false);
  assert.equal(completionPassed([], []), false);
});

test("source counterpart coverage cannot be inferred from inventory or passing implementations", () => {
  const mapping = { source: "source.json", case: "case", requiredStatus: "covered", fixtureGroup: "group" };
  const groups = { group: oracle };
  assert.deepEqual(corpusCoverageDeficits([mapping], groups), []);
  assert.ok(corpusCoverageDeficits([], groups).length);
  assert.ok(corpusCoverageDeficits([mapping], {}).length);
  assert.ok(corpusCoverageDeficits([{ ...mapping, requiredStatus: "partial" }], groups).length);
  assert.ok(corpusCoverageDeficits([{ ...mapping, requiredStatus: "framework-inapplicable" }], groups).length);
  assert.deepEqual(corpusCoverageDeficits([{ ...mapping, requiredStatus: "framework-inapplicable",
    rationale: "The positional framework has no Ruby global-registration counterpart." }], groups), []);
});

test("candidate-boundary oracles assert evaluated sources rather than just final finding counts", () => {
  const bounded = { ...oracle, expectedCandidateSources: { matcherBlocking: 1 } };
  validateOracle(bounded);
  for (const count of [-1, 1.5, "1"]) {
    assert.throws(() => validateOracle({ ...oracle, expectedCandidateSources: { matcherBlocking: count } }));
  }
  assert.ok(parityDeficits(bounded, report, 0).some((gap) => gap.includes("matcherBlocking candidates")));
  assert.deepEqual(parityDeficits(bounded, { ...report,
    analysis: { candidateSources: { matcherBlocking: { evaluated: 1 } } },
  }, 0), []);
});

test("Ruby parity rejects blanket suppression, unclassified findings and overlapping oracles", () => {
  validateOracle(oracle);
  assert.deepEqual(parityDeficits(oracle, report, 0), []);
  for (const findings of [[], [{ ...finding, suppression: {} }],
    [finding, { ...finding, rule: "near-duplicate-step" }]]) {
    assert.ok(parityDeficits(oracle, { ...report, findings }, 0).length > 0);
  }
  assert.ok(parityDeficits({ ...oracle,
    requiredFindings: [oracle.requiredFindings[0], { rule: "duplicate-handler" }],
  }, report, 0).some((message) => message.includes("ownership")));
});

test("Ruby parity checks strict completeness, counts and truncation independently", () => {
  for (const changed of [
    { corpus: { incomplete: true } }, { findingsTruncated: 1 },
    { summary: { definitionsAnalyzed: 0, featureStepsAnalyzed: 0 } },
  ]) {
    assert.ok(parityDeficits(oracle, { ...report, ...changed }, 0).length > 0);
  }
  assert.ok(parityDeficits(oracle, report, 2).length > 0);
  assert.throws(() => validateOracle({ ...oracle, requiredAbsent: [{}] }));
  assert.throws(() => validateOracle({ ...oracle, requiredAbsent: [{ rule: "duplicate-handler" }] }));
});

const groups = { group: oracle, empty: { ...oracle, requiredFindings: [], requiredAbsent: [] } };
const rubyTest = { name: "ruby_end_to_end", behaviorClass: "ruby-behavior", requiredStatus: "rust-evidence",
  evidenceTest: { file: "tests/cli.rs", name: "ruby_end_to_end" }, rationale: "runs Ruby through the binary", mentionsRuby: true };
const census = new Map([
  ["tests/cli.rs::ruby_end_to_end", rubyTest],
  ["tests/cli.rs::js_only", { ...rubyTest, name: "js_only", evidenceTest: { file: "tests/cli.rs", name: "js_only" }, mentionsRuby: false }],
  ["tests/other.rs::ts_only", { name: "ts_only", behaviorClass: "language-specific-api", requiredStatus: "language-inapplicable", rationale: "tsconfig", mentionsRuby: false }],
  ["tests/other.rs::chained", { name: "chained", behaviorClass: "ruby-behavior", requiredStatus: "rust-evidence",
    evidenceTest: { file: "tests/cli.rs", name: "ruby_end_to_end" }, rationale: "x", mentionsRuby: true }],
  ["tests/other.rs::open_ruby", { name: "open_ruby", behaviorClass: "ruby-behavior", requiredStatus: "partial", rationale: "needs a group", mentionsRuby: true }],
  ["tests/other.rs::covered_ruby", { name: "covered_ruby", behaviorClass: "ruby-behavior", requiredStatus: "covered", fixtureGroup: "group", oracleClause: "requiredFindings[0]", rationale: "ok", mentionsRuby: true }],
  ...rubySuiteEvidence("tests/ruby.rs", "#[test]\nfn ruby_suite_case() {\n  run();\n}\n"),
  ["src/main.rs::in_main", { ...rubyTest, name: "in_main", evidenceTest: { file: "src/main.rs", name: "in_main" } }],
]);
/** Builds a shared-engine census entry with overrides. */
const entry = (overrides) => ({ name: "t", behaviorClass: "shared-engine-invariant", requiredStatus: "unmapped", rationale: "why", ...overrides });

test("oracle clauses resolve only against real oracle members with their observed values", () => {
  assert.ok(oracleClauseResolves(oracle, "requiredFindings[0]"));
  assert.ok(oracleClauseResolves(oracle, "requiredAbsent[0]"));
  assert.ok(!oracleClauseResolves(oracle, "requiredFindings[1]"));
  assert.ok(!oracleClauseResolves(groups.empty, "requiredFindings[0]"));
  // Scalar oracle fields are not clauses: every oracle has them and many share their values.
  for (const scalar of ["expectedExit", "expectedExit=0", "complete", "complete=true", "definitions=2", "featureSteps", "path"]) {
    assert.ok(!oracleClauseResolves(oracle, scalar), scalar);
  }
  assert.ok(!oracleClauseResolves(oracle, "expectedCandidateSources.identicalHandler"));
  const budgeted = { ...oracle, expectedCandidateSources: { matcherBlocking: 2, identicalHandler: 1 } };
  assert.ok(oracleClauseResolves(budgeted, "expectedCandidateSources.matcherBlocking"));
  assert.ok(!oracleClauseResolves(budgeted, "expectedCandidateSources.structuralHandler"));
  assert.ok(!oracleClauseResolves(oracle, "duplication"));
  assert.ok(!oracleClauseResolves({ ...oracle, duplication: {} }, "duplication"));
  assert.ok(oracleClauseResolves({ ...oracle, duplication: { percentage: 0.0, passed: true } }, "duplication"));
  assert.ok(!oracleClauseResolves(undefined, "requiredFindings[0]"));
});

test("a test body counts as Ruby evidence only when it exercises Ruby input", () => {
  const content = "#[test]\nfn ruby_case() {\n  write(\"steps.rb\", source);\n}\n#[test]\nfn js_case() {\n  write(\"steps.ts\", { nested: 1 });\n}\n/// Runs a Ruby project with an optional definition pattern.\nfn ruby_helper() {}\n#[test]\nfn lang_case() {\n  SourceLanguage::Ruby\n}\n";
  assert.ok(testBodyMentionsRuby(content, "ruby_case"));
  // The slice stops at the test's closing brace: the Ruby helper doc after js_case is not its body.
  assert.ok(!testBodyMentionsRuby(content, "js_case"));
  assert.equal(functionBody(content, "js_case"), "fn js_case() {\n  write(\"steps.ts\", { nested: 1 });\n}");
  assert.equal(functionBody(content, "missing"), null);
  assert.ok(testBodyMentionsRuby(content, "lang_case"));
  assert.ok(!testBodyMentionsRuby(content, "missing"));
  assert.ok(!testBodyMentionsRuby("fn prose() {\n  // mentions Ruby in a comment only\n}", "prose"));
  // Braces inside strings, raw strings and comments do not end or extend the body.
  const malformed = "fn writes_broken() {\n  write(\"Given('broken', () => {\\n\");\n  let raw = r#\"fn x() { \"#; // } not a close\n}\nfn ruby_next() {\n  write(\"steps.rb\", s);\n}\n";
  assert.equal(functionBody(malformed, "writes_broken").endsWith("// } not a close\n}"), true);
  assert.ok(!testBodyMentionsRuby(malformed, "writes_broken"));
  assert.equal(functionBody("fn open() {\n  write(\"{\");\n", "open"), null);
  assert.ok(isRubySuite("tests/ruby.rs") && isRubySuite("tests/ruby_assertion_provenance.rs") && isRubySuite("src/ruby/handler.rs") && isRubySuite("src/ruby.rs"));
  assert.ok(!isRubySuite("tests/rubyish.rs") && !isRubySuite("tests/cli.rs"));
  // The test's own name is not evidence; char literals, block comments and byte strings do not unbalance the body.
  assert.ok(!testBodyMentionsRuby("fn ruby_flag_is_rejected_for_typescript_projects() {\n  write(\"steps.ts\", s);\n}", "ruby_flag_is_rejected_for_typescript_projects"));
  const literals = "fn quotes() {\n  let open = '{'; let quote = '\\''; let dq = '\"'; /* } block */ let bytes = br#\"} \"#; let b = b\"}\";\n  write(\"steps.ts\", s);\n}\nfn ruby_after() {\n  write(\"steps.rb\", s);\n}\n";
  assert.equal(functionBody(literals, "quotes").endsWith("write(\"steps.ts\", s);\n}"), true);
  assert.ok(!testBodyMentionsRuby(literals, "quotes"));
  assert.equal(functionBody("fn lifetime<'a>(x: &'a str) {\n  use_it(x);\n}", "lifetime"), "fn lifetime<'a>(x: &'a str) {\n  use_it(x);\n}");
  assert.ok(testBodyMentionsRuby(content, "js_case", "tests/ruby_assertion_provenance.rs"));
  assert.ok(testBodyMentionsRuby("fn yml() {\n  write(\"cucumber.yml\", paths);\n}", "yml"));
});

test("the class by status matrix closes the disposition escape routes", () => {
  // Ruby and shared-engine tests can never be excluded as language-inapplicable.
  assert.throws(() => validateUnitEntry("f.rs", entry({ behaviorClass: "ruby-behavior", requiredStatus: "language-inapplicable" }), groups, census), /not allowed/);
  assert.throws(() => validateUnitEntry("f.rs", entry({ requiredStatus: "language-inapplicable" }), groups, census), /not allowed/);
  assert.throws(() => validateUnitEntry("f.rs", entry({ behaviorClass: "language-specific-api", requiredStatus: "engine-invariant", stage: "sha256" }), groups, census), /not allowed/);
  // Blocking statuses report the blocker instead of throwing; partial must name the missing evidence.
  assert.equal(validateUnitEntry("f.rs", entry({}), groups, census), "f.rs:t: unmapped");
  assert.equal(validateUnitEntry("f.rs", entry({ requiredStatus: "partial" }), groups, census), "f.rs:t: partial");
  assert.throws(() => validateUnitEntry("f.rs", entry({ requiredStatus: "partial", rationale: "" }), groups, census), /must name the missing Ruby evidence/);
  // Covered needs an existing group, a resolvable valued clause and a rationale.
  assert.equal(validateUnitEntry("f.rs", entry({ requiredStatus: "covered", fixtureGroup: "group", oracleClause: "requiredAbsent[0]" }), groups, census), null);
  assert.throws(() => validateUnitEntry("f.rs", entry({ requiredStatus: "covered", fixtureGroup: "group", oracleClause: "featureSteps=0" }), groups, census), /oracleClause/);
  // Fields of another status are stale references and are rejected.
  assert.throws(() => validateUnitEntry("f.rs", entry({ requiredStatus: "partial", fixtureGroup: "group" }), groups, census), /does not belong/);
  assert.throws(() => validateUnitEntry("f.rs", entry({ requiredStatus: "engine-invariant", stage: "sha256", oracleClause: "requiredFindings[0]" }), groups, census), /does not belong/);
  assert.throws(() => validateUnitEntry("f.rs", entry({ requiredStatus: "partial", fixtureGroup: null }), groups, census), /does not belong/);
  assert.throws(() => validateUnitEntry("f.rs", entry({ requiredStatus: "covered", fixtureGroup: "missing", oracleClause: "requiredFindings[0]" }), groups, census), /missing group/);
  assert.throws(() => validateUnitEntry("f.rs", entry({ requiredStatus: "covered", fixtureGroup: "empty", oracleClause: "requiredFindings[0]" }), groups, census), /oracleClause/);
  assert.throws(() => validateUnitEntry("f.rs", entry({ requiredStatus: "covered", fixtureGroup: "group", oracleClause: "requiredFindings[0]", rationale: " " }), groups, census), /rationale/);
  // Engine invariants need a known stage.
  assert.equal(validateUnitEntry("f.rs", entry({ requiredStatus: "engine-invariant", stage: "sha256" }), groups, census), null);
  assert.throws(() => validateUnitEntry("f.rs", entry({ requiredStatus: "engine-invariant", stage: "reporting" }), groups, census), /unknown engine stage/);
  // Rust evidence must exist, exercise Ruby, be addressable, not chain, and self-reference only for Ruby tests.
  assert.equal(validateUnitEntry("tests/cli.rs", rubyTest, groups, census), null);
  /** Builds a shared-engine entry citing the given test as Rust evidence. */
  const evidence = (file, name) => entry({ requiredStatus: "rust-evidence", evidenceTest: { file, name } });
  assert.equal(validateUnitEntry("f.rs", evidence("tests/cli.rs", "ruby_end_to_end"), groups, census), null);
  assert.throws(() => validateUnitEntry("f.rs", evidence("tests/missing.rs", "t"), groups, census), /not in the census/);
  assert.throws(() => validateUnitEntry("f.rs", evidence("tests/other.rs", "ts_only"), groups, census), /is language-inapplicable, not settled Ruby evidence/);
  assert.throws(() => validateUnitEntry("f.rs", evidence("tests/other.rs", "chained"), groups, census), /not settled Ruby evidence/);
  assert.throws(() => validateUnitEntry("f.rs", evidence("tests/other.rs", "open_ruby"), groups, census), /is partial, not settled/);
  assert.equal(validateUnitEntry("f.rs", evidence("tests/other.rs", "covered_ruby"), groups, census), null);
  assert.throws(() => validateUnitEntry("tests/cli.rs", census.get("tests/cli.rs::js_only"), groups, census), /does not exercise Ruby input/);
  assert.throws(() => validateUnitEntry("src/main.rs", census.get("src/main.rs::in_main"), groups, census), /cannot be addressed/);
  assert.throws(() => validateUnitEntry("tests/f.rs", { ...evidence("tests/f.rs", "t"), behaviorClass: "shared-engine-invariant" }, groups, new Map([["tests/f.rs::t", { ...entry({}), mentionsRuby: true }]])), /only ruby-behavior/);
  // A Ruby-only suite test is evidence without being inventoried.
  assert.equal(validateUnitEntry("f.rs", evidence("tests/ruby.rs", "ruby_suite_case"), groups, census), null);
  assert.throws(() => validateUnitEntry("f.rs", evidence("tests/ruby.rs", "absent"), groups, census), /not in the census/);
});

test("evidence is addressed exactly per binary and every dependent entry is named", () => {
  assert.deepEqual(cargoTestArgs({ file: "tests/cli.rs", name: "a" }), ["test", "--test", "cli", "--", "--exact", "a"]);
  assert.deepEqual(cargoTestArgs({ file: "src/config/tests.rs", name: "paths" }), ["test", "--lib", "--", "--exact", "config::tests::paths"]);
  assert.deepEqual(cargoTestArgs({ file: "src/cli.rs", name: "parses" }), ["test", "--lib", "--", "--exact", "cli::tests::parses"]);
  assert.deepEqual(cargoTestArgs({ file: "src/model/behavior.rs", name: "codec" }), ["test", "--lib", "--", "--exact", "model::behavior::tests::codec"]);
  assert.deepEqual(cargoTestArgs({ file: "src/analysis/mod.rs", name: "m" }), ["test", "--lib", "--", "--exact", "analysis::tests::m"]);
  assert.deepEqual(cargoTestArgs({ file: "src/lib.rs", name: "root" }), ["test", "--lib", "--", "--exact", "tests::root"]);
  assert.equal(cargoTestArgs({ file: "src/main.rs", name: "x" }), null);
  assert.equal(cargoTestArgs({ file: "tests/common/mod.rs", name: "x" }), null);
  const units = { files: {
    "a.rs": { tests: [
      { name: "x", requiredStatus: "rust-evidence", evidenceTest: { file: "tests/cli.rs", name: "ruby_end_to_end" } },
      { name: "y", requiredStatus: "rust-evidence", evidenceTest: { file: "tests/cli.rs", name: "other" } },
      { name: "z", requiredStatus: "covered" },
      { name: "c", requiredStatus: "rust-evidence", evidenceTest: { file: "src/config/tests.rs", name: "yaml" } },
      { name: "s", requiredStatus: "rust-evidence", evidenceTest: { file: "src/source_adapter/tests.rs", name: "late" } },
    ] },
    "b.rs": { tests: [{ name: "w", requiredStatus: "rust-evidence", evidenceTest: { file: "tests/cli.rs", name: "ruby_end_to_end" } }] },
  } };
  const targets = evidenceTargets(units);
  assert.deepEqual(targets.map((target) => [target.name, target.referencedBy]),
    [["ruby_end_to_end", ["a.rs:x", "b.rs:w"]], ["other", ["a.rs:y"]], ["yaml", ["a.rs:c"]], ["late", ["a.rs:s"]]]);
  // One spawn per binary; library tests keep their own module path even when they share a batch.
  const batches = evidenceBatches(targets);
  assert.equal(batches.length, 2);
  assert.deepEqual(batches[0].args, ["test", "--test", "cli", "--", "--exact", "ruby_end_to_end", "other"]);
  assert.deepEqual(batches[1].args, ["test", "--lib", "--", "--exact", "config::tests::yaml", "source_adapter::tests::late"]);
  assert.deepEqual(batches[1].targets.map((target) => target.name), ["yaml", "late"]);
  assert.ok(evidenceRunPassed("running 2 tests\n..\ntest result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 126 filtered out", 2));
  assert.ok(!evidenceRunPassed("test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 127 filtered out", 2));
  assert.ok(!evidenceRunPassed("test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured", 1));
  assert.ok(!evidenceRunPassed("test result: FAILED. 1 passed; 1 failed", 2));
});

test("only an otherwise unblocked completion run executes Rust evidence, unless forced", () => {
  assert.equal(executesEvidence({ regression: false, inventoryOnly: false }), true);
  assert.equal(executesEvidence({ regression: true, inventoryOnly: false }), false);
  assert.equal(executesEvidence({ regression: false, inventoryOnly: true }), false);
  assert.equal(executesEvidence({ regression: false, inventoryOnly: false, blocked: true }), false);
  assert.equal(executesEvidence({ regression: false, inventoryOnly: false, blocked: true, forced: true }), true);
  assert.equal(executesEvidence({ regression: true, inventoryOnly: false, forced: true }), false);
});

/** Builds a language-specific entry whose rationale is the given text. */
const excluded = (rationale) => ({ name: "js_only", behaviorClass: "language-specific-api", requiredStatus: "language-inapplicable", rationale });

test("a language-inapplicable rationale must state where its Ruby residue lives", () => {
  const known = new Map([["tests/ruby.rs::ruby_case", { name: "ruby_case", requiredStatus: "rust-evidence" }],
    ["f.rs::excluded_case", { name: "excluded_case", requiredStatus: "language-inapplicable" }]]);
  const groupSet = { "load-cycle": {}, "load-relative": {}, polarity: {} };
  /** Whether a rationale resolves against the fixture groups and census above. */
  const resolves = (rationale) => residueResolves(rationale, groupSet, known);
  // Residue named by a group, a group family, a Ruby suite or a settled census entry resolves.
  assert.ok(resolves("JS/TS-only: x. Semantic residue: pinned by the `polarity` group."));
  assert.ok(resolves("JS/TS-only: x. Semantic residue: covered by the load-* groups."));
  assert.ok(resolves("JS/TS-only: x. Semantic residue: covered by load-cycle."));
  assert.ok(resolves("JS/TS-only: x. Semantic residue: pinned by `ruby_case`."));
  assert.ok(resolves("JS/TS-only: x. Semantic residue: see tests/ruby_assertion_provenance.rs."));
  assert.ok(resolves("JS/TS-only: x. Semantic residue: none; Ruby has no such syntax."));
  // A bare JS/TS rationale, an unknown name, an excluded entry or a prefix naming no group do not.
  assert.ok(!resolves("JS/TS-only: JavaScript/TypeScript extraction behaviour."));
  assert.ok(!resolves("JS/TS-only: x. Semantic residue: handled by `no_such_test`."));
  assert.ok(!resolves("JS/TS-only: x. Semantic residue: handled by `excluded_case`."));
  assert.ok(!resolves("JS/TS-only: x. Semantic residue: handled by the missing-* groups."));
  // A bare word that merely starts a group name is not a family.
  assert.ok(!resolves("JS/TS-only: x. Semantic residue: handled by the load tests."));
  assert.ok(!resolves("JS/TS-only: x. Semantic residue: handled by *."));
  assert.ok(!resolves("JS/TS-only: x. Semantic residue:"));
  assert.ok(!resolves(undefined));
  // The validator applies it to every language-inapplicable entry.
  const withGroups = { ...groups, polarity: oracle };
  assert.equal(validateUnitEntry("f.rs", excluded("JS/TS-only: x. Semantic residue: pinned by the `polarity` group."), withGroups, census), null);
  assert.throws(() => validateUnitEntry("f.rs", excluded("JS/TS-only: JavaScript/TypeScript extraction behaviour."), withGroups, census), /Semantic residue/);
});

test("test discovery sees attributes between the marker and the function, per-test outcomes blame only failures", () => {
  const source = "#[test]\nfn plain() {}\n#[test]\n#[should_panic]\nfn panics() {}\n#[test]\n#[cfg(unix)]\n#[ignore = \"slow\"]\nfn gated() {}\n";
  assert.deepEqual([...source.matchAll(TEST_FN_PATTERN)].map((match) => match[1]), ["plain", "panics", "gated"]);
  assert.deepEqual([...rubySuiteEvidence("src/ruby/x.rs", source).keys()], ["src/ruby/x.rs::plain", "src/ruby/x.rs::panics", "src/ruby/x.rs::gated"]);
  const stdout = "running 3 tests\ntest a::good ... ok\ntest a::bad ... FAILED\ntest a::skipped ... ignored\n";
  assert.deepEqual([...testOutcomes(stdout)], [["a::good", "ok"], ["a::bad", "FAILED"], ["a::skipped", "ignored"]]);
  assert.equal(testOutcomes(undefined).size, 0);
  // A raw string with many hashes does not end the body early.
  const hashes = "#".repeat(8);
  const wide = `fn wide() {\n  let s = r${hashes}"} \"# not a close"${hashes};\n  write("steps.rb", s);\n}\nfn next() {}\n`;
  assert.equal(functionBody(wide, "wide").endsWith("write(\"steps.rb\", s);\n}"), true);
  assert.ok(testBodyMentionsRuby(wide, "wide"));
});
