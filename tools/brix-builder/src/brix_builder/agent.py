"""Coder–tester–reviewer orchestration with a host-owned acceptance verdict."""

from __future__ import annotations

import json
import hashlib
from dataclasses import dataclass, field
from collections.abc import Callable
from typing import Any

from pydantic import ValidationError

from .actions import FinishAction, action_json_schema, parse_action
from .config import BuilderConfig
from .model import Message, ModelBackend
from .tools import BrixTools, ToolResult


MANDATORY_GATES = ("format", "check", "test", "quality", "diff", "package_build")


@dataclass(frozen=True)
class Evidence:
    id: str
    role: str
    action: str
    ok: bool
    status: str
    message: str
    data: dict[str, Any]


@dataclass
class EvidenceLedger:
    items: list[Evidence] = field(default_factory=list)

    def record(self, role: str, action: str, result: ToolResult) -> Evidence:
        evidence = Evidence(
            id=f"E{len(self.items) + 1:04d}",
            role=role,
            action=action,
            ok=result.ok,
            status=result.status,
            message=result.message,
            data=result.data,
        )
        self.items.append(evidence)
        return evidence

    def latest(self, action: str) -> Evidence | None:
        return next(
            (item for item in reversed(self.items) if item.action == action), None
        )


@dataclass(frozen=True)
class RoleReport:
    role: str
    status: str
    summary: str
    residual_obligations: list[str]
    evidence_ids: list[str]


@dataclass(frozen=True)
class TeamResult:
    status: str
    summary: str
    diff: str
    gates: dict[str, str]
    reports: list[RoleReport]
    evidence: list[Evidence]
    residual_obligations: list[str]


BASE_SYSTEM = """You are BrixBuilder, a local BrixMS package-development agent.
The compiler and resolved semantic model are authoritative. Never invent declarations,
syntax, package APIs, or diagnostic facts. Inspect before proposing the smallest coherent
change. Generated source is only a candidate. Do not claim success without evidence.
Do not expand capabilities, write another brick's state, activate a program, publish a
package, or execute a production boundary. Request semantic context when information is
missing. Prefer exact `edits` when changing an existing file; use full `files` for new or
small files. Return exactly one JSON action conforming to the supplied schema, with no markdown.
"""

ROLE_INSTRUCTIONS = {
    "coder": (
        "You are the coder. Inspect context, then propose the smallest complete "
        "BrixMS-only patch and use compiler tools to repair it. For an existing file, "
        "emit propose_patch with files:[] and edits:[{path,old_text,new_text}], where "
        "old_text is one exact source anchor and new_text retains that anchor plus the "
        "new BrixMS declaration. Total functions use "
        "`fn name(args: Type) -> Ret = expr` (not `function`/`end`/`return`). "
        "The host owns canonical formatting after a candidate checks -- do not burn "
        "actions on whitespace. Do not keep searching once the needed declaration and "
        "a neighboring syntax example are present."
    ),
    "tester": "You are the tester/critic. Do not propose or format files. Challenge the candidate using checks, tests, and package builds; report missing coverage honestly.",
    "reviewer": "You are the semantic reviewer/critic. Do not propose or format files. Inspect the diff and impact for scope, capability, ownership, and unsupported claims.",
    "critic": (
        "You are the independent critic. Do not propose, format, or modify files. "
        "Challenge the candidate with check, test, quality, diff, impact, and build "
        "evidence. Report failed or unavailable gates honestly."
    ),
}

ROLE_ACTIONS = {
    "coder": None,
    "tester": {
        "project_context",
        "find",
        "inspect",
        "read_source",
        "check_candidate",
        "test_candidate",
        "package_build",
        "finish",
    },
    "reviewer": {
        "project_context",
        "find",
        "inspect",
        "read_source",
        "diff_candidate",
        "impact_candidate",
        "quality_candidate",
        "finish",
    },
    "critic": {
        "project_context",
        "find",
        "inspect",
        "read_source",
        "check_candidate",
        "test_candidate",
        "quality_candidate",
        "diff_candidate",
        "impact_candidate",
        "package_build",
        "finish",
    },
}


