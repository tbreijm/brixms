#!/usr/bin/env python3
"""Validate and summarize an A9-7 raw capture JSONL as machine-readable JSON."""

from __future__ import annotations

import argparse
import json
import math
import statistics
import sys
from pathlib import Path
from typing import Any

try:
    from scripts.alpha9_scale_capture import (
        EXPECTED_PHASES,
        EXPECTED_PACKAGE_VERSION,
        SCHEMA as CAPTURE_SCHEMA,
        VALID_SIZES,
        get_nested,
        DERIVED_RELATIONS,
        RELATION_METRICS,
        get_relation_measurements,
        is_number,
    )
except ModuleNotFoundError:  # direct `python scripts/alpha9_scale_report.py`
    from alpha9_scale_capture import (
        EXPECTED_PHASES,
        EXPECTED_PACKAGE_VERSION,
        SCHEMA as CAPTURE_SCHEMA,
        VALID_SIZES,
        get_nested,
        DERIVED_RELATIONS,
        RELATION_METRICS,
        get_relation_measurements,
        is_number,
    )


SUMMARY_SCHEMA = "brix.alpha9.scale-summary@1"
MIN_QUALIFICATION_REPETITIONS = 5
PROFILES = ("single", "ten")
SOURCE_RELATIONS = (
    "root::events",
    "root::orders",
    "root::inventory",
    "root::entity_a",
    "root::entity_b",
    "root::catalogue",
    "root::decision_labels",
)
COUNTER_FIELDS = (
    "objects_written",
    "state_node_reads",
    "state_bytes_read",
    "state_bytes_written",
    "chunks",
    "operator_work",
    "matches",
    "expressions_evaluated",
)


def _parse_raw(path: Path) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    records: list[dict[str, Any]] = []
    failures: list[dict[str, Any]] = []
    try:
        source = path.open("r", encoding="utf-8")
    except OSError as exc:
        return [], [{"kind": "read_error", "detail": str(exc)}]
    with source:
        for line_no, line in enumerate(source, 1):
            if not line.strip():
                continue
            try:
                record = json.loads(line)
            except json.JSONDecodeError as exc:
                failures.append(
                    {"kind": "invalid_json", "line": line_no, "detail": str(exc)}
                )
                continue
            if not isinstance(record, dict) or record.get("schema") != CAPTURE_SCHEMA:
                failures.append(
                    {"kind": "invalid_record", "line": line_no, "detail": "wrong capture schema"}
                )
                continue
            records.append(record)
    return records, failures


def _summary(values: list[float | int]) -> dict[str, Any]:
    clean = [float(value) for value in values if is_number(value)]
    if not clean:
        return {"sample_count": 0, "median": None, "min": None, "max": None}
    return {
        "sample_count": len(clean),
        "median": statistics.median(clean),
        "min": min(clean),
        "max": max(clean),
    }


def _capture_signature(record: dict[str, Any]) -> tuple[Any, ...]:
    example = record.get("example") or {}
    fixture = example.get("fixture") or {}
    return (
        fixture.get("digest"),
        fixture.get("seed"),
        fixture.get("payload_distribution"),
        example.get("source_counts"),
    )


