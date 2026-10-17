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

The corpus check and `npm run benchmark` run every case after a failure, report each failing one under its case name, and exit 1; the benchmark then prints no results.

Update `rust-toolchain.toml` when upgrading the development and release compiler together. Run `rustup update stable` to refresh the separate stable quality checks. Use `cargo +1.90.0 test --all-targets --all-features --locked` to check minimum-version compatibility locally.

Enable the repository's pre-commit hook:

```sh
git config core.hooksPath .githooks
```

The pre-commit hook scans the staged patch for secrets and runs the quick development gate. The pre-push hook requires a clean checkout at the pushed commit, derives conditional checks from the outgoing changes, and runs the complete release-grade gate. This keeps iterative commits fast without testing a different tree from the one leaving the clone. Before the full gate, pre-push requires documentation for 100% of locally detected touched named functions and test callbacks in the cumulative diff against `origin/main`, including three context lines. Fetch `origin main` before pushing. Rust doc comments, JavaScript/TypeScript JSDoc and Python docstrings count; tests and fixtures are included. Python sources require Python 3 (`python` on Windows, `python3` elsewhere). JavaScript/TypeScript and Python ranges come from syntax trees; Rust uses declaration matching after masking comments and literals. This guard does not guarantee agreement with CodeRabbit’s independent metric. Run `node scripts/check/check-docstrings.mjs --base origin/main` to check the committed branch directly. A missing base or an unreadable supported source fails validation. Install the check tools with `npm ci --prefix scripts/check/tools` before running either gate. Run either gate directly at any time:

```sh
scripts/check/check.sh --quick
scripts/check/check.sh
```

Both gates start with `scripts/check/preflight.sh`, which checks whitespace, Rust formatting, Ruby keyword metadata, checker unit tests, language boundaries, and documentation parity before compilation. Run it directly for the fastest deterministic feedback.

Coverage is measured separately because it rebuilds the test suite with instrumentation:

```sh
cargo +stable llvm-cov --all-features --tests --locked --no-report
cargo +stable llvm-cov report --summary-only --fail-under-lines 97
```

The npm launcher’s own checks have no third-party development dependencies. The check tools, `jscpd` and the pinned TypeScript parser, live in `scripts/check/tools/` with a dedicated manifest and lockfile — install it with `npm ci --prefix scripts/check/tools` — so its whole dependency graph is pinned by integrity hash without the root manifest gaining dependencies. This avoids forcing npm to install local workspace packages intended for other operating systems and CPU architectures. CI enforces a 97% Rust line-coverage floor.

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

## Language adapters

`SourceAdapter` owns default source selection, input filtering, indirect-usage policy and source parameter resolution. Its later hooks have defaults so existing implementations remain compatible. Dependency expansion uses private registry callbacks; it does not expose glob types or discovery mutation to public adapter implementations. An adapter that is not discovered by default is selected only by definition patterns that depend on its suffix. `rubyLoadPaths` keeps its flat JSON key, precedence and public getter while Ruby owns its storage.

Ruby preparation reads and parses one bounded source snapshot for both registration and assertion proofs. A later source-byte mismatch withdraws its optional proof. File-count, aggregate-byte, per-source and proof-work limits retain conservative fallback; they do not make arbitrary runtime behavior statically knowable. Each proof collector has its own work budget; exhausting any of them withdraws all optional registration and handler proofs. Direct registrations remain subject to normal ownership checks.

After `SourceExtractionSession::finalize`, embedders may inspect `SourceFinalization.advisories` for resolved configured assertion providers that the closed-factory proof rejected. Advisories explain trust decisions; they do not remove definitions or change completeness. Incomplete source evidence remains in `uncertainties`.

Ruby step keywords come from the Gherkin dialect data pinned by `Cargo.lock`. After a `gherkin` update, regenerate them with `node scripts/check/check-ruby-step-keywords.mjs --write`; the check fails while the table is stale.

Corpus conformance compares language-neutral counts, completeness, strict exits and finding ownership. Each language lane maps source names explicitly. Desired outcomes, known implementation gaps and justified language differences remain separate:

- `check-ruby-parity.mjs --regression` and `check-language-parity.mjs --regression` block CI when an observed outcome changes. Closing a known gap also changes an outcome, so update the recorded digest in the same change.
- `check-ruby-parity.mjs` without `--regression` is the completion command. CI reports it without blocking; a green regression gate does not imply Ruby completion. Its unit-test census (`--inventory-only`) blocks CI: every Rust test name needs a Ruby mapping in the change that adds or renames it.

The Python packaging utility intentionally uses only the standard library to create deterministic `tar.gz` and ZIP archives. Node.js owns npm manifest/version validation, while Rust owns the analyzer and CLI.

## Coverage floor and uncovered lines

CI enforces 97% Rust line coverage. Reassess uncovered regions from the current artifact rather than treating this list as an inventory. Generate a fresh report with:

```sh
cargo +stable llvm-cov --all-features --tests --locked --no-report
cargo +stable llvm-cov report --show-missing-lines --fail-under-lines 97
```

