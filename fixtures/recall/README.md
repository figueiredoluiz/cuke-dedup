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
| `language` | `ecmascript`, `ruby`, or `both` for a paired case with a TypeScript and a Ruby source | the `domain` of the run's fingerprint records, where the case is run (recall and behavior corpora) |
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
| `discovery` | which files are found: exclusions, ignore files, hidden files, nested roots, default includes |
| `gate-threshold` | the duplication threshold and the exit status it decides |
| `parser-recovery` | malformed or adversarial input that must not hide valid definitions |
| `gherkin-dialect` | localized and Unicode Gherkin keywords |
| `provider-context` | which receiver or module context makes a call a registration: mixins, World modules, lifecycle hooks, nested callbacks |
| `regex-translation` | translating regular-expression syntax into the shared matcher model: classes, braces, punctuation, dot-all |
| `parameter-types` | custom parameter types and their collisions |
| `owner-identity` | the qualified owner of a definition and autoloaded owners |
| `dependency-usage` | usage reached through loaded files |
| `handler-context` | nested handler contexts and the files a handler captures |

`npm run corpus:distribution` counts cases and expectations per dimension from the manifest and names the thin cells.

`ambiguous` is for a pair whose desired outcome is undecided. Such a case lists `expectedAmbiguous`
entries: a `rule`, a `reason`, and both endpoints of one pair: `primaryPath`, `primaryLine` and non-empty `relatedLocations`. Matcher
text, one location or a message alone can match several pairs and is rejected. A finding matching one needs no owner in `expectedFindings`, is not counted as desired or
detected, and still fails a matching `expectedAbsent` entry. A case with only ambiguous expectations
accepts exit status 0 or 1, since the undecided finding may change it; operational failures (2) still
fail, and a case that also requires findings keeps its exact `expectedExit`. `variant` needs
`variantOf`, the name of a non-variant case it varies. A case with `expectedDefinitions` 0 has no
records and skips the `language` and `frameworks` check; any other case with no records fails.

An `expectedAbsent` entry passes whenever its finding is missing, including when a participant was never analyzed, the analyzer never evaluated the pair, or the pair differs in more than the case claims. For `duplicate-handler`, `near-duplicate-step`, `parameterization-candidate`, `duplicate-matcher` and `normalized-matcher`, an `isolation` object states why the finding is absent, and the corpus check verifies it against the run's `--emit-fingerprints` records and, for the handler rules, its `--emit-pairs` records:

- `participants`: `path:line` labels, with the path in the form the JSON report writes it, one distinct record each. Without it, each of the entry's `matchers` must select exactly one record. Either way the participants' matchers must be exactly the entry's `matchers`; with `relatedLocations`, their labels must be exactly the entry's primary and related locations, and with a primary alone they must include it; `primaryPath` and `primaryLine` come together. Every record must be schema `"1"` with each compared field present. A handler rule compares exactly two participants.
- `differs`: exactly the record fields that differ between participants, from `domain`, `framework`, `registration`, `matcher`, `matcherKind`, `matcherFlags`, `normalizedMatcher` and `handler.exact|normalized|alphaNormalized|structural|behavior`.
- `compared` (handler rules only) states what the analyzer did with the pair, and each state is proven from the pair records. A participant is eligible when its handler is comparable and not trivial.
- `reason`: required with `not-proposed` (`behavior-differs`) and `unevaluated-under-limit` (`candidate-limit`), and allowed nowhere else.
- `handlerDifference`: `unresolved-value`, only with `compared: false` and only when the excluded participants' handlers differ.

The states and the evidence each requires:

