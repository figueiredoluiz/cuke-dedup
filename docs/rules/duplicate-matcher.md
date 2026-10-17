# duplicate-matcher

Two definitions have the same matcher kind, source text, and effective regular-expression flags.
Handlers are irrelevant: both definitions still claim the same step at runtime. Keep one
definition or make the matchers intentionally distinct. Suppress only when the definitions cannot
coexist in one runtime scope.

## Language support

| JavaScript/TypeScript | Cucumber-Ruby |
| --- | --- |
| Supported. | Supported for static strings and `/.../` or `%r{...}` regex literals. |

Cucumber-Ruby support is experimental; see [Cucumber-Ruby](../discovery/cucumber-ruby.md).

Back to the [rule index](../rules.md#duplicate-matcher).
