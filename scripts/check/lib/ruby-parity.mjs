import assert from "node:assert/strict";
import { countMatches, findingOwners } from "./recall-oracle.mjs";

export function completionPassed(observations, blockers) {
  return observations.length > 0 && blockers.length === 0
    && observations.every((item) => item.deficits.length === 0);
}

// Counterpart coverage is independent of whether the analyzer implements the contract yet.
export function corpusCoverageDeficits(mappings, groups) {
  const gaps = [];
  for (const item of mappings) {
    const label = `${item.source}:${item.case}`;
    if (["language-inapplicable", "framework-inapplicable"].includes(item.requiredStatus)) {
      if (!item.rationale?.trim()) gaps.push(`${label}: exclusion needs a rationale`);
      continue;
    }
    if (item.requiredStatus !== "covered") gaps.push(`${label}: ${item.requiredStatus}`);
    if (!item.fixtureGroup || !groups[item.fixtureGroup]) gaps.push(`${label}: missing executable counterpart`);
  }
  if (!mappings.length) gaps.push("empty source corpus");
  return gaps;
}

// Desired outcomes are independent of observations. A missing positive, an unexpected finding,
// or lost completeness must remain a deficit even if extraction counts happen to match.
export function parityDeficits(oracle, report, exit) {
  const deficits = [];
  const check = (actual, expected, label) => {
    try {
      assert.deepEqual(actual, expected);
    } catch {
      deficits.push(`${label}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
    }
  };
  check(report.summary.definitionsAnalyzed, oracle.definitions, "definitions");
  check(report.summary.featureStepsAnalyzed, oracle.featureSteps, "feature steps");
  check(!report.corpus.incomplete, oracle.complete, "complete");
  check(exit, oracle.expectedExit, "strict exit");
  check(report.findingsTruncated, 0, "truncated findings");
  const active = report.findings.filter((finding) => finding.suppression === null);
  for (const expected of oracle.requiredFindings) {
    check(countMatches(active, expected), expected.count ?? 1, `required ${JSON.stringify(expected)}`);
  }
  for (const absent of oracle.requiredAbsent) {
    check(countMatches(active, absent), 0, `forbidden ${JSON.stringify(absent)}`);
  }
  for (const finding of active) {
    check(findingOwners(finding, oracle.requiredFindings).length, 1,
      `finding ownership ${finding.rule} at ${finding.primary.path}:${finding.primary.line}`);
  }
  for (const [key, value] of Object.entries(oracle.duplication ?? {})) {
    check(report.summary.duplication[key], value, `duplication.${key}`);
  }
  for (const [source, count] of Object.entries(oracle.expectedCandidateSources ?? {})) {
    check(report.analysis?.candidateSources?.[source]?.evaluated, count, `${source} candidates`);
  }
  return deficits;
}

export function validateOracle(oracle) {
  assert.equal(typeof oracle.path, "string");
  assert.ok(Number.isSafeInteger(oracle.definitions) && oracle.definitions >= 0);
  assert.ok(Number.isSafeInteger(oracle.featureSteps) && oracle.featureSteps >= 0);
  assert.equal(typeof oracle.complete, "boolean");
  assert.ok([0, 1, 2].includes(oracle.expectedExit), "strict exit must be explicit");
  for (const count of Object.values(oracle.expectedCandidateSources ?? {})) {
    assert.ok(Number.isSafeInteger(count) && count >= 0, "candidate counts must be nonnegative integers");
  }
  assert.ok(Array.isArray(oracle.requiredFindings));
  assert.ok(Array.isArray(oracle.requiredAbsent));
  for (const expectation of [...oracle.requiredFindings, ...oracle.requiredAbsent]) {
    assert.equal(typeof expectation.rule, "string", "expectations must name a rule");
    assert.ok(expectation.rule.length > 0);
    if (expectation.count !== undefined) {
      assert.ok(Number.isSafeInteger(expectation.count) && expectation.count > 0);
    }
  }
  for (const required of oracle.requiredFindings) {
    for (const absent of oracle.requiredAbsent) {
      const forbidsRequired = Object.entries(absent).every(([key, value]) => {
        const left = Array.isArray(value) ? [...value].sort() : value;
        const right = Array.isArray(required[key]) ? [...required[key]].sort() : required[key];
        return JSON.stringify(left) === JSON.stringify(right);
      });
      assert.ok(!forbidsRequired, "the same finding cannot be both required and forbidden");
    }
  }
}

/**
 * Unit census vocabulary. A class decides which statuses may close an entry; every closing
 * status names its evidence (a resolvable oracle clause, an adapter-independent stage, or an
 * executable Ruby test) so a disposition can be objected to one entry at a time.
 */
export const UNIT_CLASSES = ["ruby-behavior", "shared-engine-invariant", "language-specific-api"];
export const UNIT_STATUSES = ["unmapped", "covered", "partial", "language-inapplicable", "engine-invariant", "rust-evidence"];
export const ENGINE_STAGES = ["sha256", "bounded-io", "regex-compiler", "gherkin-parser", "cli-parsing",
  "config-decoding", "model-codec", "lcs-math", "matcher-text", "api-shape", "session-plumbing", "vcs-decoding"];
const ALLOWED_STATUSES = {
  "ruby-behavior": ["unmapped", "covered", "partial", "rust-evidence"],
  "shared-engine-invariant": ["unmapped", "covered", "partial", "engine-invariant", "rust-evidence"],
  "language-specific-api": ["unmapped", "covered", "partial", "language-inapplicable"],
};

/**
 * Resolves `requiredFindings[i]`, `requiredAbsent[i]`, `expectedCandidateSources.<rule>` or a
 * scalar oracle field against the named group's oracle.
 */
export function oracleClauseResolves(oracle, clause) {
  if (typeof clause !== "string" || !oracle) return false;
  let match = /^(requiredFindings|requiredAbsent)\[(\d+)\]$/.exec(clause);
  if (match) return Array.isArray(oracle[match[1]]) && Number(match[2]) < oracle[match[1]].length;
  match = /^expectedCandidateSources\.([a-z-]+)$/.exec(clause);
  if (match) return Object.hasOwn(oracle.expectedCandidateSources ?? {}, match[1]);
  return ["complete", "expectedExit", "definitions", "featureSteps", "duplication"].includes(clause)
    && Object.hasOwn(oracle, clause);
}

/**
 * Validates one census entry structurally and returns its blocker text, or null when the entry
 * closes. `census` maps "file::name" to entries so evidence references can be checked.
 */
export function validateUnitEntry(file, item, groups, census) {
  const label = `${file}:${item.name}`;
  assert.ok(UNIT_CLASSES.includes(item.behaviorClass), `${label}: missing behavioral classification`);
  assert.ok(UNIT_STATUSES.includes(item.requiredStatus), `${label}: unknown requiredStatus`);
  assert.ok(ALLOWED_STATUSES[item.behaviorClass].includes(item.requiredStatus),
    `${label}: ${item.requiredStatus} is not allowed for ${item.behaviorClass}`);
  const rationale = item.rationale?.trim();
  switch (item.requiredStatus) {
    case "covered":
      assert.ok(groups[item.fixtureGroup], `${label}: missing group`);
      assert.ok(oracleClauseResolves(groups[item.fixtureGroup], item.oracleClause),
        `${label}: oracleClause must resolve in ${item.fixtureGroup}`);
      assert.ok(rationale, `${label}: covered needs a rationale naming the observed claim`);
      return null;
    case "language-inapplicable":
      assert.ok(rationale, `${label}: inapplicable requires a rationale`);
      return null;
    case "engine-invariant":
      assert.ok(ENGINE_STAGES.includes(item.stage), `${label}: unknown engine stage`);
      assert.ok(rationale, `${label}: engine-invariant requires a rationale`);
      return null;
    case "rust-evidence": {
      const evidence = item.evidenceTest;
      assert.ok(evidence && typeof evidence.file === "string" && typeof evidence.name === "string",
        `${label}: evidenceTest needs file and name`);
      assert.ok(["binary", "in-process"].includes(item.evidenceKind), `${label}: evidenceKind must be binary or in-process`);
      const target = census.get(`${evidence.file}::${evidence.name}`);
      assert.ok(target, `${label}: evidence test ${evidence.file}:${evidence.name} is not in the census`);
      const self = evidence.file === file && evidence.name === item.name;
      assert.ok(!self || item.behaviorClass === "ruby-behavior", `${label}: only ruby-behavior tests are their own evidence`);
      const targetIsItsOwnEvidence = target.requiredStatus !== "rust-evidence"
        || (target.evidenceTest?.file === evidence.file && target.evidenceTest?.name === evidence.name);
      assert.ok(self || targetIsItsOwnEvidence, `${label}: evidence chains are rejected`);
      assert.ok(self || target.behaviorClass === "ruby-behavior", `${label}: evidence must be a Ruby test`);
      assert.ok(rationale, `${label}: rust-evidence requires a rationale`);
      return null;
    }
    default:
      return `${label}: ${item.requiredStatus}`;
  }
}

/**
 * Distinct Rust tests that completion must execute: one entry per (file, name).
 */
export function evidenceTargets(units) {
  const targets = new Map();
  for (const [file, inventory] of Object.entries(units.files)) {
    for (const item of inventory.tests) {
      if (item.requiredStatus !== "rust-evidence") continue;
      const key = `${item.evidenceTest.file}::${item.evidenceTest.name}`;
      targets.set(key, { file: item.evidenceTest.file, name: item.evidenceTest.name, referencedBy: `${file}:${item.name}` });
    }
  }
  return [...targets.values()];
}

/**
 * `cargo test` output for one exact test: exactly one test must run and pass; an ignored or
 * filtered-out test is not evidence.
 */
export function evidenceRunPassed(stdout) {
  return /test result: ok\. 1 passed; 0 failed; 0 ignored/.test(stdout);
}

/**
 * `cargo test` arguments that address exactly one test: integration tests by suite and name,
 * library tests by their module path (`src/config/tests.rs` -> `config::tests::<name>`).
 */
export function cargoTestArgs(target) {
  if (target.file.startsWith("tests/")) {
    return ["test", "-q", "--test", target.file.replace(/^tests\//, "").replace(/\.rs$/, ""), target.name, "--", "--exact"];
  }
  const modulePath = target.file.replace(/^src\//, "").replace(/\.rs$/, "").replace(/\//g, "::");
  const module = modulePath.endsWith("::tests") ? modulePath : `${modulePath}::tests`;
  return ["test", "-q", "--lib", `${module}::${target.name}`, "--", "--exact"];
}
