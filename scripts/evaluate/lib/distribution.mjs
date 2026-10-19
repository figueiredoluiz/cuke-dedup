// Distribution and coverage of the recall corpus, from its manifest alone: cases and expectations per
// rule, outcome, language, framework, axis, origin, verdict and fixture root, and the cells that are
// thin. It runs no binary, so it can say what the corpus holds but not whether it passes; the corpus
// check and `corpus:evaluate` do that. A value with few cases is where a regression is likeliest to
// go unnoticed, so the report names those instead of only totalling.

import { AXES, LANGUAGES, ORIGINS } from "../../check/lib/manifest-schema.mjs";
import { isolatable } from "../../check/lib/negative-isolation.mjs";

/** Shown for a case that lacks a label, so a gap in the manifest is a row in the report, not a crash. */
const UNLABELLED = "(unlabelled)";

/** Counts of each value, sorted by descending count then name. */
function tally(values) {
  const counts = new Map();
  for (const value of values) counts.set(value, (counts.get(value) ?? 0) + 1);
  return [...counts].sort((left, right) => right[1] - left[1] || left[0].localeCompare(right[0])).map(([value, cases]) => ({ value, cases }));
}

/**
 * The expectations a case declares for one rule, by outcome.
 * @param {object} testCase Recall manifest case.
 * @param {string} rule Rule name.
 * @returns {{positives: number, absent: number, knownMisses: number, ambiguous: number, isolated: number}}
 */
function expectationsFor(testCase, rule) {
  /** The case's entries of one kind for the rule. */
  const of = (key) => (testCase[key] || []).filter((entry) => entry.rule === rule);
  const absent = of("expectedAbsent");
  return {
    positives: of("expectedFindings").reduce((sum, entry) => sum + (entry.count ?? 1), 0),
    absent: absent.length,
    knownMisses: of("knownMisses").length,
    ambiguous: of("expectedAmbiguous").length,
    isolated: absent.filter((entry) => entry.isolation !== undefined).length,
  };
}

/**
 * Summarizes a recall manifest.
 * @param {object} manifest Parsed recall manifest.
 * @param {{floor?: number, behaviorCases?: number}} [options] `floor`: a value with this many cases or fewer
 *   is thin (default 1). `behaviorCases`: the behavior corpus's case count, which carries no labels.
 * @returns {object} Totals per dimension, per rule, an axis-by-rule matrix of case counts, fixture
 *   roots, and the thin cells.
 */
export function distribution(manifest, { floor = 1, behaviorCases = 0 } = {}) {
  const cases = manifest.cases;
  /** The rules a case declares any expectation for. */
  const ruleOf = (testCase) => [...new Set(["expectedFindings", "expectedAbsent", "knownMisses", "expectedAmbiguous"].flatMap((key) => (testCase[key] || []).map((entry) => entry.rule)))];
  const rules = [...new Set(cases.flatMap(ruleOf))].sort();
  const byRule = rules.map((rule) => {
    const touching = cases.filter((testCase) => ruleOf(testCase).includes(rule));
    const sums = touching.map((testCase) => expectationsFor(testCase, rule));
    /** The sum of one count over the cases touching the rule. */
    const total = (key) => sums.reduce((sum, entry) => sum + entry[key], 0);
    return {
      rule,
      cases: touching.length,
      positives: total("positives"),
      absent: total("absent"),
      knownMisses: total("knownMisses"),
      ambiguous: total("ambiguous"),
      isolated: total("isolated"),
      isolatable: isolatable(rule),
    };
  });
  /** The axes a case lists, none when it has no label. */
  const axesOf = (testCase) => testCase.axes ?? [];
  const matrix = Object.keys(AXES).map((axis) => ({
    axis,
    cases: cases.filter((testCase) => axesOf(testCase).includes(axis)).length,
    rules: Object.fromEntries(rules.map((rule) => [rule, cases.filter((testCase) => axesOf(testCase).includes(axis) && ruleOf(testCase).includes(rule)).length])),
  }));
  const dimensions = {
    role: tally(cases.map((testCase) => testCase.role ?? UNLABELLED)),
    origin: tally(cases.map((testCase) => testCase.origin ?? UNLABELLED)),
    language: tally(cases.map((testCase) => testCase.language ?? UNLABELLED)),
    verdict: tally(cases.map((testCase) => testCase.verdict ?? UNLABELLED)),
    framework: tally(cases.flatMap((testCase) => testCase.frameworks ?? [UNLABELLED])),
  };
  /** The values a dimension has at least one case for. */
  const present = (name) => dimensions[name].map((entry) => entry.value);
  const thin = {
    axes: matrix.filter((row) => row.cases <= floor).map((row) => ({ axis: row.axis, cases: row.cases })),
    frameworks: dimensions.framework.filter((row) => row.cases <= floor),
    rulesWithoutNegatives: byRule.filter((row) => row.absent === 0).map((row) => row.rule),
    rulesWithoutPositives: byRule.filter((row) => row.positives === 0).map((row) => row.rule),
    languagesMissing: LANGUAGES.filter((language) => !present("language").includes(language)),
    originsMissing: ORIGINS.filter((origin) => !present("origin").includes(origin)),
    casesWithoutAxes: cases.filter((testCase) => axesOf(testCase).length === 0).map((testCase) => testCase.name),
    axesOutsideVocabulary: [...new Set(cases.flatMap(axesOf))].filter((axis) => !Object.hasOwn(AXES, axis)).sort(),
  };
  return {
    cases: cases.length,
    floor,
    behaviorCases,
    dimensions,
    rules: byRule,
    axes: matrix,
    roots: tally(cases.map((testCase) => testCase.path)).map((row) => ({ root: row.value, cases: row.cases })),
    unisolatedNegatives: byRule.filter((row) => row.isolatable).reduce((sum, row) => sum + row.absent - row.isolated, 0),
    thin,
  };
}

