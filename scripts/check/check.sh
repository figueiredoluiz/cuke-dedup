#!/usr/bin/env bash

set -euo pipefail

project_root="$(git rev-parse --show-toplevel)"
cd "$project_root"

cargo +stable fmt --all -- --check
cargo +stable fmt --manifest-path fuzz/Cargo.toml -- --check
cargo +stable clippy --all-targets --all-features --locked -- -D warnings
cargo +stable test --all-targets --all-features --locked
RUSTDOCFLAGS="-D warnings" cargo +stable doc --no-deps --all-features --locked
cargo +stable package --list --allow-dirty >/dev/null
cargo +stable build --release --locked
node scripts/check/check-corpus.mjs target/release/cuke-dedup fixtures/corpus
CUKE_DEDUP_BENCH_SMOKE=1 CUKE_DEDUP_BENCH_REPEATS=1 \
  node scripts/benchmark/benchmark-analysis.mjs target/release/cuke-dedup >/dev/null

npm test
npm run skills:check
npm run check:versions
npm run pack:check
node scripts/release/prepare-npm-packages.mjs --check
python3 scripts/release/generate-third-party-licenses.py --check
python3 -m unittest discover -s scripts/release -p 'test_*.py'

git diff --check
git diff --cached --check

if git diff --cached --name-only --diff-filter=ACMR | grep -Eq '^(action\.yml|\.github/(actions|workflows)/)'; then
  if ! command -v actionlint >/dev/null 2>&1; then
    echo "actionlint is required when GitHub Actions files change." >&2
    echo "Install it from: https://github.com/rhysd/actionlint" >&2
    exit 1
  fi
  actionlint .github/workflows/*.yml
fi

if git diff --cached --name-only --diff-filter=ACMR | grep -Eq '(^|/)(Cargo\.toml|Cargo\.lock|deny\.toml)$'; then
  if ! cargo audit --version >/dev/null 2>&1; then
    echo "cargo-audit is required when Rust dependency files change." >&2
    echo "Install it with: cargo +stable install cargo-audit --version 0.22.2 --locked" >&2
    exit 1
  fi
  if ! cargo deny --version >/dev/null 2>&1; then
    echo "cargo-deny is required when Rust dependency policy files change." >&2
    echo "Install it with: cargo +stable install cargo-deny --locked" >&2
    exit 1
  fi
  cargo audit --deny warnings
  cargo audit --deny warnings --file fuzz/Cargo.lock
  cargo deny check advisories bans licenses sources
  cargo deny --manifest-path fuzz/Cargo.toml check advisories bans licenses sources
fi
