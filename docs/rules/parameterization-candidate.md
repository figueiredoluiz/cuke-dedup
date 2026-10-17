# parameterization-candidate

Handlers preserve the same control flow and calls after literal normalization, and their matchers
differ only in a small, value-like region — the token or two a parameter would stand in for — with
compatible syntax and no polarity conflict. Matchers that share a prefix but then differ in a
multi-word phrase describing different behaviour, or that conflict in polarity, are not proposed even
when literal erasure collapses their handlers into one shape. Findings remain pair-specific so
reports retain the differing literals and matcher text. Replace repeated literals with a parameter
when it makes the test vocabulary clearer.

Expected assertion values and polarity are not erased for this rule. For example,
`expect(state).toBe('ready')` and `expect(state).toBe('idle')` express distinct expectations and
do not produce a parameterization suggestion solely because their expected literals differ.

## Language support

| JavaScript/TypeScript | Cucumber-Ruby |
| --- | --- |
| Handler structures that differ in literal values. | Straight-line handlers of explicit simple-receiver calls with static string or numeric arguments; with a configured assertion provider, any supported handler abstracts its literal values. |

Cucumber-Ruby support is experimental; see [Cucumber-Ruby](../discovery/cucumber-ruby.md).

Back to the [rule index](../rules.md#parameterization-candidate).
