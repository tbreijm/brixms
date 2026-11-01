"""Real serve-protocol acceptance for request-local evidence contexts."""

import json
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from brix import BrixClient, BrixTimeoutError, RequestEvidencePolicy, path, source  # noqa: E402


# These are canonical DocumentarySnapshot digests for `doc://attacker`,
# content digest 00..00, kind `policy-clause`, scope `shipping-release`, with
# the stated disposition. Kept as frozen values so the Python test checks the
# same independent pins as the Rust evidence-environment fixture.
SATISFIED_SNAPSHOT_DIGEST = "7f6f0de0f69577c6397e0242461f8008127e24575a958ae51c1d4ba5c9f3e274"
VIOLATED_SNAPSHOT_DIGEST = "25bd23f5e10150f07d6dc5a09bfeac7047750a0978bff7d07ef2b60307c6d74c"

EVIDENCE_PROGRAM = r"""
config EvidenceRef = { uri: Str, kind: Str, scope: Str }
config DocumentedStep = { digest: Str, status: Str, uri: Str }
config Decision = Ship | Hold
input ev: EvidenceRef
rule accepted() = match validate<DocumentedStep>(ev) {
 Satisfied(_) => true
 Violated(_) => false
 Unknown(_) => false
}
propose ship priority 10 when accepted = Ship
propose hold otherwise = Hold
commit d from (ship, hold)
""".strip()


