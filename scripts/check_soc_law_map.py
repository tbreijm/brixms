#!/usr/bin/env python3
"""Validate the normative SOC semantic-law traceability manifest."""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
DEFAULT_MANIFEST = ROOT / "spec" / "conformance" / "soc-semantic-laws.json"
EXPECTED_IDS = [f"SOC-LAW-{number:02d}" for number in range(1, 13)]
VALID_STATUSES = {"enforced", "partial", "open"}

# ADR-0017 §8. A `RouteStatus::Provisional` row in `brix-semantic`'s publication
# route table is a deliberately named, reported hole in the authority fence — a
# route that exists so a live path keeps working while its soundness question is
# open. The failure mode worth guarding is not the row itself but the row
# *outliving its reason*: someone marking SOC-LAW-05 `enforced` while a
# provisional route is still in the table, and the placeholder going quiet.
#
# So couple them structurally rather than by a date or a calendar reminder: if
# the table carries a provisional row, the law must stay `partial` with a
# bounded open issue. Deterministic, offline, and it fails the `lint` job the
# moment the two disagree.
ROUTES_SOURCE = ROOT / "crates" / "brix-semantic" / "src" / "publication.rs"
PROVISIONAL_COUPLED_LAW = "SOC-LAW-05"
PROVISIONAL_ROUTE_PATTERN = re.compile(r"^\s*status:\s*RouteStatus::Provisional\s*,", re.MULTILINE)


def error(message: str) -> None:
    print(f"error: {message}", file=sys.stderr)


def repo_path(value: object, field: str, law_id: str) -> Path | None:
    if not isinstance(value, str) or not value or Path(value).is_absolute():
        error(f"{law_id}.{field} must be a non-empty repository-relative path")
        return None
    path = (ROOT / value).resolve()
    if ROOT not in path.parents:
        error(f"{law_id}.{field} escapes the repository: {value}")
        return None
    if not path.is_file():
        error(f"{law_id}.{field} does not exist: {value}")
        return None
    return path


def string_list(law: dict, field: str, law_id: str) -> bool:
    value = law.get(field)
    if not isinstance(value, list) or not value or not all(
        isinstance(item, str) and item.strip() for item in value
    ):
        error(f"{law_id}.{field} must be a non-empty list of strings")
        return False
    return True


