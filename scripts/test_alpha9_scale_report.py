import json
import tempfile
import unittest
from pathlib import Path

from scripts.alpha9_scale_capture import EXPECTED_PHASES, SCHEMA, VALID_SIZES
from scripts.alpha9_scale_report import summarize


def _record(size, profile, repetition, phase, *, verification=True, logical_bytes=None):
    batches = 1 if profile == "single" else 10
    modes = {
        "setup": "full",
        "load": "child-load",
        "open-before-edits": "child-open",
        "query-before-edits": "child-query",
        "warm-query-before-edits": "child-query-warm",
        "verify-loaded-world": "child-verify",
        "edit-add": "child-edit",
        "edit-retract": "child-edit",
        "verify-after-edits": "child-verify",
        "cleanup": "cleanup",
    }
    record = {
        "schema": SCHEMA,
        "size": size,
        "profile": profile,
        "repetition": repetition,
        "phase": phase,
        "mode": modes[phase],
        "status": "ok",
        "elapsed_s": 1.0,
        "peak_rss_bytes": 100,
        "spawn_to_main_entry_ms": 2.0,
    }
    if phase == "cleanup":
        record.update(world_path_remains=False, scratch_free_after_bytes=4 * 1024**3)
        return record

    build = {
        "rustc": "rustc fixture",
        "embedded_build_commit": "commit-fixture",
        "profile": "release",
                    "package_version": "0.1.0-alpha.9",
        "binary": "/tmp/brix",
    }
    fixture = {
        "rows": size,
        "retained_revisions": batches if phase != "setup" else 0,
        "seed": 0,
        "digest": "fixture-digest",
        "payload_distribution": {"fixed": True},
    }
    example = {
        "schema": "brix.alpha9.scale-bench@1",
        "mode": modes[phase],
        "fixture": fixture,
        "build": build,
        "source_counts": {
            "root::events": size - 6,
            "root::orders": 1,
            "root::inventory": 1,
            "root::entity_a": 1,
            "root::entity_b": 1,
            "root::catalogue": 1,
            "root::decision_labels": 1,
        },
        "derived_counts": {
            "root::activity_pages": 1,
            "root::all_entities": 1,
            "root::fulfillment": 1,
            "root::decision_value_join": 1,
        },
        "timings": {},
    }
    if phase == "setup":
        example["timings"]["init_ms"] = 1.0
    elif phase == "load":
        example["timings"].update(
            world_open_ms=1.0, load_ms=2.0, commit_ms=2.0, fixture_generation_ms=1.0
        )
        example["storage"] = {
            "logical_bytes": logical_bytes if logical_bytes is not None else 1024,
            "allocated_bytes": 2048,
            "components": {"facts": {"logical_bytes": 1024, "allocated_bytes": 2048}},
        }
        counters = {
            key: 1
            for key in (
                "objects_written", "state_node_reads", "state_bytes_read", "state_bytes_written",
                "chunks", "operator_work", "matches", "expressions_evaluated",
            )
        }
        example["load"] = {
            "batch_count": batches,
            "per_batch_commit_counters": [dict(counters) for _ in range(batches)],
        }
    elif phase == "open-before-edits":
        example["timings"]["world_open_ms"] = 1.0
    elif phase in {"query-before-edits", "warm-query-before-edits"}:
        example["timings"].update(world_open_ms=1.0, first_page_ms=1.0, complete_query_ms=2.0)
        example["query"] = {"rows": 1, "bytes": 8, "work_units": 1, "matches": 1, "expressions_evaluated": 1}
        example["derived_relations"] = {
            relation: {
                "first_page_ms": 1.0,
                "first_page_rows": 1,
                "first_page_bytes": 8,
                "complete_query_ms": 2.0,
                "pages": 1,
                "rows": 1,
                "bytes": 8,
                "canonical_content_digest": f"digest-{relation}",
            }
            for relation in (
                "root::activity_pages", "root::all_entities", "root::fulfillment", "root::decision_value_join"
            )
        }
    elif phase in {"verify-loaded-world", "verify-after-edits"}:
        example["timings"]["correctness_ms"] = 1.0
        example["correctness"] = {
            "passed": verification,
            "derived_counts_match": True,
            "named_query_rows_match": True,
            "derived_pages_match": True,
            "oracle": "passed",
        }
    elif phase in {"edit-add", "edit-retract"}:
        action = "add" if phase == "edit-add" else "retract"
        before = batches if action == "add" else batches + 1
        example["action"] = action
        example["timings"]["edit_ms"] = 1.0
        example["edit"] = {
            "elapsed_ms": 1.0,
            "before_revision": before,
            "after_revision": before + 1,
            "counters": {
                key: 1
                for key in (
                    "objects_written", "state_node_reads", "state_bytes_read", "state_bytes_written",
                    "chunks", "operator_work", "matches", "expressions_evaluated",
                )
            },
        }
    record.update(binary_sha256="binary-hash", repository_commit="repo-commit", example=example)
    return record


def _write_capture(path, *, bad_verification=None, omit=None, over_budget=None):
    with path.open("w", encoding="utf-8") as target:
        for size in sorted(VALID_SIZES):
            for profile in ("single", "ten"):
                for repetition in range(1, 6):
                    for phase in EXPECTED_PHASES:
                        identity = (size, profile, repetition, phase)
                        if identity == omit:
                            continue
                        verification = identity != bad_verification
                        logical_bytes = None
                        if profile == "ten" and identity == over_budget:
                            logical_bytes = 16 * 1024**2 + 12 * 1024 * size + 1
                        target.write(json.dumps(_record(
                            size, profile, repetition, phase,
                            verification=verification, logical_bytes=logical_bytes,
                        )) + "\n")


