"""Sidecar NDJSON protocol exposes the #79 ticket-loop workflows."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from brix_builder.cli import handle_sidecar_request
from brix_builder.config import BuilderConfig
from brix_builder.model import ScriptedBackend
from brix_builder.tickets import TicketStore


def _action(value: dict) -> str:
    return json.dumps(value)


def _proposal(name: str = "OpenOrders") -> str:
    source = (
        "package demo.orders @ 0.1.0\n"
        "entity Order { key id: String }\n"
        f"query {name}() -> Rel<{{ id: String }}> from {{ Order(id) }}\n"
    )
    return _action(
        {
            "action": "propose_patch",
            "files": [{"path": "src/world.brix", "content": source}],
            "expected_change": {"adds": [f"query {name}"]},
            "required_validation": ["check"],
            "reason": "scoped package change",
        }
    )


def _finish(role: str) -> str:
    return _action(
        {
            "action": "finish",
            "status": "validated_candidate",
            "summary": f"{role} finished",
            "evidence_ids": [],
            "residual_obligations": [],
        }
    )


def _queue(package_root: Path, name: str) -> TicketStore:
    return TicketStore(package_root.parent / f"{package_root.name}-{name}-queue", package_root)


def test_legacy_brief_one_shot_still_works(
    package_root: Path, fake_brix: Path
) -> None:
    config = BuilderConfig(
        root=package_root, brix_binary=fake_brix, max_actions=8, repair_rounds=0
    )
    store = _queue(package_root, "legacy")
    team_backend = ScriptedBackend(
        [
            _proposal(),
            _finish("coder"),
            _action({"action": "check_candidate", "reason": "test"}),
            _finish("tester"),
            _action({"action": "diff_candidate", "reason": "review"}),
            _finish("reviewer"),
        ]
    )
    response = handle_sidecar_request(
        {"brief": "Add a query"},
        config,
        team_backend,
        team_backend,
        store,
    )
    assert response["ok"] is True
    assert response["command"] == "run"
    assert "OpenOrders" in response["result"]["diff"]
    assert response["result"]["status"] in {
        "validated_candidate",
        "needs_work",
        "blocked",
    }


def test_sidecar_enqueue_inspect_run_export_cancel_resume(
    package_root: Path, fake_brix: Path, tmp_path: Path
) -> None:
    config = BuilderConfig(root=package_root, brix_binary=fake_brix, max_actions=8)
    store = _queue(package_root, "sidecar")
    coder = ScriptedBackend([_proposal("SidecarQuery"), _finish("coder")])
    critic = ScriptedBackend(
        [
            _action({"action": "check_candidate", "reason": "challenge"}),
            _finish("critic"),
        ]
    )

    enqueued = handle_sidecar_request(
        {
            "command": "enqueue",
            "ticket_id": "sidecar-1",
            "brief": "Add SidecarQuery",
            "acceptance_gates": ["check"],
            "max_iterations": 1,
        },
        config,
        coder,
        critic,
        store,
    )
    assert enqueued["ok"] is True
    assert enqueued["ticket"]["status"] == "queued"
    assert enqueued["ticket"]["spec"]["id"] == "sidecar-1"

    listed = handle_sidecar_request({"command": "tickets"}, config, coder, critic, store)
    assert listed["ok"] is True
    assert len(listed["tickets"]) == 1

    status = handle_sidecar_request({"command": "status"}, config, coder, critic, store)
    assert status["next_queued"] == "sidecar-1"
    assert status["counts"]["queued"] == 1

    inspected = handle_sidecar_request(
        {"command": "inspect-ticket", "ticket_id": "sidecar-1"},
        config,
        coder,
        critic,
        store,
    )
    assert inspected["ticket"]["spec"]["brief"] == "Add SidecarQuery"

    ran = handle_sidecar_request(
        {"command": "run-ticket", "ticket_id": "sidecar-1"},
        config,
        coder,
        critic,
        store,
    )
    assert ran["ok"] is True
    assert ran["ticket"]["status"] == "completed"
    assert "SidecarQuery" in ran["ticket"]["candidate_overlay"]["src/world.brix"]

    exported = handle_sidecar_request(
        {"command": "export-proposal", "ticket_id": "sidecar-1"},
        config,
        coder,
        critic,
        store,
    )
    assert exported["ok"] is True
    proposal = exported["proposal"]
    assert "SidecarQuery" in proposal["proposed_patch"]
    assert proposal["critic_verdict"]["status"] == "validated_candidate"
    assert proposal["base_revision"]["snapshot_sha256"]
    assert proposal["unresolved_obligations"] == []

    destination = tmp_path / "out" / "proposal.json"
    written = handle_sidecar_request(
        {
            "command": "export-proposal",
            "ticket_id": "sidecar-1",
            "destination": str(destination),
        },
        config,
        coder,
        critic,
        store,
    )
    assert written["proposal"]["destination"] == str(destination.resolve())
    assert destination.is_file()

    # Second ticket exercises cancel + resume without mutating the package.
    original = (package_root / "src/world.brix").read_text(encoding="utf-8")
    handle_sidecar_request(
        {
            "command": "enqueue",
            "ticket_id": "sidecar-2",
            "brief": "will cancel",
            "acceptance_gates": ["check"],
            "max_iterations": 2,
        },
        config,
        coder,
        critic,
        store,
    )
    cancelled = handle_sidecar_request(
        {
            "command": "cancel",
            "ticket_id": "sidecar-2",
            "reason": "operator stopped",
        },
        config,
        coder,
        critic,
        store,
    )
    assert cancelled["ticket"]["status"] == "cancelled"
    assert (package_root / "src/world.brix").read_text(encoding="utf-8") == original

    handle_sidecar_request(
        {
            "command": "enqueue",
            "ticket_id": "sidecar-3",
            "brief": "interruptible",
            "acceptance_gates": ["check"],
            "max_iterations": 2,
        },
        config,
        ScriptedBackend([_proposal("ResumeQuery"), _finish("coder")]),
        ScriptedBackend(
            [
                _action({"action": "check_candidate", "reason": "challenge"}),
                _finish("critic"),
            ]
        ),
        store,
    )
    # Mark interrupted via store API, then resume through sidecar.
    state = store.load("sidecar-3")
    state.status = "interrupted"
    state.phase = "coder"
    state.iteration = 0
    store.save(state)
    resumed = handle_sidecar_request(
        {"command": "resume", "ticket_id": "sidecar-3"},
        config,
        ScriptedBackend([_proposal("ResumeQuery"), _finish("coder")]),
        ScriptedBackend(
            [
                _action({"action": "check_candidate", "reason": "challenge"}),
                _finish("critic"),
            ]
        ),
        store,
    )
    assert resumed["ticket"]["status"] == "queued"


def test_sidecar_loop_drains_queued_tickets(
    package_root: Path, fake_brix: Path
) -> None:
    config = BuilderConfig(root=package_root, brix_binary=fake_brix, max_actions=8)
    store = _queue(package_root, "loop")
    for ticket_id, query in (("loop-a", "LoopA"), ("loop-b", "LoopB")):
        store.enqueue(
            f"Add {query}",
            ticket_id=ticket_id,
            acceptance_gates=["check"],
            max_iterations=1,
        )
    coder = ScriptedBackend(
        [
            _proposal("LoopA"),
            _finish("coder"),
            _proposal("LoopB"),
            _finish("coder"),
        ]
    )
    critic = ScriptedBackend(
        [
            _action({"action": "check_candidate", "reason": "challenge"}),
            _finish("critic"),
            _action({"action": "check_candidate", "reason": "challenge"}),
            _finish("critic"),
        ]
    )
    response = handle_sidecar_request(
        {"command": "loop"},
        config,
        coder,
        critic,
        store,
    )
    assert response["ok"] is True
    assert response["count"] == 2
    assert {item["spec"]["id"] for item in response["processed"]} == {
        "loop-a",
        "loop-b",
    }
    assert all(item["status"] == "completed" for item in response["processed"])


def test_sidecar_rejects_unknown_command(
    package_root: Path, fake_brix: Path
) -> None:
    config = BuilderConfig(root=package_root, brix_binary=fake_brix)
    store = _queue(package_root, "bad")
    backend = ScriptedBackend([])
    with pytest.raises(ValueError, match="unsupported sidecar command"):
        handle_sidecar_request(
            {"command": "apply"},
            config,
            backend,
            backend,
            store,
        )
