# Rules

Configure a rule as `off`, `warning`, or `error`. The CLI also accepts `warn` as an alias for
`warning`.

| Rule | Default | Meaning |
| --- | --- | --- |
| `duplicate-matcher` | Error | Definitions use the same effective matcher. |
| `normalized-matcher` | Error | Matchers become equivalent after normalization. |
| `ambiguous-step` | Error | A concrete feature step matches multiple definitions. |
| `overlapping-matcher` | Warning | Two matchers accept the same step text, even if no feature exercises it. |
| `duplicate-handler` | Error | Different matchers have the same alpha-normalized handler. |
| `near-duplicate-step` | Warning | Matcher wording is close and handler behavior substantially overlaps. |
| `parameterization-candidate` | Warning | Handler structures differ primarily in literal values. |
| `unused-definition` | Warning | No discovered feature step uses the definition. |

Jest-Cucumber's inline scenario steps are positional rather than global matcher registrations. `duplicate-matcher`, `normalized-matcher`, `ambiguous-step`, `overlapping-matcher`, and `unused-definition` do not apply to them. The three handler-reuse rules still apply across Jest scenarios, and `duplicate-handler` can report identical matcher text. Handler pairs involving Jest-Cucumber or Vitest Cucumber Plugin must belong to the same framework, preserving their different argument conventions.

Each rule has its own page with meaning, evidence, remediation and language support.

### duplicate-matcher

Two definitions claim the same step at runtime: same matcher kind, source text and effective flags. Handlers are irrelevant. Details: [rules/duplicate-matcher.md](rules/duplicate-matcher.md).

### normalized-matcher

Matchers differ in source but become equal after Unicode, whitespace, placeholder and regular-expression normalization. Details: [rules/normalized-matcher.md](rules/normalized-matcher.md).

### ambiguous-step

A concrete step in the discovered features matches more than one definition. Proven by the corpus, never by speculation. Details: [rules/ambiguous-step.md](rules/ambiguous-step.md).

### overlapping-matcher

Two matchers accept a synthesized step text even though no feature uses it. Complements `ambiguous-step` from definitions alone. Details: [rules/overlapping-matcher.md](rules/overlapping-matcher.md).

### duplicate-handler

Different matchers run the same handler after parameter and local-variable normalization, with equal behaviour signatures. Details: [rules/duplicate-handler.md](rules/duplicate-handler.md).

### near-duplicate-step

Matcher wording is close and the handlers share enough ordered behaviour. The handler-similarity floor is configurable (`nearDuplicateHandlerSimilarity`, default 0.70). Details: [rules/near-duplicate-step.md](rules/near-duplicate-step.md).

### parameterization-candidate

Handlers keep the same structure after literal normalization and the matchers differ only in a small value-like region. Details: [rules/parameterization-candidate.md](rules/parameterization-candidate.md).

### unused-definition

No successfully parsed feature step matches the definition. Disabled when the feature corpus is absent or incomplete. Details: [rules/unused-definition.md](rules/unused-definition.md).

## Language support

Rules run on the shared representation, but each source language supplies different evidence.
Cucumber-Ruby support is experimental; its details are in
[Cucumber-Ruby](discovery/cucumber-ruby.md).

Per-rule language support is on each rule page. Inline suppressions: JavaScript/TypeScript `// cuke-dedup:ignore RULE -- REASON`, Cucumber-Ruby `# cuke-dedup:ignore RULE -- REASON`.

## Exact groups and fuzzy pairs

Exact matcher and handler groups with up to four definitions produce a linear spanning set of pair
findings. Larger groups produce one bounded cluster finding with the complete member count and an
explicit truncation signal. Duplication thresholds still count every member.

Fuzzy and structural-similarity rules remain pair findings so individual scores and comparisons
are visible. Candidate blocking and work limits are described in
[Safety and limitations](safety-and-limitations.md).

## Duplication threshold

`threshold` is the percentage of discovered definitions permitted to participate in active,
error-level duplication findings. It accepts `0` through `100` and defaults to `0`.

```sh
cuke-dedup . --threshold 5
```

The numerator contains unique definition locations involved in these error-level rules:

- `duplicate-matcher`
- `normalized-matcher`
- `duplicate-handler`
- `near-duplicate-step`, when configured as an error
- `parameterization-candidate`, when configured as an error

A definition is counted once even when it participates in several findings. Suppressed and
baselined findings do not count. The denominator is every discovered definition, including those
without findings.

The threshold passes when the unrounded percentage is less than or equal to the configured value.
Active `ambiguous-step`, `overlapping-matcher`, and `unused-definition` errors remain independently
fatal because they represent policies other than duplication tolerance. An empty project has a
0% duplication rate.

See [Configuration](configuration.md) for per-rule severities and suppressions.