def validate_law(law: object, document: str) -> bool:
    if not isinstance(law, dict):
        error("every laws entry must be an object")
        return False

    law_id = law.get("id", "<missing-id>")
    title = law.get("title")
    ok = True
    if not isinstance(law_id, str):
        error("law id must be a string")
        return False
    if not isinstance(title, str) or not title.strip():
        error(f"{law_id}.title must be a non-empty string")
        ok = False
    elif f"## {law_id} — {title}" not in document:
        error(f"normative document is missing exact section: {law_id} — {title}")
        ok = False

    status = law.get("status")
    if status not in VALID_STATUSES:
        error(f"{law_id}.status must be one of {sorted(VALID_STATUSES)}")
        ok = False

    for field in ("authority", "normative_anchors", "implementation_anchors"):
        ok = string_list(law, field, law_id) and ok

    failure_mode = law.get("failure_mode")
    if not isinstance(failure_mode, str) or not failure_mode.strip():
        error(f"{law_id}.failure_mode must be a non-empty string")
        ok = False

    open_issues = law.get("open_issues")
    if not isinstance(open_issues, list) or not all(
        isinstance(issue, int) and issue > 0 for issue in open_issues
    ):
        error(f"{law_id}.open_issues must be a list of positive issue numbers")
        ok = False
    elif status in {"partial", "open"} and not open_issues:
        error(f"{law_id} is {status} but has no bounded open issue")
        ok = False

    for field in ("normative_anchors", "implementation_anchors"):
        values = law.get(field, [])
        if isinstance(values, list):
            for value in values:
                if repo_path(value, field, law_id) is None:
                    ok = False

    gates = law.get("executable_gates")
    if not isinstance(gates, list) or not gates:
        error(f"{law_id}.executable_gates must be a non-empty list")
        return False
    for index, gate in enumerate(gates):
        if not isinstance(gate, dict):
            error(f"{law_id}.executable_gates[{index}] must be an object")
            ok = False
            continue
        path = repo_path(gate.get("path"), f"executable_gates[{index}].path", law_id)
        tests = gate.get("tests")
        if not isinstance(tests, list) or not tests or not all(
            isinstance(test, str) and test.strip() for test in tests
        ):
            error(f"{law_id}.executable_gates[{index}].tests must be non-empty strings")
            ok = False
            continue
        if path is None:
            ok = False
            continue
        source = path.read_text()
        for test in tests:
            definition = re.compile(
                rf"^\s*(?:pub\s+)?(?:async\s+)?fn\s+{re.escape(test)}\s*\(",
                re.MULTILINE,
            )
            if definition.search(source) is None:
                error(f"{law_id} gate test not found in {path.relative_to(ROOT)}: {test}")
                ok = False
    return ok


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--manifest",
        type=Path,
        default=DEFAULT_MANIFEST,
        help="manifest to validate (defaults to the checked-in normative map)",
    )
    parser.add_argument(
        "--routes-source",
        type=Path,
        default=ROUTES_SOURCE,
        help=(
            "publication route table to read provisional-route status from "
            "(defaults to the real one; overridden by the negative gate fixture)"
        ),
    )
    args = parser.parse_args()

    try:
        manifest = json.loads(args.manifest.read_text())
    except (OSError, json.JSONDecodeError) as exc:
        error(f"cannot read semantic-law manifest: {exc}")
        return 1

    if manifest.get("schema_version") != 1:
        error("schema_version must be 1")
        return 1

    governed_by = repo_path(manifest.get("governed_by"), "governed_by", "manifest")
    if governed_by is None:
        return 1
    document = governed_by.read_text()

    laws = manifest.get("laws")
    if not isinstance(laws, list):
        error("laws must be a list")
        return 1
    ids = [law.get("id") if isinstance(law, dict) else None for law in laws]
    if ids != EXPECTED_IDS:
        error(f"law IDs must be exactly {EXPECTED_IDS}; found {ids}")
        return 1

    ok = True
    for law in laws:
        ok = validate_law(law, document) and ok
    ok = check_provisional_route_coupling(laws, args.routes_source) and ok
    if not ok:
        return 1
    print("SOC semantic-law map is complete and all traceability anchors resolve.")
    return 0


def check_provisional_route_coupling(laws: list, routes_source: Path) -> bool:
    """A provisional publication route forces its law to stay `partial`.

    See the ROUTES_SOURCE comment above. Counts `RouteStatus::Provisional`
    occurrences in the route table's own source rather than parsing Rust: the
    marker is a single deliberate token, and a textual count is exactly as
    strong as the property being guarded (is there a provisional row at all?).
    """
    try:
        source = routes_source.read_text()
    except OSError as exc:
        error(f"cannot read the publication route table: {exc}")
        return False

    # Exclude the enum declaration and any doc comment; only route rows count.
    provisional_rows = len(PROVISIONAL_ROUTE_PATTERN.findall(source))

    law = next((l for l in laws if isinstance(l, dict) and l.get("id") == PROVISIONAL_COUPLED_LAW), None)
    if law is None:
        error(f"{PROVISIONAL_COUPLED_LAW} is missing from the manifest")
        return False

    if provisional_rows and law.get("status") != "partial":
        error(
            f"{PROVISIONAL_COUPLED_LAW} is '{law.get('status')}' but "
            f"{routes_source} still carries {provisional_rows} "
            "RouteStatus::Provisional route(s). A named hole in the authority fence "
            "cannot be declared closed while it is open (ADR-0017 §8): either retire "
            "the route or hold the law at 'partial' with a bounded open issue."
        )
        return False

    if provisional_rows and not law.get("open_issues"):
        error(
            f"{PROVISIONAL_COUPLED_LAW} has a provisional publication route but no "
            "bounded open issue tracking it (ADR-0017 §8)."
        )
        return False

    return True


if __name__ == "__main__":
    raise SystemExit(main())
