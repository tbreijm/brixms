from __future__ import annotations

import json
from pathlib import Path

import pytest

from brix_builder.actions import ExpectedChange, ProposePatchAction
from brix_builder.agent import EvidenceLedger, RoleRunner
from brix_builder.config import BuilderConfig
from brix_builder.model import ModelError, ScriptedBackend
from brix_builder.tickets import TicketSpec, TicketStore, TicketWorker
from brix_builder.tools import BrixTools


def action(value: dict) -> str:
    return json.dumps(value)


def proposal(name: str = "OpenOrders", path: str = "src/world.brix") -> str:
    source = (
        "package demo.orders @ 0.1.0\n"
        "entity Order { key id: String }\n"
        f"query {name}() -> Rel<{{ id: String }}> from {{ Order(id) }}\n"
    )
    return action(
        {
            "action": "propose_patch",
            "files": [{"path": path, "content": source}],
            "expected_change": {"adds": [f"query {name}"]},
            "required_validation": ["check"],
            "reason": "small scoped package change",
        }
    )


def finish(role: str, status: str = "validated_candidate") -> str:
    return action(
        {
            "action": "finish",
            "status": status,
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


def _queue_root(package_root: Path, name: str) -> Path:
    # The durable queue must live outside the canonical workspace it
    # supervises (TicketStore enforces this); use a sibling directory keyed
    # by the package root's unique tmp_path name so parallel tests never
    # collide on a shared parent.
    return package_root.parent / f"{package_root.name}-{name}-queue"


def test_two_tickets_are_independent_and_export_replay_evidence(
    package_root: Path, fake_brix: Path, tmp_path: Path
) -> None:
    queue = TicketStore(_queue_root(package_root, "two-tickets"), package_root)
    original = (package_root / "src/world.brix").read_text(encoding="utf-8")
    for ticket_id in ("core-data-1", "core-data-2"):
        queue.enqueue(
            f"Add a pure query for {ticket_id}",
            ticket_id=ticket_id,
            acceptance_gates=["check"],
            max_iterations=1,
        )

    coder = ScriptedBackend(
        [
            proposal("FirstQuery"),
            finish("coder"),
            proposal("SecondQuery"),
            finish("coder"),
        ]
    )
    critic = ScriptedBackend([*critic_script(), *critic_script()])
    worker = TicketWorker(
        queue,
        BuilderConfig(root=package_root, brix_binary=fake_brix),
        coder,
        critic,
    )
    first = worker.run_to_terminal("core-data-1")
    second = worker.run_to_terminal("core-data-2")

    assert first.status == second.status == "completed"
    assert "FirstQuery" in first.candidate_overlay["src/world.brix"]
    assert "SecondQuery" in second.candidate_overlay["src/world.brix"]
    assert first.candidate_overlay != second.candidate_overlay
    assert (package_root / "src/world.brix").read_text(encoding="utf-8") == original
    assert len(first.base_revision.snapshot_sha256) == 64

    destination = tmp_path / "exports" / "core-data-1.json"
    exported = queue.export("core-data-1", destination)
    assert "FirstQuery" in exported["proposed_patch"]
    assert exported["oracle_evidence"][-1]["role"] == "host"
    assert exported["critic_verdict"]["status"] == "validated_candidate"
    assert exported["base_revision"] == first.base_revision.model_dump()


def test_interrupted_ticket_resumes_from_persisted_candidate_without_reproposal(
    package_root: Path, fake_brix: Path, tmp_path: Path
) -> None:
    queue = TicketStore(_queue_root(package_root, "resume"), package_root)
    queue.enqueue(
        "Add a resumable query",
        ticket_id="resume-core-data",
        acceptance_gates=["check"],
        max_iterations=2,
    )
    interrupted = TicketWorker(
        queue,
        BuilderConfig(root=package_root, brix_binary=fake_brix),
        ScriptedBackend([proposal("ResumedQuery")]),
        ScriptedBackend([]),
    )
    with pytest.raises(ModelError):
        interrupted.run_iteration("resume-core-data")

    saved = queue.load("resume-core-data")
    assert saved.status == "interrupted"
    assert "ResumedQuery" in saved.candidate_overlay["src/world.brix"]
    proposal_count = sum(
        item["action"]["action"] == "propose_patch" for item in saved.actions
    )
    assert proposal_count == 1

    queue.resume("resume-core-data")
    resumed = TicketWorker(
        queue,
        BuilderConfig(root=package_root, brix_binary=fake_brix),
        ScriptedBackend([finish("coder")]),
        ScriptedBackend(critic_script()),
    ).run_to_terminal("resume-core-data")
    assert resumed.status == "completed"
    assert "ResumedQuery" in resumed.candidate_overlay["src/world.brix"]
    assert (
        sum(item["action"]["action"] == "propose_patch" for item in resumed.actions)
        == 1
    )


def test_failed_oracle_evidence_is_fed_into_next_coder_turn(
    package_root: Path, tmp_path: Path
) -> None:
    compiler = tmp_path / "diagnostic-brix"
    compiler.write_text(
        "#!/usr/bin/env python3\n"
        "import pathlib, sys\n"
        "operand = pathlib.Path(sys.argv[2])\n"
        "files = list(operand.rglob('*.brix')) if operand.is_dir() else [operand]\n"
        "source = ''.join(path.read_text() for path in files)\n"
        "if 'BrokenQuery' in source:\n"
        "    print('BRX-DEMO-0001: broken query', file=sys.stderr)\n"
        "    raise SystemExit(1)\n"
        "print('ok')\n",
        encoding="utf-8",
    )
    compiler.chmod(0o755)
    queue = TicketStore(_queue_root(package_root, "repair"), package_root)
    queue.enqueue(
        "Repair from compiler evidence",
        ticket_id="repair-loop",
        acceptance_gates=["check"],
        max_iterations=2,
    )
    coder = ScriptedBackend(
        [
            proposal("BrokenQuery"),
            finish("coder"),
            proposal("FixedQuery"),
            finish("coder"),
        ]
    )
    worker = TicketWorker(
        queue,
        BuilderConfig(root=package_root, brix_binary=compiler),
        coder,
        ScriptedBackend([*critic_script(), *critic_script()]),
    )
    result = worker.run_to_terminal("repair-loop")
    assert result.status == "completed"
    second_iteration_prompt = coder.messages[2][1]["content"]
    assert "previous_host_gates" in second_iteration_prompt
    assert "BRX-DEMO-0001: broken query" in second_iteration_prompt
    assert "FixedQuery" in result.candidate_overlay["src/world.brix"]


def test_ticket_that_never_converges_stops_at_its_declared_budget(
    package_root: Path, tmp_path: Path
) -> None:
    """A candidate that never passes its gate must not loop forever.

    The worker keeps advancing iterations while the compiler keeps
    rejecting the same broken candidate; once max_iterations is spent it
    must stop deterministically at ``needs_work`` with the exhausted-budget
    obligation recorded, rather than requeue indefinitely.
    """

    compiler = tmp_path / "always-rejecting-brix"
    compiler.write_text(
        "#!/usr/bin/env python3\n"
        "import sys\n"
        "print('BRX-DEMO-0002: always broken', file=sys.stderr)\n"
        "raise SystemExit(1)\n",
        encoding="utf-8",
    )
    compiler.chmod(0o755)
    queue = TicketStore(_queue_root(package_root, "never-converges"), package_root)
    queue.enqueue(
        "Repair a candidate the compiler always rejects",
        ticket_id="never-converges",
        acceptance_gates=["check"],
        max_iterations=2,
    )
    coder = ScriptedBackend(
        [
            proposal("StillBroken"),
            finish("coder"),
            proposal("StillBrokenAgain"),
            finish("coder"),
        ]
    )
    worker = TicketWorker(
        queue,
        BuilderConfig(root=package_root, brix_binary=compiler),
        coder,
        ScriptedBackend([*critic_script(), *critic_script()]),
    )
    result = worker.run_to_terminal("never-converges")

    assert result.status == "needs_work"
    assert result.iteration == 2
    assert "ticket iteration budget exhausted" in result.residual_obligations
    assert any(item.startswith("check:") for item in result.residual_obligations)
    # A terminal ticket cannot be silently re-driven; the queue will not
    # hand it back out via next_queued/run_next.
    assert queue.next_queued() is None


def test_write_allowlist_rejects_out_of_scope_candidate(
    package_root: Path, fake_brix: Path, tmp_path: Path
) -> None:
    queue = TicketStore(_queue_root(package_root, "scoped"), package_root)
    original = package_root / "README.md"
    queue.enqueue(
        "Only change the package source",
        ticket_id="scoped-ticket",
        write_allowlist=["src/world.brix"],
        acceptance_gates=["check"],
        max_iterations=1,
    )
    worker = TicketWorker(
        queue,
        BuilderConfig(root=package_root, brix_binary=fake_brix),
        ScriptedBackend([proposal("Escaped", "README.md"), finish("coder")]),
        ScriptedBackend(critic_script()),
    )
    result = worker.run_to_terminal("scoped-ticket")
    assert result.status == "needs_work"
    rejected = [item for item in result.evidence if item["status"] == "rejected"]
    assert rejected and "allowlist" in rejected[0]["message"]
    assert not original.exists()
    assert not result.candidate_overlay
    assert result.spec.authority.apply_to_canonical is False
    assert result.spec.authority.arbitrary_shell is False
    assert result.spec.authority.publish is False


def test_duplicate_detection_is_scoped_to_role_and_candidate_revision(
    config: BuilderConfig,
) -> None:
    tools = BrixTools(config)
    ledger = EvidenceLedger()
    seen: set[str] = set()
    repeated = ScriptedBackend(
        [
            action({"action": "check_candidate", "reason": "first"}),
            action({"action": "check_candidate", "reason": "no progress"}),
            finish("critic"),
        ]
    )
    RoleRunner("critic", repeated, tools, ledger, 4, 8192, seen_actions=seen).run(
        "review"
    )
    assert [item.status for item in ledger.items] == ["passed", "duplicate_action"]

    patch = ProposePatchAction(
        action="propose_patch",
        files=[],
        edits=[
            {
                "path": "src/world.brix",
                "old_text": "entity Order { key id: String }",
                "new_text": (
                    "entity Order { key id: String }\n"
                    "query Added() -> Rel<{ id: String }> from { Order(id) }"
                ),
            }
        ],
        expected_change=ExpectedChange(adds=["query Added"]),
        required_validation=["check"],
        reason="change candidate revision",
    )
    assert tools.candidate.propose(patch).ok
    second = ScriptedBackend(
        [
            action({"action": "check_candidate", "reason": "candidate changed"}),
            finish("critic"),
        ]
    )
    RoleRunner("critic", second, tools, ledger, 3, 8192, seen_actions=seen).run(
        "review changed candidate"
    )
    assert ledger.items[-1].status == "passed"


def test_ticket_paths_cannot_escape_workspace() -> None:
    with pytest.raises(ValueError):
        TicketSpec(id="bad-path", brief="bad", package_path="../outside")
