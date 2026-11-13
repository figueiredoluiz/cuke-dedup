// Comparison rules for check-ruby-execution.mjs: how the registrations Ruby made are judged
// against the definitions the analyzer extracted.

/** Matcher identity: kind, text and flags. */
const identity = (item) => `${item.matcherKind} ${JSON.stringify(item.matcher)}${item.matcherFlags ? `/${item.matcherFlags}` : ""}`;

/**
 * Registrations in `left` without a counterpart in `right`, as display strings. Identity is the
 * matcher kind, text and flags: locations differ legitimately, because Cucumber records a
 * handler's own location while the analyzer reports the registration call.
 * @param {object[]} left Registrations with `path`, `line`, `matcherKind`, `matcher`, `matcherFlags`.
 * @param {object[]} right The registrations to match them against.
 * @returns {string[]} One entry per unmatched registration.
 */
export function surplus(left, right) {
  const counts = new Map();
  for (const item of right) counts.set(identity(item), (counts.get(identity(item)) ?? 0) + 1);
  return left.filter((item) => {
    const remaining = counts.get(identity(item)) ?? 0;
    counts.set(identity(item), remaining - 1);
    return remaining <= 0;
  }).map((item) => `${identity(item)} at ${item.path}:${item.line}`);
}

/**
 * Every difference between what Ruby did and what the analyzer claims for a complete suite.
 * @param {{registrations: object[]}} analyzer The analyzer's extracted definitions.
 * @param {{registrations: object[], loadErrors: object[], replaced: string[]}} ruby The harness result.
 * @returns {string[]} Problems; empty when the two agree.
 */
export function problems(analyzer, ruby) {
  return [
    ...surplus(analyzer.registrations, ruby.registrations).map((item) => `extracted but not registered by Ruby: ${item}`),
    ...surplus(ruby.registrations, analyzer.registrations).map((item) => `registered by Ruby but not extracted: ${item}`),
    ...ruby.loadErrors.map((error) => `${error.path}:${error.line ?? "?"} fails to load: ${error.error}: ${error.message}`),
    ...ruby.replaced.map((keyword) => `step keyword replaced at load time: ${keyword}`),
  ];
}
