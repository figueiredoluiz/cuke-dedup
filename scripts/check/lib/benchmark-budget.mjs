import assert from "node:assert/strict";

const deterministicFields = [
  "findingCount",
  "analysisComplete",
  "analysisTruncated",
  "candidateComparisonsEvaluated",
  "skippedCandidateComparisons",
  "truncatedStructuralClasses",
];

/** Validate deterministic benchmark outcomes and stable resource ceilings without gating on time. */
export function validateBenchmark(report, budgets) {
  assert.equal(report.schemaVersion, 3, "unsupported benchmark schema");
  assert.equal(budgets.schemaVersion, 1, "unsupported benchmark-budget schema");
  assert.equal(report.smoke, false, "performance budgets require the full benchmark profile");
  assert.ok(Array.isArray(report.results), "benchmark report has no results");
  const failures = [];
  const advisories = [];
  if (!Number.isInteger(report.binaryBytes) || report.binaryBytes <= 0) {
    failures.push("benchmark did not record a positive release binary size");
  } else if (report.binaryBytes > budgets.binaryBytes) {
    failures.push(`release binary ${report.binaryBytes} bytes exceeds ${budgets.binaryBytes}`);
  }

  const observed = new Map();
  for (const result of report.results) {
    const key = `${result.profile}:${result.definitions}`;
    if (observed.has(key)) failures.push(`${key}: duplicate benchmark result`);
    observed.set(key, result);
  }
  for (const key of observed.keys()) {
    if (!(key in budgets.cases)) failures.push(`${key}: unexpected benchmark case`);
  }
  for (const [key, budget] of Object.entries(budgets.cases)) {
    const result = observed.get(key);
    if (!result) {
      failures.push(`${key}: missing benchmark case`);
      continue;
    }
    for (const field of deterministicFields) {
      if (result[field] !== budget[field]) {
        failures.push(`${key}: ${field} ${JSON.stringify(result[field])} differs from ${JSON.stringify(budget[field])}`);
      }
    }
    if (!Number.isFinite(result.peakMemoryBytes)) {
      failures.push(`${key}: peak memory was not measured`);
    } else if (result.peakMemoryBytes > budget.peakMemoryBytes) {
      failures.push(`${key}: peak memory ${result.peakMemoryBytes} bytes exceeds ${budget.peakMemoryBytes}`);
    }
    advisories.push(`${key} ${result.analysisMedianMs} ms, ${result.peakMemoryBytes} bytes peak`);
  }
  return { advisories, failures };
}
