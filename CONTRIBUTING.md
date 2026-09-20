# Contributing to CukeDedup

Thanks for helping improve CukeDedup. Bug reports should include a small `.feature` file and step-definition sample whenever possible; remove application-specific secrets and identifiers first.

## Development setup

Install Rust through rustup, Node.js 24 LTS, npm, Python 3, Gitleaks, actionlint, cargo-audit, cargo-deny, and cargo-shear. Development, rust-analyzer, and release builds use the exact Rust version pinned in `rust-toolchain.toml` (currently 1.98.1). The minimum supported Rust version remains 1.90 in `Cargo.toml`; CI tests it separately and also checks the current stable compiler. Prepare the Rust tools once:

```sh
rustup toolchain install 1.90.0 --profile minimal
rustup toolchain install 1.98.1 --profile minimal --component clippy,rustfmt,llvm-tools-preview
rustup toolchain install stable --profile minimal --component clippy,rustfmt
cargo install cargo-llvm-cov --locked
cargo install cargo-audit --version 0.22.2 --locked
cargo install cargo-deny --version 0.20.2 --locked
cargo +stable install cargo-shear --version 1.13.4 --locked
cargo install cargo-mutants --version 27.1.0 --locked
```

Update `rust-toolchain.toml` when upgrading the development and release compiler together. Run `rustup update stable` to refresh the separate stable quality checks. Use `cargo +1.90.0 test --all-targets --all-features --locked` to check minimum-version compatibility locally.

Enable the repository's pre-commit hook:

```sh
git config core.hooksPath .githooks
```

The pre-commit hook scans the staged patch for secrets and runs the quick development gate. The pre-push hook requires a clean checkout at the pushed commit, derives conditional checks from the outgoing changes, and runs the complete release-grade gate. This keeps iterative commits fast without testing a different tree from the one leaving the clone. Run either gate directly at any time:

```sh
scripts/check/check.sh --quick
scripts/check/check.sh
```

Coverage is measured separately because it rebuilds the test suite with instrumentation:

```sh
cargo +stable llvm-cov --all-features --tests --locked --no-report
cargo +stable llvm-cov report --summary-only --fail-under-lines 97
```

The npm launcher currently has no third-party development dependencies, so contributor and CI checks run directly without an install step. The one analysis tool that needs installing, `jscpd`, lives in `scripts/check/tools/` with its own manifest and lockfile — install it with `npm ci --prefix scripts/check/tools` — so its whole dependency graph is pinned by integrity hash without the root manifest gaining dependencies. This avoids forcing npm to install local workspace packages intended for other operating systems and CPU architectures. CI enforces a 97% Rust line-coverage floor.

The full local gate always runs `actionlint`, `cargo-audit`, `cargo-deny`, and `cargo-shear`. The quick gate runs them only when the staged patch changes or deletes the corresponding Action, manifest, lock, or policy files. The dependency commands are:

```sh
cargo audit --deny warnings
cargo audit --deny warnings --file fuzz/Cargo.lock
cargo deny check advisories bans licenses sources
cargo deny --manifest-path fuzz/Cargo.toml check advisories bans licenses sources
cargo shear --deny-warnings
(cd fuzz && cargo shear --deny-warnings)
```

The dependency policy is maintained in `deny.toml`: known security advisories, unknown registries or Git sources, wildcard version requirements, and dependencies outside the approved SPDX license list fail CI. `cargo-shear --deny-warnings` rejects unused or misplaced dependencies, unlinked Rust source files, and stale suppressions in both Rust manifests. The pre-commit hook checks staged dependency changes before they are recorded, while a direct full gate checks policies unconditionally. CI additionally scans the full Git history. Before submitting a pull request, run `scripts/check/check.sh` explicitly if the pre-push hook was bypassed.

## Changes

- Keep the source-adapter → shared IR → analysis → reporter boundaries intact.
- Add a focused regression test for every bug fix. Prefer sanitized inline fixtures; do not commit third-party repositories.
- Major new functionality must include automated tests that exercise its public behavior and failure modes. Parser and discovery changes must add adversarial cases, not only happy-path examples.
- Preserve deterministic ordering and the canonical JSON schema unless the change explicitly introduces a documented schema version. Reporter changes must retain equivalent active finding counts, rule/message/location evidence, and threshold results across terminal, JSON, JSONL, HTML, and SARIF.
- A new public type is marked `#[non_exhaustive]` when the analyzer returns it and left exhaustive when a caller has to construct it, because the attribute makes a type impossible to build from outside the crate. The crate-level documentation in `src/lib.rs` states the rule and lists the constructed types; follow it rather than copying a neighbouring type.
- Use conventional commit subjects such as `fix:`, `feat:`, `test:`, `docs:`, and `chore:`.
- Do not edit generated release assets or commit build output.

