#!/usr/bin/env bash
# Mechanically extract every ```brix program block from the normative spec into
# individual fixture files. Illustrative templates use ```brix-example and retain a
# document-order slot without becoming parser fixtures. Fixtures are numbered in document
# order and tagged with the nearest preceding heading so a lane can trace a fixture back
# to its Part. Re-run whenever spec/ changes; the output dir is regenerated.
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
  /^```brix(-example)?[[:space:]]*$/ {
    n++;
    if ($0 ~ /^```brix[[:space:]]*$/) {
      in_block = 1; extracted++;
    } else {
      example_block = 1;
      next
    }
    slug = heading; gsub(/[^A-Za-z0-9]+/, "-", slug); slug = tolower(slug);
    fname = sprintf("%s/%04d-%s.brix", out, n, substr(slug, 1, 48));
    printf("// source: %s (block %d)\n", heading, n) > fname;
    next
  }
  /^```[[:space:]]*$/ && (in_block || example_block) {
    if (in_block) close(fname)
    in_block = 0; example_block = 0; next
  }
  in_block { print >> fname }
  END { printf("extracted %d brix programs to %s\n", extracted, out) > "/dev/stderr" }
' "$SPEC"

count=$(find "$OUT" -name '*.brix' | wc -l | tr -d ' ')
echo "wrote $count fixtures under $OUT"
