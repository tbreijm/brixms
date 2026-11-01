"""Immutable per-request evidence policy and trusted-host transport tests."""

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from brix import BrixClient, BrixError, RequestEvidencePolicy, path  # noqa: E402


def sample_policy(program="a" * 64, claim="b" * 64, tenant="tenant-a"):
    return {
        "schema": "brix.request-evidence-policy@1",
        "expected_program": program,
        "runtime_claim_ids": [claim],
        "documentary_digest": "c" * 64,
        "verifier": "brix.world.decision-evidence-replay@1",
        "profile": "brix.l3.finite-decision@1",
        "namespace": "shipping",
        "tenant": tenant,
        "principal": "shipping-service",
        "limits": {
            "max_input_files": 16,
            "max_input_file_bytes": 1_048_576,
            "max_input_aggregate_bytes": 4_194_304,
            "max_runtime_claims": 64,
            "max_evidence_artifact_bytes": 16_777_216,
            "max_evidence_aggregate_bytes": 67_108_864,
            "max_evidence_bundle_bytes": 8_388_608,
            "max_evidence_proof_nodes": 512,
            "max_evidence_node_bytes": 65_536,
            "max_replay_work": 500_000,
            "context_bytes": 65_536,
            "context_artifact_bytes": 65_536,
            "context_depth": 128,
        },
    }


def test_policy_is_immutable_and_detached_from_input():
    original = sample_policy()
    policy = RequestEvidencePolicy(original)
    original["tenant"] = "tenant-b"
    returned = policy.to_json_value()
    returned["runtime_claim_ids"].clear()
    assert policy.to_json_value()["tenant"] == "tenant-a"
    assert policy.to_json_value()["runtime_claim_ids"] == ["b" * 64]
    with pytest.raises(AttributeError, match="immutable"):
        policy._canonical_json = "{}"


@pytest.mark.parametrize(
    "change, message",
    [
        (lambda p: p.update(extra=True), "unknown extra"),
        (lambda p: p.update(schema="brix.request-evidence-policy@2"), "unsupported"),
        (lambda p: p.update(expected_program="evidence-selected"), "64-character hex"),
        (lambda p: p["runtime_claim_ids"].append("b" * 64), "duplicate"),
        (lambda p: p.update(verifier="caller-verifier"), "unsupported request evidence verifier"),
        (lambda p: p["limits"].update(max_replay_work=500_001), "may not exceed"),
        (lambda p: p["limits"].update(max_runtime_claims=0), "exceeds limits.max_runtime_claims"),
    ],
)
def test_policy_rejects_unsupported_or_widened_values(change, message):
    value = sample_policy()
    change(value)
    with pytest.raises(ValueError, match=message):
        RequestEvidencePolicy(value)


def test_untrusted_client_rejects_per_request_context_locally():
    client = object.__new__(BrixClient)
    client._trusted_host = False
    with pytest.raises(BrixError, match=r"requires BrixClient\(trusted_host=True\)"):
        client.run(program=path("shipping.brix"), request_context=RequestEvidencePolicy(sample_policy()))


def test_same_client_sends_each_callers_context_separately():
    client = object.__new__(BrixClient)
    client._trusted_host = True
    sent = []

    def capture(method, params, timeout=None):
        sent.append((method, params))
        return {"ok": True}

    client.result = capture
    first = RequestEvidencePolicy(sample_policy(tenant="tenant-a"))
    second = RequestEvidencePolicy(sample_policy(program="d" * 64, claim="e" * 64, tenant="tenant-b"))
    client.run(program=path("shipping.brix"), request_context=first)
    client.why(program=path("shipping.brix"), candidate="ship", request_context=second)

    assert [method for method, _ in sent] == ["run", "why"]
    assert sent[0][1]["request_context"]["tenant"] == "tenant-a"
    assert sent[0][1]["request_context"]["expected_program"] == "a" * 64
    assert sent[1][1]["request_context"]["tenant"] == "tenant-b"
    assert sent[1][1]["request_context"]["expected_program"] == "d" * 64
    assert "request_context" not in client.__dict__


def test_request_policy_is_sent_for_audit_and_verify_per_call():
    client = object.__new__(BrixClient)
    client._trusted_host = True
    sent = []
    client.result = lambda method, params, timeout=None: sent.append((method, params)) or {"ok": True}
    policy = RequestEvidencePolicy(sample_policy())
    client.audit(program=path("shipping.brix"), bundle_out="audit.json", request_context=policy)
    client.verify(
        program=path("shipping.brix"), bundle="audit.json", expect_program="a" * 64,
        request_context=policy,
    )
    assert [method for method, _ in sent] == ["audit", "verify"]
    assert all(params["request_context"] == policy.to_json_value() for _, params in sent)


def test_verify_rejects_context_for_another_program_or_l3_profile():
    client = object.__new__(BrixClient)
    client._trusted_host = True
    client.result = lambda *args, **kwargs: {"ok": True}
    with pytest.raises(ValueError, match="must equal expect_program"):
        client.verify(
            program=path("shipping.brix"), bundle="audit.json", expect_program="f" * 64,
            request_context=RequestEvidencePolicy(sample_policy()),
        )
    with pytest.raises(BrixError, match="supports only profile='finite-decision'"):
        client.verify(
            program=path("shipping.brix"), bundle="audit.json", expect_program="a" * 64,
            profile="l3-v1", request_context=RequestEvidencePolicy(sample_policy()),
        )
