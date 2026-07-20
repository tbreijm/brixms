"""Seed ticket examples must be valid, enqueueable, and runnable specs."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from brix_builder.config import BuilderConfig
from brix_builder.model import ScriptedBackend
from brix_builder.tickets import TicketStore, TicketWorker


SEEDS_ROOT = Path(__file__).resolve().parents[1] / "tickets" / "seeds"


def _seed_files() -> list[Path]:
    return sorted(SEEDS_ROOT.glob("*.json"))


def action(value: dict) -> str:
    return json.dumps(value)


def proposal(name: str = "SeedQuery") -> str:
    source = (
        "package demo.orders @ 0.1.0\n"
        "entity Order { key id: String }\n"
        f"query {name}() -> Rel<{{ id: String }}> from {{ Order(id) }}\n"
    )
    return action(
        {
            "action": "propose_patch",
            "files": [{"path": "src/world.brix", "content": source}],
            "expected_change": {"adds": [f"query {name}"]},
            "required_validation": ["check"],
            "reason": "small scoped package change",
        }
    )


def finish(role: str) -> str:
    return action(
        {
            "action": "finish",
            "status": "validated_candidate",
            "summary": f"{role} finished",
            "evidence_ids": [],
            "residual_obligations": [],
        }
    )


def critic_script() -> list[str]:
    return [
        action({"action": "check_candidate", "reason": "challenge candidate"}),
        finish("critic"),
    ]


def test_seed_directory_is_not_empty() -> None:
    assert _seed_files(), "expected at least one seed ticket under tickets/seeds/"


@pytest.mark.parametrize("seed_path", _seed_files(), ids=lambda path: path.stem)
def test_seed_ticket_is_a_valid_enqueueable_spec(
    seed_path: Path, package_root: Path, tmp_path: Path
) -> None:
    payload = json.loads(seed_path.read_text(encoding="utf-8"))
    for required in ("brief", "package_path", "acceptance_gates", "max_iterations"):
        assert required in payload, f"{seed_path.name} is missing '{required}'"

    queue = TicketStore(
        package_root.parent / f"{package_root.name}-{seed_path.stem}-queue",
        package_root,
    )
    state = queue.enqueue(
        payload["brief"],
        ticket_id=payload.get("ticket_id"),
        package_path=payload.get("package_path", "."),
        write_allowlist=payload.get("write_allowlist"),
        acceptance_gates=payload.get("acceptance_gates"),
        max_iterations=payload.get("max_iterations", 3),
        max_actions_per_role=payload.get("max_actions_per_role", 12),
        context_tokens=payload.get("context_tokens", 8192),
        metadata=payload.get("metadata"),
    )
    assert state.status == "queued"
    assert state.spec.authority.apply_to_canonical is False
    assert state.spec.authority.arbitrary_shell is False
    assert state.spec.authority.publish is False


def test_seed_ticket_runs_to_completion_with_scripted_backends(
    package_root: Path, fake_brix: Path
) -> None:
    seed_path = SEEDS_ROOT / "orders-open-query.json"
    payload = json.loads(seed_path.read_text(encoding="utf-8"))

    queue = TicketStore(
        package_root.parent / f"{package_root.name}-seed-run-queue", package_root
    )
    state = queue.enqueue(
        payload["brief"],
        ticket_id=payload["ticket_id"],
        package_path=payload.get("package_path", "."),
        write_allowlist=payload.get("write_allowlist"),
        # Keep this deterministic run cheap; the format/test/quality/diff
        # oracles are exercised elsewhere. This seed's own acceptance_gates
        # list is still validated as a spec in the parametrized test above.
        acceptance_gates=["check"],
        max_iterations=payload.get("max_iterations", 3),
        max_actions_per_role=payload.get("max_actions_per_role", 12),
        context_tokens=payload.get("context_tokens", 8192),
        metadata=payload.get("metadata"),
    )

    worker = TicketWorker(
        queue,
        BuilderConfig(root=package_root, brix_binary=fake_brix),
        ScriptedBackend([proposal(), finish("coder")]),
        ScriptedBackend(critic_script()),
    )
    result = worker.run_to_terminal(state.spec.id)

    assert result.status == "completed"
    assert "SeedQuery" in result.candidate_overlay["src/world.brix"]
    assert result.residual_obligations == []
