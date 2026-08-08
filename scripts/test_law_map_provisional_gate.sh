#!/usr/bin/env sh
# Negative fixture for the ADR-0017 §8 provisional-route coupling: prove the
# law-map gate DETECTS a `RouteStatus::Provisional` row left in the publication
# route table while SOC-LAW-05 claims `enforced` — without putting one back
# into the real table. Same discipline as test_tcb_dependency_gate.sh.
set -eu

fixture="scripts/fixtures/provisional-route.publication.rs"
output="$(mktemp)"
trap 'rm -f "$output"' EXIT

# The real manifest has SOC-LAW-05 at `enforced`; pointing the gate at a route
# table that still carries a provisional row must fail.
if python3 scripts/check_soc_law_map.py --routes-source "$fixture" >"$output" 2>&1; then
  echo "expected the provisional-route fixture to fail" >&2
  exit 1
fi

grep -F "RouteStatus::Provisional route(s)" "$output" >/dev/null
grep -F "SOC-LAW-05 is 'enforced'" "$output" >/dev/null
echo "negative provisional-route fixture rejected with an actionable diagnostic."
