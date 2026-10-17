# near-duplicate-step

Matcher wording is close and meaningful handlers share enough ordered behavior, with compatible
structural or canonical-call evidence. Similar prose alone and unrelated actions do not qualify.
Review the pair and consolidate only when both definitions express the same behavior.

The required handler-similarity floor defaults to **0.70** and is configurable through
`nearDuplicateHandlerSimilarity` (0.5–1.0). Raising it demands more handler evidence, which
suppresses systematically-named steps whose matchers are near-identical but whose handlers act on
different things; lowering it toward 0.5 restores the older, looser behaviour.

When handlers are not structurally equivalent, they must share canonical action evidence: either
a call action, including direct and fluent forms such as `page.click()` and
`page.locator(...).click()`, or an assertion with the same matcher and subject shape. Expected
assertion values and polarity remain semantic during final similarity verification.
Decorated class methods must also have compatible runtime-affecting semantics: async versus sync,
static versus instance, generator, getter, and setter differences veto a near-duplicate match.
Call evidence includes inline callback bodies syntactically; deferred assertions are not treated as
directly executed assertions.
Deferred assertion sequences must match in value, polarity, and order for a near-duplicate finding.
Shared wrapper calls cannot compensate for different deferred assertions. This conservatively skips
fuzzy matches when one callback adds or reorders assertions; exact matcher checks still run.

## Language support

| JavaScript/TypeScript | Cucumber-Ruby |
| --- | --- |
| Wording similarity plus handler-behavior overlap. | Wording similarity plus handler-behavior overlap: action streams for ordinary handlers whose every construct is modeled, configured assertion streams otherwise; a conflicting assertion is one differing event, as in JavaScript. Handlers with an unmodeled construct or an untrusted receiverless `expect` chain must be exactly equal. |

Cucumber-Ruby support is experimental; see [Cucumber-Ruby](../discovery/cucumber-ruby.md).

Back to the [rule index](../rules.md#near-duplicate-step).
