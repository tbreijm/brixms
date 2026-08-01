#!/usr/bin/env python3
"""Generate and enforce the Phase A trusted-boundary dependency inventory.

The only input is `cargo metadata --format-version 1 --no-deps`.  `--check`
compares the checked-in graph artifacts with a fresh metadata run and enforces
the settled direct-production dependency rules.  A supplied `--metadata` file
is useful for the negative fixture and does not inspect the real workspace.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
OUTPUT = ROOT / "docs" / "audit" / "issue-63"
RULES = {
    "brix-semantic": {"brix-canon"},
    "soc-core": {"brix-canon", "brix-semantic"},
    "brix-kernel": {"brix-canon", "brix-semantic"},
}


def metadata(path: Path | None) -> dict:
    if path is not None:
        return json.loads(path.read_text())
    completed = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    return json.loads(completed.stdout)


def production_dependencies(package: dict) -> list[str]:
    # Cargo uses null for ordinary dependencies. Build dependencies also enter
    # the production build and therefore are deliberately included.
    return sorted(
        dependency["name"]
        for dependency in package["dependencies"]
        if dependency.get("kind") in (None, "build")
    )


def graph(data: dict) -> tuple[list[str], list[dict], dict[str, list[str]]]:
    packages = {package["name"]: package for package in data["packages"]}
    package_ids = {package["id"]: package for package in data["packages"]}
    members = sorted(
        package_ids[member]["name"] for member in data["workspace_members"]
    )
    direct = {name: production_dependencies(packages[name]) for name in members}
    workspace_names = set(members)
    edges = [
        {"from": source, "to": target}
        for source in members
        for target in direct[source]
        if target in workspace_names
    ]
    return members, edges, direct


def transitive_closure(name: str, direct: dict[str, list[str]], members: set[str]) -> list[str]:
    seen: set[str] = set()
    pending = list(direct[name])
    while pending:
        dependency = pending.pop()
        if dependency in seen:
            continue
        seen.add(dependency)
        if dependency in members:
            pending.extend(direct[dependency])
    return sorted(seen)


def artifacts(data: dict) -> tuple[str, str]:
    members, edges, direct = graph(data)
    member_set = set(members)
    inventory = {
        "schema_version": 1,
        "generated_by": "python3 scripts/check_tcb_dependencies.py --write",
        "source": "cargo metadata --format-version 1 --no-deps",
        "workspace_crates": members,
        "production_dependency_edges": edges,
        "production_dependency_closure": {
            name: transitive_closure(name, direct, member_set) for name in members
        },
    }
    dot = ["digraph brixms_workspace {", "  rankdir=LR;", "  node [shape=box];"]
    for name in members:
        dot.append(f'  "{name}";')
    for edge in edges:
        dot.append(f'  "{edge["from"]}" -> "{edge["to"]}";')
    dot.append("}")
    return json.dumps(inventory, indent=2, sort_keys=True) + "\n", "\n".join(dot) + "\n"


def violations(data: dict) -> list[str]:
    _, _, direct = graph(data)
    errors = []
    for package, allowed in RULES.items():
        actual = set(direct.get(package, []))
        forbidden = sorted(actual - allowed)
        missing = sorted(allowed - actual)
        for dependency in forbidden:
            errors.append(
                f"forbidden trusted dependency: {package} -> {dependency} "
                f"(allowed production dependencies: {', '.join(sorted(allowed))})"
            )
        for dependency in missing:
            errors.append(
                f"missing settled trusted dependency: {package} -> {dependency}"
            )
    return errors


def main() -> int:
    parser = argparse.ArgumentParser()
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--write", action="store_true", help="regenerate checked-in artifacts")
    mode.add_argument("--check", action="store_true", help="verify artifacts and dependency rules")
    parser.add_argument("--metadata", type=Path, help="use a metadata JSON fixture instead of cargo")
    args = parser.parse_args()

    data = metadata(args.metadata)
    errors = violations(data)
    if errors:
        for error in errors:
            print(f"error: {error}", file=sys.stderr)
        return 1

    edge_json, dot = artifacts(data)
    if args.write:
        OUTPUT.mkdir(parents=True, exist_ok=True)
        (OUTPUT / "workspace-dependencies.json").write_text(edge_json)
        (OUTPUT / "workspace-dependencies.dot").write_text(dot)
        return 0

    stale = []
    for path, expected in (
        (OUTPUT / "workspace-dependencies.json", edge_json),
        (OUTPUT / "workspace-dependencies.dot", dot),
    ):
        if not path.exists() or path.read_text() != expected:
            stale.append(str(path.relative_to(ROOT)))
    if stale:
        print(
            "error: dependency inventory is stale; regenerate with "
            "python3 scripts/check_tcb_dependencies.py --write: " + ", ".join(stale),
            file=sys.stderr,
        )
        return 1
    print("TCB dependency policy and checked-in inventory are current.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