class RoleRunner:
    def __init__(
        self,
        role: str,
        model: ModelBackend,
        tools: BrixTools,
        ledger: EvidenceLedger,
        max_actions: int,
        context_tokens: int,
        event_sink: Callable[[dict[str, Any]], None] | None = None,
        seen_actions: set[str] | None = None,
    ):
        self.role = role
        self.model = model
        self.tools = tools
        self.ledger = ledger
        self.max_actions = max_actions
        self.context_tokens = context_tokens
        self.event_sink = event_sink
        self.seen_actions = seen_actions if seen_actions is not None else set()

    def run(self, brief: str, critic_context: str = "") -> RoleReport:
        schema = json.dumps(action_json_schema(), separators=(",", ":"))
        messages: list[Message] = [
            {
                "role": "system",
                "content": f"{BASE_SYSTEM}\n{ROLE_INSTRUCTIONS[self.role]}\nACTION_SCHEMA={schema}",
            },
            {
                "role": "user",
                "content": self._task_prompt(brief, critic_context),
            },
        ]
        validation_failures = 0
        research_actions = 0
        research_kinds = {"project_context", "find", "inspect", "read_source"}
        for _ in range(self.max_actions):
            raw = self.model.complete(self._bounded(messages))
            try:
                action = parse_action(raw)
            except ValidationError as error:
                validation_failures += 1
                messages.extend(
                    [
                        {"role": "assistant", "content": raw},
                        {
                            "role": "user",
                            "content": f"ACTION_REJECTED: emit one valid JSON action. {error.errors(include_url=False)}",
                        },
                    ]
                )
                if validation_failures >= 3:
                    break
                continue

            allowed = ROLE_ACTIONS[self.role]
            if allowed is not None and action.action not in allowed:
                result = ToolResult(
                    False,
                    "role_forbidden",
                    message=f"{self.role} may not use {action.action}",
                )
                evidence = self.ledger.record(self.role, action.action, result)
                messages.extend(self._tool_exchange(raw, evidence, result))
                continue

            if (
                self.role == "coder"
                and action.action in research_kinds
                and research_actions >= 3
            ):
                result = ToolResult(
                    False,
                    "synthesis_required",
                    message=(
                        "coder research budget is exhausted; the next action must be "
                        "propose_patch or finish"
                    ),
                )
                evidence = self.ledger.record(self.role, action.action, result)
                messages.extend(self._tool_exchange(raw, evidence, result))
                continue

            fingerprint = self._fingerprint(
                action.model_dump(), self.role, self.tools.candidate.revision()
            )
            if fingerprint in self.seen_actions:
                result = ToolResult(
                    False,
                    "duplicate_action",
                    message="this exact typed action was already accepted for the ticket",
                )
                evidence = self.ledger.record(self.role, action.action, result)
                self._emit(action.model_dump(), fingerprint, evidence)
                messages.extend(self._tool_exchange(raw, evidence, result))
                continue
            self.seen_actions.add(fingerprint)

            if isinstance(action, FinishAction):
                known = {item.id for item in self.ledger.items}
                evidence_ids = [item for item in action.evidence_ids if item in known]
                report = RoleReport(
                    role=self.role,
                    status=action.status,
                    summary=action.summary,
                    residual_obligations=action.residual_obligations,
                    evidence_ids=evidence_ids,
                )
                self._emit(action.model_dump(), fingerprint, None)
                return report

            result = self.tools.dispatch(action)
            evidence = self.ledger.record(self.role, action.action, result)
            self._emit(action.model_dump(), fingerprint, evidence)
            messages.extend(self._tool_exchange(raw, evidence, result))
            if self.role == "coder" and action.action in research_kinds:
                research_actions += 1
                if research_actions == 3:
                    messages.append(
                        {
                            "role": "user",
                            "content": (
                                "CODER_PHASE_CHANGE: research is complete. Your next action "
                                "must be propose_patch using a small exact edit, or finish "
                                "blocked with a concrete residual obligation."
                            ),
                        }
                    )

        return RoleReport(
            role=self.role,
            status="blocked",
            summary=f"{self.role} did not produce a valid finish action within its action budget",
            residual_obligations=["role action budget exhausted"],
            evidence_ids=[],
        )

    @staticmethod
    def _fingerprint(action: dict[str, Any], role: str, candidate_revision: str) -> str:
        normalized = dict(action)
        normalized.pop("reason", None)
        payload = json.dumps(
            {
                "role": role,
                "candidate_revision": candidate_revision,
                "action": normalized,
            },
            sort_keys=True,
            separators=(",", ":"),
        )
        return hashlib.sha256(payload.encode()).hexdigest()

    def _emit(
        self,
        action: dict[str, Any],
        fingerprint: str,
        evidence: Evidence | None,
    ) -> None:
        if self.event_sink is None:
            return
        self.event_sink(
            {
                "role": self.role,
                "action": action,
                "fingerprint": fingerprint,
                "evidence": None if evidence is None else evidence,
            }
        )

    def _task_prompt(self, brief: str, critic_context: str) -> str:
        candidate = self.tools.candidate.diff()
        bootstrap = json.dumps(
            self.tools.bootstrap_context(brief), ensure_ascii=False, sort_keys=True
        )
        return (
            f"PACKAGE_BRIEF:\n{brief}\n\n"
            f"RETRIEVED_COMPILER_CONTEXT:\n{bootstrap}\n\n"
            f"CURRENT_CANDIDATE_DIFF:\n{candidate or '(none)'}\n\n"
            f"CRITIC_CONTEXT:\n{critic_context or '(none)'}"
        )

    def _bounded(self, messages: list[Message]) -> list[Message]:
        # A conservative character proxy keeps working context near the configured
        # budget even with tokenizers that are loaded only inside the backend.
        # Reserve roughly 2K tokens for the one-action response so prompt plus
        # completion stays inside the configured working-context ceiling.
        limit = max(1024, self.context_tokens - 2048) * 4
        if sum(len(message["content"]) for message in messages) <= limit:
            return messages
        kept = [messages[0]]
        remaining = limit - len(messages[0]["content"])
        tail = []
        for message in reversed(messages[1:]):
            if len(message["content"]) > remaining:
                break
            tail.append(message)
            remaining -= len(message["content"])
        kept.extend(reversed(tail))
        return kept

    @staticmethod
    def _tool_exchange(
        raw: str, evidence: Evidence, result: ToolResult
    ) -> list[Message]:
        return [
            {"role": "assistant", "content": raw},
            {
                "role": "user",
                "content": f"TOOL_RESULT {evidence.id}: {result.model_payload()}",
            },
        ]