The Python packaging utility intentionally uses only the standard library to create deterministic `tar.gz` and ZIP archives. Node.js owns npm manifest/version validation, while Rust owns the analyzer and CLI.

## Coverage floor and uncovered lines

CI enforces a 97% Rust line-coverage floor. The lines left uncovered fall into two groups: lines with an external reason no in-process test can reach them, and reachable residual lines that simply have no test yet. The justified group is recorded below so the next coverage run can be interpreted without re-deriving each case. Regenerate the current list with:

```sh
cargo +stable llvm-cov --all-features --tests --locked --no-report
cargo +stable llvm-cov report --show-missing-lines --fail-under-lines 97
```

The listing enumerates each missed region; the summary's "Missed Lines" count also includes brace-only continuation lines inside those regions, so it reads slightly higher than the listing. An entry here must cite an external reason — an upstream contract, a platform invariant, a race window, a resource scale, compile-time evaluation, or failing I/O. Anything else is a missing test, not an entry. When a change makes a listed line reachable, write the test; when a change proves code unreachable with no defensive value, delete the code rather than adding it here.

### Excluded by an upstream guarantee

- `src/analysis/suppression.rs` (path-configured suppressions without a compiled matcher): config loading rejects invalid suppression path globs before the index is built, so a configured path always comes with a working matcher.
- `src/discovery.rs` (depth-zero arm of the walk filter): the `ignore` walker (0.4.33) decides the depth-zero entry itself and never consults the custom `filter_entry` predicate for the root.
- `src/typescript/module_resolver.rs` (missing-parent arm of `is_top_level_variable`): a `variable_declarator` always has a parent declaration node in the pinned tree-sitter grammars.
- `src/reporters/sarif.rs` (`Severity::Off` arm): analysis drops severity-off findings before any finding is recorded, so no reporter ever sees one.

### Platform and race invariants

- `src/config.rs` and `src/typescript/project_resolution.rs` (existing-ancestor walks): `Path::parent()` returns `None` only for root-like paths, and an ancestor that just passed `exists()` failing `canonicalize()` requires the path to vanish between the two calls (TOCTOU).
- `src/modes.rs` and `src/reporters/shared.rs` (no-parent arms of report-path creation): a path without a parent means writing directly to a filesystem root.

### Arithmetic overflow guards

- `src/analysis/pairs.rs` and `src/analysis/usage.rs` (`checked_add` refusal arms): u64 sums over per-definition counters; addressable memory is exhausted far below the wrap point.

### Resource-limit bails

These fire only on inputs far beyond test-practical size:

- `src/typescript/project_resolution.rs`: the workspace-scan limit trips only past 100,000 walked entries.
- `src/analysis/pairs.rs`: the structural-candidate limit, the matcher-shingle cap, the matcher-blocking event budget, and the candidate-list limit.
- `src/analysis/usage.rs`: the proposal and suppression work budgets and the regex byte cap.
- `src/cli.rs`: the unmatched-suppression safety-limit warning.

### Compile-time evaluation and test-only formatting

- `src/source_adapter.rs` (`const fn with_session`): const-evaluated into `static SOURCE_ADAPTER_REGISTRY`, and const evaluation emits no runtime coverage counters.
- `src/typescript/ast.rs` (assert-failure formatting): the format arguments of a passing `assert!` are never evaluated.

### Failing-I/O propagation

- `src/main.rs` (BrokenPipe arm), `src/reporters/terminal.rs`, `src/cli.rs`, `src/modes.rs`, `src/framework_config.rs`, and `src/config.rs`: remaining uncovered `?` paths propagate failed writes, git subprocess output, or configuration reads. The in-process suite does exercise a terminal footer write returning `BrokenPipe`; other failure paths require different failure points or external conditions such as a vanished file.

### Reachable residual

Everything the listing shows beyond the groups above — Gherkin Markdown edges, tree-sitter walk fall-throughs across the TypeScript adapters, decorated-handler diagnostic branches, and a few CLI paths such as an absolute config file or a severity-off finding — is reachable and awaiting a test, not justified.

## Corpus and benchmarks

The sanitized end-to-end fixtures and their manifest are documented in [`fixtures/corpus`](fixtures/corpus/README.md). Add a corpus case for parser, discovery, policy, or reporting regressions that benefit from a repository-shaped fixture. [`fixtures/recall`](fixtures/recall/README.md) separately records exact findings that must remain detectable, deliberate non-findings, and explicitly budgeted known misses. Never add a known miss merely to make CI pass. Build the release binary before running both corpora:

```sh
cargo build --release --locked
npm run corpus:check
```

