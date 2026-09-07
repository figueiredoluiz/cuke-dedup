# Contributing to CukeDedup

Thanks for helping improve CukeDedup. Bug reports should include a small `.feature` file and step-definition sample whenever possible; remove application-specific secrets and identifiers first.

## Development setup

Install Rust through rustup, Node.js 20 or newer, npm, Python 3, and Gitleaks. The repository pins Rust 1.90 for compatibility tests, while the local quality gate also checks the current stable toolchain. Prepare both toolchains and the coverage command once:

```sh
rustup toolchain install 1.90.0 --profile minimal --component clippy,rustfmt,llvm-tools-preview
rustup toolchain install stable --profile minimal --component clippy,rustfmt
cargo install cargo-llvm-cov --locked
```

Enable the repository's pre-commit hook:

```sh
git config core.hooksPath .githooks
```

The hook scans the staged patch for secrets before running the compliance gate. Run the same gate directly at any time:

```sh
scripts/check/check.sh
```

Coverage is measured separately because it rebuilds the test suite with instrumentation:

```sh
cargo llvm-cov --all-features --all-targets --locked --fail-under-lines 90
```

The npm launcher currently has no third-party development dependencies, so contributor and CI checks run directly without an install step. This avoids forcing npm to install local workspace packages intended for other operating systems and CPU architectures. CI enforces a 90% Rust line-coverage floor.

Install `actionlint` when changing `action.yml` or workflow files. Install `cargo-audit` and `cargo-deny` when changing Rust dependencies, then run:

```sh
cargo audit --deny warnings
cargo audit --deny warnings --file fuzz/Cargo.lock
cargo deny check advisories bans licenses sources
cargo deny --manifest-path fuzz/Cargo.toml check advisories bans licenses sources
```

The dependency policy is maintained in `deny.toml`: known security advisories, unknown registries or Git sources, wildcard version requirements, and dependencies outside the approved SPDX license list fail CI. The pre-commit gate runs the standard checks and rejects secrets in staged changes; CI additionally scans the full Git history. Before submitting a pull request, run `scripts/check/check.sh` explicitly if the hook was bypassed.

## Changes

- Keep the source-adapter → shared IR → analysis → reporter boundaries intact.
- Add a focused regression test for every bug fix. Prefer sanitized inline fixtures; do not commit third-party repositories.
- Major new functionality must include automated tests that exercise its public behavior and failure modes. Parser and discovery changes must add adversarial cases, not only happy-path examples.
- Preserve deterministic ordering and the canonical JSON schema unless the change explicitly introduces a documented schema version. Reporter changes must retain equivalent active finding counts, rule/message/location evidence, and threshold results across terminal, JSON, JSONL, HTML, and SARIF.
- Use conventional commit subjects such as `fix:`, `feat:`, `test:`, `docs:`, and `chore:`.
- Do not edit generated release assets or commit build output.

The Python packaging utility intentionally uses only the standard library to create deterministic `tar.gz` and ZIP archives. Node.js owns npm manifest/version validation, while Rust owns the analyzer and CLI.

## Corpus and benchmarks

The sanitized end-to-end fixtures and their manifest are documented in [`fixtures/corpus`](fixtures/corpus/README.md). Add a corpus case for parser, discovery, policy, or reporting regressions that benefit from a repository-shaped fixture. Build the release binary before running the corpus directly:

```sh
cargo build --release --locked
npm run corpus:check
```

Record the non-blocking scalability profiles with `npm run benchmark`. The benchmark measures wall-clock, discovery, parsing, analysis, peak resident memory where available, input counts, and findings. Set `CUKE_DEDUP_BENCH_REPEATS` to change the sample count; `CUKE_DEDUP_BENCH_SMOKE=1` selects the reduced validation profile used by local and CI checks.

## Pull requests

Describe the user-visible problem, the chosen behavior, and the validation performed. CI reads the declared MSRV from `Cargo.toml`, tests it on Linux, macOS, and Windows, publishes JUnit results, measures Rust coverage, compiles the parser fuzz target, scans Git history for secrets, validates clean-room Cargo and npm installation, validates the release binary and local GitHub Action against the corpus, tests the npm launcher on Node 20 and 24, and enforces stable Rust quality, rustdoc, dependency policy, and deterministic release packaging. Scheduled workflows run the full parser fuzz campaign and catch vulnerability disclosures, leaked credentials, parser failures, and performance regressions that occur without a source change.

Maintainers may ask for a smaller change when a pull request mixes unrelated parser, rule, reporter, and distribution behavior.

The project maintainer has final responsibility for scope, compatibility, and release decisions.
