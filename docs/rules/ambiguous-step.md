# ambiguous-step

One concrete step from the discovered feature corpus matches multiple definitions. This rule does
not speculate about steps absent from the corpus; `overlapping-matcher` handles that case. Narrow
one matcher or remove the duplicate.

The selected definitions form one comparison corpus; CukeDedup does not infer which definitions a runner loads for each feature. Before changing matchers in a monorepo or a suite with feature-local Cypress steps, check that the reported definitions actually coexist at runtime. Scope both definitions and features to that registry as described in [Corpus boundaries](../discovery/corpus-boundaries.md). Filtering only features still compares each selected step against every selected definition.

## Language support

| JavaScript/TypeScript | Cucumber-Ruby |
| --- | --- |
| Supported. | Supported for Cucumber Expressions and the translated Ruby regex subset. Other regexes are indeterminate: the run is incomplete and they produce no overlap or unused findings. |

Cucumber-Ruby support is experimental; see [Cucumber-Ruby](../discovery/cucumber-ruby.md).

Back to the [rule index](../rules.md#ambiguous-step).
