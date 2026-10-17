# duplicate-handler

Different effective matchers use the same non-trivial handler after parameter and local-variable
normalization. Empty, pending, unresolved, and otherwise non-comparable handlers are excluded.
Exact handler matches also require equal behavior signatures. Handlers whose assertion arguments
reference unresolved external values are excluded from handler comparisons; matcher checks still run.
Consider one parameterized definition, but retain separate definitions when shared implementation
is intentional domain vocabulary.

## Language support

| JavaScript/TypeScript | Cucumber-Ruby |
| --- | --- |
| Supported, with assertion evidence from recognized assertion packages. | Exact supported handler structure. Assertion evidence comes only from an explicitly configured provider; RSpec, Minitest and Capybara assertions are compared as ordinary calls. Unresolved captures, constants or receiver identity make the handler non-comparable and the run incomplete. |

Cucumber-Ruby support is experimental; see [Cucumber-Ruby](../discovery/cucumber-ruby.md).

Back to the [rule index](../rules.md#duplicate-handler).
