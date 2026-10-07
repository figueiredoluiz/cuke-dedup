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
 * status names its evidence (an oracle clause with its observed value, an adapter-independent
 * stage, or an executed Ruby test) so a disposition can be objected to one entry at a time.
 */
export const UNIT_CLASSES = ["ruby-behavior", "shared-engine-invariant", "language-specific-api"];
export const UNIT_STATUSES = ["unmapped", "covered", "partial", "language-inapplicable", "engine-invariant", "rust-evidence"];
export const ENGINE_STAGES = ["sha256", "bounded-io", "regex-compiler", "gherkin-parser", "cli-parsing",
  "config-decoding", "model-codec", "lcs-math", "matcher-text", "api-shape", "session-plumbing"];
const ALLOWED_STATUSES = {
  "ruby-behavior": ["unmapped", "covered", "partial", "rust-evidence"],
  "shared-engine-invariant": ["unmapped", "covered", "partial", "engine-invariant", "rust-evidence"],
  "language-specific-api": ["unmapped", "covered", "partial", "language-inapplicable"],
};

/**
 * Resolves a clause against a group oracle. A clause names a finding the group requires or
 * forbids (`requiredFindings[i]`, `requiredAbsent[i]`), a counted candidate source
 * (`expectedCandidateSources.<rule>`), or a non-empty `duplication` object. Scalar oracle fields
 * (`complete`, `expectedExit`, counts) are not clauses: every oracle has them and many share the
 * same values, so they cannot tie a test to one observation.
 */
export function oracleClauseResolves(oracle, clause) {
  if (typeof clause !== "string" || !oracle) return false;
  let match = /^(requiredFindings|requiredAbsent)\[(\d+)\]$/.exec(clause);
  if (match) return Array.isArray(oracle[match[1]]) && Number(match[2]) < oracle[match[1]].length;
  match = /^expectedCandidateSources\.([A-Za-z-]+)$/.exec(clause);
  if (match) return Object.hasOwn(oracle.expectedCandidateSources ?? {}, match[1]);
  if (clause === "duplication") {
    return typeof oracle.duplication === "object" && oracle.duplication !== null && Object.keys(oracle.duplication).length > 0;
  }
  return false;
}

/**
 * Integration suites whose every test exercises Ruby by construction. The census does not
 * inventory them; completion reads them for citable evidence instead.
 */
export const RUBY_SUITES = ["tests/ruby.rs", "tests/ruby_assertion_provenance.rs"];

/** Whether a file is one of the Ruby-only suites or belongs to the Ruby frontend. */
export function isRubySuite(file) {
  return RUBY_SUITES.includes(file) || file === "src/ruby.rs" || file.startsWith("src/ruby/");
}

/**
 * A test function and its name: `#[test]`, any further attributes (`#[should_panic]`,
 * `#[ignore]`, `#[cfg(...)]`), then `fn name`. Used by the inventory and the Ruby-suite scan so
 * both see the same functions.
 */
export const TEST_FN_PATTERN = /#\[test\](?:\s*#\[[^\]]*\])*\s*fn (\w+)/g;

/**
 * Whether a test exercises Ruby input: the statements of its body (its own name excluded) name a
 * `.rb` path, the Ruby source language, the Cucumber-Ruby framework, a Ruby-named helper or
 * `cucumber.yml`, or the test lives in a Ruby-only suite (`isRubySuite`). The scan reads the
 * body text as written, string literals included, because Ruby input usually enters a test as a
 * `"steps.rb"` literal; it is a heuristic that rules tests out, not a proof.
 */
export function testBodyMentionsRuby(content, name, file = "") {
  if (isRubySuite(file)) return true;
  const body = functionBody(content, name);
  if (body === null) return false;
  const statements = body.slice(body.indexOf("{"));
  return /\.rb\b|SourceLanguage::Ruby|CucumberRuby|\bruby_|cucumber\.ya?ml/.test(statements);
}

/**
 * The text of `fn name(...) { ... }` up to its matching closing brace, or null when the function
 * is absent or unterminated. Braces inside string literals (`"…"` and `b"…"` with escapes, raw
 * `r"…"`/`br#"…"#`), char literals (`'{'`, `'\''`), line comments and block comments do not
 * count, so a test that writes malformed source does not overrun its body.
 */
