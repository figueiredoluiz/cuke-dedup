# ambiguous-step

One concrete step from the discovered feature corpus matches multiple authoritative definitions. This rule
does not speculate about steps absent from the corpus; `overlapping-matcher` handles that case.
Narrow one matcher or remove the duplicate.

The selected definitions form one comparison corpus; CukeDedup does not infer which definitions a runner loads for each feature. Before changing matchers in a monorepo or a suite with feature-local Cypress steps, check that the reported definitions actually coexist at runtime. Scope both definitions and features to that registry as described in [Corpus boundaries](../discovery/corpus-boundaries.md). Filtering only features still compares each selected step against every selected definition.

Each analysis admits at most 1,000,000 owned ambiguity memberships, 25,000 finding/source groups and 64 MiB of retained ambiguity text and path payload. Membership accounting includes group/proof indices and complete related-location entries; suppressed findings consume the same allowances. Storage is admitted before retaining groups or cloning related locations. On exhaustion, previously admitted complete findings remain available, the run is marked incomplete and static matcher overlap is skipped. Complete matcher usage remains available; these limits are independent of candidate-comparison and reporter limits.

Direct feature matching separately admits at most 64,000,000 matcher automaton scans and 64,000,000 emitted definition matches. Retained match rows switch to complete-text batch recomputation above 64 MiB without lowering finding capacity. Exhausting a CPU allowance preserves ambiguity findings from completed batches, marks analysis incomplete, disables unused-definition findings and skips static overlap; partial rows are never reported or used as proofs.

## Language support

| JavaScript/TypeScript | Cucumber-Ruby |
| --- | --- |
| Supported. | Supported for Cucumber Expressions and the translated Ruby regex subset. Other regexes are indeterminate: the run is incomplete and they produce no overlap or unused findings. |

Cucumber-Ruby support is experimental; see [Cucumber-Ruby](../discovery/cucumber-ruby.md).

Back to the [rule index](../rules.md#ambiguous-step).
