/**
 * The census and duplication-summary comparisons of a case against its report, the one list both
 * the gate and the evaluation report read. `census` marks the inputs the run analyzed.
 * @param {object} testCase Behavior manifest case.
 * @param {object} report The run's JSON report.
 * @returns {{label: string, census: boolean, actual: *, expected: *}[]}
 */
export function summaryComparisons(testCase, report) {
  const { expected } = testCase;
  const { summary } = report;
  return [
    { label: "definitions", census: true, actual: summary.definitionsAnalyzed, expected: expected.definitions },
    { label: "feature steps", census: true, actual: summary.featureStepsAnalyzed, expected: expected.featureSteps },
    { label: "duplicated definitions", census: false, actual: summary.duplication?.duplicatedDefinitions, expected: expected.duplicatedDefinitions },
    { label: "percentage", census: false, actual: summary.duplication?.percentage, expected: expected.percentage },
    { label: "threshold", census: false, actual: summary.duplication?.threshold, expected: expected.threshold },
    { label: "result", census: false, actual: summary.duplication?.passed, expected: expected.passed },
  ];
}
