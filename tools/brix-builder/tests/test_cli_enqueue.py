"""`enqueue --from-file` merges a seed ticket spec with explicit CLI flags."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

import pytest

from brix_builder.cli import enqueue_kwargs


def _args(**overrides: object) -> argparse.Namespace:
    base = dict(
        brief=[],
        ticket_id=None,
        package_path=None,
        write_allowlist=None,
        acceptance_gates=None,
        max_iterations=None,
        max_actions=12,
        context=8192,
        from_file=None,
    )
    base.update(overrides)
    return argparse.Namespace(**base)


def _write_seed(tmp_path: Path, payload: dict) -> Path:
    path = tmp_path / "seed.json"
    path.write_text(json.dumps(payload), encoding="utf-8")
    return path


def test_from_file_alone_supplies_every_field(tmp_path: Path) -> None:
    seed = _write_seed(
        tmp_path,
        {
            "ticket_id": "seed-a",
            "brief": "add a read-only query",
            "package_path": "packages/orders",
            "write_allowlist": ["src/*.brix"],
            "acceptance_gates": ["check"],
            "max_iterations": 2,
            "max_actions_per_role": 6,
            "context_tokens": 4096,
            "metadata": {"domain": "orders"},
        },
    )
    kwargs = enqueue_kwargs(_args(from_file=seed))
    assert kwargs == {
        "brief": "add a read-only query",
        "ticket_id": "seed-a",
        "package_path": "packages/orders",
        "write_allowlist": ["src/*.brix"],
        "acceptance_gates": ["check"],
        "max_iterations": 2,
        "max_actions_per_role": 6,
        "context_tokens": 4096,
        "metadata": {"domain": "orders"},
    }


def test_explicit_cli_flags_override_the_seed_file(tmp_path: Path) -> None:
    seed = _write_seed(
        tmp_path,
        {
            "ticket_id": "seed-a",
            "brief": "add a read-only query",
            "package_path": ".",
            "acceptance_gates": ["check"],
            "max_iterations": 2,
        },
    )
    kwargs = enqueue_kwargs(
        _args(
            brief=["override", "brief"],
            ticket_id="cli-override",
            package_path="packages/other",
            acceptance_gates=["check", "package_build"],
            max_iterations=5,
            from_file=seed,
        )
    )
    assert kwargs["brief"] == "override brief"
    assert kwargs["ticket_id"] == "cli-override"
    assert kwargs["package_path"] == "packages/other"
    assert kwargs["acceptance_gates"] == ["check", "package_build"]
    assert kwargs["max_iterations"] == 5


def test_enqueue_without_brief_or_file_is_rejected() -> None:
    with pytest.raises(ValueError):
        enqueue_kwargs(_args())


def test_from_file_must_contain_a_json_object(tmp_path: Path) -> None:
    seed = tmp_path / "seed.json"
    seed.write_text("[1, 2, 3]", encoding="utf-8")
    with pytest.raises(ValueError):
        enqueue_kwargs(_args(from_file=seed))
