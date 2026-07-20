"""Regression coverage for the stuck-loop failure modes fixed in this pass:

cooperative cancellation, stale-`running` reclaim, a `--root` without
`brix.toml`, an empty overlay that should still complete, `diff`/`impact`
rejected as acceptance gates, the trimmed default gate list, and the
single-writer worker lock.
"""

from __future__ import annotations

import json
import os
from pathlib import Path

import pytest
from pydantic import ValidationError

from brix_builder.config import BuilderConfig
from brix_builder.model import ModelBackend, ModelError, ScriptedBackend
from brix_builder.tickets import (
    TicketSpec,
    TicketStore,
    TicketWorker,
    WorkerLock,
    WorkerLockError,
)


def action(value: dict) -> str:
    return json.dumps(value)


def proposal(name: str = "Query", path: str = "src/world.brix") -> str:
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
    return package_root.parent / f"{package_root.name}-{name}-queue"


class CancellingBackend(ModelBackend):
    """A scripted backend whose Nth call cancels the ticket it is running.

    This reproduces the real race the fix targets: `cancel` is a separate
    CLI invocation writing straight to the ticket's state file while a
    worker still holds an in-memory copy from before the cancellation.
    """

    def __init__(
        self,
        actions: list[str],
        store: TicketStore,
        ticket_id: str,
        cancel_on_call: int,
        reason: str,
    ):
        self._actions = iter(actions)
        self._store = store
        self._ticket_id = ticket_id
        self._cancel_on_call = cancel_on_call
        self._reason = reason
        self._calls = 0

    def complete(self, messages: list[dict[str, str]]) -> str:
        self._calls += 1
        if self._calls == self._cancel_on_call:
            self._store.cancel(self._ticket_id, self._reason)
        try:
            return next(self._actions)
        except StopIteration as error:
            raise ModelError("scripted backend exhausted") from error


def test_cancel_during_iteration_is_not_clobbered_by_the_in_flight_worker(
    package_root: Path, fake_brix: Path
) -> None:
    queue = TicketStore(_queue_root(package_root, "cancel-race"), package_root)
    queue.enqueue(
        "Add a query, then get cancelled mid-iteration",
        ticket_id="cancel-race",
        acceptance_gates=["check"],
        max_iterations=1,
    )
    # The 1st complete() call returns the propose_patch action, which is
    # accepted and durably saved *before* any cancellation. The 2nd call
    # cancels the ticket out from under the worker before returning the
    # coder's `finish`, simulating an operator's `cancel` CLI invocation
    # landing between two of the worker's own actions.
    coder = CancellingBackend(
        [proposal("RaceQuery"), finish("coder")],
        queue,
        "cancel-race",
        cancel_on_call=2,
        reason="operator abort",
    )
    worker = TicketWorker(
        queue,
        BuilderConfig(root=package_root, brix_binary=fake_brix),
        coder,
        ScriptedBackend([]),
    )
    result = worker.run_iteration("cancel-race")

    assert result.status == "cancelled"
    assert result.cancel_reason == "operator abort"
    # The action accepted before the cancellation must survive -- cancelling
    # is inert with respect to the candidate, not a rollback.
    assert "RaceQuery" in result.candidate_overlay["src/world.brix"]
    # The worker's own stale in-memory save (status=running, then
    # queued/new_iteration) must never have overwritten the durable
    # cancellation record.
    assert queue.load("cancel-race").status == "cancelled"


def test_stale_running_ticket_is_reclaimed_and_can_progress(
    package_root: Path, fake_brix: Path
) -> None:
    queue = TicketStore(_queue_root(package_root, "stale"), package_root)
    queue.enqueue(
        "Add a query",
        ticket_id="stale-ghost",
        acceptance_gates=["check"],
        max_iterations=1,
    )
    state = queue.load("stale-ghost")
    state.status = "running"
    state.phase = "coder"
    state.iteration = 1
    queue.save(state)
    # `save()` always stamps `updated_at` with the current time, so a killed
    # worker's ghost has to be simulated by writing the state file directly.
    path = queue._state_path("stale-ghost")
    payload = json.loads(path.read_text(encoding="utf-8"))
    payload["updated_at"] = "2000-01-01T00:00:00+00:00"
    path.write_text(json.dumps(payload), encoding="utf-8")

    # A ghost `running` ticket is invisible to next_queued -- this is
    # exactly how one ticket can silently stall an entire queue.
    assert queue.next_queued() is None

    reclaimed = queue.reclaim_stale_running(max_age_seconds=60)
    assert reclaimed == ["stale-ghost"]
    assert queue.load("stale-ghost").status == "queued"

    worker = TicketWorker(
        queue,
        BuilderConfig(root=package_root, brix_binary=fake_brix),
        ScriptedBackend([proposal("GhostQuery"), finish("coder")]),
        ScriptedBackend(critic_script()),
    )
    # run_next() reclaims automatically too, so a plain `loop` un-sticks
    # itself without an operator ever running `reclaim` by hand.
    result = worker.run_next()
    assert result is not None
    assert result.spec.id == "stale-ghost"
    assert result.status == "completed"
    assert "GhostQuery" in result.candidate_overlay["src/world.brix"]


