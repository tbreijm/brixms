#!/usr/bin/env python3
"""Smoke check an extracted Brix release package directory.

Validates that a packaged release archive directory contains all required shipped
files, reports the expected binary version, and successfully executes the complete
finite-decision external-input and legacy workflows from outside the source checkout.

Usage:
    python3 scripts/smoke_release_package.py <package_dir> <expected_version>
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

REQUIRED_SHIPPED_FILES = [
    Path(p) for p in (
        "brix", "README.md", "LICENSE",
        "examples/shipping.brix", "examples/shipping-input.brix", "examples/shipping-input.json",
    )
]

EXPECTED_SCHEMA = "brix.cli.result@1"
EXPECTED_PROFILE = "brix.l3.finite-decision@1"
COMMAND_TIMEOUT_SECONDS = 30
HEX64_PATTERN = re.compile(r"^[0-9a-fA-F]{64}$")


class SmokeCheckError(Exception):
    """Raised when any smoke validation check fails."""

def assert_64_hex(value: object, field_name: str) -> str:
    """Validate a 64-hex string using fullmatch without permissive parsing."""
    if not isinstance(value, str) or not HEX64_PATTERN.fullmatch(value):
        raise SmokeCheckError(f"Field '{field_name}' must be a 64-hex string, got {value!r}")
    return value

def run_brix_json(
    args: list[str], cwd: Path, expected_cmd: str, expected_status: str,
    expected_code: int = 0, expected_ok: bool = True, timeout: int = COMMAND_TIMEOUT_SECONDS,
) -> dict:
    """Run a brix command and validate exit code and common JSON envelope."""
    res = subprocess.run(args, cwd=str(cwd), capture_output=True, text=True, timeout=timeout)
    if res.returncode != expected_code:
        raise SmokeCheckError(f"Command {args} exited {res.returncode} (expected {expected_code}).\nStderr: {res.stderr.strip()}")
    if not res.stdout.strip():
        raise SmokeCheckError(f"Command {args} produced empty stdout.\nStderr: {res.stderr.strip()}")
    try:
        data = json.loads(res.stdout)
    except json.JSONDecodeError as err:
        raise SmokeCheckError(f"Command {args} invalid JSON: {err}\nStdout:\n{res.stdout}") from err

    if not isinstance(data, dict):
        raise SmokeCheckError(f"Command {args} response must be a JSON object, got {type(data)}")

    for key, expected in (
        ("schema", EXPECTED_SCHEMA), ("profile", EXPECTED_PROFILE),
        ("command", expected_cmd), ("ok", expected_ok), ("status", expected_status),
    ):
        actual = data.get(key)
        if actual != expected or (key == "ok" and actual is not expected):
            raise SmokeCheckError(f"Command {args} envelope mismatch '{key}': expected {expected!r}, got {actual!r}")

    return data

def assert_identities(data: dict, program: str, context: str, snapshot: str) -> None:
    """Assert program, context, and input snapshot match expected values."""
    for key, expected in (("program", program), ("context", context), ("input_snapshot", snapshot)):
        actual = data.get(key)
        if actual != expected:
            raise SmokeCheckError(f"Identity mismatch for '{key}': expected {expected}, got {actual}")

def assert_ship_derived(data: dict) -> None:
    """Assert decision selects 'ship' with grade 'Derived' and variant 'Ship'."""
    dec = data.get("decision")
    if not isinstance(dec, dict) or dec.get("candidate") != "ship" or dec.get("grade") != "Derived":
        raise SmokeCheckError(f"Decision mismatch (expected ship @ Derived): {dec}")
    val = dec.get("value")
    if not isinstance(val, dict) or val.get("variant") != "Ship":
        raise SmokeCheckError(f"Decision value variant mismatch (expected Ship): {val}")

def assert_inputs(data: dict, expected_count: int = 3, expected_grade: str = "Derived") -> None:
    """Assert inputs list has expected count and all records have expected grade."""
    inputs = data.get("inputs")
    if not isinstance(inputs, list) or len(inputs) != expected_count or any(
        not isinstance(x, dict) or x.get("grade") != expected_grade for x in inputs
    ):
        raise SmokeCheckError(f"Inputs mismatch expected {expected_count} @ {expected_grade}: {inputs!r}")

def assert_shipping_execution(data: dict, program: str, context: str, snapshot: str) -> None:
    """Assert full shipping execution state: matching identities, valid inputs, and Ship @ Derived."""
    assert_identities(data, program, context, snapshot)
    assert_inputs(data, 3)
    assert_ship_derived(data)

def assert_candidate(data: dict, name: str, expected_status: str, expected_code: str) -> None:
    """Assert candidate status and structured reason code."""
    for cand in data.get("candidates") or []:
        if isinstance(cand, dict) and cand.get("name") == name:
            code = cand.get("reason", {}).get("code") if isinstance(cand.get("reason"), dict) else None
            if cand.get("status") != expected_status or code != expected_code:
                raise SmokeCheckError(f"Candidate '{name}' mismatch: status={cand.get('status')!r}, code={code!r}")
            return
    raise SmokeCheckError(f"Candidate '{name}' not found in candidates list")

def extract_bundle_artifact(data: dict) -> dict:
    """Extract and validate audit-bundle artifact from result."""
    for art in data.get("artifacts") or []:
        if isinstance(art, dict) and art.get("kind") == "audit-bundle":
            assert_64_hex(art.get("bundle_id"), "bundle_id")
            assert_64_hex(art.get("final_chain_digest"), "final_chain_digest")
            receipts = art.get("receipt_ids")
            if not isinstance(receipts, list) or not receipts or any(not HEX64_PATTERN.fullmatch(str(r)) for r in receipts):
                raise SmokeCheckError(f"Invalid receipt_ids in audit-bundle artifact: {receipts}")
            return art
    raise SmokeCheckError(f"No audit-bundle artifact found in {data.get('artifacts')}")

def verify_shipped_files(package_dir: Path) -> Path:
    """Verify all required files exist in the package directory and have non-zero size."""
    if not package_dir.is_dir():
        raise SmokeCheckError(f"Package directory does not exist or is not a directory: {package_dir}")
    for rel_path in REQUIRED_SHIPPED_FILES:
        target = package_dir / rel_path
        if not target.is_file() or target.stat().st_size == 0:
            raise SmokeCheckError(f"Required shipped file missing or empty: '{rel_path}' ({target})")
    bin_path = (package_dir / "brix").resolve()
    if not os.access(bin_path, os.X_OK):
        raise SmokeCheckError(f"Packaged binary is not executable: {bin_path}")
    return bin_path

def verify_binary_version(bin_path: Path, expected_version: str, cwd: Path) -> None:
    """Verify `brix --version` exits 0 and outputs expected version."""
    norm_expected = expected_version.lstrip("v")
    res = subprocess.run([str(bin_path), "--version"], cwd=str(cwd), capture_output=True, text=True, timeout=15)
    expected_output = f"brix {norm_expected}"
    if res.returncode != 0 or res.stdout.strip() != expected_output:
        raise SmokeCheckError(
            f"Binary version check failed (code {res.returncode}): expected '{expected_output}', got '{res.stdout.strip()}'"
        )


def smoke_check(package_dir: Path, expected_version: str) -> None:
    """Execute the full smoke check suite against an extracted package directory."""
    package_dir = package_dir.resolve()
    norm_version = expected_version.lstrip("v")
    print(f"=== Brix Release Package Smoke Validation ===\nPackage directory: {package_dir}\nExpected version:  {norm_version}")

    with tempfile.TemporaryDirectory(prefix="brix_smoke_") as scratch_tmp:
        scratch_dir = Path(scratch_tmp).resolve()

        # Step 1: Shipped files presence and binary permissions
        bin_path = verify_shipped_files(package_dir)
        ex = package_dir / "examples"
        ship_brix, ship_in_brix, ship_in_json = ex / "shipping.brix", ex / "shipping-input.brix", ex / "shipping-input.json"
        print("[1/9] Validating required shipped files and binary permissions... OK")

        # Step 2: Packaged binary version
        verify_binary_version(bin_path, norm_version, cwd=scratch_dir)
        print(f"[2/9] Validating packaged binary version (brix {norm_version})... OK")

        def run(subcmd: str, rest: list[str | Path], exp_status: str, exp_code: int = 0, exp_ok: bool = True) -> dict:
            return run_brix_json(
                [str(bin_path), subcmd, *(str(x) for x in rest), "--json"],
                cwd=scratch_dir, expected_cmd=subcmd, expected_status=exp_status,
                expected_code=exp_code, expected_ok=exp_ok,
            )

        # Step 3: Declaration-only check (no --input)
        json_decl = run("check", [ship_in_brix], "checked-input-contract")
        if json_decl.get("context") is not None or json_decl.get("input_snapshot") is not None:
            raise SmokeCheckError(f"Expected null context and snapshot in declaration check: {json_decl}")
        decl_program = assert_64_hex(json_decl.get("program"), "declaration program")
        print(f"[3/9] Exercising declaration-only check... OK (program pin: {decl_program})")

        # Step 4: Preflight check with inputs
        json_pref = run("check", [ship_in_brix, "--input", ship_in_json], "accepted")
        preflight_context = assert_64_hex(json_pref.get("context"), "preflight context")
        preflight_snapshot = assert_64_hex(json_pref.get("input_snapshot"), "preflight input_snapshot")
        assert_shipping_execution(json_pref, decl_program, preflight_context, preflight_snapshot)
        print("[4/9] Exercising preflight check with external inputs... OK (status: accepted, ship @ Derived)")

        # Step 5: Legacy zero-input shipping execution
        json_leg = run("run", [ship_brix], "selected")
        if json_leg.get("input_snapshot") is not None:
            raise SmokeCheckError(f"Expected null input_snapshot for zero-input program: {json_leg}")
        assert_ship_derived(json_leg)
        print("[5/9] Exercising legacy zero-input shipping... OK (status: selected, ship @ Derived)")

        # Step 6: Input-aware deliberation run
        json_run = run("run", [ship_in_brix, "--input", ship_in_json], "selected")
        assert_shipping_execution(json_run, decl_program, preflight_context, preflight_snapshot)
        print("[6/9] Exercising input-aware run... OK (status: selected, ship @ Derived)")

        # Step 7: Deliberation explanation re-derivation (why ship and whynot expedite)
        for cmd, cand, exp_st, exp_code, exp_diag in (
            ("why", "ship", "selected", "selected", "ship: selected"),
            ("whynot", "expedite", "rejected-guard-false", "guard_false@1", "expedite: rejected"),
        ):
            res = run(cmd, [ship_in_brix, "--input", ship_in_json, "--candidate", cand], "explained")
            assert_identities(res, decl_program, preflight_context, preflight_snapshot)
            assert_candidate(res, cand, exp_st, exp_code)
            diags = " ".join(res.get("diagnostics") or [])
            if exp_diag not in diags:
                raise SmokeCheckError(f"'{cmd} {cand}' diagnostics missing '{exp_diag}': {diags}")
        print("[7/9] Exercising deliberation explanation (why ship & whynot expedite)... OK")

        # Step 8: Audit deliberation to a temporary bundle
        bundle_file = scratch_dir / "shipping_input.brixaudit"
        json_audit = run("audit", [ship_in_brix, "--input", ship_in_json, "--bundle", bundle_file], "audited")
        assert_identities(json_audit, decl_program, preflight_context, preflight_snapshot)
        if not bundle_file.is_file() or bundle_file.stat().st_size == 0:
            raise SmokeCheckError(f"Audit bundle was not created or is empty: {bundle_file}")
        audit_art = extract_bundle_artifact(json_audit)
        print(f"[8/9] Producing audit input bundle... OK (bundle_id: {audit_art['bundle_id']})")

        # Step 9: Independent verify with declaration pin + negative cases
        # 9a: Positive verification
        json_ver = run("verify", ["--expect-program", decl_program, ship_in_brix, bundle_file, "--input", ship_in_json], "audit-bundle-verified")
        assert_identities(json_ver, decl_program, preflight_context, preflight_snapshot)
        verify_art = extract_bundle_artifact(json_ver)
        for field in ("bundle_id", "final_chain_digest", "receipt_ids", "count"):
            if verify_art.get(field) != audit_art.get(field):
                raise SmokeCheckError(f"Artifact field '{field}' mismatch between audit and verify")

        # 9b: Negative missing inputs
        json_miss = run("verify", ["--expect-program", decl_program, ship_in_brix, bundle_file], "rejected", 1, False)
        miss_diags = json_miss.get("diagnostics") or []
        if not any(isinstance(d, str) and d.startswith("input-missing:") for d in miss_diags):
            raise SmokeCheckError(f"Expected diagnostic starting with 'input-missing:', got: {miss_diags}")

        # 9c: Changed-input preflight check and verify rejection
        with open(ship_in_json, "r", encoding="utf-8") as f:
            altered_data = json.load(f)
        altered_data["values"]["stock"]["value"] = "99"
        altered_input_file = scratch_dir / "altered_shipping_input.json"
        with open(altered_input_file, "w", encoding="utf-8") as f:
            json.dump(altered_data, f, indent=2)

        json_alt_pref = run("check", [ship_in_brix, "--input", altered_input_file], "accepted")
        assert_inputs(json_alt_pref, 3)
        if json_alt_pref.get("program") != decl_program:
            raise SmokeCheckError(f"Altered input changed program pin: {json_alt_pref.get('program')} != {decl_program}")
        alt_context = assert_64_hex(json_alt_pref.get("context"), "altered context")
        alt_snapshot = assert_64_hex(json_alt_pref.get("input_snapshot"), "altered input_snapshot")
        if alt_context == preflight_context or alt_snapshot == preflight_snapshot:
            raise SmokeCheckError("Altered input did not produce distinct context or snapshot")

        json_alt_ver = run("verify", ["--expect-program", decl_program, ship_in_brix, bundle_file, "--input", altered_input_file], "unknown", 1, False)
        alt_diags = json_alt_ver.get("diagnostics") or []
        if not any(isinstance(d, str) and "context identity mismatch" in d and alt_context in d and preflight_context in d for d in alt_diags):
            raise SmokeCheckError(f"Expected context mismatch diagnostic containing {alt_context} and {preflight_context}, got: {alt_diags}")

        print("[9/9] Verifying audit bundle with declaration program pin and testing negative input cases... OK")

    print(f"\nSmoke check PASSED: all 9 validation stages succeeded for release package.")


def main() -> int:
    parser = argparse.ArgumentParser(description="Smoke check an extracted Brix release package directory.")
    parser.add_argument("package_dir", type=Path, help="Path to extracted release package directory")
    parser.add_argument("expected_version", type=str, help="Expected version string")
    args = parser.parse_args()

    try:
        smoke_check(args.package_dir, args.expected_version)
        return 0
    except SmokeCheckError as err:
        print(f"\nSMOKE CHECK FAILED: {err}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
