"""Regression checks that the faster runner cannot report a false green gate."""

import contextlib
import io
import subprocess
import unittest
from unittest.mock import patch

import test_workspace


class VerificationExitTests(unittest.TestCase):
    def run_gate(self, test_exit, doc_exit, args=()):
        calls = []

        def cargo(command, **kwargs):
            calls.append(command)
            if command[1:3] == ["nextest", "--version"]:
                code = 0
            elif command[1:3] == ["nextest", "run"]:
                code = test_exit
            else:
                self.assertIn("--doc", command)
                code = doc_exit
            return subprocess.CompletedProcess(command, code, stdout="", stderr="")

        with patch.object(test_workspace.subprocess, "run", side_effect=cargo):
            with contextlib.redirect_stdout(io.StringIO()):
                code = test_workspace.main(list(args))
        return code, calls

    def test_test_failure_still_runs_doctests_and_remains_failure(self):
        code, calls = self.run_gate(100, 0)
        self.assertEqual(code, 1)
        self.assertIn("--doc", calls[-1])

    def test_doctest_failure_is_not_hidden_by_test_success(self):
        code, _ = self.run_gate(0, 101)
        self.assertEqual(code, 1)

    def test_invalid_or_empty_selection_is_not_a_pass(self):
        code, _ = self.run_gate(4, 0, ["--filter", "test(nonexistent)"])
        self.assertEqual(code, 1)

    def test_success_requires_both_phases_and_defaults_to_workspace(self):
        code, calls = self.run_gate(0, 0)
        self.assertEqual(code, 0)
        for command in calls[1:]:
            self.assertIn("--workspace", command)
        self.assertNotIn("--filterset", calls[1])

    def test_package_and_build_directory_apply_to_both_phases(self):
        code, calls = self.run_gate(0, 0, ["-p", "brix-kb", "--target-dir", "target/review"])
        self.assertEqual(code, 0)
        for command in calls[1:]:
            self.assertEqual(command[command.index("--package") + 1], "brix-kb")
            self.assertEqual(command[command.index("--target-dir") + 1],
                             str(test_workspace.Path("target/review").resolve()))


if __name__ == "__main__":
    unittest.main()
