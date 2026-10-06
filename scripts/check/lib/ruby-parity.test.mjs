import assert from "node:assert/strict";
import test from "node:test";
import { cargoTestArgs, completionPassed, corpusCoverageDeficits, evidenceRunPassed, evidenceTargets, executesEvidence, oracleClauseResolves, parityDeficits, validateOracle, validateUnitEntry } from "./ruby-parity.mjs";

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
const census = new Map([
  ["tests/cli.rs::ruby_end_to_end", { name: "ruby_end_to_end", behaviorClass: "ruby-behavior", requiredStatus: "rust-evidence",
    evidenceTest: { file: "tests/cli.rs", name: "ruby_end_to_end" }, evidenceKind: "binary", rationale: "runs Ruby through the binary" }],
  ["tests/other.rs::ts_only", { name: "ts_only", behaviorClass: "language-specific-api", requiredStatus: "language-inapplicable", rationale: "tsconfig" }],
  ["tests/other.rs::chained", { name: "chained", behaviorClass: "ruby-behavior", requiredStatus: "rust-evidence",
    evidenceTest: { file: "tests/cli.rs", name: "ruby_end_to_end" }, evidenceKind: "binary", rationale: "x" }],
]);
/** Builds a shared-engine census entry with overrides. */
const entry = (overrides) => ({ name: "t", behaviorClass: "shared-engine-invariant", requiredStatus: "unmapped", rationale: "why", ...overrides });

test("oracle clauses resolve only against real oracle members", () => {
  assert.ok(oracleClauseResolves(oracle, "requiredFindings[0]"));
  assert.ok(oracleClauseResolves(oracle, "requiredAbsent[0]"));
  assert.ok(oracleClauseResolves(oracle, "expectedExit"));
  assert.ok(!oracleClauseResolves(oracle, "requiredFindings[1]"));
  assert.ok(!oracleClauseResolves(groups.empty, "requiredFindings[0]"));
  assert.ok(!oracleClauseResolves(oracle, "expectedCandidateSources.identicalHandler"));
  const budgeted = { ...oracle, expectedCandidateSources: { matcherBlocking: 2, identicalHandler: 1 } };
  assert.ok(oracleClauseResolves(budgeted, "expectedCandidateSources.matcherBlocking"));
  assert.ok(oracleClauseResolves(budgeted, "expectedCandidateSources.identicalHandler"));
  assert.ok(!oracleClauseResolves(budgeted, "expectedCandidateSources.structuralHandler"));
  assert.ok(!oracleClauseResolves(oracle, "path"));
  assert.ok(!oracleClauseResolves(oracle, "duplication"));
  assert.ok(!oracleClauseResolves({ ...oracle, duplication: {} }, "duplication"));
  assert.ok(oracleClauseResolves({ ...oracle, duplication: { percentage: 0.0, passed: true } }, "duplication"));
  assert.ok(!oracleClauseResolves(undefined, "complete"));
});

