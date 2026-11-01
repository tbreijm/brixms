#!/usr/bin/env python3
"""Capture repeated JSON records from the alpha.8 R0 synthetic world harness.

This only wraps the existing `r0_open_bench` executable. It does not make its
fixture Atlas-equivalent; see docs/performance/alpha9-baseline.md.
"""
import argparse
import json
import subprocess
import sys
import time
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("binary", type=Path)
    parser.add_argument("facts", type=int)
    parser.add_argument("repetitions", type=int)
    parser.add_argument("output", type=Path)
    parser.add_argument("--timeout-seconds", type=float, default=3600)
    args = parser.parse_args()
    if args.repetitions < 1:
        parser.error("repetitions must be positive")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    failed = False
    with args.output.open("w", encoding="utf-8") as out:
        for repetition in range(args.repetitions):
            started = time.time()
            try:
                proc = subprocess.run(
                    [str(args.binary), str(args.facts)],
                    check=False,
                    text=True,
                    capture_output=True,
                    timeout=args.timeout_seconds,
                )
                if proc.returncode == 0:
                    record = json.loads(proc.stdout)
                    status = "ok"
                else:
                    record = {"stdout": proc.stdout, "stderr": proc.stderr}
                    status = "failed"
                record["capture"] = {
                    "status": status,
                    "returncode": proc.returncode,
                    "repetition": repetition + 1,
                    "started_unix_s": started,
                    "elapsed_s": time.time() - started,
                    "harness_binary": str(args.binary.resolve()),
                }
            except subprocess.TimeoutExpired as exc:
                record = {
                    "capture": {
                        "status": "timeout",
                        "timeout_seconds": args.timeout_seconds,
                        "repetition": repetition + 1,
                        "started_unix_s": started,
                        "elapsed_s": time.time() - started,
                        "harness_binary": str(args.binary.resolve()),
                        "stdout": exc.stdout.decode("utf-8", errors="replace") if isinstance(exc.stdout, bytes) else exc.stdout,
                        "stderr": exc.stderr.decode("utf-8", errors="replace") if isinstance(exc.stderr, bytes) else exc.stderr,
                    }
                }
                status = "timeout"
            out.write(json.dumps(record, sort_keys=True, separators=(",", ":")) + "\n")
            out.flush()
            if record["capture"]["status"] == "ok":
                print(
                    f"{repetition + 1}/{args.repetitions}: "
                    f"load={record['load_total_ms']:.1f}ms "
                    f"disk={record['disk_bytes']}B "
                    f"open={record['cold_open']['cold_open_ms']:.2f}ms",
                    file=sys.stderr,
                )
            else:
                failed = True
                print(f"{repetition + 1}/{args.repetitions}: {status}", file=sys.stderr)
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