def _validate_record(record: dict[str, Any], failures: list[dict[str, Any]]) -> None:
    size = record.get("size")
    profile = record.get("profile")
    rep = record.get("repetition")
    phase = record.get("phase")
    example = record.get("example") or {}
    status = record.get("status")
    location = {"size": size, "profile": profile, "repetition": rep, "phase": phase}

    if size not in VALID_SIZES or profile not in PROFILES:
        failures.append({"kind": "invalid_identity", **location})
        return
    if phase not in EXPECTED_PHASES:
        failures.append({"kind": "unexpected_phase", **location, "status": status})
        return
    if not isinstance(rep, int) or rep < 1:
        failures.append({"kind": "invalid_repetition", **location})
        return
    if status != "ok":
        failures.append(
            {
                "kind": "phase_failure",
                **location,
                "status": status,
                "detail": record.get("failure") or record.get("resource_failure"),
            }
        )
        return

    # Cleanup is a harness-owned phase and intentionally has no example JSON.
    if phase == "cleanup":
        if record.get("mode") != "cleanup":
            failures.append({"kind": "phase_mode_mismatch", **location, "expected": "cleanup"})
        if record.get("world_path_remains") is not False:
            failures.append({"kind": "cleanup_incomplete", **location})
        free_after = record.get("scratch_free_after_bytes")
        if not is_number(free_after) or free_after < 3 * 1024**3:
            failures.append({"kind": "cleanup_free_space_below_floor", **location})
        return

    expected_modes = {
        "setup": "full",
        "load": "child-load",
        "open-before-edits": "child-open",
        "query-before-edits": "child-query",
        "warm-query-before-edits": "child-query-warm",
        "verify-loaded-world": "child-verify",
        "edit-add": "child-edit",
        "edit-retract": "child-edit",
        "verify-after-edits": "child-verify",
    }
    if record.get("mode") != expected_modes.get(phase):
        failures.append(
            {"kind": "phase_mode_mismatch", **location, "expected": expected_modes.get(phase), "actual": record.get("mode")}
        )

    if not isinstance(example, dict):
        failures.append({"kind": "missing_example", **location})
        return
    if example.get("schema") != "brix.alpha9.scale-bench@1":
        failures.append({"kind": "invalid_example_schema", **location})
    for field in ("binary_sha256", "repository_commit"):
        if not isinstance(record.get(field), str) or not record[field]:
            failures.append({"kind": "missing_build_identity", **location, "field": field})
    build = example.get("build")
    if not isinstance(build, dict):
        failures.append({"kind": "missing_build_metadata", **location})
    else:
        for field in ("binary", "rustc", "embedded_build_commit"):
            if not isinstance(build.get(field), str) or not build[field]:
                failures.append({"kind": "missing_build_metadata", **location, "field": field})
    if not isinstance(build, dict) or build.get("package_version") != EXPECTED_PACKAGE_VERSION:
        failures.append({"kind": "wrong_package_version", **location, "expected": EXPECTED_PACKAGE_VERSION})
    fixture = example.get("fixture")
    if not isinstance(fixture, dict) or fixture.get("rows") != size:
        failures.append({"kind": "fixture_row_mismatch", **location})
    expected_batches = 1 if profile == "single" else 10
    if phase != "setup" and isinstance(fixture, dict):
        if fixture.get("retained_revisions") != expected_batches:
            failures.append({"kind": "retained_revision_mismatch", **location})
        if not isinstance(fixture.get("digest"), str) or not fixture["digest"]:
            failures.append({"kind": "missing_fixture_digest", **location})

    if not is_number(record.get("elapsed_s")):
        failures.append({"kind": "missing_process_wall_time", **location})
    if not is_number(record.get("peak_rss_bytes")):
        failures.append({"kind": "missing_peak_rss", **location})
    if not is_number(record.get("spawn_to_main_entry_ms")):
        failures.append({"kind": "missing_startup_time", **location})

    timings = example.get("timings") or {}
    if phase == "setup":
        required = [("timings.init_ms", timings.get("init_ms"))]
    elif phase == "load":
        storage = example.get("storage") or {}
        load = example.get("load") or {}
        required = [
            ("timings.world_open_ms", timings.get("world_open_ms")),
            ("timings.load_ms", timings.get("load_ms")),
            ("timings.commit_ms", timings.get("commit_ms")),
            ("timings.fixture_generation_ms", timings.get("fixture_generation_ms")),
            ("storage.logical_bytes", storage.get("logical_bytes")),
            ("storage.allocated_bytes", storage.get("allocated_bytes")),
        ]
        if load.get("batch_count") != expected_batches:
            failures.append({"kind": "load_batch_count_mismatch", **location})
        commits = load.get("per_batch_commit_counters")
        if not isinstance(commits, list) or len(commits) != expected_batches:
            failures.append({"kind": "load_commit_profile_count_mismatch", **location})
        else:
            for commit_index, counters in enumerate(commits):
                for field in COUNTER_FIELDS:
                    if not is_number((counters or {}).get(field)):
                        failures.append(
                            {
                                "kind": "missing_load_counter",
                                **location,
                                "batch": commit_index + 1,
                                "counter": field,
                            }
                        )
        components = storage.get("components")
        if not isinstance(components, dict) or not components:
            failures.append({"kind": "missing_storage_components", **location})
        else:
            for component, values in components.items():
                if not isinstance(values, dict) or any(
                    not is_number(values.get(field)) for field in ("logical_bytes", "allocated_bytes")
                ):
                    failures.append(
                        {"kind": "invalid_storage_component", **location, "component": component}
                    )
        source_counts = example.get("source_counts")
        if (
            not isinstance(source_counts, dict)
            or set(source_counts) != set(SOURCE_RELATIONS)
            or any(not isinstance(count, int) or isinstance(count, bool) or count < 0 for count in source_counts.values())
        ):
            failures.append({"kind": "missing_source_counts", **location})
        elif sum(source_counts.values()) != size:
            failures.append(
                {"kind": "source_count_sum_mismatch", **location, "sum": sum(source_counts.values()), "expected": size}
            )
    elif phase == "open-before-edits":
        required = [("timings.world_open_ms", timings.get("world_open_ms"))]
    elif phase in {"query-before-edits", "warm-query-before-edits"}:
        query = example.get("query") or {}
        required = [
            ("timings.world_open_ms", timings.get("world_open_ms")),
            ("timings.first_page_ms", timings.get("first_page_ms")),
            ("timings.complete_query_ms", timings.get("complete_query_ms")),
            ("query.rows", query.get("rows")),
            ("query.bytes", query.get("bytes")),
            ("query.work_units", query.get("work_units")),
            ("query.matches", query.get("matches")),
            ("query.expressions_evaluated", query.get("expressions_evaluated")),
        ]
        _, relation_error = get_relation_measurements(example)
        if relation_error:
            failures.append(
                {"kind": "missing_relation_measurement", **location, "detail": relation_error}
            )
    elif phase in {"verify-loaded-world", "verify-after-edits"}:
        correctness = example.get("correctness") or {}
        required = [("timings.correctness_ms", timings.get("correctness_ms"))]
        for key in ("passed", "derived_counts_match", "named_query_rows_match"):
            if correctness.get(key) is not True:
                failures.append({"kind": "verification_failed_or_missing", **location, "field": key})
        if correctness.get("oracle") != "passed":
            failures.append({"kind": "oracle_failed_or_missing", **location})
        if "derived_pages_match" in correctness and correctness.get("derived_pages_match") is not True:
            failures.append({"kind": "derived_pages_verification_failed", **location})
        derived_counts = example.get("derived_counts")
        if (
            not isinstance(derived_counts, dict)
            or set(derived_counts) != set(DERIVED_RELATIONS)
            or any(not isinstance(count, int) or isinstance(count, bool) or count < 0 for count in derived_counts.values())
        ):
            failures.append({"kind": "missing_derived_counts", **location})
        source_counts = example.get("source_counts")
        if (
            not isinstance(source_counts, dict)
            or set(source_counts) != set(SOURCE_RELATIONS)
            or any(not isinstance(count, int) or isinstance(count, bool) or count < 0 for count in source_counts.values())
            or sum(source_counts.values()) != size
        ):
            failures.append({"kind": "missing_source_counts", **location})
    elif phase in {"edit-add", "edit-retract"}:
        edit = example.get("edit") or {}
        action = "add" if phase == "edit-add" else "retract"
        if example.get("action") != action:
            failures.append({"kind": "edit_action_mismatch", **location, "expected": action})
        required = [
            ("timings.edit_ms", timings.get("edit_ms")),
            ("edit.elapsed_ms", edit.get("elapsed_ms")),
            ("edit.before_revision", edit.get("before_revision")),
            ("edit.after_revision", edit.get("after_revision")),
        ]
        counters = edit.get("counters") or {}
        for field in COUNTER_FIELDS:
            if not is_number(counters.get(field)):
                failures.append({"kind": "missing_edit_counter", **location, "counter": field})
        before = expected_batches + (1 if action == "retract" else 0)
        if edit.get("before_revision") != before or edit.get("after_revision") != before + 1:
            failures.append({"kind": "edit_revision_mismatch", **location})
    else:
        required = [("elapsed_s", record.get("elapsed_s"))]

    for field, value in required:
        if not is_number(value):
            failures.append({"kind": "missing_phase_measurement", **location, "field": field})


