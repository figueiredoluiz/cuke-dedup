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
  `src/framework_config.rs`, `src/config.rs`: `?` paths that propagate failed writes, git
  subprocess output or configuration reads. The suite exercises one terminal footer write returning
  `BrokenPipe`; the others need external conditions such as a vanished file.

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

`CUKE_DEDUP_MATCHER_WINDOW=1` forces one definition per matcher-compilation window so every
definition pair is cross-window; it changes memory and time, never findings. The full gate runs the
corpus once normally and once this way.

`npm run corpus:evaluate` runs the same recall cases without asserting and reports, per rule, true and
false positives and negatives, known misses, precision, recall and F1, plus each entry that disagrees
with its expectation (missing, unowned, excess, a known miss that is now found). Recall counts known
misses as desired findings, as the gate's baseline does. It is a report, not a gate: it exits 1 only
when a case cannot be observed. `--case NAME` limits the run; `--json FILE` and `--markdown FILE`
write the report (markdown goes to stdout when neither is given). On the current corpus every
disagreement count is zero except the one known miss, because the gate already fails on any other.

`npm run corpus:distribution` summarizes the recall manifest without running the analyzer: cases and
expectations per rule, outcome, language, framework, axis, origin, verdict and fixture root, an
axis-by-rule matrix, and the thin cells (axes and frameworks with `--floor N` cases or fewer, rules with
no negative or no positive, languages and origins with no case, cases without axes, handler and matcher
negatives without isolation). A case missing a label is a row in the report, not an error.
`--json FILE` and `--markdown FILE` write the report; markdown goes to stdout when neither is given.
The case labels it reads are described in the recall corpus README.

`npm run corpus:candidate-recall` asks, for every pair finding a recall case expects, whether a
candidate source proposed the pair, from `--emit-pairs` records. It reports candidate recall per
pair rule, how many positives each source proposed and how many only it proposed (what removing the
source would lose), and names each declared pair that was never reported with where it stopped:

- `definitions-not-extracted`: too few definitions were extracted for a pair the expectation has not
  already had credited.
- `never-proposed`: the definitions exist, no pair record covers them, and the run evaluated every
  pair it generated.
- `skipped-by-limit`: as above, but a candidate or verification limit applied, and only evaluated
  pairs are recorded, so generation cannot be blamed.
- `evaluated-no-finding`: a pair record exists and no finding was reported.
- `unresolved`: the expectation names a selector the join cannot apply (a message, a cluster shape, a
  related count, or only part of its locations).

A counted expectation is diagnosed from the pairs not already credited to a finding of its rule. A
cluster finding counts as reached through the pairs among its members that produced its rule. A
finding claimed by more than one expectation, or a cluster whose related locations the report truncated, joins no positive and is listed separately. Paths are compared in the form the JSON report writes them (control and invisible-format characters removed). Pairs of
`ambiguous-step`, `overlapping-matcher` and `unused-definition` have no pair record and are not
covered. Options and exit codes are those of `corpus:evaluate`. On the synthetic corpus every positive
is reached, because a reported finding was necessarily evaluated; the report is informative for known
misses and for labelled real projects.

## Language adapters and parity

- `SourceAdapter` owns default source selection, input filtering, indirect-usage policy and
  parameter-type resolution. Later hooks have defaults, so existing adapters stay compatible.
  Dependency expansion uses private registry callbacks and exposes no glob types or discovery
  mutation.
- An adapter not discovered by default is selected only by definition patterns that depend on its
  suffix. `rubyLoadPaths` keeps its flat JSON key, precedence and public getter while Ruby owns the
  storage.
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
  source belongs to exactly one scope. Review jscpd warnings about skipped oversized files.
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
