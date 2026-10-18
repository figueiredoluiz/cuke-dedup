// Case metadata for the recall manifest. The fields classify a case for the evaluation reports
// (per-rule, per-language, per-framework counts); none of them changes what a case asserts. A label
// that the run can contradict is checked against the run: `role` against the case's expectations,
// `language` and `frameworks` against the `--emit-fingerprints` records. A decorative field would
// let the labels drift from the cases, so the others are format-checked only.

import assert from "node:assert/strict";
import { findingMatches } from "./recall-oracle.mjs";

export const ROLES = Object.freeze(["positive", "negative", "ambiguous", "variant"]);
export const ORIGINS = Object.freeze(["synthetic", "real", "regression"]);
export const LANGUAGES = Object.freeze(["ecmascript", "ruby"]);
export const VERDICTS = Object.freeze(["reviewed", "generated"]);

/**
 * Whether an ambiguity entry names one pair rather than a class of findings. `findingMatches`
 * ignores a falsy selector, and neither matcher text nor one endpoint identifies a pair when
 * definitions are shared, so the entry needs both endpoints: the primary location and the related
 * locations.
 * @param {object} entry An `expectedAmbiguous` entry.
 * @returns {boolean}
 */
function identifiesPair(entry) {
  const labels = entry.relatedLocations;
  return typeof entry.primaryPath === "string" && entry.primaryPath !== ""
    && Number.isSafeInteger(entry.primaryLine) && entry.primaryLine > 0
    && Array.isArray(labels) && labels.length > 0 && labels.every((label) => typeof label === "string" && label !== "");
}

const SLUG = /^[a-z][a-z0-9-]*$/;
const DATE = /^(\d{4})-(\d{2})-(\d{2})$/;

/**
 * Asserts `value` is one of `allowed`.
 * @param {string} name Case name, for the message.
 * @param {string} field Field being checked.
 * @param {unknown} value
 * @param {readonly string[]} allowed
 */
function assertOneOf(name, field, value, allowed) {
  assert.ok(allowed.includes(value), `${name}: ${field} must be one of ${allowed.join(", ")}, got ${JSON.stringify(value)}`);
}

/**
 * Asserts `value` is a non-empty array of distinct slugs.
 * @param {string} name Case name, for the message.
 * @param {string} field Field being checked.
 * @param {unknown} value
 */
function assertSlugs(name, field, value) {
  assert.ok(Array.isArray(value) && value.length > 0, `${name}: ${field} must be a non-empty array`);
  for (const entry of value) {
    assert.ok(typeof entry === "string" && SLUG.test(entry), `${name}: ${field} entry ${JSON.stringify(entry)} must be a lowercase slug`);
  }
  assert.equal(new Set(value).size, value.length, `${name}: ${field} repeats an entry`);
}

/**
 * Asserts `value` is a real calendar date, `YYYY-MM-DD`, not after `today`.
 * @param {string} name Case name, for the message.
 * @param {unknown} value
 * @param {string} today `YYYY-MM-DD`.
 */
function assertDate(name, value, today) {
  const parts = typeof value === "string" ? DATE.exec(value) : null;
  assert.ok(parts, `${name}: classifiedAt must be YYYY-MM-DD, got ${JSON.stringify(value)}`);
  const date = new Date(`${value}T00:00:00Z`);
  assert.ok(date.toISOString().startsWith(value), `${name}: classifiedAt ${value} is not a calendar date`);
  assert.ok(value <= today, `${name}: classifiedAt ${value} is in the future`);
}

/**
 * Validates one case's metadata and its consistency with the case's own expectations.
 * @param {object} testCase Manifest case.
 * @param {string} today `YYYY-MM-DD`, the latest allowed `classifiedAt`.
 */
