import assert from "node:assert/strict";

// Validate the language-neutral core of a corpus case's expected outcome.
export function validateOutcomeContract(expected, focusRule) {
  assert.ok(Number.isSafeInteger(expected.definitions) && expected.definitions >= 0);
  assert.ok(Number.isSafeInteger(expected.featureSteps) && expected.featureSteps >= 0);
  assert.equal(typeof expected.complete, "boolean");
  assert.ok(Array.isArray(expected.requiredFindings));
  assert.ok(Array.isArray(expected.requiredAbsent));
  const expectations = [...expected.requiredFindings, ...expected.requiredAbsent];
  for (const finding of expectations) {
    assert.equal(typeof finding.rule, "string");
    assert.ok(finding.rule.length > 0);
    if (finding.count !== undefined) assert.ok(Number.isSafeInteger(finding.count) && finding.count > 0);
  }
  assert.ok(expectations.some((finding) => finding.rule === focusRule));
  for (const finding of expected.requiredFindings) {
    assert.ok(!expected.requiredAbsent.some((absent) => absent.rule === finding.rule));
  }
}

// Normalize observable analysis results for conformance and regression comparisons.
export function normalizeOutcome(report, exit, physicalToLogicalSuffixes = {}) {
  const location = ({ path, line }) => {
    for (const [physicalSuffix, logicalSuffix] of Object.entries(physicalToLogicalSuffixes)) {
      if (path.endsWith(physicalSuffix)) {
        return { path: `${path.slice(0, -physicalSuffix.length)}${logicalSuffix}`, line };
      }
    }
    return { path, line };
  };
  const sorted = (items) => items.sort((a, b) => {
    const left = JSON.stringify(a), right = JSON.stringify(b);
    return left < right ? -1 : left > right ? 1 : 0;
  });
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
