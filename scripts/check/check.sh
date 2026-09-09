#!/usr/bin/env bash

set -euo pipefail

mode="full"
if (( $# > 1 )); then
  echo "usage: scripts/check/check.sh [--quick|--full]" >&2
  exit 2
fi
case "${1:-}" in
  "") ;;
  --quick) mode="quick" ;;
  --full) ;;
  *)
    echo "usage: scripts/check/check.sh [--quick|--full]" >&2
    exit 2
    ;;
esac

project_root="$(git rev-parse --show-toplevel)"
cd "$project_root"

check_actions="${CUKE_DEDUP_CHECK_ACTIONS:-}"
if [[ -z "$check_actions" ]]; then
  check_actions=0
  if [[ "$mode" == "full" ]] ||
    git diff --cached --name-only --diff-filter=ACMRD | grep -E '^(action\.ya?ml|\.github/(actions|workflows)/)' >/dev/null; then
    check_actions=1
  fi
fi
if [[ "$check_actions" != "0" && "$check_actions" != "1" ]]; then
  echo "CUKE_DEDUP_CHECK_ACTIONS must be 0 or 1." >&2
  exit 2
fi

check_dependencies="${CUKE_DEDUP_CHECK_DEPENDENCIES:-}"
if [[ -z "$check_dependencies" ]]; then
  check_dependencies=0
  if [[ "$mode" == "full" ]] ||
    git diff --cached --name-only --diff-filter=ACMRD | grep -E '(^|/)(Cargo\.toml|Cargo\.lock|deny\.toml)$' >/dev/null; then
    check_dependencies=1
  fi
fi
if [[ "$check_dependencies" != "0" && "$check_dependencies" != "1" ]]; then
  echo "CUKE_DEDUP_CHECK_DEPENDENCIES must be 0 or 1." >&2
  exit 2
fi

cargo +stable fmt --all -- --check
cargo +stable fmt --manifest-path fuzz/Cargo.toml -- --check
cargo +stable clippy --all-targets --all-features --locked -- -D warnings
cargo +stable test --all-targets --all-features --locked

if [[ "$mode" == "full" ]]; then
  RUSTDOCFLAGS="-D warnings" cargo +stable doc --no-deps --all-features --locked
  cargo +stable package --list --allow-dirty >/dev/null
  cargo +stable build --release --locked
  node scripts/check/check-corpus.mjs target/release/cuke-dedup fixtures/corpus
  CUKE_DEDUP_BENCH_SMOKE=1 CUKE_DEDUP_BENCH_REPEATS=1 \
    node scripts/benchmark/benchmark-analysis.mjs target/release/cuke-dedup >/dev/null
fi

npm test
npm run skills:check
npm run check:versions

if [[ "$mode" == "full" ]]; then
  npm run pack:check
  node scripts/release/prepare-npm-packages.mjs --check
  python3 scripts/release/generate-third-party-licenses.py --check
fi

python3 -m unittest discover -s scripts/release -p 'test_*.py'

git diff --check
git diff --cached --check

if [[ "$check_actions" == "1" ]]; then
  if ! command -v actionlint >/dev/null 2>&1; then
    echo "actionlint is required by the full gate and when GitHub Actions files change." >&2
    echo "Install it from: https://github.com/rhysd/actionlint" >&2
    exit 1
  fi
  actionlint .github/workflows/*.yml
fi

if [[ "$check_dependencies" == "1" ]]; then
  if ! cargo audit --version >/dev/null 2>&1; then
    echo "cargo-audit is required by the full gate and when Rust dependency files change." >&2
    echo "Install it with: cargo +stable install cargo-audit --version 0.22.2 --locked" >&2
    exit 1
  fi
  if ! cargo deny --version >/dev/null 2>&1; then
    echo "cargo-deny is required by the full gate and when Rust dependency policy files change." >&2
    echo "Install it with: cargo +stable install cargo-deny --locked" >&2
    exit 1
  fi
  cargo audit --deny warnings
  cargo audit --deny warnings --file fuzz/Cargo.lock
  cargo deny check advisories bans licenses sources
  cargo deny --manifest-path fuzz/Cargo.toml check advisories bans licenses sources
fi
