// Local verdict workflow for the real-project corpus. The sampler draws findings per rule and writes
// a labels file with empty verdicts; the reviewer fills them in by hand. The report computes
// precision from reviewed verdicts only. Labels name file locations, so they stay out of the
// repository: the sampler refuses any path git does not ignore.

import { compare, digest } from "./real-corpus.mjs";

/** Verdicts a reviewer may record; an unset verdict is `null` and counts as unreviewed. */
export const VERDICTS = Object.freeze(["true-positive", "false-positive", "unclear"]);

/** Labels schema version. */
const LABELS_SCHEMA = 1;

/**
 * The ids drawn for one rule: distinct ids ordered by a digest of seed, rule and id, capped at `perRule`.
 * @param {{id: string}[]} list The rule's findings.
 * @param {string} rule
 * @param {{perRule: number, seed: string}} options
 * @returns {string[]}
 */
function drawIds(list, rule, { perRule, seed }) {
  /** The draw order key of one finding. */
  const order = (id) => digest([seed, rule, id]);
  return [...new Set(list.map((entry) => entry.id))].sort((a, b) => compare(order(a), order(b))).slice(0, perRule);
}

/**
 * Draws up to `perRule` findings per rule. The draw orders findings by a digest of seed, rule and
 * id, so it is reproducible and does not depend on report order.
 * @param {object} snapshot Snapshot the run produced.
 * @param {Map<string, string[]>} locations `path:line:column` locations by finding id.
 * @param {{perRule: number, seed: string}} options
 * @returns {object} A labels file with every verdict unset.
 */
export function buildLabels(snapshot, locations, { perRule, seed }) {
  if (!Number.isInteger(perRule) || perRule < 1) throw new Error("perRule must be a positive integer");
  const samples = Object.entries(snapshot.findings).flatMap(([rule, list]) =>
    drawIds(list, rule, { perRule, seed }).map((id) => ({ id, rule, locations: locations.get(id), verdict: null, note: "" })));
  return { schemaVersion: LABELS_SCHEMA, suite: snapshot.suite, commit: snapshot.commit, seed, perRule, samples };
}

/**
 * Why a labels file must not be written to `path`, or `null`. A path git does not ignore could be
 * committed with file locations in it. The atomic create separately refuses an existing file.
 * @param {string} path
 * @param {{ignored: boolean}} state
 * @returns {string|null}
 */
export function labelsPathProblem(path, { ignored }) {
  if (!ignored) return `${path} is not ignored by git; labels name source locations and must stay local`;
  return null;
}

/**
 * Checks a labels file against the committed snapshot it labels.
 * @param {object} labels Parsed labels file.
 * @param {object} snapshot Committed snapshot of the same suite.
 * @returns {object[]} The samples.
 */
export function validateLabels(labels, snapshot) {
  const problems = [];
  if (labels?.schemaVersion !== LABELS_SCHEMA) problems.push(`schemaVersion must be ${LABELS_SCHEMA}`);
  if (labels?.suite !== snapshot.suite) problems.push(`labels are for suite ${labels?.suite}, not ${snapshot.suite}`);
  if (labels?.commit !== snapshot.commit) problems.push(`labels are for revision ${labels?.commit}, the snapshot pins ${snapshot.commit}`);
  const seen = new Set();
  for (const sample of Array.isArray(labels?.samples) ? labels.samples : []) {
    if (sample === null || typeof sample !== "object") {
      problems.push(`sample ${JSON.stringify(sample)} is not an object`);
      continue;
    }
    if (seen.has(sample.id)) problems.push(`duplicate sample ${sample.id}`);
    seen.add(sample.id);
    if (!(Object.hasOwn(snapshot.findings, sample.rule) ? snapshot.findings[sample.rule] : []).some((entry) => entry.id === sample.id)) problems.push(`sample ${sample.id} is not a ${sample.rule} finding in the snapshot`);
    if (sample.verdict !== null && !VERDICTS.includes(sample.verdict)) problems.push(`sample ${sample.id}: verdict ${JSON.stringify(sample.verdict)} is not one of ${VERDICTS.join(", ")} or null`);
  }
  if (!Array.isArray(labels?.samples)) problems.push("samples must be an array");
  else if (!Number.isInteger(labels.perRule) || labels.perRule < 1 || typeof labels.seed !== "string") problems.push("perRule and seed must record the draw");
  else {
    // Dropping or swapping samples after the draw would bias the precision, so the draw is re-derived.
    for (const [rule, list] of Object.entries(snapshot.findings)) {
      const drawn = drawIds(list, rule, labels);
      const kept = new Set(labels.samples.filter((sample) => sample?.rule === rule).map((sample) => sample.id));
      if (kept.size !== drawn.length || !drawn.every((id) => kept.has(id))) problems.push(`samples for ${rule} differ from the draw of seed ${labels.seed}, ${labels.perRule} per rule`);
    }
  }
  if (problems.length > 0) throw new Error(`invalid labels for ${snapshot.suite}:\n- ${problems.join("\n- ")}`);
  return labels.samples;
}

/** A row of counts for `rule` over `samples`; precision uses reviewed true and false positives only. */
function rowFor(rule, samples) {
  /** How many samples carry `verdict`. */
  const count = (verdict) => samples.filter((sample) => sample.verdict === verdict).length;
  const tp = count("true-positive");
  const fp = count("false-positive");
  return {
    rule,
    sampled: samples.length,
    tp,
    fp,
    unclear: count("unclear"),
    unreviewed: count(null),
    reviewed: tp + fp,
    precision: tp + fp === 0 ? null : tp / (tp + fp),
  };
}

/**
 * Per-rule precision from reviewed verdicts, per suite and pooled across suites.
 * @param {{suite: string, samples: object[]}[]} sets Validated samples per suite.
 * @returns {{suites: {suite: string, rows: object[]}[], pooled: object[]}} Rows sorted by rule.
 */
export function precisionReport(sets) {
  /** Rows for every rule in `samples`, sorted by rule. */
  const rows = (samples) => [...new Set(samples.map((sample) => sample.rule))].sort()
    .map((rule) => rowFor(rule, samples.filter((sample) => sample.rule === rule)));
  return {
    suites: sets.map(({ suite, samples }) => ({ suite, rows: rows(samples) })),
    pooled: rows(sets.flatMap(({ samples }) => samples)),
  };
}

/** A precision cell: the ratio with its sample size, or `n/a` when nothing was reviewed. */
function cell({ precision, reviewed }) {
  return `${precision === null ? "n/a" : `${(precision * 100).toFixed(1)}%`} (n=${reviewed})`;
}

/**
 * Renders a precision report as Markdown. Every ratio is printed with its sample size, and
 * unreviewed and unclear samples are listed beside it instead of being folded into it.
 * @param {object} report Result of {@link precisionReport}.
 * @returns {string}
 */
export function renderPrecision(report) {
  /** The Markdown table lines for `rows`. */
  const table = (rows) => [
    "| rule | sampled | true-positive | false-positive | unclear | unreviewed | precision |",
    "| --- | ---: | ---: | ---: | ---: | ---: | --- |",
    ...rows.map((row) => `| ${row.rule} | ${row.sampled} | ${row.tp} | ${row.fp} | ${row.unclear} | ${row.unreviewed} | ${cell(row)} |`),
  ];
  const sections = [...report.suites.map(({ suite, rows }) => [`### ${suite}`, "", ...table(rows)])];
  if (report.suites.length > 1) sections.push(["### all suites", "", ...table(report.pooled)]);
  return `${sections.map((lines) => lines.join("\n")).join("\n\n")}\n`;
}