The summary includes brace-only continuation lines, so its missed-line count can exceed the region listing. The categories below explain defensive paths that may remain uncovered; each exemption needs an upstream, platform, race, resource or I/O reason. Other uncovered lines need tests. Test a path when it becomes reachable; remove unreachable code without defensive value.

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

### Test-only formatting

- `src/typescript/ast.rs` (assert-failure formatting): the format arguments of a passing `assert!` are never evaluated.

### Failing-I/O propagation

- `src/main.rs` (BrokenPipe arm), `src/reporters/terminal.rs`, `src/cli.rs`, `src/modes.rs`, `src/framework_config.rs`, and `src/config.rs`: remaining uncovered `?` paths propagate failed writes, git subprocess output, or configuration reads. The in-process suite does exercise a terminal footer write returning `BrokenPipe`; other failure paths require different failure points or external conditions such as a vanished file.

### Reachable residual

Uncovered regions outside the justified categories need tests. Inspect the current report: historical examples can become covered, as `SourceAdapterRegistration::with_session` did through runtime tests.

## Corpus and benchmarks

The sanitized end-to-end fixtures and their manifest are documented in [`fixtures/corpus`](fixtures/corpus/README.md). Add a corpus case for parser, discovery, policy, or reporting regressions that benefit from a repository-shaped fixture. [`fixtures/recall`](fixtures/recall/README.md) separately records exact findings that must remain detectable, deliberate non-findings, and explicitly budgeted known misses. Never add a known miss merely to make CI pass. Build the release binary before running both corpora:

```sh
cargo build --release --locked
npm run corpus:check
```

`npm run duplication:check` ratchets production and test scopes against measured percentage and absolute duplicated-line ceilings. It rejects growth and stale slack, so record improvements. Every Rust source must belong to exactly one scope. Review warnings for staged files omitted by jscpd: oversized-file skips can understate duplication.

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

Record the scalability profiles with `npm run benchmark`. The benchmark measures wall-clock, discovery, parsing, analysis, peak resident memory where available, input counts, findings, deterministic comparison counters, and release binary size. The scheduled workflow runs `npm run benchmark:check` to enforce the full case inventory, deterministic outcomes, peak-memory ceilings, binary-size ceiling, and growth ceilings. Absolute elapsed times remain advisory because hosted-runner timing is noisy. Growth is gated as a ratio of median parse times between two sizes of one profile measured in the same run (`scaling` in `scripts/check/benchmark-budgets.json`); a small-size median under 50 ms fails as too small to gate. Lower a ceiling when a change reduces that growth. A ratio does not catch a uniform constant-factor slowdown. Set `CUKE_DEDUP_BENCH_REPEATS` to change the sample count, `CUKE_DEDUP_BENCH_SMOKE=1` to select the reduced validation profile used by local and CI checks, or `CUKE_DEDUP_BENCH_PROFILE=usage-scale` to isolate one profile.

Refactoring and performance changes must not change analyzer output. `npm run check:equivalence` builds the analyzer at a baseline ref (`--ref`, default the newest `v*` tag reachable from `HEAD`), caches it under `target/equivalence`, and runs the corpus, matcher-window corpus, Ruby regression, language-parity and benchmark checkers and the `tests/cli.rs` integration tests (`cli`) with a shim in place of `target/release/cuke-dedup`. The tests reach the shim through `CARGO_BIN_EXE_cuke-dedup`; the one test that resolves the binary at compile time with `cargo_bin!` bypasses it. Every invocation runs both binaries and compares exit status, stdout, stderr, written or deleted reports and an updated `--baseline` file after removing timing metrics, the `--baseline-from-ref` checkout path and the version string. The report directory (configured or `--output`) and, under `--update-baseline`, the `--baseline` file are located with `--print-config`; both binaries write to them in turn. Each run is compared on every file it creates, rewrites or deletes inside those locations (a symbolic link by its target, never followed, the location itself included; invalid UTF-8 by its exact bytes; another file type fails the check), and after the baseline run they are restored (rewritten and deleted files back, created files and directories removed), so both binaries start from the same state. The check compares file presence, exact content (text normalized only when valid UTF-8), file kind and link targets; permissions, ownership, extended attributes and hard-link identity are out of scope, since the analyzer writes only reports and a baseline; the checker or test sees only the candidate's output. A difference, or a suite that makes no analyzer invocation, fails the check; a failing checker oracle or test is reported separately. `--suites corpus,cli` selects suites. Build the candidate first with `cargo build --release --locked`; the `cli` suite builds the test executable with `cargo test --no-run`. The check runs on macOS and Linux.

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

The script requires Git and updates all version sites and both Cargo lockfiles atomically. It rejects unexpected file shapes, restores changes if a lockfile refresh fails, and preserves line endings. Maintain release notes in [GitHub Releases](https://github.com/figueiredoluiz/cuke-dedup/releases).

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

Experimental Ruby releases require the ordinary project and packaging checks and the Ruby regression gate to pass. The separate Ruby completion census tracks work toward broader support; pending evidence or design entries do not by themselves block an experimental release. Keep known limitations documented and assess serious regressions and security exposure before publication.
