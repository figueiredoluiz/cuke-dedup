// Aggregates classified recall cases into per-rule metrics and renders them. Precision, recall and
// F1 come from counts the gate does not print: the gate stops at the first disagreement, so a
// passing corpus has no false positives or negatives and this report mostly confirms that. It is
// informative once a case is allowed to disagree (ambiguous, a known miss) or a real project is
// labelled.

/** A ratio rounded to four places, or `null` when its denominator is zero. */
function ratio(numerator, denominator) {
  return denominator === 0 ? null : Math.round((numerator / denominator) * 10000) / 10000;
}

/**
 * Per-rule precision, recall and F1 for summed counts. Recall counts known misses as desired
 * findings, as the corpus gate's baseline does.
 * @param {{tp: number, fp: number, fn: number, tn: number, knownMisses: number}} counts
 * @returns {{precision: number|null, recall: number|null, f1: number|null}}
 */
export function metrics({ tp, fp, fn, knownMisses }) {
  // F1 from the counts, not from the rounded ratios: it is defined whenever anything was expected
  // or reported, including when nothing was detected.
  return { precision: ratio(tp, tp + fp), recall: ratio(tp, tp + fn + knownMisses), f1: ratio(2 * tp, 2 * tp + fp + fn + knownMisses) };
}

/**
 * Sums classified cases into a report.
 * @param {object[]} classified Results of `classifyCase`.
 * @returns {{cases: number, observed: number, unobservable: object[], ambiguous: number,
 *   totals: object, rules: object[], entries: object[]}} Rules are sorted by name; `entries` are the
 *   findings and expectations that disagree, with known misses listed last.
 */
export function aggregate(classified) {
  const byRule = new Map();
  const casesByRule = new Map();
  for (const result of classified.filter((candidate) => candidate.observable)) {
    for (const [rule, counts] of Object.entries(result.rules)) {
      const sum = byRule.get(rule) ?? { tp: 0, fp: 0, fn: 0, tn: 0, knownMisses: 0 };
      for (const key of Object.keys(sum)) sum[key] += counts[key];
      byRule.set(rule, sum);
      casesByRule.set(rule, (casesByRule.get(rule) ?? 0) + 1);
    }
  }
  const rules = [...byRule.keys()].sort().map((rule) => ({ rule, cases: casesByRule.get(rule), ...byRule.get(rule), ...metrics(byRule.get(rule)) }));
  const totals = rules.reduce(
    (sum, row) => Object.fromEntries(Object.keys(sum).map((key) => [key, sum[key] + row[key]])),
    { tp: 0, fp: 0, fn: 0, tn: 0, knownMisses: 0 },
  );
  const entries = classified.flatMap((result) => result.entries);
  /** Whether an entry is an open known miss. */
  const known = (entry) => entry.kind === "known-miss";
  return {
    cases: classified.length,
    observed: classified.filter((result) => result.observable).length,
    unobservable: classified.filter((result) => !result.observable).map(({ name, runFailure }) => ({ name, runFailure })),
    ambiguous: classified.reduce((sum, result) => sum + result.ambiguous, 0),
    totals: { ...totals, ...metrics(totals) },
    rules,
    entries: [...entries.filter((entry) => !known(entry)), ...entries.filter(known)],
  };
}

/** A metric as a percentage with one decimal, or a dash when undefined. */
function percent(value) {
  return value === null ? "-" : `${(value * 100).toFixed(1)}%`;
}

/**
 * Renders a report as markdown.
 * @param {object} report Result of {@link aggregate}.
 * @returns {string}
 */
export function renderMarkdown(report) {
  const lines = [
    "# Corpus evaluation",
    "",
    `${report.observed} of ${report.cases} recall cases observed; ${report.ambiguous} ambiguous findings set aside.`,
    "",
    "| Rule | Cases | TP | FP | FN | Known misses | TN | Precision | Recall | F1 |",
    "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
  ];
  for (const row of [...report.rules, { rule: "**all**", cases: "", ...report.totals }]) {
    lines.push(`| ${row.rule} | ${row.cases} | ${row.tp} | ${row.fp} | ${row.fn} | ${row.knownMisses} | ${row.tn} | ${percent(row.precision)} | ${percent(row.recall)} | ${percent(row.f1)} |`);
  }
  if (report.unobservable.length > 0) {
    lines.push("", "## Cases that could not be observed", "");
    for (const { name, runFailure } of report.unobservable) lines.push(`- \`${name}\`: ${runFailure}`);
  }
  lines.push("", "## Entries to review", "");
  if (report.entries.length === 0) lines.push("None.");
  for (const entry of report.entries) {
    const what = entry.observed ? `${entry.observed.primary} ${entry.observed.matchers?.join(" / ") ?? entry.observed.message}` : JSON.stringify(entry.expected?.matchers ?? entry.expected);
    lines.push(`- \`${entry.case}\` ${entry.rule} **${entry.kind}**: ${what}`);
  }
  return `${lines.join("\n")}\n`;
}