class BrixBuilderTeam:
    def __init__(self, config: BuilderConfig, model: ModelBackend):
        self.config = config.normalized()
        self.model = model
        self.tools = BrixTools(self.config)
        self.ledger = EvidenceLedger()

    def run(self, brief: str) -> TeamResult:
        reports = [self._role("coder").run(brief)]
        if not self.tools.candidate.overlay:
            return self._result(reports, ["coder produced no candidate patch"])

        reports.append(self._role("tester").run(brief, reports[-1].summary))
        reports.append(
            self._role("reviewer").run(brief, self._reports_context(reports))
        )

        for round_number in range(1, self.config.repair_rounds + 1):
            check = self.tools.check_candidate()
            check_evidence = self.ledger.record("host", "check_candidate", check)
            build = self.tools.package_build()
            build_evidence = self.ledger.record("host", "package_build", build)
            if check.ok and build.ok:
                break
            critic_context = (
                f"REPAIR_ROUND={round_number}\n"
                f"{self._reports_context(reports[-2:])}\n"
                f"{check_evidence.id} check: {check.model_payload()}\n"
                f"{build_evidence.id} package_build: {build.model_payload()}"
            )
            reports.append(self._role("coder").run(brief, critic_context))
            reports.append(self._role("tester").run(brief, reports[-1].summary))
            reports.append(
                self._role("reviewer").run(brief, self._reports_context(reports[-2:]))
            )

        self._authoritative_gates()
        failed = [
            f"{gate}: {self._gate_status(gate)}"
            for gate in MANDATORY_GATES
            if self._gate_status(gate) != "passed"
        ]
        return self._result(reports, failed)

    def _role(self, role: str) -> RoleRunner:
        return RoleRunner(
            role,
            self.model,
            self.tools,
            self.ledger,
            self.config.max_actions,
            self.config.context_tokens,
        )

    def _authoritative_gates(self) -> None:
        actions = (
            ("format_candidate", self.tools.format_candidate),
            ("check_candidate", self.tools.check_candidate),
            ("test_candidate", lambda: self.tools.test_candidate(_test_action())),
            (
                "quality_candidate",
                lambda: self.tools.quality_candidate(_quality_action()),
            ),
            ("diff_candidate", self.tools.diff_candidate),
            ("impact_candidate", self.tools.impact_candidate),
            ("package_build", self.tools.package_build),
        )
        for action, invoke in actions:
            self.ledger.record("host", action, invoke())

    def _gate_status(self, gate: str) -> str:
        evidence = self.ledger.latest(
            f"{gate}_candidate" if gate not in {"package_build"} else gate
        )
        if evidence is None:
            return "missing"
        return "passed" if evidence.ok else evidence.status

    def _result(self, reports: list[RoleReport], residual: list[str]) -> TeamResult:
        gates = {gate: self._gate_status(gate) for gate in MANDATORY_GATES}
        role_residual = [
            item for report in reports for item in report.residual_obligations
        ]
        role_rejections = [
            f"{report.role} reported {report.status}"
            for report in reports
            if report.status != "validated_candidate"
        ]
        all_residual = list(
            dict.fromkeys([*residual, *role_residual, *role_rejections])
        )
        status = (
            "validated_candidate"
            if not all_residual and all(value == "passed" for value in gates.values())
            else "needs_work"
        )
        summary = (
            "candidate passed every mandatory host gate"
            if status == "validated_candidate"
            else "candidate remains inert; one or more mandatory gates are unresolved"
        )
        return TeamResult(
            status=status,
            summary=summary,
            diff=self.tools.candidate.diff(),
            gates=gates,
            reports=reports,
            evidence=list(self.ledger.items),
            residual_obligations=all_residual,
        )

    @staticmethod
    def _reports_context(reports: list[RoleReport]) -> str:
        return "\n".join(
            f"{report.role}: {report.status}: {report.summary}" for report in reports
        )


def _test_action():
    from .actions import TestCandidateAction

    return TestCandidateAction(
        action="test_candidate", reason="mandatory host acceptance gate"
    )


def _quality_action():
    from .actions import QualityCandidateAction

    return QualityCandidateAction(
        action="quality_candidate",
        reason="mandatory host acceptance gate",
        profile="standard",
    )
