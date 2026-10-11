#!/usr/bin/env bash

set -euo pipefail

project_root="$(git rev-parse --show-toplevel)"
cd "$project_root"

git diff --check
git diff --cached --check

cargo +stable fmt --all -- --check
cargo +stable fmt --manifest-path fuzz/Cargo.toml -- --check

node scripts/check/check-ruby-step-keywords.mjs
node --test scripts/check/lib/*.test.mjs
node scripts/check/check-language-boundary.mjs
node scripts/check/check-docs-parity.mjs