def test_reclaim_stale_running_leaves_a_freshly_running_ticket_alone(
    package_root: Path,
) -> None:
    queue = TicketStore(_queue_root(package_root, "fresh"), package_root)
    queue.enqueue(
        "Add a query",
        ticket_id="fresh-running",
        acceptance_gates=["check"],
        max_iterations=1,
    )
    state = queue.load("fresh-running")
    state.status = "running"
    queue.save(state)

    assert queue.reclaim_stale_running(max_age_seconds=900) == []
    assert queue.load("fresh-running").status == "running"


def test_ticket_root_without_brix_toml_is_rejected(tmp_path: Path) -> None:
    not_a_package = tmp_path / "tools-dir"
    not_a_package.mkdir()
    (not_a_package / "README.md").write_text("not a BrixMS package", encoding="utf-8")
    queue = TicketStore(tmp_path / "queue", not_a_package)

    with pytest.raises(ValueError, match="brix.toml"):
        queue.enqueue("do something", ticket_id="no-brix-toml")


def test_verify_only_ticket_completes_without_a_patch_when_gates_already_pass(
    package_root: Path, fake_brix: Path
) -> None:
    queue = TicketStore(_queue_root(package_root, "verify-only"), package_root)
    queue.enqueue(
        "The package already satisfies its gates; do not change anything",
        ticket_id="verify-only",
        acceptance_gates=["check"],
        max_iterations=1,
    )
    worker = TicketWorker(
        queue,
        BuilderConfig(root=package_root, brix_binary=fake_brix),
        # The coder never proposes a patch -- it only inspects and finishes.
        ScriptedBackend(
            [
                action({"action": "check_candidate", "reason": "already correct"}),
                finish("coder"),
            ]
        ),
        ScriptedBackend(critic_script()),
    )
    result = worker.run_to_terminal("verify-only")

    assert result.status == "completed"
    assert result.candidate_overlay == {}


def test_acceptance_gates_reject_informational_diff_and_impact() -> None:
    with pytest.raises(ValidationError, match="informational-only"):
        TicketSpec(id="bad-gate-diff", brief="x", acceptance_gates=["check", "diff"])
    with pytest.raises(ValidationError, match="informational-only"):
        TicketSpec(id="bad-gate-impact", brief="x", acceptance_gates=["impact"])


def test_enqueue_default_gates_include_executable_oracles(
    package_root: Path,
) -> None:
    queue = TicketStore(_queue_root(package_root, "defaults"), package_root)
    state = queue.enqueue("Add a query", ticket_id="defaults-ticket")
    assert state.spec.acceptance_gates == [
        "format",
        "check",
        "test",
        "quality",
        "package_build",
    ]


def test_worker_lock_refuses_a_second_concurrent_holder(tmp_path: Path) -> None:
    queue_root = tmp_path / "queue"
    queue_root.mkdir()
    lock_path = queue_root / "worker.lock"
    # A real, currently-alive PID that is not this test process -- exactly
    # what a second `loop` invocation observes while a first one is still
    # running. (The lock is deliberately re-entrant for the *same* pid, so
    # writing our own pid here would not exercise the conflict.)
    lock_path.write_text(str(os.getppid()), encoding="utf-8")

    with pytest.raises(WorkerLockError, match="already holds"):
        with WorkerLock(queue_root):
            pass


def test_worker_lock_reclaims_a_lock_left_by_a_dead_process(tmp_path: Path) -> None:
    queue_root = tmp_path / "queue"
    queue_root.mkdir()
    lock_path = queue_root / "worker.lock"
    # Far beyond any realistic pid_max, so this can never collide with a
    # live process.
    lock_path.write_text(str(2**30), encoding="utf-8")

    with WorkerLock(queue_root):
        assert lock_path.read_text(encoding="utf-8").strip() != str(2**30)