Ratchet copy-paste duplication with `npm run duplication:check`. Two scopes — production and test
sources — each pinned at the value measured today, not an aspirational one: a gate that is red on
arrival gets ignored, which is worse than no gate. The gate fails both when a scope exceeds its
ceiling **and** when it sits far enough below one that the ceiling has gone stale, so an improvement
has to be recorded rather than left as slack a later regression can reclaim. Every Rust source in
the repository must belong to exactly one scope; a new file in neither fails until it is classified. Each scope carries both a percentage ceiling and an absolute duplicated-line ceiling, because a
percentage alone falls whenever unique code is added and so can hide copied code accumulating. The
gate also names any non-trivial staged file jscpd did not analyse, because jscpd skips oversized
files silently and a skipped file lowers the percentage — a measurement here once reported 0.71%
where the real figure was 2.88% for that reason.

Guard matcher-analysis memory with `npm run memory:check`. It compiles a deterministic corpus at
two sizes and holds the **marginal** bytes per definition — the cost that scales with corpus size,
as distinct from fixed startup — under a budget, with a work floor so a run that analyses less
cannot pass more easily. Lower the budget when a change improves the figure; never raise it to make
a regression pass, and re-pin it from CI rather than a local run, since peak resident memory is
platform-dependent.

Matchers are compiled in windows rather than all at once. `CUKE_DEDUP_MATCHER_WINDOW` overrides the
window size and exists so the corpus gate can force one definition per window, which makes every
definition pair cross-window and exercises the merge paths that a fixture-sized corpus would
otherwise never reach. It changes memory and running time, never findings.

Record the non-blocking scalability profiles with `npm run benchmark`. The benchmark measures wall-clock, discovery, parsing, analysis, peak resident memory where available, input counts, and findings. Set `CUKE_DEDUP_BENCH_REPEATS` to change the sample count, `CUKE_DEDUP_BENCH_SMOKE=1` to select the reduced validation profile used by local and CI checks, or `CUKE_DEDUP_BENCH_PROFILE=usage-scale` to isolate one profile.

The scheduled mutation workflow holds the core similarity and evidence contracts to a 90% score. Its checked-in configuration keeps local execution bounded; run the same focused measurement with:

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

## Pull requests

Describe the user-visible problem, the chosen behavior, and the validation performed. CI reads the declared MSRV from `Cargo.toml`, tests it on Linux, macOS, and Windows, publishes JUnit results, measures Rust coverage, compiles the parser fuzz target, scans Git history for secrets, validates clean-room Cargo and npm installation, validates the release binary and local GitHub Action against the corpus, tests the npm launcher on Node.js 24 LTS with an additional Node.js 20 compatibility check, and enforces stable Rust quality, rustdoc, dependency policy, and deterministic release packaging. Scheduled workflows run the full parser fuzz campaign, focused mutation analysis, and scalability profiles, and catch vulnerability disclosures or leaked credentials that occur without a source change.

Maintainers may ask for a smaller change when a pull request mixes unrelated parser, rule, reporter, and distribution behavior.

## Releasing

The version appears in both Cargo manifests, both Cargo lockfiles, the root and eight platform npm
manifests, the npm lockfile, the `action.yml` and `release.yml` version inputs, and the Action pin
in `README.md` and `docs/ci-and-baselines.md`. Never edit those by hand. Bump them together:

```bash
npm run bump:version -- 0.3.0
```

The script reads the current version from `Cargo.toml`, rewrites every dependent site, promotes the
accumulated `## [Unreleased]` changelog notes into a dated section with a compare link, and opens a
fresh `Unreleased` heading. It refuses to run when a target file no longer matches its expected
version site or when `Unreleased` is empty, and if a lockfile refresh fails after the manifests
are written it restores every file it touched, so a failed bump always leaves the tree as it was
found. Line endings are preserved, so a CRLF checkout stays on CRLF. Pass `RELEASE_DATE=YYYY-MM-DD`
to override the changelog date.

Then confirm and commit:

```bash
npm run check:versions
git diff
```

`check:versions` also runs in the development gate and in CI, where `EXPECTED_VERSION` and
`RELEASE_TAG` additionally pin the agreed version to the dispatched release and its tag.

## Release prerequisites

Cargo, npm, the Git tag, and the GitHub release use one version. npm publication uses OIDC trusted
publishing and never reads a long-lived registry token. Before a release, every package in the npm
matrix must authorize the `figueiredoluiz/cuke-dedup` repository, `.github/workflows/release.yml`,
and `npm` environment on npmjs.com, with direct `npm publish` allowed. The workflow rejects npm
clients older than the trusted-publishing minimum.

The project maintainer has final responsibility for scope, compatibility, and release decisions.
