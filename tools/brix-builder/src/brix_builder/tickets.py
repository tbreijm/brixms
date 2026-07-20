"""Durable, inert ticket queue for the local two-role BrixBuilder worker."""

from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
import uuid
from dataclasses import asdict
from datetime import UTC, datetime
from pathlib import Path, PurePosixPath
from typing import Any, Literal

from pydantic import BaseModel, ConfigDict, Field, field_validator

from .actions import QualityCandidateAction, TestCandidateAction
from .agent import Evidence, EvidenceLedger, RoleReport, RoleRunner
from .config import BuilderConfig
from .model import ModelBackend
from .tools import BrixTools, CandidatePackage, ToolResult


Gate = Literal["format", "check", "test", "quality", "diff", "impact", "package_build"]
TicketStatus = Literal[
    "queued", "running", "interrupted", "needs_work", "completed", "cancelled"
]
TicketPhase = Literal["new_iteration", "coder", "critic", "gates", "done"]
TERMINAL_STATUSES = {"needs_work", "completed", "cancelled"}
# Default gates for package tickets. `test`/`quality` are executable after #78
# (empty packages pass with 0 scenarios / standard profile). They still
# fail-closed as `unavailable` when the toolchain emits BRX-TEST-0001 or
# BRX-QUALITY-0003; tickets that hit that stay `needs_work` instead of
# falsely completing.
DEFAULT_ACCEPTANCE_GATES: tuple[Gate, ...] = (
    "format",
    "check",
    "test",
    "quality",
    "package_build",
)
# `diff`/`impact` are informational-only reports (see `BrixTools.diff_candidate`
# and `impact_candidate`): the former always returns `ok=False` because no
# semantic-diff oracle exists, and the latter never claims a resolved verdict.
# Using either as an acceptance gate makes a ticket impossible to complete;
# `TicketSpec` rejects them outright instead of letting the queue spin.
INFORMATIONAL_GATES = frozenset({"diff", "impact"})
DEFAULT_WRITE_ALLOWLIST = (
    "src/*.brix",
    "src/**/*.brix",
    "tests/*.brix",
    "tests/**/*.brix",
    "brix.toml",
    "OWNER.md",
    "README.md",
)


def _now() -> str:
    return datetime.now(UTC).isoformat()


class TicketCancelled(Exception):
    """Raised internally when a concurrent `cancel` is observed mid-iteration."""

    def __init__(self, reason: str | None = None):
        super().__init__(reason or "ticket was cancelled")
        self.reason = reason


class WorkerLockError(RuntimeError):
    pass


class TicketAuthority(BaseModel):
    """Authority is deliberately non-configurable in v1."""

    model_config = ConfigDict(extra="forbid", strict=True)
    apply_to_canonical: Literal[False] = False
    arbitrary_shell: Literal[False] = False
    publish: Literal[False] = False
    production_boundaries: Literal[False] = False


class TicketSpec(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True)

    id: str = Field(min_length=3, max_length=80)
    brief: str = Field(min_length=1, max_length=20_000)
    package_path: str = "."
    write_allowlist: list[str] = Field(
        default_factory=lambda: list(DEFAULT_WRITE_ALLOWLIST),
        min_length=1,
        max_length=64,
    )
    acceptance_gates: list[Gate] = Field(
        default_factory=lambda: list(DEFAULT_ACCEPTANCE_GATES),
        min_length=1,
    )
    max_iterations: int = Field(default=3, ge=1, le=20)
    max_actions_per_role: int = Field(default=12, ge=2, le=40)
    context_tokens: int = Field(default=8192, ge=4096, le=12_288)
    authority: TicketAuthority = Field(default_factory=TicketAuthority)
    metadata: dict[str, str] = Field(default_factory=dict)

    @field_validator("id")
    @classmethod
    def safe_id(cls, value: str) -> str:
        if not re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9._-]+", value):
            raise ValueError("ticket id must be filesystem-safe")
        return value

    @field_validator("package_path")
    @classmethod
    def relative_package_path(cls, value: str) -> str:
        path = PurePosixPath(value)
        if path.is_absolute() or ".." in path.parts:
            raise ValueError("package_path must stay inside the configured workspace")
        return path.as_posix()

    @field_validator("write_allowlist")
    @classmethod
    def safe_allowlist(cls, values: list[str]) -> list[str]:
        for value in values:
            path = PurePosixPath(value)
            if path.is_absolute() or ".." in path.parts or value.startswith("."):
                raise ValueError("write allowlist patterns must be package-relative")
        return list(dict.fromkeys(values))

    @field_validator("acceptance_gates")
    @classmethod
    def unique_gates(cls, values: list[Gate]) -> list[Gate]:
        unique = list(dict.fromkeys(values))
        informational = sorted(INFORMATIONAL_GATES & set(unique))
        if informational:
            raise ValueError(
                f"{informational} are informational-only reports and can never "
                "resolve to ok=True; they cannot be used as acceptance gates "
                "(request them as ordinary role tool calls instead)"
            )
        return unique


