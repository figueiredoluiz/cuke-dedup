import assert from "node:assert/strict";

const deterministicFields = [
  "findingCount",
  "analysisComplete",
  "analysisTruncated",
  "candidateComparisonsEvaluated",
  "skippedCandidateComparisons",
  "truncatedStructuralClasses",
];

/** Median timings a scaling rule may compare. */
const scalingMetrics = ["medianMs", "discoveryMedianMs", "parsingMedianMs", "analysisMedianMs"];

/** Smallest median, in milliseconds, that a scaling ratio may divide by; faster cases are noise. */
export const MIN_SCALING_BASE_MS = 50;

/**
 * Validate deterministic benchmark outcomes, stable resource ceilings and superlinear growth.
 * Absolute time is advisory; growth is gated as a ratio of medians from the same run.
 */
export function validateBenchmark(report, budgets) {
  assert.equal(report.schemaVersion, 3, "unsupported benchmark schema");
  assert.equal(budgets.schemaVersion, 2, "unsupported benchmark-budget schema");
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
  // An absent or empty map would silently disable every growth check.
  const scalingRules = Object.entries(budgets.scaling ?? {});
  if (scalingRules.length === 0) failures.push("benchmark budgets define no scaling rules");
  for (const [profile, scaling] of scalingRules) {
    // A malformed rule would otherwise compare against undefined or NaN and pass silently.
    const malformed = scalingRuleErrors(scaling);
    if (malformed.length > 0) {
      failures.push(...malformed.map((error) => `${profile}: ${error}`));
      continue;
    }
    for (const size of [scaling.small, scaling.large]) {
      if (!(`${profile}:${size}` in budgets.cases)) failures.push(`${profile}:${size}: scaling case has no budget`);
    }
    const small = observed.get(`${profile}:${scaling.small}`);
    const large = observed.get(`${profile}:${scaling.large}`);
    if (!small || !large) {
      failures.push(`${profile}: scaling needs cases ${scaling.small} and ${scaling.large}`);
      continue;
    }
    const base = small[scaling.metric];
    const top = large[scaling.metric];
    if (!Number.isFinite(base) || !Number.isFinite(top)) {
      failures.push(`${profile}: ${scaling.metric} was not measured`);
    } else if (base < MIN_SCALING_BASE_MS) {
      failures.push(`${profile}:${scaling.small} ${scaling.metric} ${base} ms is too small to gate growth (minimum ${MIN_SCALING_BASE_MS} ms)`);
    } else {
      // Compare the exact ratio; round only for display, so 6.004 cannot pass a 6x ceiling.
      const ratio = top / base;
      const shown = ratio.toFixed(2);
      if (ratio > scaling.maxRatio) {
        failures.push(`${profile}: ${scaling.metric} grows ${shown}x from ${scaling.small} to ${scaling.large} definitions, above ${scaling.maxRatio}x`);
      }
      advisories.push(`${profile} ${scaling.small}->${scaling.large} ${shown}x (limit ${scaling.maxRatio}x)`);
    }
  }
  return { advisories, failures };
}

/** Problems with a scaling rule's own fields: sizes, metric and ceiling. */
function scalingRuleErrors(scaling) {
  const errors = [];
  const { small, large, metric, maxRatio } = scaling ?? {};
  if (!Number.isInteger(small) || !Number.isInteger(large) || small <= 0 || large <= small) {
    errors.push(`small and large must be positive integers with small < large (got ${small} and ${large})`);
  }
  if (!scalingMetrics.includes(metric)) {
    errors.push(`metric must be one of ${scalingMetrics.join(", ")} (got ${metric})`);
  }
  if (!Number.isFinite(maxRatio) || maxRatio <= 1) {
    errors.push(`maxRatio must be a finite number above 1 (got ${maxRatio})`);
  }
  return errors;
}