/** A table row from cells. */
function row(...cells) {
  return `| ${cells.join(" | ")} |`;
}

/**
 * Renders the distribution as markdown.
 * @param {object} report Result of {@link distribution}.
 * @returns {string}
 */
export function renderDistribution(report) {
  const lines = [`# Recall corpus distribution`, "", `${report.cases} cases. A value is thin at ${report.floor} case${report.floor === 1 ? "" : "s"} or fewer.`, ""];
  for (const [name, values] of Object.entries(report.dimensions)) {
    lines.push(`## By ${name}`, "", row("Value", "Cases"), row("---", "---:"));
    for (const entry of values) lines.push(row(entry.value, entry.cases));
    lines.push("");
  }
  lines.push("## By rule", "", row("Rule", "Cases", "Positives", "Absent", "Isolated", "Known misses", "Ambiguous"), row("---", "---:", "---:", "---:", "---:", "---:", "---:"));
  for (const entry of report.rules) lines.push(row(entry.rule, entry.cases, entry.positives, entry.absent, entry.isolatable ? entry.isolated : "n/a", entry.knownMisses, entry.ambiguous));
  const ruleNames = report.rules.map((entry) => entry.rule);
  lines.push("", "## By axis and rule (cases)", "", row("Axis", "Cases", ...ruleNames), row("---", "---:", ...ruleNames.map(() => "---:")));
  for (const entry of report.axes) lines.push(row(entry.axis, entry.cases, ...ruleNames.map((rule) => entry.rules[rule] || "·")));
  lines.push("", "## Fixture roots", "", row("Root", "Cases"), row("---", "---:"));
  for (const entry of report.roots) lines.push(row(entry.root, entry.cases));
  lines.push("", "## Thin cells", "");
  const { thin } = report;
  /** A comma-separated list, or "none". */
  const list = (items) => (items.length === 0 ? "none" : items.join(", "));
  lines.push(`- Axes at or below the floor: ${list(thin.axes.map((entry) => `${entry.axis} (${entry.cases})`))}`);
  lines.push(`- Frameworks at or below the floor: ${list(thin.frameworks.map((entry) => `${entry.value} (${entry.cases})`))}`);
  lines.push(`- Rules with no negative: ${list(thin.rulesWithoutNegatives)}`);
  lines.push(`- Rules with no positive: ${list(thin.rulesWithoutPositives)}`);
  lines.push(`- Languages with no case: ${list(thin.languagesMissing)}`);
  lines.push(`- Origins with no case: ${list(thin.originsMissing)}`);
  lines.push(`- Cases with no axes: ${list(thin.casesWithoutAxes)}`);
  lines.push(`- Axes outside the vocabulary: ${list(thin.axesOutsideVocabulary)}`);
  lines.push(`- Handler and matcher negatives without isolation: ${report.unisolatedNegatives}`);
  lines.push(`- Behavior corpus cases, which carry no labels: ${report.behaviorCases}`);
  return `${lines.join("\n")}\n`;
}