class BaseRevision(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True)
    snapshot_sha256: str
    git_head: str | None = None


class TicketState(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True)

    spec: TicketSpec
    status: TicketStatus = "queued"
    created_at: str
    updated_at: str
    base_revision: BaseRevision
    base_files: dict[str, str]
    candidate_overlay: dict[str, str] = Field(default_factory=dict)
    expected_change: dict[str, Any] = Field(default_factory=dict)
    required_validation: list[str] = Field(default_factory=list)
    iteration: int = 0
    phase: TicketPhase = "new_iteration"
    gate_cursor: int = 0
    actions: list[dict[str, Any]] = Field(default_factory=list)
    evidence: list[dict[str, Any]] = Field(default_factory=list)
    reports: list[dict[str, Any]] = Field(default_factory=list)
    gate_results: dict[str, dict[str, Any]] = Field(default_factory=dict)
    seen_action_fingerprints: list[str] = Field(default_factory=list)
    residual_obligations: list[str] = Field(default_factory=list)
    cancel_reason: str | None = None


class TicketStore:
    """One atomic JSON state file per ticket; no source checkout is used as state."""

    def __init__(self, queue_root: Path, workspace_root: Path):
        self.queue_root = queue_root.expanduser().resolve()
        self.workspace_root = workspace_root.expanduser().resolve()
        if (
            self.queue_root == self.workspace_root
            or self.workspace_root in self.queue_root.parents
        ):
            raise ValueError("ticket queue must be outside the canonical workspace")
        self.tickets_root = self.queue_root / "tickets"
        self.tickets_root.mkdir(parents=True, exist_ok=True)

    def enqueue(
        self,
        brief: str,
        *,
        ticket_id: str | None = None,
        package_path: str = ".",
        write_allowlist: list[str] | None = None,
        acceptance_gates: list[Gate] | None = None,
        max_iterations: int = 3,
        max_actions_per_role: int = 12,
        context_tokens: int = 8192,
        metadata: dict[str, str] | None = None,
    ) -> TicketState:
        spec = TicketSpec(
            id=ticket_id or f"ticket-{uuid.uuid4().hex[:12]}",
            brief=brief,
            package_path=package_path,
            write_allowlist=write_allowlist or list(DEFAULT_WRITE_ALLOWLIST),
            acceptance_gates=acceptance_gates or list(DEFAULT_ACCEPTANCE_GATES),
            max_iterations=max_iterations,
            max_actions_per_role=max_actions_per_role,
            context_tokens=context_tokens,
            metadata=metadata or {},
        )
        path = self._package_path(spec)
        candidate = CandidatePackage(path, spec.write_allowlist)
        timestamp = _now()
        state = TicketState(
            spec=spec,
            created_at=timestamp,
            updated_at=timestamp,
            base_revision=self._base_revision(path, candidate),
            base_files=candidate.base,
        )
        if self._state_path(spec.id).exists():
            raise ValueError(f"ticket already exists: {spec.id}")
        self.save(state)
        return state

    def load(self, ticket_id: str) -> TicketState:
        path = self._state_path(ticket_id)
        if not path.is_file():
            raise KeyError(f"unknown ticket: {ticket_id}")
        return TicketState.model_validate_json(path.read_text(encoding="utf-8"))

    def save(self, state: TicketState) -> None:
        state.updated_at = _now()
        path = self._state_path(state.spec.id)
        path.parent.mkdir(parents=True, exist_ok=True)
        temporary = path.with_suffix(f".{os.getpid()}.tmp")
        temporary.write_text(state.model_dump_json(indent=2) + "\n", encoding="utf-8")
        temporary.replace(path)

    def list(self) -> list[TicketState]:
        states = [
            TicketState.model_validate_json(path.read_text(encoding="utf-8"))
            for path in sorted(self.tickets_root.glob("*/state.json"))
        ]
        return sorted(states, key=lambda state: (state.created_at, state.spec.id))

    def next_queued(self) -> TicketState | None:
        return next((state for state in self.list() if state.status == "queued"), None)

    def cancel(self, ticket_id: str, reason: str) -> TicketState:
        state = self.load(ticket_id)
        if state.status == "completed":
            raise ValueError("a completed ticket cannot be cancelled")
        state.status = "cancelled"
        state.cancel_reason = reason
        state.residual_obligations = list(
            dict.fromkeys([*state.residual_obligations, f"cancelled: {reason}"])
        )
        self.save(state)
        return state

    def resume(self, ticket_id: str) -> TicketState:
        state = self.load(ticket_id)
        if state.status in {"completed", "cancelled"}:
            raise ValueError(f"cannot resume a {state.status} ticket")
        if (
            state.phase == "new_iteration"
            and state.iteration >= state.spec.max_iterations
        ):
            raise ValueError("ticket iteration budget is exhausted")
        state.status = "queued"
        self.save(state)
        return state

    def reclaim_stale_running(self, max_age_seconds: float = 900.0) -> list[str]:
        """Requeue `running` tickets abandoned by a crashed or killed worker.

        Only a live worker process advances a ticket past `running`; if that
        process dies mid-iteration (hard kill, crash, a forced branch or
        worktree switch out from under it), the ticket's on-disk status
        stays `running` forever. `next_queued`/`run_next` never select a
        `running` ticket, so one ghost silently stalls the entire queue
        behind it. Treat any `running` ticket whose state file has not been
        touched for `max_age_seconds` as abandoned and put it back on the
        queue at its current phase (`run_iteration` resumes mid-phase
        exactly like an explicit `resume` of an interrupted ticket).
        """

        reclaimed: list[str] = []
        threshold = datetime.now(UTC).timestamp() - max_age_seconds
        for state in self.list():
            if state.status != "running":
                continue
            if datetime.fromisoformat(state.updated_at).timestamp() >= threshold:
                continue
            state.status = "queued"
            state.residual_obligations = list(
                dict.fromkeys(
                    [
                        *state.residual_obligations,
                        "reclaimed from an abandoned running worker",
                    ]
                )
            )
            self.save(state)
            reclaimed.append(state.spec.id)
        return reclaimed

    def export(self, ticket_id: str, destination: Path) -> dict[str, Any]:
        state = self.load(ticket_id)
        candidate = CandidatePackage(
            self._package_path(state.spec), state.spec.write_allowlist
        )
        candidate.restore(
            state.base_files,
            state.candidate_overlay,
            state.expected_change,
            state.required_validation,
        )
        critics = [item for item in state.reports if item.get("role") == "critic"]
        host_evidence = [item for item in state.evidence if item.get("role") == "host"]
        result = {
            "ticket": state.spec.model_dump(),
            "status": state.status,
            "base_revision": state.base_revision.model_dump(),
            "proposed_patch": candidate.diff(),
            "oracle_evidence": host_evidence,
            "critic_verdict": critics[-1] if critics else None,
            "unresolved_obligations": state.residual_obligations,
        }
        target = destination.expanduser().resolve()
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(
            json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        return result

    def package_path(self, spec: TicketSpec) -> Path:
        return self._package_path(spec)

    def _package_path(self, spec: TicketSpec) -> Path:
        path = (self.workspace_root / spec.package_path).resolve()
        if path != self.workspace_root and self.workspace_root not in path.parents:
            raise ValueError("ticket package escapes the configured workspace")
        if not path.is_dir():
            raise ValueError(f"ticket package is not a directory: {path}")
        if not (path / "brix.toml").is_file():
            raise ValueError(
                "ticket package root has no brix.toml; point --root (and "
                f"--package) at a BrixMS package, not the toolchain or "
                f"builder checkout: {path}"
            )
        return path

    def _state_path(self, ticket_id: str) -> Path:
        if not re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9._-]+", ticket_id):
            raise ValueError("invalid ticket id")
        return self.tickets_root / ticket_id / "state.json"

    @staticmethod
    def _base_revision(path: Path, candidate: CandidatePackage) -> BaseRevision:
        git_head = None
        try:
            completed = subprocess.run(
                ["git", "-C", str(path), "rev-parse", "HEAD"],
                capture_output=True,
                text=True,
                timeout=5,
                check=False,
            )
            if completed.returncode == 0:
                git_head = completed.stdout.strip()
        except (OSError, subprocess.TimeoutExpired):
            pass
        return BaseRevision(
            snapshot_sha256=_snapshot_hash(candidate.base), git_head=git_head
        )


