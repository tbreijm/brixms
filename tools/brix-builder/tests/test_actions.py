from __future__ import annotations

import pytest
from pydantic import ValidationError

from brix_builder.actions import ProposePatchAction, parse_action


def test_action_protocol_is_discriminated_and_forbids_extra_fields() -> None:
    action = parse_action('{"action":"find","query":"Order","reason":"inspect"}')
    assert action.action == "find"

    with pytest.raises(ValidationError):
        parse_action(
            '{"action":"find","query":"Order","reason":"inspect","shell":"rm"}'
        )


def test_candidate_paths_must_be_relative() -> None:
    with pytest.raises(ValidationError):
        ProposePatchAction.model_validate(
            {
                "action": "propose_patch",
                "files": [{"path": "../outside.brix", "content": ""}],
                "expected_change": {},
                "required_validation": ["check"],
                "reason": "bad path",
            }
        )