export function functionBody(content, name) {
  const start = content.search(new RegExp(`\\bfn ${name}\\b`));
  if (start < 0) return null;
  const open = content.indexOf("{", start);
  if (open < 0) return null;
  let depth = 0;
  for (let i = open; i < content.length; i++) {
    const char = content[i];
    const next = content[i + 1];
    if (char === "/" && next === "/") {
      i = content.indexOf("\n", i);
      if (i < 0) break;
      continue;
    }
    if (char === "/" && next === "*") {
      // Rust block comments nest: `/* a /* b */ } */` closes only at the second terminator.
      let nesting = 1;
      for (i += 2; i < content.length && nesting > 0; i++) {
        if (content.startsWith("/*", i)) { nesting++; i++; } else if (content.startsWith("*/", i)) { nesting--; i++; }
      }
      if (nesting > 0) break;
      i--;
      continue;
    }
    // Rust allows up to 255 hashes around a raw string; the slice is long enough for any.
    const raw = /^b?r(#*)"/.exec(content.slice(i, i + 260));
    if (raw && !/\w/.test(content[i - 1] ?? "")) {
      const close = `"${raw[1]}`;
      const end = content.indexOf(close, i + raw[0].length);
      if (end < 0) break;
      i = end + close.length - 1;
      continue;
    }
    if (char === '"') {
      i++;
      while (i < content.length && content[i] !== '"') i += content[i] === "\\" ? 2 : 1;
      continue;
    }
    // A char literal is a quote, one (possibly escaped) character, and a quote; a lifetime
    // (`'a`) has no closing quote and is left alone.
    const literal = /^'(\\.|[^'\\])'/.exec(content.slice(i, i + 4));
    if (char === "'" && literal) {
      i += literal[0].length - 1;
      continue;
    }
    if (char === "{") depth++;
    else if (char === "}" && --depth === 0) return content.slice(start, i + 1);
  }
  return null;
}

/**
 * Evidence entries for a Ruby-only suite (`tests/ruby.rs`, `src/ruby/**`) that the census does
 * not inventory: every test there exercises Ruby by construction and may be cited as evidence.
 */
export function rubySuiteEvidence(file, content) {
  const entries = new Map();
  for (const match of content.matchAll(TEST_FN_PATTERN)) {
    entries.set(`${file}::${match[1]}`, {
      name: match[1], behaviorClass: "ruby-behavior", requiredStatus: "rust-evidence",
      evidenceTest: { file, name: match[1] }, mentionsRuby: true, external: true,
    });
  }
  return entries;
}

/**
 * Validates one census entry structurally and returns its blocker text, or null when the entry
 * closes. `census` maps "file::name" to entries; an entry carries `mentionsRuby` when its test
 * body references Ruby, which every evidence target must.
 */
export function validateUnitEntry(file, item, groups, census) {
  const label = `${file}:${item.name}`;
  assert.ok(UNIT_CLASSES.includes(item.behaviorClass), `${label}: missing behavioral classification`);
  assert.ok(UNIT_STATUSES.includes(item.requiredStatus), `${label}: unknown requiredStatus`);
  assert.ok(ALLOWED_STATUSES[item.behaviorClass].includes(item.requiredStatus),
    `${label}: ${item.requiredStatus} is not allowed for ${item.behaviorClass}`);
  const rationale = item.rationale?.trim();
  const allowed = { covered: ["fixtureGroup", "oracleClause"], "engine-invariant": ["stage"], "rust-evidence": ["evidenceTest"] }[item.requiredStatus] ?? [];
  for (const field of ["fixtureGroup", "oracleClause", "stage", "evidenceTest"]) {
    assert.ok(allowed.includes(field) || item[field] === undefined,
      `${label}: ${field} does not belong to a ${item.requiredStatus} entry`);
  }
  switch (item.requiredStatus) {
    case "covered":
      assert.ok(groups[item.fixtureGroup], `${label}: missing group`);
      assert.ok(oracleClauseResolves(groups[item.fixtureGroup], item.oracleClause),
        `${label}: oracleClause must resolve in ${item.fixtureGroup}`);
      assert.ok(rationale, `${label}: covered needs a rationale naming the observed claim`);
      return null;
    case "language-inapplicable":
      assert.ok(rationale, `${label}: inapplicable requires a rationale`);
      assert.ok(/^JS\/TS-only: [^.\s][^]*?\.(\s|$)/.test(rationale),
        `${label}: a language-inapplicable rationale must start with "JS/TS-only: <API>." naming the JS/TS-only API`);
      assert.ok(residueResolves(rationale, groups, census),
        `${label}: a language-inapplicable rationale must end with "Semantic residue:" naming none, or a Ruby group, suite or census entry that carries the shared behavior`);
      return null;
    case "engine-invariant":
      assert.ok(ENGINE_STAGES.includes(item.stage), `${label}: unknown engine stage`);
      assert.ok(rationale, `${label}: engine-invariant requires a rationale`);
      return null;
    case "rust-evidence": {
      const evidence = item.evidenceTest;
      assert.ok(evidence && typeof evidence.file === "string" && typeof evidence.name === "string",
        `${label}: evidenceTest needs file and name`);
      assert.ok(cargoTestArgs(evidence), `${label}: evidence test ${evidence.file} cannot be addressed by cargo`);
      const target = census.get(`${evidence.file}::${evidence.name}`);
      assert.ok(target, `${label}: evidence test ${evidence.file}:${evidence.name} is not in the census`);
      const self = evidence.file === file && evidence.name === item.name;
      assert.ok(!self || item.behaviorClass === "ruby-behavior", `${label}: only ruby-behavior tests are their own evidence`);
      // A cited test must itself be settled Ruby evidence: its own evidence, or a covered Ruby
      // test. A chain to a different test, or a target the census still lists as partial or
      // unmapped, settles nothing.
      const selfEvidenced = target.requiredStatus === "rust-evidence"
        && target.evidenceTest?.file === evidence.file && target.evidenceTest?.name === evidence.name;
      assert.ok(self || selfEvidenced || target.requiredStatus === "covered",
        `${label}: evidence test ${evidence.name} is ${target.requiredStatus}, not settled Ruby evidence`);
      assert.ok(self || target.behaviorClass === "ruby-behavior", `${label}: evidence must be a Ruby test`);
      assert.ok(target.mentionsRuby === true, `${label}: evidence test ${evidence.name} does not exercise Ruby input`);
      assert.ok(rationale, `${label}: rust-evidence requires a rationale`);
      return null;
    }
    case "partial":
      assert.ok(rationale, `${label}: partial must name the missing Ruby evidence`);
      return `${label}: partial`;
    default:
      return `${label}: ${item.requiredStatus}`;
  }
}

