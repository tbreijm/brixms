#!/bin/sh
# Keep one MLX Qwen process warm for overnight brix-builder loops.
# Load once, then point workers at --backend server (no per-ticket startup).
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../../.." && pwd)
builder_root="$repo_root/tools/brix-builder"
model="${BRIX_BUILDER_MODEL:-mlx-community/Qwen3.5-4B-MLX-4bit}"
port="${BRIX_BUILDER_PORT:-8080}"
pid_file="${BRIX_BUILDER_SERVER_PID:-$HOME/.local/state/brix-builder/mlx-server.pid}"
log_file="${BRIX_BUILDER_SERVER_LOG:-$HOME/.local/state/brix-builder/mlx-server.log}"

mkdir -p "$(dirname -- "$pid_file")"

if [ -f "$pid_file" ] && kill -0 "$(cat "$pid_file")" 2>/dev/null; then
  echo "mlx server already running (pid $(cat "$pid_file")) on :$port"
  echo "  log: $log_file"
  exit 0
fi

if [ ! -x "$builder_root/.venv/bin/python" ]; then
  echo "missing $builder_root/.venv — create it and pip install -e '.[mlx,dev]'" >&2
  exit 1
fi

# shellcheck disable=SC2086
nohup "$builder_root/.venv/bin/python" -m mlx_lm.server \
  --model "$model" \
  --port "$port" \
  >"$log_file" 2>&1 &
echo $! >"$pid_file"

echo "started mlx server pid $(cat "$pid_file") model=$model port=$port"
echo "  log: $log_file"
echo "  wait until /v1/models answers, then:"
echo "  ./scripts/run-local.sh --backend server --endpoint http://127.0.0.1:$port/v1 loop"