def _pid_is_alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    except OSError:
        return False
    return True


class WorkerLock:
    """Best-effort single-writer lock for one durable ticket queue.

    Two independent `loop`/`run-ticket` processes advancing the same
    on-disk queue race to `save()` the same ticket file; the loser's write
    can silently clobber the winner's progress (or resurrect a ticket the
    winner just finished). A lock file recording the live PID prevents a
    second worker from starting against the same queue. A lock left by a
    process that is no longer running is treated as stale and reclaimed.
    """

    def __init__(self, queue_root: Path):
        self.path = queue_root / "worker.lock"

    def __enter__(self) -> "WorkerLock":
        self.path.parent.mkdir(parents=True, exist_ok=True)
        if self.path.is_file():
            try:
                held_by = int(self.path.read_text(encoding="utf-8").strip())
            except ValueError:
                held_by = None
            if held_by is not None and held_by != os.getpid() and _pid_is_alive(held_by):
                raise WorkerLockError(
                    f"another brix-builder worker (pid {held_by}) already holds "
                    f"this queue's lock: {self.path}"
                )
        self.path.write_text(str(os.getpid()), encoding="utf-8")
        return self

    def __exit__(self, *exc_info: object) -> None:
        try:
            if (
                self.path.is_file()
                and self.path.read_text(encoding="utf-8").strip() == str(os.getpid())
            ):
                self.path.unlink()
        except OSError:
            pass


