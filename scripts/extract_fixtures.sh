#!/usr/bin/env bash
# Mechanically extract every ```brix code block from the normative spec into
# individual fixture files (Ring0_Build_Plan.md §1.3: "every ```brix block in the
# spec, extracted mechanically"). Fixtures are numbered in document order and
# tagged with the nearest preceding heading so a lane can trace a fixture back to
# its Part. Re-run whenever spec/ changes; the output dir is regenerated.
set -euo pipefail

SPEC="${1:-spec/BrixMS_v9_0.md}"
OUT="${2:-crates/brix-ast/tests/fixtures/spec}"

if [ ! -f "$SPEC" ]; then
  echo "spec not found: $SPEC" >&2
  exit 1
fi

rm -rf "$OUT"
mkdir -p "$OUT"

awk -v out="$OUT" '
  /^#{1,6}[[:space:]]/ { heading = $0; sub(/^#+[[:space:]]*/, "", heading) }
  /^```brix[[:space:]]*$/ {
    in_block = 1; n++;
    slug = heading; gsub(/[^A-Za-z0-9]+/, "-", slug); slug = tolower(slug);
    fname = sprintf("%s/%04d-%s.brix", out, n, substr(slug, 1, 48));
    printf("// source: %s (block %d)\n", heading, n) > fname;
    next
  }
  /^```[[:space:]]*$/ && in_block { in_block = 0; close(fname); next }
  in_block { print >> fname }
  END { printf("extracted %d brix blocks to %s\n", n, out) > "/dev/stderr" }
' "$SPEC"

count=$(find "$OUT" -name '*.brix' | wc -l | tr -d ' ')
echo "wrote $count fixtures under $OUT"
