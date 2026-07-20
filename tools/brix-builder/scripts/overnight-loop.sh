#!/bin/sh
# Overnight drain of the durable ticket queue against a warm local Qwen server.
# Does not apply patches to live packages -- export-proposal when tickets complete.
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/../../.." && pwd)
builder_root="$repo_root/tools/brix-builder"
port="${BRIX_BUILDER_PORT:-8080}"
endpoint="http://127.0.0.1:${port}/v1"
# Any package with brix.toml works as --root; tickets carry their own base_files.
root="${BRIX_BUILDER_ROOT:-$repo_root/packages/brix.math}"
log_file="${BRIX_BUILDER_LOOP_LOG:-$HOME/.local/state/brix-builder/overnight-loop.log}"
pid_file="${BRIX_BUILDER_LOOP_PID:-$HOME/.local/state/brix-builder/overnight-loop.pid}"

mkdir -p "$(dirname -- "$log_file")"

"$builder_root/scripts/serve-model.sh"

echo "waiting for warm model at $endpoint ..."
i=0
while [ "$i" -lt 120 ]; do
  if "$builder_root/.venv/bin/python" - <<PY 2>/dev/null
import urllib.request
urllib.request.urlopen("$endpoint/models", timeout=2).read()
print("ok")
PY
  then
    break
  fi
  i=$((i + 1))
  sleep 2
done

if ! "$builder_root/.venv/bin/python" - <<PY 2>/dev/null
import urllib.request
urllib.request.urlopen("$endpoint/models", timeout=2).read()
PY
then
  echo "model server did not become ready; see ~/.local/state/brix-builder/mlx-server.log" >&2
  exit 1
fi

if [ -f "$pid_file" ] && kill -0 "$(cat "$pid_file")" 2>/dev/null; then
  echo "overnight loop already running (pid $(cat "$pid_file"))"
  exit 0
fi

# Reclaim abandoned running tickets, then drain the queue. One process keeps the
# server warm for every ticket -- no per-ticket MLX startup.
# PYTHONUNBUFFERED so ticket status lines show up in the log immediately.
PYTHONUNBUFFERED=1 nohup "$builder_root/scripts/run-local.sh" \
  --root "$root" \
  --backend server \
  --endpoint "$endpoint" \
  loop >>"$log_file" 2>&1 &
echo $! >"$pid_file"

echo "overnight loop pid $(cat "$pid_file")"
echo "  root: $root"
echo "  log:  $log_file"
echo "  watch: tail -f $log_file"
echo "  queue: $builder_root/scripts/run-local.sh --root $root tickets"
