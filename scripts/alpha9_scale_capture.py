#!/usr/bin/env python3
"""Capture bounded, repeated A9-7 benchmark invocations as raw JSONL.

Each repetition creates exactly one private world, measures setup/load/open/
query/edit phases in separate processes, and runs correctness checks separately
from the timed phases. The private world is removed after that repetition; no
benchmark history is compacted or deleted while it is being measured.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import platform
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import uuid
from pathlib import Path
from typing import Any


SCHEMA = "brix.alpha9.scale-capture@1"
EXAMPLE_SCHEMA = "brix.alpha9.scale-bench@1"
EXPECTED_PACKAGE_VERSION = "0.1.0-alpha.9"
VALID_SIZES = {8_000, 80_000, 800_000}
DERIVED_RELATIONS = (
    "root::activity_pages",
    "root::all_entities",
    "root::fulfillment",
    "root::decision_value_join",
)
RELATION_METRICS = (
    "first_page_ms",
    "first_page_rows",
    "first_page_bytes",
    "complete_query_ms",
    "pages",
    "rows",
    "bytes",
)
MAX_STDOUT_BYTES = 16 * 1024**2
MAX_STDERR_BYTES = 1024 * 1024
EXPECTED_PHASES = (
    "setup",
    "load",
    "open-before-edits",
    "query-before-edits",
    "warm-query-before-edits",
    "verify-loaded-world",
    "edit-add",
    "edit-retract",
    "verify-after-edits",
    "cleanup",
)
RESOURCE_LAUNCHER = r'''import json, os, resource, subprocess, sys
fd = int(sys.argv[1])
command = sys.argv[2:]
child = subprocess.Popen(command)
child_returncode = child.wait()
usage = resource.getrusage(resource.RUSAGE_CHILDREN)
rss = int(usage.ru_maxrss)
if sys.platform != "darwin":
    rss *= 1024
os.write(fd, json.dumps({"peak_rss_bytes": rss, "child_returncode": child_returncode}).encode("utf-8"))
sys.exit(child_returncode if child_returncode >= 0 else 128 - child_returncode)
'''


def free_bytes(path: Path) -> int:
    return shutil.disk_usage(path).free


def parse_peak_rss(stderr: str) -> int | None:
    # macOS time(1) prints: "123456 maximum resident set size". Accept the
    # reverse order too, as some localized/platform variants do.
    number = r"([\d,]+)"
    patterns = [
        rf"{number}\s+maximum resident set size",
        rf"maximum resident set size\s+{number}",
    ]
    for pattern in patterns:
        match = re.search(pattern, stderr, re.IGNORECASE)
        if match:
            return int(match.group(1).replace(",", ""))
    return None


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def current_commit(repo_root: Path) -> str | None:
    try:
        proc = subprocess.run(
            ["git", "-C", str(repo_root), "rev-parse", "HEAD"],
            check=False,
            text=True,
            capture_output=True,
            timeout=5,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    if proc.returncode != 0:
        return None
    commit = proc.stdout.strip()
    return commit if re.fullmatch(r"[0-9a-f]{40,64}", commit) else None


def is_number(value: Any) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)


def get_nested(value: Any, path: tuple[str, ...]) -> Any:
    for segment in path:
        if not isinstance(value, dict) or segment not in value:
            return None
        value = value[segment]
    return value


def get_relation_measurements(example: dict[str, Any]) -> tuple[dict[str, dict[str, Any]], str | None]:
    """Normalize relation measurements represented as a map or named array."""
    raw = example.get("relation_queries", example.get("derived_relations"))
    entries: dict[str, dict[str, Any]] = {}
    if isinstance(raw, dict):
        entries = {str(name): value for name, value in raw.items() if isinstance(value, dict)}
    elif isinstance(raw, list):
        for item in raw:
            if not isinstance(item, dict):
                continue
            name = item.get("relation", item.get("name", item.get("qualified_name")))
            metrics = item.get("measurements", item.get("metrics", item))
            if isinstance(name, str) and isinstance(metrics, dict):
                entries[name] = metrics
    else:
        return {}, "missing relation_queries/derived_relations object or array"

    missing = sorted(set(DERIVED_RELATIONS) - set(entries))
    if missing:
        return entries, f"missing derived relation measurements: {', '.join(missing)}"
    for relation in DERIVED_RELATIONS:
        values = entries[relation]
        for field in RELATION_METRICS:
            if not is_number(values.get(field)):
                return entries, f"{relation}.{field} missing or invalid"
        digest = values.get("canonical_content_digest")
        if not isinstance(digest, str) or not digest:
            return entries, f"{relation}.canonical_content_digest missing or invalid"
    return entries, None


def phase_measurement_requirements(mode: str) -> tuple[tuple[str, ...], ...]:
    timings = {
        "full": (("timings", "init_ms"),),
        "child-load": (
            ("timings", "world_open_ms"),
            ("timings", "load_ms"),
            ("timings", "commit_ms"),
            ("timings", "fixture_generation_ms"),
            ("storage", "logical_bytes"),
            ("storage", "allocated_bytes"),
        ),
        "child-open": (("timings", "world_open_ms"),),
        "child-query": (
            ("timings", "world_open_ms"),
            ("timings", "first_page_ms"),
            ("timings", "complete_query_ms"),
            ("query", "rows"),
            ("query", "bytes"),
            ("query", "work_units"),
            ("query", "matches"),
            ("query", "expressions_evaluated"),
        ),
        "child-query-warm": (
            ("timings", "world_open_ms"),
            ("timings", "first_page_ms"),
            ("timings", "complete_query_ms"),
            ("query", "rows"),
            ("query", "bytes"),
            ("query", "work_units"),
            ("query", "matches"),
            ("query", "expressions_evaluated"),
        ),
        "child-verify": (("timings", "correctness_ms"),),
        "child-edit": (
            ("timings", "edit_ms"),
            ("edit", "elapsed_ms"),
            ("edit", "before_revision"),
            ("edit", "after_revision"),
            ("edit", "counters"),
        ),
    }
    return timings[mode]


def terminate_group(proc: subprocess.Popen[Any]) -> None:
    try:
        os.killpg(proc.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        proc.wait()


def invoke(
    binary: Path,
    args: list[str],
    *,
    scratch: Path,
    free_floor: int,
    timeout_s: float,
    size: int,
    profile: str,
    repetition: int,
    mode: str,
    binary_sha256: str,
    repository_commit: str | None,
    expected_batches: int,
    host_metadata: dict[str, Any] | None = None,
    expected_action: str | None = None,
) -> dict[str, Any]:
    target_command = [str(binary), *args]
    started_unix = time.time()
    before = free_bytes(scratch)
    record: dict[str, Any] = {
        "schema": SCHEMA,
        "size": size,
        "profile": profile,
        "repetition": repetition,
        "mode": mode,
        "command": None,
        "target_command": target_command,
        "binary": str(binary.resolve()),
        "binary_sha256": binary_sha256,
        "repository_commit": repository_commit,
        "host": {
            "platform": platform.platform(),
            "machine": platform.machine(),
            "hardware_model": (
                host_metadata.get("hardware_model", host_metadata.get("model"))
                if host_metadata is not None
                else None
            ),
            "cpu_count": (
                host_metadata.get("cpu_count", host_metadata.get("cpus"))
                if host_metadata is not None
                else None
            ),
            "memory_bytes": (
                host_metadata.get("memory_bytes", host_metadata.get("ram_bytes"))
                if host_metadata is not None
                else None
            ),
            "host_metadata_note": (
                "hardware and OS values are operator-provided; see operator_metadata"
                if host_metadata is not None
                else "hardware model and memory unavailable; provide --host-metadata"
            ),
            "python": platform.python_version(),
            "operator_metadata": host_metadata,
        },
        "host_metadata_sha256": (
            hashlib.sha256(
                json.dumps(host_metadata, sort_keys=True, separators=(",", ":")).encode("utf-8")
            ).hexdigest()
            if host_metadata is not None
            else None
        ),
        "started_unix_s": started_unix,
        "spawn_started_unix_ns": None,
        "spawn_to_main_entry_ms": None,
        "spawn_to_main_entry_label": "launcher-inclusive Python Popen to Rust main entry",
        "scratch_free_before_bytes": before,
        "scratch_free_after_bytes": None,
        "timeout_seconds": timeout_s,
        "status": None,
        "returncode": None,
        "elapsed_s": None,
        "peak_rss_bytes": None,
        "example": None,
        "stdout": "",
        "stderr": "",
    }
    if before < free_floor:
        record.update(
            status="resource_failure",
            resource_failure=f"free scratch space {before} below floor {free_floor}",
            elapsed_s=0.0,
        )
        record["scratch_free_after_bytes"] = free_bytes(scratch)
        return record

    started = time.monotonic()
    proc: subprocess.Popen[bytes] | None = None
    out = ""
    err = ""
    failure: str | None = None
    try:
        # Temporary files avoid deadlock if either subprocess stream exceeds
        # the OS pipe buffer. Readbacks are capped so a malformed child cannot
        # make the capture process consume unbounded memory.
        with tempfile.TemporaryFile(dir=scratch) as stdout_file, tempfile.TemporaryFile(
            dir=scratch
        ) as stderr_file, tempfile.TemporaryFile(dir=scratch) as rss_file:
            rss_fd = rss_file.fileno()
            command = [sys.executable, "-c", RESOURCE_LAUNCHER, str(rss_fd), *target_command]
            record["command"] = command
            record["spawn_started_unix_ns"] = time.time_ns()
            proc = subprocess.Popen(
                command,
                stdout=stdout_file,
                stderr=stderr_file,
                start_new_session=True,
                pass_fds=(rss_fd,),
            )
            while proc.poll() is None:
                elapsed = time.monotonic() - started
                current_free = free_bytes(scratch)
                if current_free < free_floor:
                    failure = f"free scratch space {current_free} below floor {free_floor}"
                    terminate_group(proc)
                    break
                if elapsed >= timeout_s:
                    failure = f"timeout after {timeout_s} seconds"
                    terminate_group(proc)
                    break
                time.sleep(0.2)
            proc.wait()
            elapsed = time.monotonic() - started
            stdout_size = os.fstat(stdout_file.fileno()).st_size
            stderr_size = os.fstat(stderr_file.fileno()).st_size
            rss_file.seek(0)
            rss_bytes = rss_file.read(4096)
            if rss_bytes:
                try:
                    rss_report = json.loads(rss_bytes.decode("utf-8"))
                    record["peak_rss_bytes"] = rss_report.get("peak_rss_bytes")
                    record["child_returncode"] = rss_report.get("child_returncode")
                except (UnicodeDecodeError, json.JSONDecodeError):
                    record["rss_report_error"] = "invalid resource-launcher side channel"
            stdout_file.seek(0)
            stderr_file.seek(max(0, stderr_size - MAX_STDERR_BYTES))
            out_bytes = stdout_file.read(MAX_STDOUT_BYTES + 1)
            err_bytes = stderr_file.read(MAX_STDERR_BYTES + 1)
            record["stdout_bytes"] = stdout_size
            record["stderr_bytes"] = stderr_size
            record["stdout_truncated"] = stdout_size > MAX_STDOUT_BYTES
            record["stderr_truncated"] = stderr_size > MAX_STDERR_BYTES
            out = out_bytes[:MAX_STDOUT_BYTES].decode("utf-8", errors="replace")
            err = err_bytes[:MAX_STDERR_BYTES].decode("utf-8", errors="replace")
    except OSError as exc:
        failure = f"spawn failed: {exc}"
        elapsed = time.monotonic() - started
        if proc is not None:
            terminate_group(proc)

    record["elapsed_s"] = elapsed
    record["scratch_free_after_bytes"] = free_bytes(scratch)
    record["returncode"] = proc.returncode if proc else None
    record["stdout"] = out
    record["stderr"] = err
    # The direct-child resource launcher avoids the restricted `time -l`
    # sysctl path and returns per-invocation RUSAGE_CHILDREN for exactly one
    # measured executable. `parse_peak_rss` remains tested for raw time output.
    record["peak_rss_source"] = "resource.getrusage(RUSAGE_CHILDREN) side channel"
    if record["peak_rss_bytes"] is None:
        record["peak_rss_bytes"] = parse_peak_rss(err)
        if record["peak_rss_bytes"] is not None:
            record["peak_rss_source"] = "/usr/bin/time -l fallback"
    if failure and failure.startswith("free scratch"):
        record.update(status="resource_failure", resource_failure=failure)
        return record
    if failure:
        record.update(status="timeout", failure=failure)
        return record
    if proc is None:
        record.update(status="spawn_failure", failure="subprocess did not start")
        return record
    child_returncode = record.get("child_returncode")
    if child_returncode != 0:
        status = (
            "oom_or_signal"
            if (isinstance(child_returncode, int) and child_returncode < 0)
            or (child_returncode is None and proc.returncode < 0)
            else "process_failure"
        )
        record.update(status=status, failure=f"measured executable exited {child_returncode}")
        return record
    if proc.returncode != 0:
        record.update(status="launcher_failure", failure=f"resource launcher exited {proc.returncode}")
        return record
    if record.get("stdout_truncated"):
        record.update(status="invalid_output", failure="stdout exceeded capture size limit")
        return record
    try:
        example = json.loads(out)
    except json.JSONDecodeError as exc:
        record.update(status="invalid_output", failure=str(exc))
        return record
    if not isinstance(example, dict) or example.get("schema") != EXAMPLE_SCHEMA:
        record.update(
            status="invalid_output",
            failure=f"expected object with schema {EXAMPLE_SCHEMA}",
            parsed_output=example,
        )
        return record
    example_mode = example.get("mode")
    record["example_mode"] = example_mode
    if mode != "child-edit" and example_mode != mode:
        record.update(
            status="invalid_output",
            failure=f"expected example mode {mode}, got {example_mode!r}",
            parsed_output=example,
        )
        return record
    if expected_action is not None and example.get("action") != expected_action:
        record.update(
            status="invalid_output",
            failure=f"expected edit action {expected_action}, got {example.get('action')!r}",
            parsed_output=example,
        )
        return record
    if mode in {"child-query", "child-query-warm"}:
        _, relation_error = get_relation_measurements(example)
        if relation_error:
            record.update(
                status="missing_measurement",
                missing_measurements=["relation_queries"],
                failure=relation_error,
                parsed_output=example,
            )
            return record
    fixture = example.get("fixture")
    if not isinstance(fixture, dict) or fixture.get("rows") != size:
        record.update(
            status="invalid_output",
            failure=f"fixture row count does not match requested size {size}",
            parsed_output=example,
        )
        return record
    if mode != "full" and fixture.get("retained_revisions") != expected_batches:
        record.update(
            status="invalid_output",
            failure=(
                f"expected {expected_batches} fixture revisions, "
                f"got {fixture.get('retained_revisions')!r}"
            ),
            parsed_output=example,
        )
        return record
    if mode != "full" and not isinstance(fixture.get("digest"), str):
        record.update(
            status="missing_measurement",
            missing_measurements=["fixture.digest"],
            failure="post-load phase did not report a fixture digest",
            parsed_output=example,
        )
        return record

    build = example.get("build")
    if not isinstance(build, dict):
        record.update(
            status="missing_measurement",
            missing_measurements=["build"],
            failure="example did not report build identity",
            parsed_output=example,
        )
        return record
    if build.get("package_version") != EXPECTED_PACKAGE_VERSION:
        record.update(
            status="invalid_output",
            failure=f"expected package {EXPECTED_PACKAGE_VERSION}, got {build.get('package_version')!r}",
            parsed_output=example,
        )
        return record
    if build.get("profile") != "release":
        record.update(
            status="invalid_output",
            failure=f"expected release profile, got {build.get('profile')!r}",
            parsed_output=example,
        )
        return record
    if not isinstance(build.get("rustc"), str) or not build["rustc"]:
        record.update(
            status="missing_measurement",
            missing_measurements=["build.rustc"],
            failure="example did not report Rust toolchain version",
            parsed_output=example,
        )
        return record
    embedded_commit = build.get("embedded_build_commit")
    if not isinstance(embedded_commit, str) or not embedded_commit:
        record.update(
            status="missing_measurement",
            missing_measurements=["build.embedded_build_commit"],
            failure="example did not report its embedded source commit",
            parsed_output=example,
        )
        return record
    if repository_commit is not None and embedded_commit != repository_commit:
        record.update(
            status="invalid_output",
            failure=(
                f"binary commit {embedded_commit} differs from capture checkout "
                f"{repository_commit}"
            ),
            parsed_output=example,
        )
        return record
    if Path(str(build.get("binary", ""))).resolve() != binary.resolve():
        record.update(
            status="invalid_output",
            failure="example current_exe path differs from supplied benchmark binary",
            parsed_output=example,
        )
        return record
    correctness = example.get("correctness")
    if mode == "child-verify" and not (
        isinstance(correctness, dict)
        and correctness.get("passed") is True
        and correctness.get("oracle") == "passed"
        and correctness.get("derived_counts_match") is True
        and correctness.get("named_query_rows_match") is True
        and (
            "derived_pages_match" not in correctness
            or correctness.get("derived_pages_match") is True
        )
    ):
        record.update(
            status="correctness_failure",
            failure="child verification did not report correctness.passed=true",
            parsed_output=example,
        )
        return record
    if mode == "child-load" and get_nested(example, ("load", "batch_count")) != expected_batches:
        record.update(
            status="invalid_output",
            failure=f"load batch count did not equal {expected_batches}",
            parsed_output=example,
        )
        return record
    if mode == "child-edit":
        before_revision = get_nested(example, ("edit", "before_revision"))
        after_revision = get_nested(example, ("edit", "after_revision"))
        expected_before = expected_batches + (1 if expected_action == "retract" else 0)
        if before_revision != expected_before or after_revision != expected_before + 1:
            record.update(
                status="invalid_output",
                failure=(
                    f"edit revision sequence mismatch: expected "
                    f"{expected_before}->{expected_before + 1}, got "
                    f"{before_revision!r}->{after_revision!r}"
                ),
                parsed_output=example,
            )
            return record

    missing_phase_fields = [
        ".".join(path)
        for path in phase_measurement_requirements(mode)
        if (value := get_nested(example, path)) is None
        or (path[-1] != "counters" and not is_number(value))
        or (path[-1] == "counters" and not isinstance(value, dict))
    ]
    if missing_phase_fields:
        record.update(
            status="missing_measurement",
            missing_measurements=missing_phase_fields,
            failure="phase-specific measurements were missing or invalid",
            parsed_output=example,
        )
        return record
    record["example"] = example
    entry_ns = example.get("main_entry_unix_ns")
    if isinstance(entry_ns, int) and record["spawn_started_unix_ns"] is not None:
        startup_ms = (
            entry_ns - record["spawn_started_unix_ns"]
        ) / 1_000_000
        if startup_ms >= 0:
            record["spawn_to_main_entry_ms"] = startup_ms
    record["stdout"] = ""
    missing = []
    if record["peak_rss_bytes"] is None:
        missing.append("peak_rss_bytes")
    if record["spawn_to_main_entry_ms"] is None:
        missing.append("spawn_to_main_entry_ms")
    if missing:
        record.update(
            status="missing_measurement",
            missing_measurements=missing,
            failure="one or more required process measurements were unavailable",
        )
    else:
        record["status"] = "ok"
    return record


def append_record(stream, record: dict[str, Any]) -> None:
    stream.write(json.dumps(record, sort_keys=True, separators=(",", ":")) + "\n")
    stream.flush()


def skipped_record(
    *, size: int, profile: str, repetition: int, mode: str, phase: str, reason: str
) -> dict[str, Any]:
    return {
        "schema": SCHEMA,
        "size": size,
        "profile": profile,
        "repetition": repetition,
        "mode": mode,
        "phase": phase,
        "status": "skipped_dependency_failed",
        "failure": reason,
    }


def read_capture(path: Path) -> tuple[list[dict[str, Any]], list[str]]:
    records: list[dict[str, Any]] = []
    errors: list[str] = []
    with path.open("r", encoding="utf-8") as source:
        for line_number, line in enumerate(source, 1):
            if not line.strip():
                continue
            try:
                record = json.loads(line)
            except json.JSONDecodeError as exc:
                errors.append(f"line {line_number}: invalid JSON ({exc})")
                continue
            if not isinstance(record, dict) or record.get("schema") != SCHEMA:
                errors.append(f"line {line_number}: unsupported capture record")
                continue
            records.append(record)
    return records, errors


def median_range(values: list[float]) -> str:
    if not values:
        return "—"
    ordered = sorted(values)
    midpoint = len(ordered) // 2
    median = (
        ordered[midpoint]
        if len(ordered) % 2
        else (ordered[midpoint - 1] + ordered[midpoint]) / 2
    )
    return f"{median:.2f} [{ordered[0]:.2f}–{ordered[-1]:.2f}]"


def phase_report_values(record: dict[str, Any]) -> tuple[list[float], list[float], str]:
    example = record.get("example") or {}
    phase = record.get("phase")
    rss_mib = [
        record["peak_rss_bytes"] / (1024 * 1024)
        for _ in [0]
        if is_number(record.get("peak_rss_bytes"))
    ]
    value_paths: dict[str, tuple[tuple[str, ...], ...]] = {
        "setup": (("timings", "init_ms"),),
        "load": (
            ("timings", "world_open_ms"),
            ("timings", "load_ms"),
            ("storage", "logical_bytes"),
        ),
        "open-before-edits": (("timings", "world_open_ms"),),
        "query-before-edits": (
            ("timings", "world_open_ms"),
            ("timings", "first_page_ms"),
            ("timings", "complete_query_ms"),
            ("query", "rows"),
            ("query", "bytes"),
            ("query", "work_units"),
            ("query", "matches"),
            ("query", "expressions_evaluated"),
        ),
        "warm-query-before-edits": (
            ("timings", "world_open_ms"),
            ("timings", "first_page_ms"),
            ("timings", "complete_query_ms"),
            ("query", "rows"),
            ("query", "bytes"),
            ("query", "work_units"),
            ("query", "matches"),
            ("query", "expressions_evaluated"),
        ),
        "verify-loaded-world": (("timings", "correctness_ms"),),
        "verify-after-edits": (("timings", "correctness_ms"),),
    }
    if phase in {"edit-add", "edit-retract"}:
        values = [get_nested(example, ("edit", "elapsed_ms"))]
        counters = get_nested(example, ("edit", "counters")) or {}
        labels = ["edit_ms"]
        for key in ("operator_work", "matches", "expressions_evaluated", "state_bytes_read", "state_bytes_written"):
            if is_number(counters.get(key)):
                values.append(counters[key])
                labels.append(key)
        pairs = [(label, value) for label, value in zip(labels, values) if is_number(value)]
    else:
        paths = value_paths.get(phase, ())
        labels = [".".join(path[-1:]) for path in paths]
        pairs = [
            (label, value)
            for label, path in zip(labels, paths)
            if is_number(value := get_nested(example, path))
        ]
    metrics = ", ".join(f"{name}={value:g}" for name, value in pairs)
    elapsed_ms = [record["elapsed_s"] * 1000 for _ in [0] if is_number(record.get("elapsed_s"))]
    return elapsed_ms, rss_mib, metrics or "—"


def summarize_capture(path: Path, expected_repetitions: int = 5) -> str:
    records, errors = read_capture(path)
    by_cell: dict[tuple[int, str], dict[tuple[int, str], list[dict[str, Any]]]] = {}
    failures: list[dict[str, Any]] = []
    commits: set[str] = set()
    binary_hashes: set[str] = set()
    for record in records:
        size = record.get("size")
        profile = record.get("profile")
        repetition = record.get("repetition")
        phase = record.get("phase")
        if not isinstance(size, int) or profile not in {"single", "ten"}:
            errors.append("record is missing valid size/profile fields")
            continue
        if isinstance(record.get("repository_commit"), str):
            commits.add(record["repository_commit"])
        if isinstance(record.get("binary_sha256"), str):
            binary_hashes.add(record["binary_sha256"])
        if record.get("status") != "ok":
            failures.append(record)
        if isinstance(repetition, int) and isinstance(phase, str):
            cell = by_cell.setdefault((size, profile), {})
            cell.setdefault((repetition, phase), []).append(record)

    lines = [
        "# Alpha.9 scale capture summary",
        "",
        f"Raw JSONL: `{path}`",
        f"Expected valid repetitions per size/profile/phase: {expected_repetitions}",
        f"Capture checkout commits: {', '.join(sorted(commits)) or 'unavailable'}",
        f"Release binary SHA-256 values: {', '.join(sorted(binary_hashes)) or 'unavailable'}",
        "",
    ]
    all_cells_complete = True
    all_ten_budgets_pass = True

    for size in sorted(VALID_SIZES):
        for profile in ("single", "ten"):
            cell = by_cell.get((size, profile), {})
            repetitions = sorted({rep for rep, _ in cell})
            complete_reps = []
            for repetition in repetitions:
                complete = True
                for phase in EXPECTED_PHASES:
                    phase_records = cell.get((repetition, phase), [])
                    if len(phase_records) != 1 or phase_records[0].get("status") != "ok":
                        complete = False
                        break
                if complete:
                    complete_reps.append(repetition)
            complete_count = len(complete_reps)
            cell_complete = complete_count >= expected_repetitions
            all_cells_complete &= cell_complete
            lines.extend(
                [
                    f"## {size:,} facts — {profile} batch profile",
                    "",
                    f"Valid complete repetitions: **{complete_count}/{expected_repetitions}** "
                    + ("(complete)" if cell_complete else "(incomplete)"),
                    "",
                    "| Phase | Valid / expected | Process wall ms, median [range] | Peak RSS MiB, median [range] | Example metrics |",
                    "|---|---:|---:|---:|---|",
                ]
            )
            for phase in EXPECTED_PHASES:
                phase_records = [
                    r
                    for (rep, candidate), grouped in cell.items()
                    if candidate == phase
                    for r in grouped
                ]
                valid = [r for r in phase_records if r.get("status") == "ok"]
                walls: list[float] = []
                rss: list[float] = []
                metrics_list: list[str] = []
                for record in valid:
                    elapsed_ms, rss_values, metrics = phase_report_values(record)
                    walls.extend(elapsed_ms)
                    rss.extend(rss_values)
                    metrics_list.append(metrics)
                unique_metrics = sorted(set(metrics_list))
                metric_display = unique_metrics[0] if len(unique_metrics) == 1 else ("varies across repetitions" if unique_metrics else "—")
                lines.append(
                    f"| {phase} | {len(valid)}/{expected_repetitions} | "
                    f"{median_range(walls)} | {median_range(rss)} | {metric_display} |"
                )

            if profile == "ten":
                budget = 16 * 1024**2 + 12 * 1024 * size
                load_records = [
                    r
                    for (rep, phase), grouped in cell.items()
                    if phase == "load"
                    for r in grouped
                    if r.get("status") == "ok"
                ]
                logical_values = [
                    get_nested(r.get("example"), ("storage", "logical_bytes"))
                    for r in load_records
                ]
                logical_values = [value for value in logical_values if is_number(value)]
                budget_pass = (
                    len(logical_values) >= expected_repetitions
                    and all(value <= budget for value in logical_values)
                )
                all_ten_budgets_pass &= budget_pass
                lines.extend(
                    [
                        "",
                        f"Ten-revision disk budget: {budget:,} logical bytes; measured "
                        f"max {max(logical_values):,.0f} bytes across {len(logical_values)} valid load samples; "
                        f"{'PASS' if budget_pass else 'NOT MET'}.",
                    ]
                )
            lines.append("")

    complete = all_cells_complete and all_ten_budgets_pass and not errors
    lines.extend(
        [
            "## Capture qualification",
            "",
            "**QUALIFIED**" if complete else "**NOT QUALIFIED**",
            "",
            f"Valid complete size/profile cells: {sum(1 for size in VALID_SIZES for profile in ('single', 'ten') if len([rep for rep in {r.get('repetition') for r in by_cell.get((size, profile), {})} if isinstance(rep, int) and all(len(by_cell[(size, profile)].get((rep, phase), [])) == 1 and by_cell[(size, profile)][(rep, phase)][0].get('status') == 'ok' for phase in EXPECTED_PHASES)]) >= expected_repetitions)}/6.",
            "",
        ]
    )
    if failures or errors:
        lines.extend(["## Failures and missing records", ""])
        for error in errors:
            lines.append(f"- Capture parse/identity error: {error}")
        for record in failures:
            lines.append(
                f"- {record.get('size')} / {record.get('profile')} / repetition "
                f"{record.get('repetition')} / {record.get('phase')}: "
                f"{record.get('status')} — {record.get('failure', record.get('resource_failure', 'no detail'))}"
            )
        lines.append("")
    else:
        lines.extend(["No failed or incomplete capture records.", ""])
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path, nargs="?", help="release alpha9_scale_bench executable")
    parser.add_argument("output", type=Path, nargs="?", help="new raw JSONL destination (must not exist)")
    parser.add_argument("--summarize", type=Path, help="summarize an existing raw capture JSONL and exit")
    parser.add_argument("--sizes", nargs="+", type=int, default=[8_000, 80_000, 800_000])
    parser.add_argument("--profiles", nargs="+", choices=["single", "ten"], default=["single", "ten"])
    parser.add_argument("--repetitions", type=int, default=5)
    parser.add_argument("--seed", type=int, default=0)
    parser.add_argument("--timeout-seconds", type=float, default=7200)
    parser.add_argument("--min-free-gib", type=float, default=3.0)
    parser.add_argument("--scratch-root", type=Path, default=Path("/private/tmp"))
    parser.add_argument(
        "--host-metadata",
        type=Path,
        help="operator-provided JSON host metadata (for example filtered sysctl results)",
    )
    parser.add_argument(
        "--allow-large",
        action="store_true",
        help="allow the configured 80k/800k sequential runs",
    )
    args = parser.parse_args()

    if args.summarize is not None:
        if args.binary is not None or args.output is not None:
            parser.error("--summarize cannot be combined with capture positional arguments")
        if not args.summarize.is_file():
            parser.error(f"capture JSONL does not exist: {args.summarize}")
        print(summarize_capture(args.summarize))
        return 0

    if args.binary is None or args.output is None:
        parser.error("capture mode requires BINARY and OUTPUT; or use --summarize JSONL")

    if args.repetitions < 1:
        parser.error("--repetitions must be positive")
    if args.timeout_seconds <= 0:
        parser.error("--timeout-seconds must be positive")
    if args.min_free_gib < 3.0:
        parser.error("--min-free-gib cannot be lower than the required 3 GiB floor")
    if not args.sizes or any(size not in VALID_SIZES for size in args.sizes):
        parser.error("--sizes must use only 8000, 80000, and 800000")
    if not args.allow_large and any(size > 8_000 for size in args.sizes):
        parser.error("sizes above 8000 require --allow-large")
    if not args.binary.is_file():
        parser.error(f"benchmark binary does not exist: {args.binary}")
    binary = args.binary.resolve()
    binary_sha256 = sha256_file(binary)
    repository_commit = current_commit(Path.cwd())
    host_metadata = None
    if args.host_metadata is not None:
        try:
            host_metadata = json.loads(args.host_metadata.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            parser.error(f"cannot read --host-metadata JSON: {exc}")
        if not isinstance(host_metadata, dict):
            parser.error("--host-metadata must contain a JSON object")
    args.scratch_root.mkdir(parents=True, exist_ok=True)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    free_floor = int(args.min_free_gib * 1024**3)
    any_failure = False

    # Never append a second run to an existing raw series: that could make
    # duplicate/incomplete repetitions look like the required five samples.
    with args.output.open("x", encoding="utf-8") as output:
        for size in args.sizes:
            for profile in args.profiles:
                batches = 1 if profile == "single" else 10
                for repetition in range(1, args.repetitions + 1):
                    run_root = args.scratch_root / f"brix-a9-7-{uuid.uuid4().hex}"
                    world_dir = run_root / "world"
                    try:
                        run_root.mkdir()
                    except OSError as exc:
                        append_record(
                            output,
                            {
                                "schema": SCHEMA,
                                "size": size,
                                "profile": profile,
                                "repetition": repetition,
                                "mode": "workspace-create",
                                "phase": "workspace-create",
                                "status": "resource_failure",
                                "failure": str(exc),
                                "scratch_free_bytes": free_bytes(args.scratch_root),
                            },
                        )
                        any_failure = True
                        continue
                    try:
                        common = [
                            "--facts",
                            str(size),
                            "--seed",
                            str(args.seed),
                            "--world-dir",
                            str(world_dir),
                        ]
                        phases = [
                            (
                                "setup",
                                "full",
                                ["--full", *common, "--batches", str(batches)],
                            ),
                            ("load", "child-load", ["--child-load", str(world_dir)]),
                            (
                                "open-before-edits",
                                "child-open",
                                ["--child-open", str(world_dir)],
                            ),
                            (
                                "query-before-edits",
                                "child-query",
                                ["--child-query", str(world_dir)],
                            ),
                            (
                                "warm-query-before-edits",
                                "child-query-warm",
                                ["--child-query-warm", str(world_dir)],
                            ),
                            (
                                "verify-loaded-world",
                                "child-verify",
                                ["--child-verify", str(world_dir)],
                            ),
                            (
                                "edit-add",
                                "child-edit",
                                ["--child-edit", str(world_dir), "add"],
                            ),
                            (
                                "edit-retract",
                                "child-edit",
                                ["--child-edit", str(world_dir), "retract"],
                            ),
                            (
                                "verify-after-edits",
                                "child-verify",
                                ["--child-verify", str(world_dir)],
                            ),
                        ]
                        phase_failure: str | None = None
                        for phase, mode, mode_args in phases:
                            record = invoke(
                                binary,
                                mode_args,
                                scratch=args.scratch_root,
                                free_floor=free_floor,
                                timeout_s=args.timeout_seconds,
                                size=size,
                                profile=profile,
                                repetition=repetition,
                                mode=mode,
                                binary_sha256=binary_sha256,
                                repository_commit=repository_commit,
                                expected_batches=batches,
                                host_metadata=host_metadata,
                                expected_action=(
                                    phase.removeprefix("edit-")
                                    if phase in {"edit-add", "edit-retract"}
                                    else None
                                ),
                            )
                            record["phase"] = phase
                            record["batches"] = batches
                            record["os_cache_state"] = (
                                "unknown; fresh process, preceded by fixture load and earlier checks"
                                if phase in {
                                    "open-before-edits",
                                    "query-before-edits",
                                    "warm-query-before-edits",
                                }
                                else "not applicable or uncontrolled"
                            )
                            append_record(output, record)
                            if record["status"] != "ok":
                                any_failure = True
                                phase_failure = (
                                    f"phase {phase} status was {record['status']}"
                                )
                                break
                        if phase_failure:
                            failed_phase_index = next(
                                (
                                    index
                                    for index, p in enumerate(phases)
                                    if p[0] == phase
                                ),
                                len(phases) - 1,
                            )
                            for skipped_phase, skipped_mode, _ in phases[
                                failed_phase_index + 1 :
                            ]:
                                append_record(
                                    output,
                                    skipped_record(
                                        size=size,
                                        profile=profile,
                                        repetition=repetition,
                                        mode=skipped_mode,
                                        phase=skipped_phase,
                                        reason=phase_failure,
                                    ),
                                )
                    finally:
                        cleanup_started = time.monotonic()
                        cleanup_error = None
                        try:
                            if run_root.exists():
                                shutil.rmtree(run_root)
                        except OSError as exc:
                            cleanup_error = str(exc)
                        world_path_remains = run_root.exists()
                        cleanup_failures = []
                        if cleanup_error or world_path_remains:
                            cleanup_failures.append("cleanup_failed")
                        free_after_cleanup = free_bytes(args.scratch_root)
                        if free_after_cleanup < free_floor:
                            cleanup_failures.append("resource_failure")
                        cleanup_record = {
                            "schema": SCHEMA,
                            "size": size,
                            "profile": profile,
                            "repetition": repetition,
                            "mode": "cleanup",
                            "phase": "cleanup",
                            "status": cleanup_failures[0] if cleanup_failures else "ok",
                            "failure_conditions": cleanup_failures,
                            "failure": (
                                f"cleanup error={cleanup_error!r}; world path remains={world_path_remains}; "
                                f"free bytes={free_after_cleanup}, floor={free_floor}"
                                if cleanup_failures
                                else None
                            ),
                            "elapsed_s": time.monotonic() - cleanup_started,
                            "cleanup_error": cleanup_error,
                            "world_path_remains": world_path_remains,
                            "scratch_free_after_bytes": free_after_cleanup,
                        }
                        append_record(output, cleanup_record)
                        if cleanup_record["status"] != "ok":
                            any_failure = True
                    print(
                        f"size={size} profile={profile} repetition={repetition}: "
                        f"phase records captured",
                        file=sys.stderr,
                    )
    return 1 if any_failure else 0


if __name__ == "__main__":
    raise SystemExit(main())