class TicketWorker:
    """Run bounded coder/critic iterations; only fixed Brix tools hold authority."""

    def __init__(
        self,
        store: TicketStore,
        config: BuilderConfig,
        coder: ModelBackend,
        critic: ModelBackend,
    ):
        self.store = store
        self.config = config.normalized()
        self.coder = coder
        self.critic = critic

    def run_iteration(self, ticket_id: str) -> TicketState:
        state = self.store.load(ticket_id)
        if state.status in TERMINAL_STATUSES:
            raise ValueError(f"cannot run a {state.status} ticket")
        if state.phase == "new_iteration":
            if state.iteration >= state.spec.max_iterations:
                state.status = "needs_work"
                state.phase = "done"
                state.residual_obligations = ["ticket iteration budget exhausted"]
                self.store.save(state)
                return state
            state.iteration += 1
            state.phase = "coder"
            state.gate_cursor = 0
        state.status = "running"
        self._save_unless_cancelled(state)
        package_config = BuilderConfig(
            root=self.store.package_path(state.spec),
            brix_binary=self.config.brix_binary,
            model=self.config.model,
            endpoint=self.config.endpoint,
            context_tokens=state.spec.context_tokens,
            max_actions=state.spec.max_actions_per_role,
            repair_rounds=0,
            request_timeout_seconds=self.config.request_timeout_seconds,
        ).normalized()
        tools = BrixTools(package_config, state.spec.write_allowlist)
        tools.candidate.restore(
            state.base_files,
            state.candidate_overlay,
            state.expected_change,
            state.required_validation,
        )
        ledger = EvidenceLedger([Evidence(**item) for item in state.evidence])
        seen = set(state.seen_action_fingerprints)

        def persist_event(event: dict[str, Any]) -> None:
            fingerprint = event["fingerprint"]
            if fingerprint not in state.seen_action_fingerprints:
                state.seen_action_fingerprints.append(fingerprint)
            evidence = event["evidence"]
            state.actions.append(
                {
                    "sequence": len(state.actions) + 1,
                    "iteration": state.iteration,
                    "role": event["role"],
                    "fingerprint": fingerprint,
                    "action": event["action"],
                    "evidence_id": None if evidence is None else evidence.id,
                }
            )
            self._snapshot_runtime(state, tools, ledger)
            self._save_unless_cancelled(state)

        previous_critics = [
            report for report in state.reports if report.get("role") == "critic"
        ]
        previous = json.dumps(
            {
                "previous_host_gates": state.gate_results,
                "previous_critic_verdict": (
                    previous_critics[-1] if previous_critics else None
                ),
                "previous_residual_obligations": state.residual_obligations,
            },
            ensure_ascii=False,
            sort_keys=True,
        )
        try:
            if state.phase == "coder":
                coder_report = RoleRunner(
                    "coder",
                    self.coder,
                    tools,
                    ledger,
                    state.spec.max_actions_per_role,
                    state.spec.context_tokens,
                    persist_event,
                    seen,
                ).run(state.spec.brief, previous)
                state.reports.append(
                    {**asdict(coder_report), "iteration": state.iteration}
                )
                state.phase = "critic"
                self._snapshot_runtime(state, tools, ledger)
                self._save_unless_cancelled(state)

            coder_report = self._iteration_report(state, "coder")
            if state.phase == "critic":
                critic_report = RoleRunner(
                    "critic",
                    self.critic,
                    tools,
                    ledger,
                    state.spec.max_actions_per_role,
                    state.spec.context_tokens,
                    persist_event,
                    seen,
                ).run(state.spec.brief, coder_report.summary)
                state.reports.append(
                    {**asdict(critic_report), "iteration": state.iteration}
                )
                state.phase = "gates"
                self._snapshot_runtime(state, tools, ledger)
                self._save_unless_cancelled(state)

            critic_report = self._iteration_report(state, "critic")
            if state.phase == "gates":
                self._run_host_gates(state, tools, ledger)
            gate_results = state.gate_results
            self._snapshot_runtime(state, tools, ledger)
            failed = [
                f"{gate}: {result['status']}"
                for gate, result in gate_results.items()
                if not result["ok"]
            ]
            residual = [
                *failed,
                *coder_report.residual_obligations,
                *critic_report.residual_obligations,
            ]
            if critic_report.status != "validated_candidate":
                residual.append(f"critic reported {critic_report.status}")
            # An empty overlay only blocks completion when either (a) the
            # candidate does not already satisfy every configured
            # acceptance gate, or (b) the coder actually tried and failed to
            # get a patch accepted (a rejected propose_patch, e.g. outside
            # the write allowlist or a bad edit anchor). A ticket scoped to
            # "verify this package is already correct" -- where the coder
            # never attempts a patch and the base already passes every
            # gate -- must be able to complete without ever writing an
            # overlay; otherwise it can never resolve to anything but
            # needs_work.
            gates_all_ok = bool(gate_results) and all(
                result["ok"] for result in gate_results.values()
            )
            had_rejected_patch_attempt = any(
                item.get("role") == "coder"
                and item.get("action") == "propose_patch"
                and not item.get("ok", True)
                for item in state.evidence
            )
            if not tools.candidate.overlay and (
                had_rejected_patch_attempt or not gates_all_ok
            ):
                residual.append("coder produced no candidate patch")
            state.residual_obligations = list(dict.fromkeys(residual))
            if not state.residual_obligations:
                state.status = "completed"
                state.phase = "done"
            elif state.iteration >= state.spec.max_iterations:
                state.status = "needs_work"
                state.phase = "done"
                state.residual_obligations = list(
                    dict.fromkeys(
                        [
                            *state.residual_obligations,
                            "ticket iteration budget exhausted",
                        ]
                    )
                )
            else:
                state.status = "queued"
                state.phase = "new_iteration"
            self._save_unless_cancelled(state)
            return state
        except TicketCancelled:
            # A concurrent `cancel` already recorded the cancellation
            # durably while this iteration was in flight. Do not write our
            # stale in-memory `state` (status=running/queued) back over it
            # -- just report the durable cancelled record as-is.
            return self.store.load(ticket_id)
        except Exception:
            self._snapshot_runtime(state, tools, ledger)
            state.status = "interrupted"
            self.store.save(state)
            raise

    def run_to_terminal(self, ticket_id: str) -> TicketState:
        state = self.store.load(ticket_id)
        while state.status not in TERMINAL_STATUSES:
            if state.status == "interrupted":
                state = self.store.resume(ticket_id)
            state = self.run_iteration(ticket_id)
        return state

    def run_next(self) -> TicketState | None:
        self.store.reclaim_stale_running()
        state = self.store.next_queued()
        return None if state is None else self.run_iteration(state.spec.id)

    def _save_unless_cancelled(self, state: TicketState) -> None:
        """Persist `state` unless another process already cancelled the ticket.

        `cancel()` is an independent CLI invocation that writes
        `status=cancelled` straight to the ticket's state file while this
        worker still holds an in-memory `state` object from before the
        cancellation. Saving that stale copy (with `status=running` or
        `queued`) would silently resurrect a cancelled ticket, which is
        exactly why `cancel` used to look like it did nothing. Check the
        durable record immediately before every write instead.
        """

        try:
            on_disk = self.store.load(state.spec.id)
        except KeyError:
            on_disk = None
        if on_disk is not None and on_disk.status == "cancelled":
            raise TicketCancelled(on_disk.cancel_reason)
        self.store.save(state)

    def _run_host_gates(
        self, state: TicketState, tools: BrixTools, ledger: EvidenceLedger
    ) -> dict[str, dict[str, Any]]:
        invocations = {
            "format": tools.format_candidate,
            "check": tools.check_candidate,
            "test": lambda: tools.test_candidate(
                TestCandidateAction(action="test_candidate", reason="ticket gate")
            ),
            "quality": lambda: tools.quality_candidate(
                QualityCandidateAction(
                    action="quality_candidate", reason="ticket gate", profile="standard"
                )
            ),
            "diff": tools.diff_candidate,
            "impact": tools.impact_candidate,
            "package_build": tools.package_build,
        }
        # ``gate_cursor`` makes the gate phase itself resumable: if the worker
        # is interrupted partway through the acceptance gates, restarting the
        # iteration replays only the gates that have not yet produced host
        # evidence, instead of re-running every gate (some of which -- test,
        # quality, package_build -- can be expensive or slow).
        for index, gate in enumerate(state.spec.acceptance_gates):
            if index < state.gate_cursor:
                continue
            result: ToolResult = invocations[gate]()
            evidence = ledger.record("host", gate, result)
            state.gate_results[gate] = {
                "ok": result.ok,
                "status": result.status,
                "message": result.message,
                "data": result.data,
                "evidence_id": evidence.id,
            }
            state.actions.append(
                {
                    "sequence": len(state.actions) + 1,
                    "iteration": state.iteration,
                    "role": "host",
                    "action": {"action": gate},
                    "evidence_id": evidence.id,
                }
            )
            state.gate_cursor = index + 1
            self._snapshot_runtime(state, tools, ledger)
            self._save_unless_cancelled(state)
        return state.gate_results

    @staticmethod
    def _iteration_report(state: TicketState, role: str) -> RoleReport:
        """Reconstruct a persisted role report for the current iteration.

        On resume, a phase already recorded in ``state.reports`` (coder or
        critic) is not re-run -- its typed action history and evidence are
        already durable. This rebuilds the in-memory ``RoleReport`` so the
        gate/residual-obligation logic below can treat a freshly-run phase
        and a resumed, previously-completed phase identically.
        """

        for report in reversed(state.reports):
            if (
                report.get("role") == role
                and report.get("iteration") == state.iteration
            ):
                return RoleReport(
                    role=report["role"],
                    status=report["status"],
                    summary=report["summary"],
                    residual_obligations=list(report["residual_obligations"]),
                    evidence_ids=list(report["evidence_ids"]),
                )
        raise RuntimeError(
            f"missing {role} report for ticket {state.spec.id} iteration {state.iteration}"
        )

    @staticmethod
    def _snapshot_runtime(
        state: TicketState, tools: BrixTools, ledger: EvidenceLedger
    ) -> None:
        state.candidate_overlay = dict(tools.candidate.overlay)
        state.expected_change = dict(tools.candidate.expected_change)
        state.required_validation = list(tools.candidate.required_validation)
        state.evidence = [asdict(item) for item in ledger.items]


def _snapshot_hash(files: dict[str, str]) -> str:
    digest = hashlib.sha256()
    for rel, content in sorted(files.items()):
        digest.update(rel.encode())
        digest.update(b"\0")
        digest.update(content.encode())
        digest.update(b"\0")
    return digest.hexdigest()
