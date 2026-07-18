#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../../.." && pwd)
builder_root="$repo_root/tools/brix-builder"

if [ ! -x "$repo_root/target/debug/brix" ]; then
  cargo build --manifest-path "$repo_root/Cargo.toml" -p brix-cli
fi

exec "$builder_root/.venv/bin/brix-builder" \
  --brix "$repo_root/target/debug/brix" \
  "$@"
