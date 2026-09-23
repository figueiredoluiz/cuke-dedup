// Pure finding↔expectation matching for the recall corpus, extracted from check-corpus.mjs so the
// oracle can be unit-tested without spawning the binary. The shipped bug this guards (PR #39): one
// finding satisfying two overlapping expectations while a second finding went unclassified, yet the
// totals balanced and the harness reported 100% green with a real finding missing. `findingOwners`
// exposes exactly that — a finding must have exactly one owner.

/**
 * Renders a source location as the `path:line` form the manifest uses for related locations.
 * @param {{path: string, line: number}} location
 * @returns {string}
 */
export function locationLabel(location) {
  return `${location.path}:${location.line}`;
}

/**
 * Returns whether one finding satisfies one manifest expectation. Every field is optional; omitting
 * one widens the match, so an expectation asserts exactly what it names and nothing more.
 * @param {object} finding
 * @param {object} expected
 * @returns {boolean}
 */
export function findingMatches(finding, expected) {
  if (finding.rule !== expected.rule) return false;
  if (expected.primaryPath && finding.primary.path !== expected.primaryPath) return false;
  if (expected.primaryLine && finding.primary.line !== expected.primaryLine) return false;
  if (expected.relatedLocations) {
    const actual = finding.related.map(locationLabel).sort();
    const wanted = [...expected.relatedLocations].sort();
    if (actual.length !== wanted.length) return false;
    if (!actual.every((label, index) => label === wanted[index])) return false;
  }
  if (expected.relatedCount !== undefined && finding.related.length !== expected.relatedCount) {
    return false;
  }
  if (expected.messageIncludes && !finding.message.includes(expected.messageIncludes)) {
    return false;
  }
  if (expected.cluster) {
    const cluster = finding.evidence?.cluster;
    if (!cluster) return false;
    for (const [field, value] of Object.entries(expected.cluster)) {
      if (cluster[field] !== value) return false;
    }
  }
  if (expected.cluster === false && finding.evidence?.cluster) {
    return false;
  }
  if (expected.matchers) {
    const comparison = finding.evidence.comparison;
    if (!comparison) return false;
    const actual = [comparison.leftMatcher, comparison.rightMatcher].sort();
    const wanted = [...expected.matchers].sort();
    return (
      actual.length === wanted.length && actual.every((matcher, index) => matcher === wanted[index])
    );
  }
  return true;
}

/**
 * The expectations a finding satisfies. Exactly one is the contract; zero means unclassified, and
 * more than one means overlapping expectations that could mask a second finding.
 * @param {object} finding
 * @param {object[]} expectedFindings
 * @returns {object[]}
 */
export function findingOwners(finding, expectedFindings) {
  return expectedFindings.filter((expected) => findingMatches(finding, expected));
}

/**
 * How many findings match one expectation.
 * @param {object[]} findings
 * @param {object} expected
 * @returns {number}
 */
export function countMatches(findings, expected) {
  return findings.filter((finding) => findingMatches(finding, expected)).length;
}

/**
 * Total findings the expectation set stands for; `count` defaults to 1 per entry.
 * @param {object[]} expectedFindings
 * @returns {number}
 */
export function expectedTotal(expectedFindings) {
  return expectedFindings.reduce((total, expected) => total + (expected.count ?? 1), 0);
}
