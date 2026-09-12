# Recall corpus

The `review-gap-*` cases cover the five remaining review groups: namespace-alias
mutation, equivalent assertion syntax (named-default imports and wrapped callees),
lexical TDZ shadowing, nested asymmetric matchers, and candidate filtering.
Each run selects one input shape from `review-gaps/` with `--definitions` so
unrelated cases cannot contaminate its findings. The candidate checks require one
useful comparison and retain a true finding under a one-candidate limit; they do
not claim to reproduce starvation at default limits. Positive controls complement
explicit non-findings. These are required contracts, not newly accepted known misses.

This corpus protects what CukeDedup finds, not only aggregate finding counts.
Each case in `manifest.json` records one of three outcomes:

- `expectedFindings`: behavior the analyzer supports and must retain;
- `expectedAbsent`: intentional non-findings that protect precision;
- `knownMisses`: desired findings not implemented yet.

When a known miss is fixed, move it to `expectedFindings` and reduce
`knownMissBaseline`. New known misses must not be added to make CI pass.

The fixtures are synthetic and contain no private project data.

The `precision-*` cases exercise assertion provenance through the built CLI:
trusted/unrelated ESM and CJS imports, different external and local values,
inline/local equivalence, nested shadowing, erased/runtime namespaces, decorators,
and genuine duplicates with mixed modules or renamed local variables. Exact
finding expectations reject unexpected errors and warnings; positive controls
prevent an analyzer that suppresses all handler findings from passing. Uncertain
external values must not establish handler equivalence. Inline/local equivalence
is a required detection contract, including when the value is held in a local constant.