test("the class by status matrix closes the disposition escape routes", () => {
  // Ruby and shared-engine tests can never be excluded as language-inapplicable.
  assert.throws(() => validateUnitEntry("f.rs", entry({ behaviorClass: "ruby-behavior", requiredStatus: "language-inapplicable" }), groups, census), /not allowed/);
  assert.throws(() => validateUnitEntry("f.rs", entry({ requiredStatus: "language-inapplicable" }), groups, census), /not allowed/);
  assert.throws(() => validateUnitEntry("f.rs", entry({ behaviorClass: "language-specific-api", requiredStatus: "engine-invariant", stage: "sha256" }), groups, census), /not allowed/);
  // Blocking statuses report the blocker instead of throwing.
  assert.equal(validateUnitEntry("f.rs", entry({}), groups, census), "f.rs:t: unmapped");
  assert.equal(validateUnitEntry("f.rs", entry({ requiredStatus: "partial" }), groups, census), "f.rs:t: partial");
  // Covered needs an existing group, a resolvable clause and a rationale.
  assert.equal(validateUnitEntry("f.rs", entry({ requiredStatus: "covered", fixtureGroup: "group", oracleClause: "requiredAbsent[0]" }), groups, census), null);
  assert.throws(() => validateUnitEntry("f.rs", entry({ requiredStatus: "covered", fixtureGroup: "missing", oracleClause: "complete" }), groups, census), /missing group/);
  assert.throws(() => validateUnitEntry("f.rs", entry({ requiredStatus: "covered", fixtureGroup: "empty", oracleClause: "requiredFindings[0]" }), groups, census), /oracleClause/);
  assert.throws(() => validateUnitEntry("f.rs", entry({ requiredStatus: "covered", fixtureGroup: "group", oracleClause: "complete", rationale: " " }), groups, census), /rationale/);
  // Engine invariants need a known stage.
  assert.equal(validateUnitEntry("f.rs", entry({ requiredStatus: "engine-invariant", stage: "sha256" }), groups, census), null);
  assert.throws(() => validateUnitEntry("f.rs", entry({ requiredStatus: "engine-invariant", stage: "reporting" }), groups, census), /unknown engine stage/);
  // Rust evidence must exist, be a Ruby test, not chain, and self-reference only for Ruby tests.
  const ruby = census.get("tests/cli.rs::ruby_end_to_end");
  assert.equal(validateUnitEntry("tests/cli.rs", ruby, groups, census), null);
  assert.equal(validateUnitEntry("f.rs", entry({ requiredStatus: "rust-evidence", evidenceTest: { file: "tests/cli.rs", name: "ruby_end_to_end" }, evidenceKind: "binary" }), groups, census), null);
  assert.throws(() => validateUnitEntry("f.rs", entry({ requiredStatus: "rust-evidence", evidenceTest: { file: "f.rs", name: "t" }, evidenceKind: "binary" }), groups, census), /not in the census/);
  assert.throws(() => validateUnitEntry("f.rs", entry({ requiredStatus: "rust-evidence", evidenceTest: { file: "tests/other.rs", name: "ts_only" }, evidenceKind: "binary" }), groups, census), /must be a Ruby test/);
  assert.throws(() => validateUnitEntry("f.rs", entry({ requiredStatus: "rust-evidence", evidenceTest: { file: "tests/other.rs", name: "chained" }, evidenceKind: "binary" }), groups, census), /chains/);
  assert.throws(() => validateUnitEntry("f.rs", entry({ requiredStatus: "rust-evidence", evidenceTest: { file: "f.rs", name: "t" }, evidenceKind: "binary" }), groups, new Map([["f.rs::t", entry({})]])), /only ruby-behavior/);
  assert.throws(() => validateUnitEntry("f.rs", entry({ requiredStatus: "rust-evidence", evidenceTest: { file: "tests/cli.rs", name: "ruby_end_to_end" }, evidenceKind: "remote" }), groups, census), /evidenceKind/);
});

test("rust evidence is executed and only an exact single pass counts", () => {
  const units = { files: { "a.rs": { tests: [
    { name: "x", requiredStatus: "rust-evidence", evidenceTest: { file: "tests/cli.rs", name: "ruby_end_to_end" } },
    { name: "y", requiredStatus: "rust-evidence", evidenceTest: { file: "tests/cli.rs", name: "ruby_end_to_end" } },
    { name: "z", requiredStatus: "covered" },
  ] } } };
  assert.deepEqual(evidenceTargets(units).map((target) => target.name), ["ruby_end_to_end"]);
  assert.ok(evidenceRunPassed("running 1 test\n.\ntest result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 127 filtered out"));
  assert.ok(!evidenceRunPassed("test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured"));
  assert.ok(!evidenceRunPassed("test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 128 filtered out"));
  assert.ok(!evidenceRunPassed("test result: FAILED. 0 passed; 1 failed"));
});

test("evidence tests are addressed exactly, by suite or by module path", () => {
  assert.deepEqual(cargoTestArgs({ file: "tests/cli.rs", name: "ruby_end_to_end" }), ["test", "-q", "--test", "cli", "ruby_end_to_end", "--", "--exact"]);
  assert.deepEqual(cargoTestArgs({ file: "src/config/tests.rs", name: "paths" }), ["test", "-q", "--lib", "config::tests::paths", "--", "--exact"]);
  assert.deepEqual(cargoTestArgs({ file: "src/cli.rs", name: "parses" }), ["test", "-q", "--lib", "cli::tests::parses", "--", "--exact"]);
  assert.deepEqual(cargoTestArgs({ file: "src/model/behavior.rs", name: "codec" }), ["test", "-q", "--lib", "model::behavior::tests::codec", "--", "--exact"]);
});

test("only completion mode executes Rust evidence", () => {
  assert.equal(executesEvidence({ regression: false, inventoryOnly: false }), true);
  assert.equal(executesEvidence({ regression: true, inventoryOnly: false }), false);
  assert.equal(executesEvidence({ regression: false, inventoryOnly: true }), false);
});
