Given(/\Avalue [\]a{,3}]\z/) { first() }
Then(%r{\Avalue [\]a{,3}]\z}) { second() }
Then("value 0") { control() }
# `\]` is a class member, so `{,3}` stays inside the class and `value {` matches both regexes.
# A translator that closed the class at `\]` would read `{,3}` as a quantifier: both regexes
# would then miss `value {`, and the ambiguity would turn into two unused definitions.
