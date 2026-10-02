import assert from "node:assert/strict";
import { countMatches } from "./recall-oracle.mjs";
import { normalizeOutcome, validateOutcomeContract } from "./behavior-spec.mjs";

// Compatibility wrapper for paired TypeScript/Ruby conformance.
export function outcome(report, exit) {
  return normalizeOutcome(report, exit, { "steps.ts": "steps", "steps.rb": "steps" });
}

export function deficits(expected, report, exit) {
  const actual = outcome(report, exit);
  const gaps = [];
  for (const key of ["definitions", "featureSteps", "complete"]) {
    if (actual[key] !== expected[key]) gaps.push(`${key}: expected ${expected[key]}, got ${actual[key]}`);
  }
  if (actual.truncated !== 0) gaps.push("findings were truncated");
  if (![0, 1, 2].includes(exit)) gaps.push(`invalid analyzer exit: ${exit}`);
  if (!actual.complete && exit !== 2) gaps.push("incomplete analysis did not fail strict mode");
  const active = report.findings.filter((item) => item.suppression === null);
  if (actual.complete && exit !== Number(active.some((item) => item.severity === "error"))) {
    gaps.push("complete analysis exit disagrees with active error findings");
  }
  for (const wanted of expected.requiredFindings) {
    const count = countMatches(active, wanted);
    if (count !== (wanted.count ?? 1)) gaps.push(`${wanted.rule}: expected ${wanted.count ?? 1}, got ${count}`);
  }
  for (const forbidden of expected.requiredAbsent) {
    if (countMatches(active, forbidden)) gaps.push(`forbidden ${forbidden.rule}`);
  }
  return gaps;
}

export function classify(left, right) {
  if (left.error || right.error) return "unmeasured";
  if (!left.deficits.length && right.deficits.length) return "ruby-behind";
  if (left.deficits.length && !right.deficits.length) return "typescript-behind";
  const equal = JSON.stringify(left.outcome) === JSON.stringify(right.outcome);
  if (left.deficits.length && right.deficits.length) return equal ? "shared-limitation" : "both-have-gaps";
  return equal ? "equivalent" : "outcome-difference";
}

export function validateManifest(manifest) {
  assert.equal(manifest.schemaVersion, 1);
  assert.match(manifest.baselineCommit, /^[a-f0-9]{40}$/);
  assert.ok(manifest.cases.length > 0);
  const ids = new Set();
  for (const item of manifest.cases) {
    assert.match(item.id, /^[a-z0-9-]+$/);
    assert.ok(!ids.has(item.id), `duplicate case: ${item.id}`);
    ids.add(item.id);
    for (const language of ["typescript", "ruby"]) assert.equal(typeof item.sources[language], "string");
    assert.equal(typeof item.feature, "string");
    assert.equal(typeof item.intent, "string");
    validateOutcomeContract(item.expected, item.rule);
  }
}
