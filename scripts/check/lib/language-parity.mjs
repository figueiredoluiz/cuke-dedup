import assert from "node:assert/strict";
import { countMatches } from "./recall-oracle.mjs";

// Compare observable semantics, not language-specific fingerprint IDs or source spelling.
export function outcome(report, exit) {
  const location = ({ path, line }) => ({ path: path.replace(/steps\.(ts|rb)$/, "steps"), line });
  const sorted = (items) => items.sort((a, b) => JSON.stringify(a).localeCompare(JSON.stringify(b)));
  return {
    definitions: report.summary.definitionsAnalyzed,
    featureSteps: report.summary.featureStepsAnalyzed,
    complete: !report.corpus.incomplete,
    truncated: report.findingsTruncated,
    exit,
    findings: sorted(report.findings.filter((item) => item.suppression === null).map((item) => ({
      rule: item.rule,
      severity: item.severity,
      primary: location(item.primary),
      related: sorted(item.related.map(location)),
    }))),
  };
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
    assert.ok(Number.isSafeInteger(item.expected.definitions) && item.expected.definitions >= 0);
    assert.ok(Number.isSafeInteger(item.expected.featureSteps) && item.expected.featureSteps >= 0);
    assert.equal(typeof item.expected.complete, "boolean");
    assert.ok(Array.isArray(item.expected.requiredFindings));
    assert.ok(Array.isArray(item.expected.requiredAbsent));
    for (const finding of [...item.expected.requiredFindings, ...item.expected.requiredAbsent]) {
      assert.equal(typeof finding.rule, "string");
      assert.ok(finding.rule.length > 0);
      if (finding.count !== undefined) assert.ok(Number.isSafeInteger(finding.count) && finding.count > 0);
    }
    assert.ok([...item.expected.requiredFindings, ...item.expected.requiredAbsent].some((e) => e.rule === item.rule));
    for (const finding of item.expected.requiredFindings) {
      assert.ok(!item.expected.requiredAbsent.some((absent) => absent.rule === finding.rule));
    }
  }
}