def _metric_for(phase: str, record: dict[str, Any]) -> dict[str, float | int | None]:
    example = record.get("example") or {}
    values: dict[str, float | int | None] = {
        "process_wall_ms": record.get("elapsed_s") * 1000 if is_number(record.get("elapsed_s")) else None,
        "peak_rss_bytes": record.get("peak_rss_bytes"),
        "spawn_to_main_entry_ms": record.get("spawn_to_main_entry_ms"),
    }
    if phase in {"setup", "load", "open-before-edits", "query-before-edits", "warm-query-before-edits", "verify-loaded-world", "verify-after-edits"}:
        for key, value in (example.get("timings") or {}).items():
            if is_number(value):
                values[key] = value
    if phase == "load":
        storage = example.get("storage") or {}
        for key in ("logical_bytes", "allocated_bytes"):
            if is_number(storage.get(key)):
                values[f"storage_{key}"] = storage[key]
        for component, amounts in (storage.get("components") or {}).items():
            if isinstance(amounts, dict):
                for key in ("logical_bytes", "allocated_bytes"):
                    if is_number(amounts.get(key)):
                        values[f"storage_component_{component}_{key}"] = amounts[key]
    if phase in {"query-before-edits", "warm-query-before-edits"}:
        query = example.get("query") or {}
        for key in ("rows", "bytes", "work_units", "matches", "expressions_evaluated"):
            if is_number(query.get(key)):
                values[f"query_{key}"] = query[key]
        relations, error = get_relation_measurements(example)
        if error is None:
            for relation in DERIVED_RELATIONS:
                metrics = relations[relation]
                for key in (*RELATION_METRICS, "canonical_content_digest"):
                    value = metrics.get(key)
                    if is_number(value):
                        values[f"relation_{relation}_{key}"] = value
    if phase in {"edit-add", "edit-retract"}:
        edit = example.get("edit") or {}
        if is_number(edit.get("elapsed_ms")):
            values["edit_ms"] = edit["elapsed_ms"]
        for key, value in (edit.get("counters") or {}).items():
            if is_number(value):
                values[f"counter_{key}"] = value
    return values


