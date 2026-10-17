# Recall corpus

The `review-gap-*` cases cover the five remaining review groups: namespace-alias
mutation, equivalent assertion syntax (named-default imports and wrapped callees),
lexical TDZ shadowing, nested asymmetric matchers, and candidate filtering.
Each run selects one input shape from `review-gaps/` with `--definitions` so
unrelated cases cannot contaminate its findings. The candidate checks require one
useful comparison and retain a true finding under a one-candidate limit; they do
not claim to reproduce starvation at default limits. Positive controls complement
explicit non-findings. These are required contracts, not newly accepted known misses.

Computed-key cases distinguish static factory trust from dynamic keys and possible alias mutations.
The mutation case runs separately so it cannot invalidate the positive control's namespace.
The deferred-assertion case retains matching callbacks while rejecting conflicting values,
negation, and unresolved inline calls. Exact total finding counts also reject unlisted pairings.

This corpus protects what CukeDedup finds, not only aggregate finding counts.
Each case in `manifest.json` records one of three outcomes:

- `expectedFindings`: behavior the analyzer supports and must retain;
- `expectedAbsent`: intentional non-findings that protect precision;
- `knownMisses`: desired findings not implemented yet.

When a known miss is fixed, move it to `expectedFindings` and reduce
`knownMissBaseline`. New known misses must not be added to make CI pass.

An `expectedAbsent` entry passes whenever its finding is missing, including when a participant was
never analyzed or the pair differs in more than the case claims. For `duplicate-handler`,
`near-duplicate-step`, `parameterization-candidate`, `duplicate-matcher` and `normalized-matcher`,
an `isolation` object states why the finding is absent, and the corpus check verifies it against
the run's `--emit-fingerprints` records:

- `participants`: `path:line` labels, one distinct record each. Without it, each of the entry's
  `matchers` must select exactly one record. Either way the participants' matchers must be exactly the
  entry's `matchers`; with `relatedLocations`, their labels must be exactly the entry's primary and
  related locations, and with a primary alone they must include it; `primaryPath` and
  `primaryLine` come together. Every record must be schema
  `"1"` with each compared field present.
- `differs`: exactly the record fields that differ between participants, from `domain`,
  `framework`, `registration`, `matcher`, `matcherKind`, `matcherFlags`, `normalizedMatcher` and
  `handler.exact|normalized|alphaNormalized|structural|behavior`.
- `compared` (handler rules only): `true` when every participant is comparable and not trivial;
  `false` when the pair was excluded from comparison. An excluded pair must have equal handler
  fields, so differing handlers cannot also explain the absence.

`unisolatedNegativeBaseline` counts entries of those rules without `isolation`. It is asserted
exactly: declare an isolation and lower it. `unused-definition`, `ambiguous-step` and
`overlapping-matcher` also depend on feature steps and take no `isolation`.

The fixtures are synthetic and contain no private project data.

Every recall case pins `expectedFeatureSteps`, including cases without feature files. The count is extracted keyword step records, not runner executions: feature and rule `Background` steps each count once in their source position, while a populated `Scenario Outline` contributes its step lines once per `Examples` data row. The `gherkin-axes` cases separately pin backgrounds and rules, outline rows, continuations, and classic plus Markdown files; `gherkin-attachments` pins that tags, data-table rows, and doc-string lines do not add steps. The earlier simple and mixed Gherkin cases remain as controls.

The `precision-*` cases exercise assertion provenance through the built CLI:
trusted/unrelated ESM and CJS imports, different external and local values,
inline/local equivalence, nested shadowing, erased/runtime namespaces, decorators,
and genuine duplicates with mixed modules or renamed local variables. Exact
finding expectations reject unexpected errors and warnings; positive controls
prevent an analyzer that suppresses all handler findings from passing. Uncertain
external values must not establish handler equivalence. Inline/local equivalence
is a required detection contract, including when the value is held in a local constant.
