# Development

Maintainer reference for the gates, coverage rules and tooling behind [CONTRIBUTING.md](../CONTRIBUTING.md).
Release steps are in [releasing.md](releasing.md).

## Toolchains

- `rust-toolchain.toml` pins the development and release compiler (`1.98.1`). Update it when
  upgrading both together.
- `Cargo.toml` declares the MSRV (`1.90`). Check it locally with
  `cargo +1.90.0 test --all-targets --all-features --locked`.
- `rustup update stable` refreshes the separate stable quality checks (clippy, rustdoc, shear).

## Hooks and gates

| Gate | Runs | Contents |
| --- | --- | --- |
| `scripts/check/preflight.sh` | start of both gates | whitespace, `rustfmt`, Ruby keyword metadata, checker unit tests, language boundaries, docs parity; no compilation |
| `scripts/check/check.sh --quick` | pre-commit | preflight, clippy, tests; dependency checks only when the staged patch touches an Action, manifest, lock or policy file |
| `scripts/check/check.sh` | pre-push, CI | everything: rustdoc with `-D warnings`, `cargo package --list`, release build, corpus, matcher-window corpus, Ruby and language parity regression, memory, duplication, benchmark smoke, skills, versions, pack, `actionlint`, `cargo-audit`, `cargo-deny`, `cargo-shear` |

The pre-commit hook also scans the staged patch for secrets. The pre-push hook requires a clean
checkout at the pushed commit, derives conditional checks from the outgoing changes, and runs the
full gate, so the tree that is tested is the tree that leaves the clone.

The corpus check and `npm run benchmark` run every case after a failure, report each failing case
under its name, then exit 1; the benchmark prints no results after a failure.

### Docstring guard

Pre-push requires documentation for 100% of locally detected touched named functions and test
callbacks in the cumulative diff against `origin/main`, three context lines included.

- Rust doc comments, JavaScript/TypeScript JSDoc and Python docstrings count; tests and fixtures
  are included.
- JavaScript/TypeScript and Python ranges come from syntax trees; Rust uses declaration matching
  after masking comments and literals.
- Python sources need Python 3 (`python` on Windows, `python3` elsewhere).
- A missing base or an unreadable supported source fails validation.
- The guard does not guarantee agreement with CodeRabbit's independent metric.

Run it directly: `node scripts/check/check-docstrings.mjs --base origin/main`.

### Language boundary

`npm run check:boundaries` counts how much language and framework knowledge the language-neutral core still holds and compares each count with `scripts/check/language-boundary.json`. The `core` section of the baseline is meant to shrink to empty; the `registry` section (`src/source_adapter.rs`, which registers language adapters) keeps its rows, because that file may name languages; frameworks come from the contracts adapters supply, so no shared file names them; its counts are still compared with the baseline, so a change there also needs a baseline update.