def _policy(program_id: str, documentary_digest: str, tenant: str) -> RequestEvidencePolicy:
    return RequestEvidencePolicy(
        {
            "schema": "brix.request-evidence-policy@1",
            "expected_program": program_id,
            "runtime_claim_ids": [],
            "documentary_digest": documentary_digest,
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
    )


def _write_fixture_inputs(directory: Path) -> tuple[Path, Path, Path]:
    input_path = directory / "claim.json"
    input_path.write_text(
        json.dumps(
            {
                "schema": "brix.input@5",
                "values": {
                    "ev": {
                        "type": "record",
                        "nominal": "EvidenceRef",
                        "fields": [
                            {"name": "kind", "value": {"type": "string", "value": "policy-clause"}},
                            {"name": "scope", "value": {"type": "string", "value": "shipping-release"}},
                            {"name": "uri", "value": {"type": "string", "value": "doc://attacker"}},
                        ],
                    }
                },
            },
            separators=(",", ":"),
        ),
        encoding="utf-8",
    )

    def write_env(filename: str, digest: str, disposition: str) -> Path:
        evidence = directory / filename
        documentary = {
            "expected_digest": digest,
            "entries": [
                {
                    "evidence_id": "doc://attacker",
                    "content_digest": "00" * 32,
                    "evidence_kind": "policy-clause",
                    "authority_scope": "shipping-release",
                    "disposition": disposition,
                }
            ],
        }
        if disposition == "Violated":
            documentary["entries"][0]["reason"] = "denied"
        evidence.write_text(
            json.dumps(
                {"schema": "brix.evidence.env@1", "documentary": documentary},
                separators=(",", ":"),
            ),
            encoding="utf-8",
        )
        return evidence

    satisfied_path = write_env("satisfied-evidence.json", SATISFIED_SNAPSHOT_DIGEST, "Satisfied")
    violated_path = write_env("violated-evidence.json", VIOLATED_SNAPSHOT_DIGEST, "Violated")
    return input_path, satisfied_path, violated_path


def _assert_result_ok(result):
    assert result["ok"] is True, result.get("diagnostics", result)


def test_same_client_keeps_contradictory_evidence_contexts_request_local(
    brix_bin, tmp_path, monkeypatch
):
    input_path, satisfied_path, violated_path = _write_fixture_inputs(tmp_path)
    with BrixClient(brix_bin=brix_bin, cwd=tmp_path, trusted_host=True) as client:
        checked = client.check(program=source(EVIDENCE_PROGRAM))
        _assert_result_ok(checked)
        program_id = checked["program"]
        policy_a = _policy(program_id, SATISFIED_SNAPSHOT_DIGEST, "tenant-a")
        policy_b = _policy(program_id, VIOLATED_SNAPSHOT_DIGEST, "tenant-b")

        # Even a correct ambient digest is ignored by stdio. Without a host
        # request context the documentary evidence cannot authorize Ship.
        monkeypatch.setenv("BRIX_EXPECT_EVIDENCE_DIGEST", SATISFIED_SNAPSHOT_DIGEST)
        with BrixClient(brix_bin=brix_bin, cwd=tmp_path) as untrusted:
            ambient = untrusted.run(
                program=source(EVIDENCE_PROGRAM),
                inputs=[path(input_path), path(satisfied_path)],
            )
        assert ambient["ok"] is False
        assert ambient["status"] == "rejected"

        def run_with(policy, evidence_path):
            return client.run(
                program=source(EVIDENCE_PROGRAM),
                inputs=[path(input_path), path(evidence_path)],
                request_context=policy,
            )

        # Concurrent callers share one client and choose contradictory pins.
        # The policy is serialized with each request, never saved on the client.
        with ThreadPoolExecutor(max_workers=2) as pool:
            future_a = pool.submit(run_with, policy_a, satisfied_path)
            future_b = pool.submit(run_with, policy_b, violated_path)
            result_a = future_a.result()
            result_b = future_b.result()
        _assert_result_ok(result_a)
        _assert_result_ok(result_b)
        assert result_a["decision"]["candidate"] == "ship"
        assert result_b["decision"]["candidate"] == "hold"
        assert result_a["request_context"] != result_b["request_context"]

        # A host-pinned snapshot cannot be omitted or substituted by a valid
        # snapshot admitted under another caller's context.
        missing = client.run(
            program=source(EVIDENCE_PROGRAM),
            inputs=[path(input_path)],
            request_context=policy_a,
        )
        assert missing["ok"] is False
        assert missing["status"] == "rejected"
        substituted = run_with(policy_a, violated_path)
        assert substituted["ok"] is False
        assert substituted["status"] == "rejected"

        # A changed expected program fails this invocation. A later correct
        # request on the same process still succeeds with policy A.
        wrong_program = _policy("f" * 64, SATISFIED_SNAPSHOT_DIGEST, "tenant-a")
        refused = run_with(wrong_program, satisfied_path)
        assert refused["ok"] is False
        assert refused["status"] == "rejected"
        recovered = run_with(policy_a, satisfied_path)
        _assert_result_ok(recovered)
        assert recovered["decision"]["candidate"] == "ship"
        assert recovered["request_context"] == result_a["request_context"]


def test_portable_audit_replays_after_inputs_are_removed_and_rejects_wrong_context(
    brix_bin, tmp_path
):
    input_path, satisfied_path, _ = _write_fixture_inputs(tmp_path)
    bundle_path = tmp_path / "audit-request.json"
    with BrixClient(brix_bin=brix_bin, cwd=tmp_path, trusted_host=True) as auditor:
        checked = auditor.check(program=source(EVIDENCE_PROGRAM))
        _assert_result_ok(checked)
        program_id = checked["program"]
        policy_a = _policy(program_id, SATISFIED_SNAPSHOT_DIGEST, "tenant-a")
        policy_b = _policy(program_id, VIOLATED_SNAPSHOT_DIGEST, "tenant-b")
        audit = auditor.audit(
            program=source(EVIDENCE_PROGRAM),
            bundle_out=str(bundle_path),
            inputs=[path(input_path), path(satisfied_path)],
            request_context=policy_a,
        )
        _assert_result_ok(audit)
        assert audit["decision"]["candidate"] == "ship"
        assert audit["request_context"]

    # The wrapper carries the original input/evidence bytes, not host paths.
    input_path.unlink()
    satisfied_path.unlink()
    original_wrapper = bundle_path.read_bytes()
    with BrixClient(brix_bin=brix_bin, cwd=tmp_path, trusted_host=True) as verifier:
        tampered_wrapper = json.loads(original_wrapper)
        tampered_wrapper["request_context_id"] = "f" * 64
        bundle_path.write_text(
            json.dumps(tampered_wrapper, separators=(",", ":")), encoding="utf-8"
        )
        forged = verifier.verify(
            program=source(EVIDENCE_PROGRAM),
            bundle=str(bundle_path),
            expect_program=program_id,
            request_context=policy_a,
        )
        assert forged["ok"] is False
        bundle_path.write_bytes(original_wrapper)

        wrong = verifier.verify(
            program=source(EVIDENCE_PROGRAM),
            bundle=str(bundle_path),
            expect_program=program_id,
            request_context=policy_b,
        )
        assert wrong["ok"] is False

        replay = verifier.verify(
            program=source(EVIDENCE_PROGRAM),
            bundle=str(bundle_path),
            expect_program=program_id,
            request_context=policy_a,
        )
        _assert_result_ok(replay)
        assert replay["request_context"] == audit["request_context"]


def test_timed_out_request_context_does_not_leak_into_next_call(brix_bin, tmp_path):
    input_path, satisfied_path, violated_path = _write_fixture_inputs(tmp_path)
    with BrixClient(brix_bin=brix_bin, cwd=tmp_path, trusted_host=True) as client:
        checked = client.check(program=source(EVIDENCE_PROGRAM))
        _assert_result_ok(checked)
        program_id = checked["program"]
        policy_a = _policy(program_id, SATISFIED_SNAPSHOT_DIGEST, "tenant-a")
        policy_b = _policy(program_id, VIOLATED_SNAPSHOT_DIGEST, "tenant-b")

        with pytest.raises(BrixTimeoutError):
            client.run(
                program=source(EVIDENCE_PROGRAM),
                inputs=[path(input_path), path(satisfied_path)],
                request_context=policy_a,
                timeout=0.0,
            )

        # The server may finish the abandoned request, but its late result is
        # discarded before this response. The new call's policy still selects
        # its own contradictory evidence and context.
        result_b = client.run(
            program=source(EVIDENCE_PROGRAM),
            inputs=[path(input_path), path(violated_path)],
            request_context=policy_b,
            timeout=10.0,
        )
        _assert_result_ok(result_b)
        assert result_b["decision"]["candidate"] == "hold"
        assert result_b["request_context"]
