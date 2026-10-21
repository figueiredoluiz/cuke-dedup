// A report `analysis` section for tests that read the pair census, built from the shared source list
// so a test census cannot fall out of step with `PAIR_SOURCES`.

import { PAIR_SOURCES } from "./pair-join.mjs";

/**
 * The report's `analysis` section for a run whose pair-proposing sources evaluated `evaluated` pairs
 * and skipped `skipped` comparisons. Matcher overlap always holds counts of its own (4 evaluated, 3
 * skipped) that the report's totals include and a pair check must ignore, so a test using this census
 * with no skips also controls for that.
 * @param {object} [counts]
 * @param {number} [counts.evaluated] Pairs evaluated, all by the identical-handler source.
 * @param {number} [counts.skipped] Comparisons skipped, all by matcher blocking.
 * @param {number} [counts.structural] Truncated structural classes.
 * @returns {object}
 */
export function census({ evaluated = 0, skipped = 0, structural = 0 } = {}) {
  return {
    truncated: true,
    candidateComparisonsEvaluated: evaluated + 4,
    skippedCandidateComparisons: skipped + 3,
    truncatedStructuralClasses: structural,
    candidateSources: {
      ...Object.fromEntries(PAIR_SOURCES.map((name) => [name, {
        evaluated: name === "identicalHandler" ? evaluated : 0,
        skipped: name === "matcherBlocking" ? skipped : 0,
      }])),
      matcherOverlap: { evaluated: 4, skipped: 3 },
    },
  };
}
