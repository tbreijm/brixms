#!/bin/sh
set -eu

builder_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
model=${BRIX_BUILDER_MODEL:-mlx-community/Qwen3.5-4B-MLX-4bit}
data_dir=${BRIX_BUILDER_DATA:-$builder_root/data/brix-builder}
adapter_path=${BRIX_BUILDER_ADAPTER:-$builder_root/adapters/brix-builder-v0}
iterations=${BRIX_BUILDER_ITERS:-1000}

for split in train valid test; do
  if [ ! -s "$data_dir/$split.jsonl" ]; then
    echo "brix-builder train: missing non-empty $data_dir/$split.jsonl" >&2
    exit 2
  fi
done

exec mlx_lm.lora \
  --model "$model" \
  --train \
  --test \
  --data "$data_dir" \
  --fine-tune-type lora \
  --batch-size 1 \
  --grad-accumulation-steps 8 \
  --num-layers 4 \
  --grad-checkpoint \
  --mask-prompt \
  --max-seq-length 8192 \
  --iters "$iterations" \
  --adapter-path "$adapter_path"
