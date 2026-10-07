#!/usr/bin/env python3
"""Run workspace tests concurrently with nextest, followed by Cargo doctests.

No tests are filtered by default. Ignored scale/release tests still require their
explicit qualification commands, as they do with cargo test --workspace.
"""

import argparse
from pathlib import Path
import shlex
import subprocess
import sys
import time


ROOT = Path(__file__).resolve().parent.parent


def positive_int(value):
    parsed = int(value)
    if parsed < 1:
        raise argparse.ArgumentTypeError("must be at least 1")
    return parsed


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("-p", "--package", action="append", default=[],
                        help="limit both phases to this package (repeatable)")
    parser.add_argument("-E", "--filter", help="nextest filter; doctests still cover selected packages")
    parser.add_argument("-j", "--jobs", type=positive_int, default=4,
                        help="concurrent test processes (default: 4)")
    parser.add_argument("--build-jobs", type=positive_int, default=2,
                        help="concurrent compilation jobs (default: 2)")
    parser.add_argument("--target-dir", type=Path,
                        help="reuse this Cargo target directory for both phases")
    parser.add_argument("--release", action="store_true")
    args = parser.parse_args(argv)

    shared = []
    if args.package:
        for package in args.package:
            shared.extend(["--package", package])
    else:
        shared.append("--workspace")
    if args.target_dir:
        # Resolve before changing cwd, including paths supplied from elsewhere.
        shared.extend(["--target-dir", str(args.target_dir.resolve())])
    if args.release:
        shared.append("--release")

    try:
        available = subprocess.run(
            ["cargo", "nextest", "--version"], cwd=ROOT,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        )
    except FileNotFoundError:
        print("cargo is required; install the pinned Rust toolchain first.", file=sys.stderr)
        return 2
    if available.returncode:
        print("cargo-nextest is required (also used by CI). Install it from "
              "https://nexte.st/docs/installation/ or run cargo test --workspace.",
              file=sys.stderr)
        print(available.stderr.strip(), file=sys.stderr)
        return 2

    tests = ["cargo", "nextest", "run", *shared, "--profile", "local",
             "--test-threads", str(args.jobs), "--build-jobs", str(args.build_jobs),
             "--no-fail-fast", "--retries", "0", "--no-tests", "fail",
             "--ignore-default-filter"]
    if args.filter:
        tests.extend(["--filterset", args.filter])
    docs = ["cargo", "test", "--doc", *shared, "--jobs", str(args.build_jobs),
            "--no-fail-fast"]

    results = []
    started = time.monotonic()
    print("Scope: " + (", ".join(args.package) if args.package else "entire workspace")
          + (f"; test filter: {args.filter}" if args.filter else "; all non-ignored tests"),
          flush=True)
    for label, command in [("Unit/integration tests", tests), ("Doctests", docs)]:
        print(f"\n{label}: {shlex.join(command)}", flush=True)
        phase_started = time.monotonic()
        completed = subprocess.run(command, cwd=ROOT)
        elapsed = time.monotonic() - phase_started
        results.append((label, completed.returncode, elapsed))
        print(f"{label}: exit {completed.returncode}, {elapsed:.2f}s", flush=True)

    print(f"\nVerification finished in {time.monotonic() - started:.2f}s", flush=True)
    for label, code, elapsed in results:
        print(f"  {'PASS' if code == 0 else 'FAIL'} {label}: {elapsed:.2f}s")
    # Always run doctests, but never hide an earlier failure with their success.
    return 1 if any(code != 0 for _, code, _ in results) else 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        print("\nVerification interrupted; no pass result recorded.", file=sys.stderr)
        sys.exit(130)