- Scope: every Rust file under `src/`; hidden non-Rust files such as `.DS_Store` and hidden symbolic links such as Emacs lock files (`.#a.rs`) are skipped. `src/ruby.rs`, `src/ruby/`, `src/typescript.rs` and `src/typescript/` (adapters), `src/lib.rs` and `src/main.rs` (composition), dedicated `tests` files and `src/reporters/templates/` are not scanned. `src/source_adapter.rs` is the registry and has its own `registry` section; every other file is `core`. The check fails closed on an unknown file type, a symbolic link that is not hidden, Rust the scanner cannot tokenize, an unsupported `use` form (wildcard import of `crate::*` or of a `module` or `grammar` path, a `super::` wildcard other than a plain `super::*` chain that stays inside the file's inline modules, ambiguous or cyclic alias) and a `type` item that names a `module` or `grammar` path.
- Edge kinds: `module` (`crate::ruby`, `crate::typescript`), `marker` (`method:<name>:` string literals, any name), `name` (identifiers with a snake_case, camelCase or digit-delimited segment spelling ruby, typescript, javascript, tsx, jest, vitest, playwright, cypress or cucumberjs; `TypeScript`, `java_script` and `cucumber_js` match as two adjacent segments), `key` (whole-token string literals such as `"rubyLoadPaths"` or a framework id such as `"jest-cucumber"` containing those words) and `grammar` (`tree_sitter_<lang>` crates).
- Lexical, not name resolution: comments and prose strings are ignored, file-wide `use` aliases are resolved, inline `#[cfg(test)]` modules count, macro expansions, markers built at runtime (`format!`) and run-together lowercase names (`rubyist`) are not seen, and kinds overlap (`crate::ruby` counts as `module` and as `name:ruby`).
- Baseline rows are `[file, edge, count]`, sorted, one per line. An added edge fails as growth; a removed edge or lowered count fails as stale until the baseline is lowered.
- `--root PATH` and `--manifest PATH` select another checkout or baseline file.
- After removing edges, run `node scripts/check/check-language-boundary.mjs --update`. It rewrites the baseline and refuses growth; `--update --allow-growth` records an increase (needs review) and also regenerates a missing or unreadable baseline, which records every current edge as new: review that diff against the base branch.

### Check tools

`scripts/check/tools/` holds `jscpd` and the pinned TypeScript parser with their own manifest and
lockfile (`npm ci --prefix scripts/check/tools`). The root manifest gains no dependencies, the
graph is pinned by integrity hash, and npm is not forced to install platform packages meant for
other operating systems. The npm launcher's own checks have no third-party development dependencies.

### Dependency policy

```sh
cargo audit --deny warnings
cargo audit --deny warnings --file fuzz/Cargo.lock
cargo deny check advisories bans licenses sources
cargo deny --manifest-path fuzz/Cargo.toml check advisories bans licenses sources
cargo shear --deny-warnings
(cd fuzz && cargo shear --deny-warnings)
```

`deny.toml` fails CI on known advisories, unknown registries or Git sources, wildcard version
requirements and licenses outside the approved SPDX list. `cargo-shear --deny-warnings` rejects
unused or misplaced dependencies, unlinked Rust sources and stale suppressions in both manifests.
CI additionally scans the full Git history for secrets.

## Coverage

CI enforces a 97% Rust line-coverage floor. Coverage is measured separately because it rebuilds
the suite with instrumentation:

```sh
cargo +stable llvm-cov --all-features --tests --locked --no-report
cargo +stable llvm-cov report --show-missing-lines --fail-under-lines 97
```

The summary counts brace-only continuation lines, so its missed-line count can exceed the region
listing. Reassess uncovered regions from the current artifact; the list below explains categories,
not an inventory.

Rules:

- An uncovered line needs an external reason: an upstream contract, a platform invariant, a race
  window, resource scale, const evaluation or failing I/O.
- Test a path when it becomes reachable.
- Code that is unreachable and has no defensive value is deleted, not documented.

### Excluded by an upstream guarantee

- `src/analysis/suppression.rs`, path suppressions without a compiled matcher: config loading
  rejects invalid suppression path globs before the index is built.
- `src/discovery.rs`, depth-zero arm of the walk filter: the `ignore` walker (0.4.33) decides the
  root entry itself and never consults `filter_entry` for it.
- `src/typescript/module_resolver.rs`, missing-parent arm of `is_top_level_variable`: a
  `variable_declarator` always has a parent declaration in the pinned grammars.
- `src/reporters/sarif.rs`, `Severity::Off` arm: analysis drops severity-off findings before any
  reporter runs.

### Platform and race invariants

- `src/config.rs`, `src/typescript/project_resolution.rs`, existing-ancestor walks:
  `Path::parent()` is `None` only for root-like paths; an ancestor that passed `exists()` and then
  fails `canonicalize()` vanished between the calls.
- `src/modes.rs`, `src/reporters/shared.rs`, no-parent arms of report-path creation: a path
  without a parent is a filesystem root.

### Arithmetic overflow guards

- `src/analysis/pairs.rs`, `src/analysis/usage.rs`, `checked_add` refusal arms: u64 sums over
  per-definition counters; memory is exhausted far below the wrap point.

### Resource-limit bails

Fire only on inputs far beyond test-practical size:

- `src/typescript/project_resolution.rs`: workspace-scan limit past 100,000 entries.
- `src/analysis/pairs.rs`: structural-candidate limit, matcher-shingle cap, matcher-blocking event
  budget, candidate-list limit.
- `src/analysis/usage.rs`: proposal and suppression work budgets, regex byte cap.
- `src/cli.rs`: unmatched-suppression safety-limit warning.

### Test-only formatting

- `src/typescript/ast.rs`: format arguments of a passing `assert!` are never evaluated.

### Failing-I/O propagation

- `src/main.rs` (BrokenPipe arm), `src/reporters/terminal.rs`, `src/cli.rs`, `src/modes.rs`,
  `src/framework_descriptor.rs`, `src/typescript/framework_config.rs`, `src/config.rs`: `?` paths
  that propagate failed writes, git subprocess output or configuration reads. The suite exercises
  one terminal footer write returning `BrokenPipe`; the others need external conditions such as a
  vanished file.

### Reachable residual

Anything uncovered outside these categories needs a test. Historical examples do become covered;
`SourceAdapterRegistration::with_session` did through runtime tests.

## Corpus and recall

- [`fixtures/corpus`](../fixtures/corpus/README.md): repository-shaped end-to-end cases with a
  summary oracle.
- [`fixtures/recall`](../fixtures/recall/README.md): findings that must stay detectable
  (`expectedFindings`), deliberate non-findings (`expectedAbsent`, with `isolation` where it
  explains the absence) and budgeted known misses. Never add a known miss to make CI pass.

```sh
cargo build --release --locked
npm run corpus:check
```

The corpus check runs every recall case with `--emit-fingerprints`, and the cases that declare a handler-rule isolation also with `--emit-pairs`. A handler-rule `isolation` is held against both: the fingerprint records show how the participants differ, and the pair records show whether the analyzer evaluated, excluded, never proposed or skipped the pair (the `compared` states in the [recall README](../fixtures/recall/README.md)). A missing or cut-short pair file, a record count that differs from the census of the pair-proposing sources, an ambiguous join or a record shape the check does not model rejects the case. `unisolatedNegativeBaseline` is 0.

`CUKE_DEDUP_MATCHER_WINDOW=1` forces one definition per matcher-compilation window so every
definition pair is cross-window; it changes memory and time, never findings. The full gate runs the
corpus once normally and once this way.

`npm run corpus:evaluate` runs the recall, behavior, language-parity and Ruby-parity corpora without asserting
and reports, per corpus and rule, true and false positives and negatives, known misses, precision,
recall and F1, plus each entry that disagrees with its expectation. A recall absence with an `isolation` is scored a true negative only when the same checks as the gate hold, pair records included; otherwise it is listed as `isolation-failed`. It is a report, not a gate. Exit
codes: 0 when every selected case was scored, 1 when a case could not be observed or scored, 2 for
an unknown case or corpus. `--corpus NAME` (`recall`, `behavior`, `language-parity` or `ruby-parity`) and `--case
NAME` limit the run; `--json FILE` and `--markdown FILE` write the report (markdown goes to stdout
when neither is given). Corpora stay separate: every row of the table and of the JSON `rules` list
names its corpus (and a language for the paired corpus), each corpus has a total row, and
`ruleTotals` sums each rule across all corpora under the corpus name `all`. Every corpus counts
findings. Recall counts known misses as desired findings, as the gate's baseline does. A
disagreement shows as a nonzero false positive or negative count. The gates fail on one that is not
recorded, so on the current corpora the counts are zero apart from the known miss; a gap the paired
gate records, or an entry that is listed but not scored (completeness, exit), can still appear. A
case counts as observed when every one of its runs was scored.

What a count means per corpus:

| Corpus | Observed by | Oracle | TP | FP | FN | TN |
| --- | --- | --- | --- | --- | --- | --- |
| recall | `observeRecallCase` | `expectedFindings`, `expectedAbsent`, `knownMisses` | expected finding reported | unowned or excess finding | expected finding missing | absence holds |
| behavior | `observeBehaviorCase` | `expected.byRule` counts | `min(expected, observed)` per rule | observed above expected, or a rule not listed | expected above observed | always 0 (no absences declared) |
| language-parity | `observeParityLane`, once per language | `expected.requiredFindings` and `requiredAbsent` | required finding reported | excess over a count, or a finding matching `requiredAbsent` | required finding missing | absence holds |
| ruby-parity | `observeRubyGroup` | the group's `requiredFindings` and `requiredAbsent` | as language-parity | as language-parity, plus any finding no required finding owns | as language-parity | absence holds |

A behavior case pins counts per rule, not individual findings, so it is scored on counts. A case
with no expected findings and none observed contributes only to the case count. Its exit status,
duplication summary and stderr messages are listed as entries when they disagree and are not scored,
so a case can show full recall and still have entries to review.

The paired corpus runs each case in TypeScript and in Ruby against the same oracle, so a row per
language shows where the two disagree with the contract. A finding no required finding owns is not
scored there, because its gate does not require ownership; the Ruby-parity gate does, so it is a
false positive in that corpus. Completeness and exit status (a Ruby
group declares its exit; a paired case implies it as the gate does) are listed as entries when they
disagree and are not scored. The Ruby-parity oracle's `duplication` map is not read, so a group can show
full recall and precision and still fail its gate on it. A finding owned by more than one required
finding is listed and credited to none of them, so each owner shows as missing.

A case that cannot be scored is listed under "Cases that could not be scored" with the cause and
counts for nothing, never as clean: a binary that did not start, or hit a process error such as
exceeding the output limit even when it wrote a report, an exit status the corpus does not allow, no
JSON report, truncated findings (parity corpora), a definition or feature-step count (and, for recall
and the parity corpora, a declared candidate-source count) that differs from the one the case
declares, or a label or report shape the classifier does not model (a missing or malformed
`byRule`, census counts or `duplication` summary, `requiredFindings` or `requiredAbsent`, a rule-less
expectation; a well-typed summary that disagrees with the case is an entry instead).

`npm run corpus:distribution` summarizes the recall manifest without running the analyzer: cases and
expectations per rule, outcome, language, framework, axis, origin, verdict and fixture root, an
axis-by-rule matrix, and the thin cells (axes and frameworks with `--floor N` cases or fewer, rules with
no negative or no positive, languages and origins with no case, cases without axes, handler and matcher
negatives without isolation), followed by an "All corpora" section that compares the recall, behavior,
language-parity and Ruby-parity corpora side by side (cases, languages, frameworks, roles, origins, and
cases per axis per corpus) and names axes no case uses. A case missing a label is a row in the report,
not an error.
`--json FILE` and `--markdown FILE` write the report; markdown goes to stdout when neither is given.
Every case in every corpus carries the labels described in the recall corpus README; the behavior and
recall corpora check `language` and `frameworks` against the run, and the parity corpora check format and
role.

`npm run corpus:candidate-recall` asks, for every pair finding a recall case expects, whether a
candidate source proposed the pair, from `--emit-pairs` records. It reports candidate recall per
pair rule, how many positives each source proposed and how many only it proposed (what removing the
source would lose), and names each declared pair that was never reported with where it stopped:

- `definitions-not-extracted`: too few definitions were extracted for a pair the expectation has not
  already had credited.
- `never-proposed`: the definitions exist, no pair record covers them, and the run evaluated every
  pair it generated.
- `skipped-by-limit`: as above, but a candidate or verification limit applied to a pair-proposing
  source (matcher overlap's own limit does not count), and only evaluated pairs are recorded, so
  generation cannot be blamed.
- `evaluated-no-finding`: a pair record exists and no finding was reported.
- `unresolved`: the expectation names a selector the join cannot apply (a message, a cluster shape, a
  related count, or only part of its locations).

A counted expectation is diagnosed from the pairs not already credited to a finding of its rule. A
cluster finding counts as reached through the pairs among its members that produced its rule. A
finding claimed by more than one expectation, or a cluster whose related locations the report truncated, joins no positive and is listed separately. Paths are compared in the form the JSON report writes them (control and invisible-format characters removed). Pairs of
`ambiguous-step`, `overlapping-matcher` and `unused-definition` have no pair record and are not
covered. A case cannot be observed when its report names a candidate source the join does not model or holds a census count it cannot read, when its pair file holds a different number of records than the census counts, or when a pair record is malformed or two records join one pair. Options are `--binary`, `--case`, `--json` and `--markdown`; it exits 1 when a case cannot be
observed and 2 for an unknown case, and reads the recall corpus only. On the synthetic corpus every positive is reached, because a reported finding was necessarily evaluated; the report is informative for known
misses and for labelled real projects.

### Real-project corpus

The synthetic corpora select for the shapes their authors thought of, so recall measured on them is biased. `npm run corpus:real` runs the built binary over pinned revisions of open-source BDD suites. No third-party source is committed; the repository holds only a manifest and sanitized snapshots under `evaluation/real/`.

- `evaluation/real/manifest.json` lists suites: `name` (the checkout directory name), `repo`, `commit` (a full 40-hex revision; a branch, tag or abbreviation is rejected), `root` (relative to the checkout), `framework` (`playwright-bdd` or `cucumber-js`, the label the analyzer gives the suite's definitions) and optional `expectedExits` (default `[0, 1]`).
- The analyzer runs inside the suite root with that directory's own configuration file, as a user would, so the pinned tree fixes the configuration too.
- Checkouts are read from `--references DIR` (default `references.local/repos`), one directory per suite. A checkout that is missing, not at the pinned revision, or has modified, untracked or ignored files (other than `node_modules` and `.DS_Store`) is an error that names the offending entries, never skipped. A run with another exit status, no report, fingerprint or pair file, a report whose findings list is truncated (the report keeps at most 10,000) or no definition of the declared framework is an error too.
- `evaluation/real/snapshots/<name>.json` holds counts and digests: definitions, feature steps, files, findings per rule, suppressed findings, pairs evaluated per rule and candidate source, candidates evaluated and skipped, and per finding `{id, fp}`. `id` digests the rule and the finding's locations; `fp` digests the rule and the definitions' `--emit-fingerprints` digests. No path, matcher, handler text or excerpt is stored, and `assertSanitized` rejects any other key or free-form value. Digests are comparable within one analyzer build, so a change to fingerprinting moves every `fp`; regenerate with `--update`. An incomplete corpus or a work limit does not reject a run; it is recorded in the census and shows as drift when it changes. A suite without a snapshot is reported as such, and `--gate` exits 2 on it. Options a command does not use (`--gate`, `--update` and `--json` for `sample` and `verdicts`; `--per-rule` and `--seed` for `run` and `verdicts`; `--labels` for `run`) are usage errors.
- `npm run corpus:real` (report-only) prints per-rule drift against the committed snapshots: `added` and `removed` count findings present on one side only, `changed` counts findings at the same locations whose definitions' digests differ. `--gate` exits 1 on any drift, including census, exit status and candidate counts. `--update` rewrites the snapshots. `--suite NAME` selects one suite, `--json FILE` writes the drift (only when every suite was observed), and `--binary`, `--references`, `--manifest` and `--snapshots` override the binary, checkout directory, manifest and snapshot directory. Exit 2 is a usage error, a suite that could not be observed, or a suite without a snapshot when using `--gate`.
- `npm run corpus:real -- sample --suite NAME [--per-rule N] [--seed S] [--labels FILE]` draws up to N findings per rule (default 10, reproducible per seed) and writes a labels file with `id`, `rule`, source locations, `verdict: null` and `note`. The default path is `real-labels/<suite>.json` beside the references directory. The sampler refuses an existing file and any path inside the work tree that git does not ignore, because labels name source locations; the default path needs `references.local/` ignored, for example in `.git/info/exclude`. It requires the run to match the committed snapshot.
- The reviewer sets each `verdict` to `true-positive`, `false-positive` or `unclear`. `npm run corpus:real -- verdicts [--labels DIR]` (exit 2 when no labels file exists) prints per-rule precision, true positives over reviewed true and false positives, with the sample size beside every ratio; unclear and unreviewed samples are counted but not in the ratio, and a rule with nothing reviewed prints `n/a`. Labels for another revision, an unknown finding or an invalid verdict are errors.

## Language adapters and parity

- `SourceAdapter` owns default source selection, input filtering and indirect-usage policy.
  Adapters return parameter-type declarations in `Extraction.parameter_types`; the core merges
  them with configured patterns (`analysis::merge_parameter_types`), whatever the language of the
  first definition file. Optional hooks have defaults.
  A stateful adapter (`StatefulSourceAdapter`, registered with
  `SourceAdapterRegistration::with_session`) owns per-run session state and may expand
  dependencies: its hook adds loaded sources, edges and gaps to `DiscoveredFiles`, reading the
  configured excludes through `ExcludeSet`; discovery then re-sorts and de-duplicates definitions.
  A framework contract may carry a `FrameworkDescriptor` (`FrameworkContract::with_discovery`).
  When the configuration names no feature patterns, descriptors run in registry order and the
  first match sets the patterns and their origin; its patterns are normalized like Cucumber
  `paths`. A descriptor error becomes a configuration warning, except an input-limit error from
  `DiscoveryInput::read`.
  `Framework` and the types holding one (`StepDefinition`, `AnalysisResult`) deserialize against
  the built-in adapters; a caller framework id reads back through `AdapterRegistry::framework`,
  `FrameworkSeed` or `RegistrySeed<T>`, which reads any `T` with a registry's ids in scope.
- An adapter not discovered by default is selected only by definition patterns that depend on its
  suffix. Ruby owns its configuration keys: the `ruby`
  section and the flat `rubyLoadPaths` alias are flattened into the core config from
  `ruby::config::RawRubyConfig`, which rejects both forms in one source; `--print-config` keeps the
  flat key.
- Ruby preparation parses one bounded source snapshot for registration and assertion proofs; a
  later source-byte mismatch withdraws the optional proof. File-count, byte, per-source and
  proof-work limits fall back conservatively; exhausting any collector withdraws every optional
  registration and handler proof. Direct registrations keep normal ownership checks.
- After `SourceExtractionSession::finalize`, `SourceFinalization.advisories` lists configured
  assertion providers rejected by the closed-factory proof. Advisories explain trust decisions;
  they remove no definitions and change no completeness. Incomplete evidence stays in
  `uncertainties`.
- Ruby step keywords come from the Gherkin dialect data pinned in `Cargo.lock`. After a `gherkin`
  update run `node scripts/check/check-ruby-step-keywords.mjs --write`; the check fails while the
  table is stale.

Parity lanes compare language-neutral counts, completeness, strict exits and finding ownership:

| Command | Role |
| --- | --- |
| `check-ruby-parity.mjs --regression`, `check-language-parity.mjs --regression` | Block CI when an observed outcome changes. Closing a known gap changes an outcome: update the recorded digest in the same change. |
| `check-ruby-parity.mjs` | Completion command; CI reports it without blocking. A green regression gate does not imply Ruby completion. |
| `check-ruby-parity.mjs --inventory-only` | Unit-test census; blocks CI. Every Rust test name needs a Ruby mapping in the change that adds or renames it. |

## Duplication, memory, benchmarks

- `npm run duplication:check` ratchets production and test scopes against percentage and absolute
  duplicated-line ceilings; it rejects growth and stale slack, so record improvements. Every Rust
  source belongs to exactly one scope, by file: `tests.rs` files, files under a `tests` directory and
  `fuzz/` are tests, the rest of `src/` is production. A test module therefore lives in its own
  `tests.rs` (`#[cfg(test)] mod tests;`). The check fails on a test-only module in a production file
  that is written inline, declared under another name or with `#[path]`, or made test-only by an
  inner `#![cfg(…)]`, and on a `mod tests` that is not test-only; "test-only" means its `cfg` cannot
  hold without `test`, so `cfg(not(test))` and `cfg(any(windows, test))` modules are not affected.
  It also fails on shapes it does not evaluate: `cfg_attr` or a path-qualified `cfg` on a module or
  inner attribute, and a test-only module named by a macro fragment. Test-only helpers such as
  `#[cfg(test)] fn` stay in the production figure. Review jscpd warnings about skipped oversized
  files.
- `npm run memory:check` compiles a deterministic corpus at two sizes and holds the marginal bytes
  per definition under a budget, with a work floor. Lower the budget when a change improves the
  figure; never raise it. Re-pin from CI, since peak resident memory is platform-dependent.
- `npm run benchmark` records wall-clock, discovery, parsing and analysis time, peak memory where
  available, input counts, findings, comparison counters and binary size. The scheduled workflow
  runs `npm run benchmark:check`: full case inventory, deterministic outcomes, peak-memory and
  binary-size ceilings, and growth ceilings. Growth is a ratio of median parse times between two
  sizes of one profile in the same run (`scaling` in `scripts/check/benchmark-budgets.json`); a
  small-size median under 50 ms fails as too small to gate; a ratio does not catch a uniform
  constant-factor slowdown. Absolute times are advisory.
- Environment: `CUKE_DEDUP_BENCH_REPEATS` (sample count), `CUKE_DEDUP_BENCH_SMOKE=1` (reduced
  profile used by local and CI checks), `CUKE_DEDUP_BENCH_PROFILE=<name>` (one profile).

## Output equivalence

Refactoring and performance changes must not change analyzer output. `npm run check:equivalence`:

- Builds the analyzer at a baseline ref (`--ref`, default the newest `v*` tag reachable from
  `HEAD`) and caches it under `target/equivalence`.
- Runs the corpus, matcher-window corpus, Ruby regression, language-parity and benchmark checkers,
  and the `tests/cli.rs` integration tests (`cli`), with a shim in place of
  `target/release/cuke-dedup`. The tests reach the shim through `CARGO_BIN_EXE_cuke-dedup`; the one
  test that resolves the binary with `cargo_bin!` bypasses it.
- For every invocation runs both binaries and compares exit status, stdout, stderr, written or
  deleted reports and an updated `--baseline` file, after removing timing metrics, the
  `--baseline-from-ref` checkout path and the version string.
- Locates the report directory (configured or `--output`) and, under `--update-baseline`, the
  baseline file with `--print-config`. Every file created, rewritten or deleted inside those locations is compared
  (a symbolic link by its target, never followed; invalid UTF-8 by exact bytes; another file type
  fails), then restored after the baseline run so both binaries start from the same state.
- Compares presence, content, file kind and link targets. Permissions, ownership, extended
  attributes and hard-link identity are out of scope.
- Fails on any difference or on a suite that makes no analyzer invocation; a failing checker
  oracle or test is reported separately. `--suites corpus,cli` selects suites.

Build the candidate first with `cargo build --release --locked`; the `cli` suite builds the test
executable with `cargo test --no-run`. Runs on macOS and Linux.

## Mutation score

The scheduled mutation workflow holds the core similarity and evidence contracts to a 90% score.
Run the same focused measurement locally:

```sh
cargo mutants \
  --file 'src/analysis/similarity.rs' \
  --file 'src/analysis/evidence.rs' \
  --file 'src/analysis/pairs.rs' \
  --file 'src/analysis/suppression.rs' \
  --re 'src/analysis/(similarity|evidence|suppression)\.rs|CandidateSources::|ComparisonClasses::|comparison_classes|intern_class|behavior_event_ids|CandidateGeneration::mark_verification_truncated|pair_similarity_work|candidate_suppression_work|definition_comparison_bytes|matrix_work|multipartite_count_sizes|insert_matcher_blocking_candidates|matcher_shingle_keys|MatcherPosting::|meaningful_handler|covered_by_structural_source|can_reach_handler_similarity_gate|consider_matcher_blocking_pair|try_insert_matcher_blocking_candidate' \
  --jobs 1 \
  --jobserver-tasks 2
node scripts/check/check-mutation-score.mjs mutants.out/outcomes.json 90
```

## Packaging notes

The Python packaging utility uses only the standard library to create deterministic `tar.gz` and
ZIP archives. Node.js owns npm manifest and version validation; Rust owns the analyzer and CLI.

## CI inventory

CI reads the MSRV from `Cargo.toml` and tests it on Linux, macOS and Windows, publishes JUnit
results, measures Rust coverage, compiles the parser fuzz target, scans Git history for secrets,
validates clean-room Cargo and npm installation, validates the release binary and the local GitHub
Action against the corpus, tests the npm launcher on Node.js 24 LTS with a Node.js 20 compatibility
check, and enforces stable Rust quality, rustdoc, dependency policy and deterministic release
packaging. Scheduled workflows run the full fuzz campaign, focused mutation analysis and the
scalability profiles, and catch disclosures or leaked credentials that occur without a source change.
