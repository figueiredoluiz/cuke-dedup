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

### duplicate-matcher

Two definitions have the same matcher kind, source text, and effective regular-expression flags.
Handlers are irrelevant: both definitions still claim the same step at runtime. Keep one
definition or make the matchers intentionally distinct. Suppress only when the definitions cannot
coexist in one runtime scope.

### normalized-matcher

Matchers become equal after Unicode, whitespace, placeholder, and regular-expression
normalization, although their original source differs. Distinct matcher kinds or meaningful regex
flags remain distinct. Consolidate the definitions or rewrite the intended distinction so it
survives normalization.

### ambiguous-step

One concrete step from the discovered feature corpus matches multiple definitions. This rule does
not speculate about steps absent from the corpus; `overlapping-matcher` handles that case. Narrow
one matcher or remove the duplicate.

### overlapping-matcher

Static analysis can synthesize a concrete Cucumber Expression accepted by two definitions, even
if no discovered feature currently uses it. Equivalent matchers already covered by duplicate
rules are not repeated, and unsupported regular expressions are never reversed into samples.

The rule complements `ambiguous-step`: ambiguity is demonstrated by a discovered feature step,
while overlap is demonstrated from definitions alone. Work shares `maxCandidateComparisons` with
other pair rules and retains at most 10,000 findings. Reaching a limit marks analysis incomplete.

### duplicate-handler

Different effective matchers use the same non-trivial handler after parameter and local-variable
normalization. Empty, pending, unresolved, and otherwise non-comparable handlers are excluded.
Exact handler matches also require equal behavior signatures. Handlers whose assertion arguments
reference unresolved external values are excluded from handler comparisons; matcher checks still run.
Consider one parameterized definition, but retain separate definitions when shared implementation
is intentional domain vocabulary.

### near-duplicate-step

Matcher wording is close and meaningful handlers share at least 50% ordered behavior, with
compatible structural or canonical-call evidence. Similar prose alone and unrelated actions do
not qualify. Review the pair and consolidate only when both definitions express the same behavior.

When handlers are not structurally equivalent, they must share canonical action evidence: either
a call action, including direct and fluent forms such as `page.click()` and
`page.locator(...).click()`, or an assertion with the same matcher and subject shape. Expected
assertion values and polarity remain semantic during final similarity verification.
Decorated class methods must also have compatible runtime-affecting semantics: async versus sync,
static versus instance, generator, getter, and setter differences veto a near-duplicate match.
Call evidence includes inline callback bodies syntactically; deferred assertions are not treated as
directly executed assertions.

### parameterization-candidate

Handlers preserve the same control flow and calls after literal normalization, and their matcher
texts are sufficiently similar. Findings remain pair-specific so reports retain the differing
literals and matcher text. Replace repeated literals with a parameter when it makes the test
vocabulary clearer.

Expected assertion values and polarity are not erased for this rule. For example,
`expect(state).toBe('ready')` and `expect(state).toBe('idle')` express distinct expectations and
do not produce a parameterization suggestion solely because their expected literals differ.

### unused-definition

No successfully parsed concrete feature step matches a definition. This rule is disabled when the
feature corpus is absent or incomplete because the run cannot prove non-use. Remove the
definition, add the missing scenario, or suppress it when external or generated features provide
the usage.

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