class Alpha9ScaleReportTests(unittest.TestCase):
    def test_complete_synthetic_capture_summarizes(self):
        with tempfile.TemporaryDirectory(prefix="a9-scale-report-") as tmp:
            path = Path(tmp) / "capture.jsonl"
            _write_capture(path)
            summary = summarize(path)
            self.assertTrue(summary["qualified"], summary["failures"])
            self.assertEqual(len(summary["cells"]), len(VALID_SIZES) * 2)
            self.assertTrue(all(cell["passed"] for cell in summary["cells"]))

    def test_previous_package_version_cannot_qualify_release(self):
        with tempfile.TemporaryDirectory(prefix="a9-scale-report-") as tmp:
            path = Path(tmp) / "capture.jsonl"
            _write_capture(path)
            records = [json.loads(line) for line in path.read_text().splitlines()]
            for record in records:
                if record.get("example"):
                    record["example"]["build"]["package_version"] = "0.1.0-alpha.8"
            path.write_text("".join(json.dumps(record) + "\n" for record in records))
            summary = summarize(path)
            self.assertFalse(summary["qualified"])
            self.assertTrue(any(f["kind"] == "wrong_package_version" for f in summary["failures"]))

    def test_missing_and_failed_verification_never_qualify(self):
        with tempfile.TemporaryDirectory(prefix="a9-scale-report-") as tmp:
            path = Path(tmp) / "capture.jsonl"
            _write_capture(
                path,
                bad_verification=(8000, "single", 1, "verify-loaded-world"),
                omit=(8000, "single", 1, "verify-after-edits"),
            )
            summary = summarize(path)
            self.assertFalse(summary["qualified"])
            kinds = {failure["kind"] for failure in summary["failures"]}
            self.assertIn("verification_failed_or_missing", kinds)
            self.assertIn("phase_record_count_mismatch", kinds)

    def test_ten_revision_logical_disk_floor_is_enforced(self):
        with tempfile.TemporaryDirectory(prefix="a9-scale-report-") as tmp:
            path = Path(tmp) / "capture.jsonl"
            _write_capture(path, over_budget=(8000, "ten", 1, "load"))
            summary = summarize(path)
            self.assertFalse(summary["qualified"])
            cell = next(cell for cell in summary["cells"] if cell["size"] == 8000 and cell["profile"] == "ten")
            self.assertFalse(cell["disk_budget"]["passed"])
            self.assertTrue(any(f["kind"] == "disk_budget_miss_or_missing_samples" for f in summary["failures"]))

    def test_derived_page_verification_must_pass_when_reported(self):
        with tempfile.TemporaryDirectory(prefix="a9-scale-report-") as tmp:
            path = Path(tmp) / "capture.jsonl"
            _write_capture(path)
            records = [json.loads(line) for line in path.read_text().splitlines()]
            for record in records:
                if record.get("phase") == "verify-loaded-world" and record.get("size") == 8000:
                    record["example"]["correctness"]["derived_pages_match"] = False
                    break
            path.write_text("".join(json.dumps(record) + "\n" for record in records))
            summary = summarize(path)
            self.assertFalse(summary["qualified"])
            self.assertTrue(any(f["kind"] == "derived_pages_verification_failed" for f in summary["failures"]))

    def test_query_page_content_is_stable_across_repetitions_and_warm_pass(self):
        with tempfile.TemporaryDirectory(prefix="a9-scale-report-") as tmp:
            path = Path(tmp) / "capture.jsonl"
            _write_capture(path)
            records = [json.loads(line) for line in path.read_text().splitlines()]
            for record in records:
                if (
                    record.get("phase") == "warm-query-before-edits"
                    and record.get("size") == 8000
                    and record.get("profile") == "single"
                    and record.get("repetition") == 1
                ):
                    record["example"]["query"]["rows"] += 1
                    record["example"]["derived_relations"]["root::fulfillment"]["canonical_content_digest"] = "changed"
                    break
            path.write_text("".join(json.dumps(record) + "\n" for record in records))
            summary = summarize(path)
            self.assertFalse(summary["qualified"])
            kinds = {failure["kind"] for failure in summary["failures"]}
            self.assertIn("inconsistent_named_query_pages", kinds)
            self.assertIn("inconsistent_derived_relation_pages", kinds)

    def test_load_source_counts_must_sum_to_requested_facts(self):
        with tempfile.TemporaryDirectory(prefix="a9-scale-report-") as tmp:
            path = Path(tmp) / "capture.jsonl"
            _write_capture(path)
            records = [json.loads(line) for line in path.read_text().splitlines()]
            for record in records:
                if record.get("phase") == "load" and record.get("size") == 8000:
                    record["example"]["source_counts"]["root::events"] -= 1
                    break
            path.write_text("".join(json.dumps(record) + "\n" for record in records))
            summary = summarize(path)
            self.assertFalse(summary["qualified"])
            self.assertTrue(any(f["kind"] == "source_count_sum_mismatch" for f in summary["failures"]))

    def test_verified_derived_counts_match_paged_relation_rows(self):
        with tempfile.TemporaryDirectory(prefix="a9-scale-report-") as tmp:
            path = Path(tmp) / "capture.jsonl"
            _write_capture(path)
            records = [json.loads(line) for line in path.read_text().splitlines()]
            for record in records:
                if record.get("phase") in {"verify-loaded-world", "verify-after-edits"} and record.get("size") == 8000:
                    record["example"]["derived_counts"]["root::activity_pages"] = 2
            path.write_text("".join(json.dumps(record) + "\n" for record in records))
            summary = summarize(path)
            self.assertFalse(summary["qualified"])
            self.assertTrue(any(f["kind"] == "derived_count_relation_mismatch" for f in summary["failures"]))


if __name__ == "__main__":
    unittest.main()