export function validateCaseMetadata(testCase, today) {
  const { name } = testCase;
  assertOneOf(name, "role", testCase.role, ROLES);
  assertOneOf(name, "origin", testCase.origin, ORIGINS);
  assertOneOf(name, "language", testCase.language, LANGUAGES);
  assertOneOf(name, "verdict", testCase.verdict, VERDICTS);
  assertSlugs(name, "frameworks", testCase.frameworks);
  assertDate(name, testCase.classifiedAt, today);
  if (testCase.axes !== undefined) assertSlugs(name, "axes", testCase.axes);

  const findings = testCase.expectedFindings || [];
  const ambiguous = testCase.expectedAmbiguous;
  if (testCase.role === "ambiguous") {
    assert.ok(Array.isArray(ambiguous) && ambiguous.length > 0, `${name}: role ambiguous needs expectedAmbiguous entries`);
    for (const entry of ambiguous) {
      assert.ok(entry.rule, `${name}: an expectedAmbiguous entry names no rule: ${JSON.stringify(entry)}`);
      assert.ok(entry.reason, `${name}: an expectedAmbiguous entry needs a reason: ${JSON.stringify(entry)}`);
      // A rule alone, or a selector that spans several pairs, would exempt unrelated findings.
      assert.ok(
        identifiesPair(entry),
        `${name}: an expectedAmbiguous entry must identify one pair with primaryPath, primaryLine and relatedLocations: ${JSON.stringify(entry)}`,
      );
    }
  } else {
    assert.equal(ambiguous, undefined, `${name}: expectedAmbiguous needs role ambiguous`);
  }
  if (testCase.role === "positive") assert.ok(findings.length > 0, `${name}: role positive needs expectedFindings`);
  if (testCase.role === "negative") assert.equal(findings.length, 0, `${name}: role negative has expectedFindings; use positive`);
  if (testCase.role === "variant") {
    assert.ok(typeof testCase.variantOf === "string" && testCase.variantOf, `${name}: role variant needs variantOf`);
  } else {
    assert.equal(testCase.variantOf, undefined, `${name}: variantOf needs role variant`);
  }
}

/**
 * Validates metadata that relates cases to each other: a variant names another, non-variant case.
 * @param {object[]} cases Every manifest case.
 */
export function validateVariantReferences(cases) {
  const byName = new Map(cases.map((testCase) => [testCase.name, testCase]));
  for (const testCase of cases.filter((candidate) => candidate.role === "variant")) {
    const target = byName.get(testCase.variantOf);
    assert.ok(target, `${testCase.name}: variantOf names no case: ${testCase.variantOf}`);
    assert.notEqual(target.role, "variant", `${testCase.name}: variantOf must name the base case, not another variant`);
  }
}

/**
 * Holds the labels the run can contradict against its `--emit-fingerprints` records. Only a case
 * that expects no definitions may have no records.
 * @param {object} testCase Manifest case.
 * @param {object[]} records Parsed fingerprint records of the case's run.
 */
export function validateAgainstRecords(testCase, records) {
  // An empty record set proves nothing about a case that analyzes definitions: the run lost them.
  if (testCase.expectedDefinitions === 0) {
    assert.equal(records.length, 0, `${testCase.name}: the case expects no definitions, but the run wrote fingerprint records`);
    return;
  }
  assert.ok(records.length > 0, `${testCase.name}: no fingerprint records, but the case expects definitions`);
  const domains = [...new Set(records.map((record) => record.domain))];
  assert.deepEqual(domains, [testCase.language], `${testCase.name}: language label disagrees with the analyzed definitions`);
  const frameworks = [...new Set(records.map((record) => record.framework))].sort();
  assert.deepEqual([...testCase.frameworks].sort(), frameworks, `${testCase.name}: frameworks label disagrees with the analyzed definitions`);
}

/**
 * Splits a case's active findings into those the case owns and those it declares ambiguous. An
 * ambiguous finding is neither desired nor forbidden, so it is outside the recall ratio and needs
 * no owner among `expectedFindings`.
 * @param {object[]} findings Active findings of one case.
 * @param {object[]} ambiguous The case's `expectedAmbiguous` entries.
 * @returns {{owned: object[], ambiguous: object[]}}
 */
export function partitionAmbiguous(findings, ambiguous) {
  /** Whether any ambiguous entry selects the finding. */
  const declared = (finding) => ambiguous.some((entry) => findingMatches(finding, entry));
  return {
    owned: findings.filter((finding) => !declared(finding)),
    ambiguous: findings.filter(declared),
  };
}

/**
 * Whether a run's exit status is acceptable for a case. A case whose only expectations are
 * ambiguous has no required finding that could drive the status, so it accepts either
 * finding-dependent status (0 or 1) and still rejects an operational failure (2). A case that also
 * requires findings keeps its exact status: an ambiguous finding shifting it means the case should
 * be split.
 * @param {number|null} status Exit status of the run.
 * @param {number} expectedExit The case's `expectedExit`, 0 when absent.
 * @param {object} testCase Manifest case.
 * @returns {boolean}
 */
export function exitAcceptable(status, expectedExit, testCase) {
  const onlyAmbiguous = (testCase.expectedAmbiguous || []).length > 0 && (testCase.expectedFindings || []).length === 0;
  // Status 2 is never neutral, even when the case declares it as the expected status.
  if (onlyAmbiguous) return [0, 1].includes(status) && [0, 1].includes(expectedExit);
  return status === expectedExit;
}
