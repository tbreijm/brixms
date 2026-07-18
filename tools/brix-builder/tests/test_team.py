from __future__ import annotations

import json

from brix_builder.agent import BrixBuilderTeam
from brix_builder.config import BuilderConfig
from brix_builder.model import ScriptedBackend


def action(value: dict) -> str:
    return json.dumps(value)


def test_actor_critics_cannot_override_an_unresolved_host_gate(
    config: BuilderConfig,
) -> None:
    proposed = (
        "package demo.orders @ 0.1.0\n"
        "entity Order { key id: String }\n"
        "query AllOrders() -> Rel<{ id: String }> from { Order(id) }\n"
    )
    model = ScriptedBackend(
        [
            action({"action": "project_context", "reason": "inspect package"}),
            action(
                {
                    "action": "propose_patch",
                    "files": [{"path": "src/world.brix", "content": proposed}],
                    "expected_change": {"adds": ["query AllOrders"]},
                    "required_validation": [
                        "check",
                        "format",
                        "test",
                        "quality",
                        "diff",
                        "package_build",
                    ],
                    "reason": "smallest change",
                }
            ),
            action(
                {
                    "action": "finish",
                    "status": "validated_candidate",
                    "summary": "coder done",
                    "evidence_ids": [],
                }
            ),
            action({"action": "check_candidate", "reason": "static gate"}),
            action({"action": "test_candidate", "reason": "test gate"}),
            action(
                {
                    "action": "finish",
                    "status": "validated_candidate",
                    "summary": "tester accepts",
                    "evidence_ids": [],
                }
            ),
            action({"action": "diff_candidate", "reason": "scope review"}),
            action({"action": "impact_candidate", "reason": "impact review"}),
            action(
                {
                    "action": "finish",
                    "status": "validated_candidate",
                    "summary": "reviewer accepts",
                    "evidence_ids": [],
                }
            ),
        ]
    )

    result = BrixBuilderTeam(config, model).run("Add an all-orders query")
    assert result.status == "needs_work"
    assert result.gates["diff"] == "partial"
    assert {value for key, value in result.gates.items() if key != "diff"} == {"passed"}
    assert "AllOrders" in result.diff
    assert all(
        item.role in {"coder", "tester", "reviewer", "host"} for item in result.evidence
    )
