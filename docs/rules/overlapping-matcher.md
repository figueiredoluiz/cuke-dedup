# overlapping-matcher

Static analysis can synthesize a concrete Cucumber Expression accepted by two definitions, even
if no discovered feature currently uses it. Equivalent matchers already covered by duplicate
rules are not repeated, and unsupported regular expressions are never reversed into samples.

The rule complements `ambiguous-step`: ambiguity is demonstrated by a discovered feature step,
while overlap is demonstrated from definitions alone. Work shares `maxCandidateComparisons` with
other pair rules and retains at most 10,000 findings. Exceeding a limit and truncating the scan marks analysis
incomplete.
Scanning definitions for witnesses evaluates no comparison, so a limit equal to the comparisons a
corpus needs stays complete while its witness scans fit within four times the limit. The pass is
incomplete when an overlap pair exceeds the remaining budget, witness scans exceed four times the
limit, more than 10,000 findings would be retained, or suppression lookups exceed their work limit.

## Language support

| JavaScript/TypeScript | Cucumber-Ruby |
| --- | --- |
| Supported. | Supported for Cucumber Expressions and the translated Ruby regex subset. Other regexes are indeterminate: the run is incomplete and they produce no overlap or unused findings. |

Cucumber-Ruby support is experimental; see [Cucumber-Ruby](../discovery/cucumber-ruby.md).

Back to the [rule index](../rules.md#overlapping-matcher).
