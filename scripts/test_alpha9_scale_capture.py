import json
import sys
import tempfile
import time
import unittest
from pathlib import Path

from scripts import alpha9_scale_capture as capture


class Alpha9ScaleCaptureTests(unittest.TestCase):
    def test_relation_measurements_accept_named_array_and_require_four_relations(self):
        entries = [
            {
                "relation": name,
                "measurements": {
                    "first_page_ms": 1,
                    "first_page_rows": 1,
                    "first_page_bytes": 8,
                    "complete_query_ms": 2,
                    "pages": 1,
                    "rows": 1,
                    "bytes": 8,
                    "canonical_content_digest": f"digest-{name}",
                },
            }
            for name in capture.DERIVED_RELATIONS
        ]
        measurements, error = capture.get_relation_measurements({"relation_queries": entries})
        self.assertIsNone(error)
        self.assertEqual(set(measurements), set(capture.DERIVED_RELATIONS))
        _, error = capture.get_relation_measurements({"relation_queries": entries[:-1]})
        self.assertIn("missing derived relation", error)

    def test_parse_mac_time_rss_orders(self):
        self.assertEqual(
            capture.parse_peak_rss("123456 maximum resident set size"), 123456
        )
        self.assertEqual(
            capture.parse_peak_rss("maximum resident set size 1,234,567"), 1234567
        )
        self.assertIsNone(capture.parse_peak_rss("no rss line"))

    def test_large_child_stderr_does_not_block_capture(self):
        with tempfile.TemporaryDirectory(prefix="a9-scale-capture-test-") as tmp:
            scratch = Path(tmp)
            payload = {
                "schema": "brix.alpha9.scale-bench@1",
                "mode": "child-open",
                "fixture": {"rows": 8000, "retained_revisions": 1, "digest": "d"},
                "build": {
                    "binary": str(Path(sys.executable).resolve()),
                    "profile": "release",
                    "package_version": "0.1.0-alpha.9",
                    "rustc": "rustc test",
                    "embedded_build_commit": "test-commit",
                },
                "timings": {"world_open_ms": 1.5},
            }
            code = (
                "import json,sys,time; "
                "sys.stderr.write('x' * 262144); "
                f"payload={payload!r}; payload['main_entry_unix_ns']=time.time_ns(); "
                "print(json.dumps(payload))"
            )
            record = capture.invoke(
                Path(sys.executable),
                ["-c", code],
                scratch=scratch,
                free_floor=0,
                timeout_s=10,
                size=8000,
                profile="single",
                repetition=1,
                mode="child-open",
                binary_sha256=capture.sha256_file(Path(sys.executable)),
                repository_commit=None,
                expected_batches=1,
            )
            self.assertIn(record["status"], {"ok", "missing_measurement"}, record)
            self.assertGreater(record["stderr_bytes"], 200_000)
            self.assertFalse(record["stdout_truncated"])
            self.assertIsNotNone(record["spawn_to_main_entry_ms"])
            if sys.platform == "darwin":
                self.assertGreater(record["peak_rss_bytes"], 0)
                self.assertEqual(
                    record["peak_rss_source"],
                    "resource.getrusage(RUSAGE_CHILDREN) side channel",
                )
            self.assertEqual(record["example"]["mode"], "child-open")


if __name__ == "__main__":
    unittest.main()
