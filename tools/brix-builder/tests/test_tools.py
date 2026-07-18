from __future__ import annotations

from brix_builder.actions import ProposePatchAction
from brix_builder.config import BuilderConfig
from brix_builder.tools import BrixTools, ToolResult


def proposal() -> ProposePatchAction:
    return ProposePatchAction.model_validate(
        {
            "action": "propose_patch",
            "files": [
                {
                    "path": "src/world.brix",
                    "content": (
                        "package demo.orders @ 0.1.0\n"
                        "entity Order { key id: String }\n"
                        "query AllOrders() -> Rel<{ id: String }> from { Order(id) }\n"
                    ),
                }
            ],
            "expected_change": {"adds": ["query AllOrders"]},
            "required_validation": ["check", "package_build"],
            "reason": "add requested query",
        }
    )


def test_candidate_is_in_memory_and_compiler_tools_use_a_temporary_tree(
    config: BuilderConfig,
) -> None:
    tools = BrixTools(config)
    live_before = (config.root / "src" / "world.brix").read_text(encoding="utf-8")

    result = tools.dispatch(proposal())
    assert result.ok
    assert "AllOrders" in tools.candidate.diff()
    assert tools.check_candidate().ok
    assert tools.package_build().ok
    assert (config.root / "src" / "world.brix").read_text(
        encoding="utf-8"
    ) == live_before


def test_allowlist_rejects_non_package_code(config: BuilderConfig) -> None:
    action = proposal().model_copy(
        update={
            "files": [
                proposal().files[0].model_copy(update={"path": "runtime/agent.py"})
            ]
        }
    )
    result = BrixTools(config).dispatch(action)
    assert not result.ok
    assert result.status == "rejected"


def test_fail_closed_test_and_quality_diagnostics_are_unavailable() -> None:
    for capability, code in (
        ("brix test", "BRX-TEST-0001"),
        ("brix quality", "BRX-QUALITY-0001"),
    ):
        raw = ToolResult(
            False,
            "failed",
            data={"exit_code": 1, "stdout": f'{{"code":"{code}"}}', "stderr": ""},
        )
        result = BrixTools._classify_unimplemented(raw, capability)
        assert not result.ok
        assert result.status == "unavailable"
        assert capability in result.message


def test_exact_edit_changes_only_the_candidate(config: BuilderConfig) -> None:
    tools = BrixTools(config)
    action = ProposePatchAction.model_validate(
        {
            "action": "propose_patch",
            "edits": [
                {
                    "path": "src/world.brix",
                    "old_text": "entity Order { key id: String }",
                    "new_text": "entity Order { key id: String; note: String }",
                }
            ],
            "expected_change": {"adds": ["Order.note"]},
            "required_validation": ["check"],
            "reason": "bounded existing-file change",
        }
    )

    result = tools.dispatch(action)
    assert result.ok
    assert result.data["exact_edits"] == 1
    assert "note: String" in tools.candidate.diff()
    assert "note: String" not in (config.root / "src" / "world.brix").read_text()