- `true`: the analyzer evaluated the pair and the rule did not fire. Eligible participants; exactly one pair record joins them, with `meaningfulHandlers: true`; its `rules` omit the entry's rule, since otherwise the finding was produced and then suppressed or collapsed.
- `false`: the analyzer excluded the pair from comparison. A participant is not comparable or is trivial, or eligible participants whose pair record has `meaningfulHandlers: false` (runtime-incompatible handlers; the record is the analyzer's verdict, and the check requires the participants to differ in `framework` (the handler domain: scenario-local, plugin-global or shared) or, for Ruby, in file (lexical capture), the two things the verdict reads besides the method semantics in the behavior signature, which would be a handler difference and a second reason for the absence). A record that exists never says the handlers are meaningful while a participant is ineligible; eligible participants with no record are rejected, since nothing then shows the exclusion. Handler fields must be equal, because differing handlers would be a second reason for the absence, unless `handlerDifference` is declared.
- `"not-proposed"`: no candidate source proposed the pair. Eligible participants; no pair record; a report in which no pair-proposing source (`normalizedMatcher`, `identicalHandler`, `structuralHandler`, `matcherBlocking`) skipped a comparison and no structural class was truncated; the missing record in a run that skipped nothing shows no source, matcher blocking included, proposed the pair; `differs` must include `handler.behavior` and `normalizedMatcher`, the keys the other three sources group by, since an unproposed pair with equal keys would be unexplained.
- `"unevaluated-under-limit"`: an explicit exemption for a case that runs under a candidate limit. Eligible participants; no pair record; `differs` includes `handler.behavior` and `normalizedMatcher`, as for `not-proposed`; a report in which a pair-proposing source skipped a comparison or a structural class was truncated. The census counts skips without naming the pairs, so it cannot show whether the limit dropped this pair or no source would have proposed it; the entry claims only that the pair was not evaluated while limits applied, and pins that no finding appears then.

Whenever a pair record joins the participants, in any state, its `relationships` must agree with the fingerprint records: `sameHandler` is `handler.alphaNormalized` and `handler.behavior` both equal, `sameHandlerStructure` is `handler.structural` equal, `sameStructure` is `handler.structural` and `handler.behavior` both equal, and `sameDeferredAssertions` may be false only when `handler.behavior` differs. `exactMatcher` is false when `matcher` or `matcherKind` differs, true when neither differs and `matcherFlags` and `domain` are equal, and open otherwise (raw flags can differ with equal effective flags, and each dialect reads them its own way). `normalizedMatcher` follows `normalizedMatcher` and `matcherKind` the same way, and is false for a positional pair.

Choose the state in this order: `true`, by reworking the fixture or the participant selection so the pair is evaluated (the identical-handler source proposes a handler group's anchor against the other matcher groups, so a later duplicate's pair with another framework's definition has no record while the anchor's does); `false` when the exclusion is visible in the evidence; `not-proposed` where the separation happens at candidate generation. A provenance negative whose handlers differ in a value the analyzer reads is that case: the behavior signature keys the identical-handler and structural-handler sources, and matcher blocking drops value-only differences and pairs that cannot reach the handler similarity gate. Padding the handlers until matcher blocking proposes the pair would let a handler similarity threshold decide the outcome, a second reason for the absence. `unevaluated-under-limit` is only for a case that runs under a candidate limit.

`handlerDifference: unresolved-value` is for an excluded pair whose handlers differ because they reference a value the analyzer cannot resolve (a mutable binding, a nested declaration or a shadowed one). Both participants must be non-comparable and equal in `handler.behavior`. Equal handlers would change what such a case tests: the value substitution it forbids would no longer make a difference.

The run must provide the evidence. A handler-rule isolation rejects the case when the run wrote no pair file, when the pair file holds a different number of records than the pair-proposing sources evaluated (summed from the report's `candidateSources`; matcher overlap writes no pair record and its counts are ignored), when a pair record is outside schema `"1"`, when several records join one pair, when a definition key (path, line and column in the report's form) is shared by several fingerprint records, as when one source position registers several steps and the pair cannot be told apart, and when an endpoint's `definitionFingerprint` is not the fingerprint record's. An unknown `compared` value, `reason` or `handlerDifference` is rejected, and so is `reason` or `handlerDifference` where it does not apply. The gate and `corpus:evaluate` run only a case that declares a handler-rule isolation with `--emit-pairs`; `corpus:candidate-recall` requests them for every case. A rule its case configuration switches off also leaves a pair record's `rules` without it, which no evidence distinguishes, so a unit test rejects an isolation on a disabled rule. Matcher rules are decided by the keys in the fingerprint records and take no pair evidence.

`unisolatedNegativeBaseline` counts entries of those rules without `isolation`. It is asserted exactly and is 0: a new handler or matcher negative declares an isolation. `unused-definition`, `ambiguous-step` and `overlapping-matcher` also depend on feature steps and take no `isolation`.

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
