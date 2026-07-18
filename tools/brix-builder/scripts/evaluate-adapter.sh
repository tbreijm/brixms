#!/bin/sh
set -eu

builder_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
model=${BRIX_BUILDER_MODEL:-mlx-community/Qwen3.5-4B-MLX-4bit}
data_dir=${BRIX_BUILDER_DATA:-$builder_root/data/brix-builder}
adapter_path=${BRIX_BUILDER_ADAPTER:-$builder_root/adapters/brix-builder-v0}

exec mlx_lm.lora \
  --model "$model" \
  --adapter-path "$adapter_path" \
  --data "$data_dir" \
  --test \
  --test-batches -1