def summarize(path: Path, required_repetitions: int = 5) -> dict[str, Any]:
    if required_repetitions < MIN_QUALIFICATION_REPETITIONS:
        raise ValueError(
            f"qualification requires at least {MIN_QUALIFICATION_REPETITIONS} repetitions per cell"
        )
    records, failures = _parse_raw(path)
    cells: dict[tuple[int, str], dict[tuple[int, str], list[dict[str, Any]]]] = {}
    global_signatures: dict[str, set[str]] = {
        "binary_sha256": set(),
        "repository_commit": set(),
        "rustc": set(),
        "embedded_build_commit": set(),
        "host_metadata_sha256": set(),
    }
    fixture_signatures: dict[int, set[str]] = {}
    derived_signatures: dict[int, set[str]] = {}
    derived_count_values: dict[int, dict[str, int]] = {}
    relation_row_values: dict[tuple[int, str], set[int]] = {}
    record_validation_ok: set[int] = set()
    host_metadata_presence: set[str] = set()
    for record in records:
        example = record.get("example") or {}
        build = example.get("build") or {}
        for key in ("binary_sha256", "repository_commit", "host_metadata_sha256"):
            value = record.get(key)
            if value:
                global_signatures[key].add(str(value))
        if record.get("phase") != "cleanup":
            host_metadata_presence.add(str(record.get("host_metadata_sha256") or "<omitted>"))
        for key, value in (
            ("rustc", build.get("rustc")),
            ("embedded_build_commit", build.get("embedded_build_commit")),
        ):
            if value:
                global_signatures[key].add(str(value))
        size = record.get("size")
        profile = record.get("profile")
        repetition = record.get("repetition")
        phase = record.get("phase")
        if isinstance(size, int) and isinstance(profile, str) and isinstance(repetition, int) and isinstance(phase, str):
            cells.setdefault((size, profile), {}).setdefault((repetition, phase), []).append(record)
            if phase == "load" and record.get("status") == "ok":
                signature = json.dumps(_capture_signature(record), sort_keys=True, separators=(",", ":"))
                fixture_signatures.setdefault(size, set()).add(signature)
            if phase in {"verify-loaded-world", "verify-after-edits"} and record.get("status") == "ok":
                derived = (record.get("example") or {}).get("derived_counts")
                if isinstance(derived, dict):
                    derived_signatures.setdefault(size, set()).add(
                        json.dumps(derived, sort_keys=True, separators=(",", ":"))
                    )
                    if all(isinstance(derived.get(name), int) and not isinstance(derived.get(name), bool) for name in DERIVED_RELATIONS):
                        derived_count_values[size] = {name: derived[name] for name in DERIVED_RELATIONS}
        failure_count = len(failures)
        _validate_record(record, failures)
        if len(failures) == failure_count:
            record_validation_ok.add(id(record))

    for field, values in global_signatures.items():
        optional = field == "host_metadata_sha256"
        if len(values) != 1 and (values or not optional):
            failures.append({"kind": "inconsistent_global_identity", "field": field, "values": sorted(values)})
    for size, signatures in fixture_signatures.items():
        if len(signatures) != 1:
            failures.append({"kind": "inconsistent_fixture", "size": size, "signature_count": len(signatures)})
    for size, signatures in derived_signatures.items():
        if len(signatures) != 1:
            failures.append({"kind": "inconsistent_derived_counts", "size": size, "signature_count": len(signatures)})
    if len(host_metadata_presence) > 1:
        failures.append(
            {"kind": "inconsistent_global_identity", "field": "host_metadata_presence", "values": sorted(host_metadata_presence)}
        )
    query_page_signatures: dict[int, set[tuple[Any, Any]]] = {}
    relation_page_signatures: dict[tuple[int, str], set[tuple[Any, ...]]] = {}
    for record in records:
        if (
            record.get("phase") not in {"query-before-edits", "warm-query-before-edits"}
            or record.get("status") != "ok"
            or id(record) not in record_validation_ok
        ):
            continue
        size = record.get("size")
        example = record.get("example") or {}
        query = example.get("query") or {}
        if isinstance(size, int):
            query_page_signatures.setdefault(size, set()).add((query.get("rows"), query.get("bytes")))
            relation_values, relation_error = get_relation_measurements(example)
            if relation_error is None:
                for relation in DERIVED_RELATIONS:
                    metrics = relation_values[relation]
                    if isinstance(metrics.get("rows"), int) and not isinstance(metrics.get("rows"), bool):
                        relation_row_values.setdefault((size, relation), set()).add(metrics["rows"])
                    relation_page_signatures.setdefault((size, relation), set()).add(
                        (
                            metrics.get("rows"),
                            metrics.get("bytes"),
                            metrics.get("first_page_rows"),
                            metrics.get("first_page_bytes"),
                            metrics.get("canonical_content_digest"),
                        )
                    )
    for size, signatures in query_page_signatures.items():
        if len(signatures) != 1:
            failures.append(
                {"kind": "inconsistent_named_query_pages", "size": size, "signature_count": len(signatures)}
            )
    for (size, relation), signatures in relation_page_signatures.items():
        if len(signatures) != 1:
            failures.append(
                {
                    "kind": "inconsistent_derived_relation_pages",
                    "size": size,
                    "relation": relation,
                    "signature_count": len(signatures),
                }
            )
    for (size, relation), row_values in relation_row_values.items():
        expected = derived_count_values.get(size, {}).get(relation)
        if expected is not None and (len(row_values) != 1 or expected not in row_values):
            failures.append(
                {
                    "kind": "derived_count_relation_mismatch",
                    "size": size,
                    "relation": relation,
                    "derived_count": expected,
                    "query_row_counts": sorted(row_values),
                }
            )

    cell_results: list[dict[str, Any]] = []
    all_cells_complete = True
    all_ten_budgets_pass = True
    for size in sorted(VALID_SIZES):
        for profile in PROFILES:
            cell = cells.get((size, profile), {})
            reps = sorted({rep for rep, _ in cell})
            phase_summaries: dict[str, Any] = {}
            for phase in EXPECTED_PHASES:
                phase_records = [
                    r
                    for (rep, candidate_phase), group in cell.items()
                    if candidate_phase == phase
                    for r in group
                ]
                valid = [
                    r for r in phase_records
                    if r.get("status") == "ok" and id(r) in record_validation_ok
                ]
                if len(phase_records) != len(valid):
                    # Failures were already recorded; this count makes gaps
                    # explicit to consumers of the summary JSON.
                    pass
                metrics_by_key: dict[str, list[float | int]] = {}
                for record in valid:
                    for key, value in _metric_for(phase, record).items():
                        if is_number(value):
                            metrics_by_key.setdefault(key, []).append(value)
                phase_summaries[phase] = {
                    "valid_samples": len(valid),
                    "expected_samples": required_repetitions,
                    "status_counts": {
                        status: sum(r.get("status") == status for r in phase_records)
                        for status in sorted({str(r.get("status")) for r in phase_records})
                    },
                    "metrics": {key: _summary(values) for key, values in sorted(metrics_by_key.items())},
                }

            complete_reps = []
            for repetition in reps:
                if all(
                    len(cell.get((repetition, phase), [])) == 1
                    and cell[(repetition, phase)][0].get("status") == "ok"
                    and id(cell[(repetition, phase)][0]) in record_validation_ok
                    for phase in EXPECTED_PHASES
                ):
                    complete_reps.append(repetition)
            expected_rep_ids = set(range(1, required_repetitions + 1))
            complete = len(complete_reps) >= required_repetitions and expected_rep_ids.issubset(complete_reps)
            if not complete:
                failures.append(
                    {
                        "kind": "incomplete_cell",
                        "size": size,
                        "profile": profile,
                        "required_repetitions": required_repetitions,
                        "complete_repetitions": complete_reps,
                    }
                )
            all_cells_complete &= complete

            budget: dict[str, Any] | None = None
            if profile == "ten":
                max_bytes = 16 * 1024**2 + 12 * 1024 * size
                load_records = [
                    r
                    for (rep, phase), group in cell.items()
                    if phase == "load"
                    for r in group
                    if r.get("status") == "ok" and id(r) in record_validation_ok
                ]
                bytes_values = [
                    get_nested(r.get("example"), ("storage", "logical_bytes"))
                    for r in load_records
                ]
                bytes_values = [value for value in bytes_values if is_number(value)]
                budget_pass = len(bytes_values) >= required_repetitions and all(value <= max_bytes for value in bytes_values)
                if not budget_pass:
                    failures.append(
                        {
                            "kind": "disk_budget_miss_or_missing_samples",
                            "size": size,
                            "profile": profile,
                            "budget_bytes": max_bytes,
                            "sample_count": len(bytes_values),
                            "max_measured_bytes": max(bytes_values) if bytes_values else None,
                        }
                    )
                all_ten_budgets_pass &= budget_pass
                budget = {
                    "limit_logical_bytes": max_bytes,
                    "sample_count": len(bytes_values),
                    "measured": _summary(bytes_values),
                    "passed": budget_pass,
                }

            # Validate that one repetition has one record per required phase;
            # a duplicate or a missing phase cannot inflate the five-sample count.
            for repetition in sorted(expected_rep_ids | set(reps)):
                for phase in EXPECTED_PHASES:
                    group = cell.get((repetition, phase), [])
                    if len(group) != 1:
                        failures.append(
                            {
                                "kind": "phase_record_count_mismatch",
                                "size": size,
                                "profile": profile,
                                "repetition": repetition,
                                "phase": phase,
                                "record_count": len(group),
                            }
                        )
            cell_failures = [
                failure
                for failure in failures
                if failure.get("size") == size and failure.get("profile") == profile
            ]
            verified = [
                record
                for (rep, phase), group in cell.items()
                if phase == "verify-loaded-world" and rep in complete_reps
                for record in group
                if record.get("status") == "ok" and id(record) in record_validation_ok
            ]
            example = (verified[0].get("example") or {}) if verified else {}
            query_records = [
                record
                for (rep, phase), group in cell.items()
                if phase == "query-before-edits" and rep in complete_reps
                for record in group
                if record.get("status") == "ok" and id(record) in record_validation_ok
            ]
            relation_page_outputs: dict[str, Any] = {}
            if query_records:
                query_example = query_records[0].get("example") or {}
                relation_values, relation_error = get_relation_measurements(query_example)
                if relation_error is None:
                    for relation in DERIVED_RELATIONS:
                        metrics = relation_values[relation]
                        relation_page_outputs[relation] = {
                            key: metrics.get(key)
                            for key in (*RELATION_METRICS, "canonical_content_digest")
                        }
            cell_results.append(
                {
                    "size": size,
                    "profile": profile,
                    "complete_repetitions": complete_reps,
                    "complete_repetition_count": len(complete_reps),
                    "required_repetitions": required_repetitions,
                    "passed": complete and not cell_failures and (budget is None or budget["passed"]),
                    "disk_budget": budget,
                    "source_counts": example.get("source_counts"),
                    "derived_counts": example.get("derived_counts"),
                    "named_query_pages": (
                        {
                            "rows": (query_records[0].get("example") or {}).get("query", {}).get("rows"),
                            "bytes": (query_records[0].get("example") or {}).get("query", {}).get("bytes"),
                        }
                        if query_records
                        else None
                    ),
                    "derived_relation_pages": relation_page_outputs,
                    "phases": phase_summaries,
                }
            )

    consistency_failed = any(
        failure.get("kind") in {"inconsistent_global_identity", "inconsistent_fixture", "inconsistent_derived_counts"}
        for failure in failures
    )
    qualified = all_cells_complete and all_ten_budgets_pass and not consistency_failed and not failures
    return {
        "schema": SUMMARY_SCHEMA,
        "raw_jsonl": str(path),
        "required_repetitions": required_repetitions,
        "expected_cells": len(VALID_SIZES) * len(PROFILES),
        "qualified": qualified,
        "global_identity": {key: sorted(values) for key, values in global_signatures.items()},
        "cells": cell_results,
        "failures": failures,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("raw_jsonl", type=Path)
    parser.add_argument("--output", type=Path, help="new summary JSON path (must not exist)")
    parser.add_argument("--repetitions", type=int, default=5)
    args = parser.parse_args()
    if args.repetitions < MIN_QUALIFICATION_REPETITIONS:
        parser.error(f"--repetitions must be at least {MIN_QUALIFICATION_REPETITIONS}")
    summary = summarize(args.raw_jsonl, args.repetitions)
    payload = json.dumps(summary, indent=2, sort_keys=True) + "\n"
    if args.output:
        try:
            args.output.parent.mkdir(parents=True, exist_ok=True)
            with args.output.open("x", encoding="utf-8") as output:
                output.write(payload)
        except OSError as exc:
            parser.error(f"cannot write summary: {exc}")
    else:
        sys.stdout.write(payload)
    return 0 if summary["qualified"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