/**
 * Whether an exclusion rationale states where its Ruby residue lives. The clause after
 * `Semantic residue:` is either `none` (the behavior has no Ruby counterpart) or names at least
 * one executable Ruby group (or a prefix naming a group family), a Ruby-only suite, or a census entry that is not itself excluded;
 * a rationale that names only the JS/TS API is a blanket exclusion and is rejected.
 */
export function residueResolves(rationale, groups, census) {
  const clause = /Semantic residue:\s*(.*)$/s.exec(rationale ?? "")?.[1];
  if (!clause) return false;
  if (/^none\b/i.test(clause)) return true;
  const names = new Set();
  for (const entry of census.values()) if (entry.requiredStatus !== "language-inapplicable") names.add(entry.name);
  const tokens = (clause.match(/[A-Za-z0-9_][A-Za-z0-9_./*-]*/g) ?? []).map((token) => token.replace(/[.,;]+$/, ""));
  const groupNames = Object.keys(groups);
  // A group family is named by its prefix (`load-*`, `handler-context`), which must match a group.
  /** Whether a hyphenated or starred token is the prefix of at least one executable group name. */
  const family = (token) => /-|\*$/.test(token) && token.replace(/-?\*$/, "") !== "" && groupNames.some((group) => group.startsWith(`${token.replace(/-?\*$/, "")}-`));
  return tokens.some((token) => Object.hasOwn(groups, token) || family(token) || isRubySuite(token) || names.has(token));
}

/**
 * Distinct Rust tests that completion must execute, grouped by the cargo target that runs them;
 * every census entry depending on a test is listed so a failure names all of them.
 */
export function evidenceTargets(units) {
  const targets = new Map();
  for (const [file, inventory] of Object.entries(units.files)) {
    for (const item of inventory.tests) {
      if (item.requiredStatus !== "rust-evidence") continue;
      const key = `${item.evidenceTest.file}::${item.evidenceTest.name}`;
      const target = targets.get(key) ?? { file: item.evidenceTest.file, name: item.evidenceTest.name, referencedBy: [] };
      target.referencedBy.push(`${file}:${item.name}`);
      targets.set(key, target);
    }
  }
  return [...targets.values()];
}

/** Where evidence test binaries build, so the analyzed binary under `target/` is never rewritten. */
export const EVIDENCE_TARGET_DIR = "target/evidence";

/**
 * Cargo arguments addressing one test exactly. Integration suites are `tests/<suite>.rs`;
 * library tests are addressed by module path (`src/config/tests.rs` -> `config::tests::<name>`,
 * `src/analysis/mod.rs` -> `analysis::tests::<name>`, `src/lib.rs` -> `tests::<name>`). Returns
 * null for files cargo cannot address this way (`src/main.rs`, files under `tests/<dir>/`).
 */
export function cargoTestArgs(target) {
  const { file, name } = target;
  if (/^tests\/[^/]+\.rs$/.test(file)) {
    return ["test", "--test", file.slice("tests/".length, -".rs".length), "--", "--exact", name];
  }
  if (!file.startsWith("src/") || file === "src/main.rs") return null;
  const segments = file.slice("src/".length, -".rs".length).split("/");
  if (segments.at(-1) === "mod" || segments.at(-1) === "lib") segments.pop();
  if (segments.at(-1) !== "tests") segments.push("tests");
  return ["test", "--lib", "--", "--exact", `${segments.join("::")}::${name}`];
}

/**
 * Groups evidence targets by the cargo binary that runs them and keeps each target's own
 * fully qualified test name, so one spawn per binary runs every requested test exactly. libtest
 * accepts several filters after one `--exact` (`cargo test --lib -- --exact a b` ran both: `2 passed`).
 */
export function evidenceBatches(targets) {
  const batches = new Map();
  for (const target of targets) {
    const args = cargoTestArgs(target);
    const key = args.slice(0, -1).join(" ");
    const batch = batches.get(key) ?? { prefix: args.slice(0, -1), names: [], targets: [] };
    batch.names.push(args.at(-1));
    batch.targets.push(target);
    batches.set(key, batch);
  }
  return [...batches.values()].map(({ prefix, names, targets: members }) => ({ targets: members, args: [...prefix, ...names] }));
}

/**
 * Per-test outcomes from libtest's `test <name> ... ok|FAILED|ignored` lines, keyed by the
 * printed name, which is the `--exact` filter of each target. The `- should panic` suffix and
 * `ignored, <reason>` are libtest variants of the same line. Only the run section is read: the
 * `failures:` section that follows echoes captured test output, which may itself contain lines
 * shaped like results.
 */
export function testOutcomes(stdout) {
  const outcomes = new Map();
  const run = (stdout ?? "").split(/^failures:$/m)[0];
  for (const match of run.matchAll(/^test (\S+)(?: - should panic)? \.\.\. (\w+)/gm)) outcomes.set(match[1], match[2]);
  return outcomes;
}

/**
 * libtest's summary line for a run: the last `test result:` line, which follows any captured
 * output in the `failures:` section. Null when the run printed none.
 */
export function testSummary(stdout) {
  return [...(stdout ?? "").matchAll(/^test result: .*$/gm)].at(-1)?.[0] ?? null;
}

/**
 * The evidence targets of a batch that are not established, with their outcome; the only
 * decision point for evidence, on the success path as well as the failure path. A run is
 * complete when cargo exited without a spawn error or signal, libtest printed a `test result`
 * summary whose passed, failed and ignored counts each match its per-test lines, and the exit
 * status agrees with that summary:
 * status 0 with `ok` and no failures, or a nonzero status with `FAILED` and at least one failure.
 * A complete run blames each requested test that did not print `ok`, so an unrelated passing
 * summary establishes nothing. An incomplete run blames every target, even ones that printed `ok`.
 */
export function evidenceFailures(run, batch) {
  const outcomes = testOutcomes(run.stdout);
  const summary = /^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored/.exec(testSummary(run.stdout) ?? "");
  const failures = Number(summary?.[3]);
  const statusAgrees = run.status === 0 ? summary?.[1] === "ok" && failures === 0 : summary?.[1] === "FAILED" && failures > 0;
  /** How many per-test lines printed the given outcome. */
  const printed = (outcome) => [...outcomes.values()].filter((value) => value === outcome).length;
  const completed = !run.error && !run.signal && summary !== null && statusAgrees
    && Number(summary[2]) === printed("ok") && failures === printed("FAILED") && Number(summary[4]) === printed("ignored")
    && outcomes.size === printed("ok") + printed("FAILED") + printed("ignored");
  const names = batch.args.slice(-batch.targets.length);
  return batch.targets
    .map((target, index) => ({ target, outcome: completed ? outcomes.get(names[index]) ?? "did not run" : "incomplete run" }))
    .filter(({ outcome }) => outcome !== "ok");
}

/**
 * Rust evidence runs only in completion mode, and only once every other blocker and group
 * deficit is gone (or when asked with `--evidence`): regression mode guards observed outcomes,
 * the inventory and corpus-coverage modes validate bookkeeping without executing anything, and a
 * run that is already blocked gains nothing from compiling the test harness.
 */
export function executesEvidence({ regression, inventoryOnly, blocked = false, forced = false }) {
  return !regression && !inventoryOnly && (forced || !blocked);
}
