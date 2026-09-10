# Contributing to CukeDedup

Thanks for helping improve CukeDedup. Bug reports should include a small `.feature` file and step-definition sample whenever possible; remove application-specific secrets and identifiers first.

## Development setup

Install Rust through rustup, Node.js 20 or newer, npm, Python 3, Gitleaks, actionlint, cargo-audit, and cargo-deny. The repository pins Rust 1.90 for compatibility tests, while the local quality gate also checks the current stable toolchain. Prepare the Rust tools once:

```sh
rustup toolchain install 1.90.0 --profile minimal --component clippy,rustfmt,llvm-tools-preview
rustup toolchain install stable --profile minimal --component clippy,rustfmt
cargo install cargo-llvm-cov --locked
cargo install cargo-audit --version 0.22.2 --locked
cargo install cargo-deny --version 0.20.2 --locked
cargo install cargo-mutants --version 27.1.0 --locked
```

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
cargo llvm-cov --all-features --all-targets --locked --fail-under-lines 90
```

The npm launcher currently has no third-party development dependencies, so contributor and CI checks run directly without an install step. This avoids forcing npm to install local workspace packages intended for other operating systems and CPU architectures. CI enforces a 90% Rust line-coverage floor.

The full local gate always runs `actionlint`, `cargo-audit`, and `cargo-deny`. The quick gate runs them only when the staged patch changes or deletes the corresponding Action, manifest, lock, or policy files. The dependency commands are:

```sh
cargo audit --deny warnings
cargo audit --deny warnings --file fuzz/Cargo.lock
cargo deny check advisories bans licenses sources
cargo deny --manifest-path fuzz/Cargo.toml check advisories bans licenses sources
```

The dependency policy is maintained in `deny.toml`: known security advisories, unknown registries or Git sources, wildcard version requirements, and dependencies outside the approved SPDX license list fail CI. The pre-commit hook checks staged dependency changes before they are recorded, while a direct full gate checks policies unconditionally. CI additionally scans the full Git history. Before submitting a pull request, run `scripts/check/check.sh` explicitly if the pre-push hook was bypassed.

## Changes

- Keep the source-adapter → shared IR → analysis → reporter boundaries intact.
- Add a focused regression test for every bug fix. Prefer sanitized inline fixtures; do not commit third-party repositories.
- Major new functionality must include automated tests that exercise its public behavior and failure modes. Parser and discovery changes must add adversarial cases, not only happy-path examples.
- Preserve deterministic ordering and the canonical JSON schema unless the change explicitly introduces a documented schema version. Reporter changes must retain equivalent active finding counts, rule/message/location evidence, and threshold results across terminal, JSON, JSONL, HTML, and SARIF.
- Use conventional commit subjects such as `fix:`, `feat:`, `test:`, `docs:`, and `chore:`.
- Do not edit generated release assets or commit build output.

The Python packaging utility intentionally uses only the standard library to create deterministic `tar.gz` and ZIP archives. Node.js owns npm manifest/version validation, while Rust owns the analyzer and CLI.

## Corpus and benchmarks

The sanitized end-to-end fixtures and their manifest are documented in [`fixtures/corpus`](fixtures/corpus/README.md). Add a corpus case for parser, discovery, policy, or reporting regressions that benefit from a repository-shaped fixture. [`fixtures/recall`](fixtures/recall/README.md) separately records exact findings that must remain detectable, deliberate non-findings, and explicitly budgeted known misses. Never add a known miss merely to make CI pass. Build the release binary before running both corpora:

```sh
cargo build --release --locked
npm run corpus:check
```

Record the non-blocking scalability profiles with `npm run benchmark`. The benchmark measures wall-clock, discovery, parsing, analysis, peak resident memory where available, input counts, and findings. Set `CUKE_DEDUP_BENCH_REPEATS` to change the sample count, `CUKE_DEDUP_BENCH_SMOKE=1` to select the reduced validation profile used by local and CI checks, or `CUKE_DEDUP_BENCH_PROFILE=usage-scale` to isolate one profile.

The scheduled mutation workflow holds the core similarity and evidence contracts to a 90% score. Its checked-in configuration keeps local execution bounded; run the same focused measurement with:

```sh
cargo mutants --file 'src/analysis/similarity.rs' --file 'src/analysis/evidence.rs' --jobs 1 --jobserver-tasks 2
node scripts/check/check-mutation-score.mjs mutants.out/outcomes.json 90
```

## Pull requests

Describe the user-visible problem, the chosen behavior, and the validation performed. CI reads the declared MSRV from `Cargo.toml`, tests it on Linux, macOS, and Windows, publishes JUnit results, measures Rust coverage, compiles the parser fuzz target, scans Git history for secrets, validates clean-room Cargo and npm installation, validates the release binary and local GitHub Action against the corpus, tests the npm launcher on Node 20 and 24, and enforces stable Rust quality, rustdoc, dependency policy, and deterministic release packaging. Scheduled workflows run the full parser fuzz campaign, focused mutation analysis, and scalability profiles, and catch vulnerability disclosures or leaked credentials that occur without a source change.

Maintainers may ask for a smaller change when a pull request mixes unrelated parser, rule, reporter, and distribution behavior.

The project maintainer has final responsibility for scope, compatibility, and release decisions.
