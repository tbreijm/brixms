from __future__ import annotations

import json
from pathlib import Path

import pytest

from brix_builder.config import BuilderConfig
from brix_builder.model import ModelError, ScriptedBackend
from brix_builder.tickets import TicketStore, TicketWorker
from brix_builder.tools import CandidatePackage


def _action(value: dict) -> str:
    return json.dumps(value)


def _proposal() -> str:
    return _action(
        {
            "action": "propose_patch",
            "files": [
                {
                    "path": "src/world.brix",
                    "content": (
                        "package demo.orders @ 0.1.0\n"
                        "entity Order { key id: String }\n"
                        "query OpenOrders() -> Rel<{ id: String }> "
                        "from { Order(id) }\n"
                    ),
                }
            ],
            "expected_change": {"adds": ["query OpenOrders"]},
            "required_validation": ["check"],
            "reason": "small scoped change",
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


def _queue_root(package_root: Path, name: str) -> Path:
    return package_root.parent / f"{package_root.name}-{name}-queue"


def test_compiler_tools_never_use_canonical_package_as_working_directory(
    package_root: Path, tmp_path: Path
) -> None:
    compiler = tmp_path / "cwd-writing-brix"
    compiler.write_text(
        "#!/usr/bin/env python3\n"
        "from pathlib import Path\n"
        "Path('.compiler-was-here').write_text('mutated')\n",
        encoding="utf-8",
    )
    compiler.chmod(0o755)
    queue = TicketStore(_queue_root(package_root, "cwd"), package_root)
    queue.enqueue(
        "Add a query",
        ticket_id="cwd-boundary",
        acceptance_gates=["check"],
        max_iterations=1,
    )
    worker = TicketWorker(
        queue,
        BuilderConfig(root=package_root, brix_binary=compiler),
        ScriptedBackend([_proposal(), _finish("coder")]),
        ScriptedBackend([_finish("critic")]),
    )

    assert worker.run_to_terminal("cwd-boundary").status == "completed"
    assert not (package_root / ".compiler-was-here").exists()


def test_interruption_after_completed_coder_phase_is_resumable_at_iteration_limit(
    package_root: Path, fake_brix: Path, tmp_path: Path
) -> None:
    queue = TicketStore(_queue_root(package_root, "resume"), package_root)
    queue.enqueue(
        "Add a query",
        ticket_id="phase-resume",
        acceptance_gates=["check"],
        max_iterations=1,
    )
    worker = TicketWorker(
        queue,
        BuilderConfig(root=package_root, brix_binary=fake_brix),
        ScriptedBackend([_proposal(), _finish("coder")]),
        ScriptedBackend([]),
    )

    with pytest.raises(ModelError):
        worker.run_iteration("phase-resume")

    saved = queue.load("phase-resume")
    assert saved.status == "interrupted"
    assert saved.reports[-1]["role"] == "coder"
    assert "OpenOrders" in saved.candidate_overlay["src/world.brix"]
    assert queue.resume("phase-resume").status == "queued"


def test_candidate_snapshot_does_not_follow_source_symlinks_outside_package(
    package_root: Path, tmp_path: Path
) -> None:
    del tmp_path
    external = package_root.parent / f"{package_root.name}-outside-secret.brix"
    external.write_text("package private.secret @ 0.1.0\n", encoding="utf-8")
    (package_root / "src" / "leak.brix").symlink_to(external)

    candidate = CandidatePackage(package_root)

    assert "src/leak.brix" not in candidate.base
