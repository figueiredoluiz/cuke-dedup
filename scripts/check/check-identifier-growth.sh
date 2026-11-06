#!/usr/bin/env bash
set -euo pipefail

output="$(mktemp)"
trap 'rm -f "$output"' EXIT
if ! cargo test --release --lib --locked \
  typescript::handler::tests::identifier_membership_growth_stays_below_quadratic \
  -- --exact --ignored --test-threads=1 >"$output" 2>&1; then
  cat "$output"
  exit 1
fi
cat "$output"
if ! grep -Eq '^test result: ok\. 1 passed; 0 failed;' "$output"; then
  echo "Expected exactly one passing identifier growth test." >&2
  exit 1
fi
