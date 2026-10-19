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

Every case also carries metadata that classifies it for evaluation reports. None of it changes what
the case asserts; `scripts/check/lib/manifest-schema.mjs` validates it on every corpus run.

| Field | Values | Checked against |
| --- | --- | --- |
| `role` | `positive` (has `expectedFindings`), `negative` (has none), `ambiguous`, `variant` | the case's own expectations |
| `origin` | `synthetic`, `real`, `regression` (added for a reviewer finding or shipped bug) | format only |
| `language` | `ecmascript`, `ruby` | the `domain` of the run's fingerprint records |
| `frameworks` | distinct lowercase slugs, in any order | the frameworks of the run's fingerprint records |
| `verdict` | `reviewed`, `generated` (labels written by a tool, not yet read by a person) | format only |
| `classifiedAt` | `YYYY-MM-DD`, not in the future | format only |
| `axes` | one or more distinct axes from the closed vocabulary below | membership in the vocabulary |

Axes are what a case varies. The vocabulary is closed (`AXES` in `scripts/check/lib/manifest-schema.mjs`),
so a report can count cases per axis; add an axis there, with its meaning, before a case uses it.

| Axis | Varies |
| --- | --- |
| `registration-form` | how a step is registered: call, alias, wrapper, decorator, ambient global, legacy entrypoint |
| `framework-api` | a framework's own registration API or scoping: cucumber-js, Cypress, Playwright BDD, jest-cucumber, the Vitest plugin |
| `module-resolution` | how registrations are found across modules: barrels, re-exports, path aliases, workspace packages, cross-package imports |
| `import-form` | named, namespace, default or `require` imports |
| `module-kind` | how an assertion module is trusted: declared, undeclared, known factory, erased at runtime |
| `binding-form` | function declarations, `const`, `let`, parameters and destructuring |
| `scope` | block, nested function, shadowing and the temporal dead zone |
| `value-provenance` | where a compared value comes from: literal, constant, mutable, external, local, decorator |
| `mutation-trust` | writes that revoke trust: alias, rest copy, computed key, negation, `require` |
| `assertion-shape` | assertion subjects, chains, callbacks, deferred and wrapped assertions, computed access |
| `handler-shape` | handler structure: direct against fluent, varied shape, local-function expansion, inline locals, renaming |
| `matcher-kind` | Cucumber Expression against regular expression |
| `matcher-flags` | effective regular-expression flags and their order |
| `matcher-normalization` | Unicode, whitespace, placeholders, capture groups, anchors and quantifiers |
| `matcher-wording` | fuzzy wording and polarity |
| `matcher-overlap` | overlap synthesized from definitions alone |
| `cluster-size` | the boundary between pair findings and one cluster finding |
| `candidate-limits` | candidate filtering and the comparison budget |
| `ambiguity-origin` | where definitions and steps come from for `ambiguous-step` |
| `gherkin-structure` | backgrounds, rules, outlines, continuation keywords and attachments |
| `gherkin-source` | classic and Markdown files, step sources, empty Markdown |
| `control` | the simplest case of a rule, varying nothing, kept so a regression in the common path is not lost |

`npm run corpus:distribution` counts cases and expectations per dimension from the manifest and names the thin cells.

`ambiguous` is for a pair whose desired outcome is undecided. Such a case lists `expectedAmbiguous`
entries: a `rule`, a `reason`, and both endpoints of one pair: `primaryPath`, `primaryLine` and non-empty `relatedLocations`. Matcher
text, one location or a message alone can match several pairs and is rejected. A finding matching one needs no owner in `expectedFindings`, is not counted as desired or
detected, and still fails a matching `expectedAbsent` entry. A case with only ambiguous expectations
accepts exit status 0 or 1, since the undecided finding may change it; operational failures (2) still
fail, and a case that also requires findings keeps its exact `expectedExit`. `variant` needs
`variantOf`, the name of a non-variant case it varies. A case with `expectedDefinitions` 0 has no
records and skips the `language` and `frameworks` check; any other case with no records fails.

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
- `compared` (handler rules only): `true` when every participant is comparable and not trivial
  and the run's report shows no truncation and no skipped candidate or structural-class
  comparison (records show eligibility, not that the pair was evaluated);
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
