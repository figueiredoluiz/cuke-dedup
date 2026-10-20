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

/** The corpora in report order; a result naming another corpus sorts after them. */
export const CORPUS_ORDER = ["recall", "behavior", "language-parity", "ruby-parity"];

/** Orders two corpus names: known corpora in report order, then any other by name. */
const byCorpus = (x, y) => (CORPUS_ORDER.indexOf(x) + 1 || 99) - (CORPUS_ORDER.indexOf(y) + 1 || 99) || x.localeCompare(y);

/** A key identifying a case across corpora; a paired case run in two lanes is one case. */
const caseKey = (result) => `${result.corpus}\0${result.name}`;

/** The count fields every row carries. */
const KEYS = ["tp", "fp", "fn", "tn", "knownMisses"];

/** Counts with every field zero. */
const zero = () => Object.fromEntries(KEYS.map((key) => [key, 0]));

/** Adds `counts` into `sum`. */
function add(sum, counts) {
  for (const key of KEYS) sum[key] += counts[key];
  return sum;
}

/**
 * Sums counts keyed by a function of (result, rule) into rows with metrics, sorted by corpus order,
 * then lane, then rule.
 * @param {object[]} observed Observable classified results.
 * @param {(result: object) => object} identity The row identity fields a result contributes to.
 * @returns {object[]} Rows `{corpus, lane, rule, cases, ...counts, ...metrics}`.
 */
function rows(observed, identity) {
  const byKey = new Map();
  for (const result of observed) {
    const who = identity(result);
    for (const [rule, counts] of Object.entries(result.rules)) {
      const key = JSON.stringify([who, rule]);
      const row = byKey.get(key) ?? { ...who, rule, cases: new Set(), ...zero() };
      add(row, counts);
      row.cases.add(caseKey(result));
      byKey.set(key, row);
    }
  }
  return [...byKey.values()]
    .sort((x, y) => byCorpus(x.corpus, y.corpus) || (x.lane ?? "").localeCompare(y.lane ?? "") || x.rule.localeCompare(y.rule))
    .map((row) => ({ ...row, cases: row.cases.size, ...metrics(row) }));
}

/** Sums the counts of rows into one total with metrics. */
function total(list) {
  const sum = list.reduce((acc, row) => add(acc, row), zero());
  return { ...sum, ...metrics(sum) };
}

/**
 * Sums classified cases into a report. Corpora stay separate: `rules` has one row per corpus,
 * lane and rule, and `corpora` one summary per corpus. `ruleTotals` sums each rule across all
 * corpora and lanes, so a case with two lanes counts once per lane there.
 * @param {object[]} classified Results of the classifiers, each naming its `corpus` and `lane`.
 * @returns {{cases: number, observed: number, unobservable: object[], ambiguous: number,
 *   totals: object, corpora: object[], rules: object[], ruleTotals: object[], entries: object[]}}
 *   `entries` are the findings and expectations that disagree, with known misses listed last.
 */
export function aggregate(classified) {
  const observed = classified.filter((result) => result.observable);
  const ruleRows = rows(observed, (result) => ({ corpus: result.corpus, lane: result.lane ?? null }));
  const ruleTotals = rows(observed, () => ({ corpus: "all", lane: null }));
  /** Distinct cases of a list of results. */
  const caseCount = (list) => new Set(list.map(caseKey)).size;
  const corpora = [...new Set(classified.map((result) => result.corpus))].sort(byCorpus).map((corpus) => {
    const own = classified.filter((result) => result.corpus === corpus);
    return {
      corpus,
      cases: caseCount(own),
      observed: caseCount(own.filter((result) => result.observable)),
      unobservable: own.filter((result) => !result.observable).length,
      totals: total(ruleRows.filter((row) => row.corpus === corpus)),
    };
  });
  const entries = classified.flatMap((result) => result.entries);
  /** Whether an entry is an open known miss. */
  const known = (entry) => entry.kind === "known-miss";
  return {
    cases: caseCount(classified),
    observed: caseCount(observed),
    unobservable: classified.filter((result) => !result.observable).map(({ corpus, lane, name, runFailure }) => ({ corpus, lane, name, runFailure })),
    ambiguous: classified.reduce((sum, result) => sum + result.ambiguous, 0),
    totals: total(ruleRows),
    corpora,
    rules: ruleRows,
    ruleTotals,
    entries: [...entries.filter((entry) => !known(entry)), ...entries.filter(known)],
  };
}

/** A metric as a percentage with one decimal, or a dash when undefined. */
function percent(value) {
  return value === null ? "-" : `${(value * 100).toFixed(1)}%`;
}

/**
 * Renders a report as markdown. Every row names its corpus: one table lists each corpus's rules
 * (with a lane for the paired corpus) followed by that corpus's total, then the rules summed across
 * all corpora.
 * @param {object} report Result of {@link aggregate}.
 * @returns {string}
 */
export function renderMarkdown(report) {
  const lines = [
    "# Corpus evaluation",
    "",
    `${report.observed} of ${report.cases} cases observed in ${report.corpora.length} ${report.corpora.length === 1 ? "corpus" : "corpora"}; ${report.ambiguous} ambiguous findings set aside; entries to review: ${report.entries.length}.`,
    "",
    "| Corpus | Lane | Rule | Cases | TP | FP | FN | Known misses | TN | Precision | Recall | F1 |",
    "| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |",
  ];
  /** One table row. */
  const line = (row) => `| ${row.corpus} | ${row.lane ?? "-"} | ${row.rule} | ${row.cases} | ${row.tp} | ${row.fp} | ${row.fn} | ${row.knownMisses} | ${row.tn} | ${percent(row.precision)} | ${percent(row.recall)} | ${percent(row.f1)} |`;
  for (const { corpus, observed, totals } of report.corpora) {
    for (const row of report.rules.filter((candidate) => candidate.corpus === corpus)) lines.push(line(row));
    lines.push(line({ corpus, lane: null, rule: "**all rules**", cases: observed, ...totals }));
  }
  for (const row of report.ruleTotals) lines.push(line(row));
  lines.push(line({ corpus: "all", lane: null, rule: "**all rules**", cases: report.observed, ...report.totals }));
  if (report.unobservable.length > 0) {
    lines.push("", "## Cases that could not be scored", "");
    for (const { corpus, lane, name, runFailure } of report.unobservable) lines.push(`- ${corpus}${lane ? ` (${lane})` : ""} \`${name}\`: ${runFailure}`);
  }
  lines.push("", "## Entries to review", "");
  if (report.entries.length === 0) lines.push("None.");
  for (const entry of report.entries) {
    const what = entry.detail ?? (entry.observed ? `${entry.observed.primary} ${entry.observed.matchers?.join(" / ") ?? entry.observed.message}` : JSON.stringify(entry.expected?.matchers ?? entry.expected));
    const why = entry.error ? ` (${entry.error})` : "";
    lines.push(`- ${entry.corpus}${entry.lane ? ` (${entry.lane})` : ""} \`${entry.case}\` ${entry.rule} **${entry.kind}**: ${what}${why}`);
  }
  return `${lines.join("\n")}\n`;
}
