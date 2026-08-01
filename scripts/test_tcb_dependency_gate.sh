#!/usr/bin/env sh
set -eu

fixture="scripts/fixtures/forbidden-kernel-dependency.metadata.json"
output="$(mktemp)"
trap 'rm -f "$output"' EXIT

if python3 scripts/check_tcb_dependencies.py --check --metadata "$fixture" >"$output" 2>&1; then
  echo "expected the forbidden-edge fixture to fail" >&2
  exit 1
fi

grep -F "forbidden trusted dependency: brix-kernel -> soc-regimes" "$output" >/dev/null
echo "negative dependency fixture rejected with an actionable diagnostic."
