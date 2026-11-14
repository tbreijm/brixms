"""Host policy validation and trusted named-query SDK surface."""

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from brix import BrixClient, BrixError, HostQueryPolicy  # noqa: E402


def sample_policy():
    return {
        "schema": "brix.host-query-policy@1",
        "profile": {
            "profile_id": "brix.host-scope.partition@1",
            "partition_fields": {"catalogue::catalogue_entries": "tenant_id"},
            "global_relations": [],
        },
        "namespace": "catalogue",
        "tenant": "tenant-a",
        "principal": "trusted-test-host",
        "valid_time_start": 0,
        "valid_time_end": 100,
        "validity_start": 0,
        "validity_end": 100,
        "step_budget": 1000000,
        "depth_budget": 100,
        "limits": {
            "bytes": 65536,
            "assumptions": 256,
            "depth": 128,
            "artifact_bytes": 65536,
        },
        "query_limits": {
            "page_rows": 128,
            "max_input_rows": 10000,
            "max_input_bytes": 16777216,
            "max_eval_work": 1000000,
            "max_eval_matches": 100000,
            "max_output_rows": 10000,
        },
    }


def test_host_query_policy_is_immutable_and_detached_from_input():
    input_policy = sample_policy()
    policy = HostQueryPolicy(input_policy)

    input_policy["tenant"] = "tenant-b"
    returned = policy.to_json_value()
    returned["tenant"] = "tenant-c"

    assert policy.to_json_value()["tenant"] == "tenant-a"
    with pytest.raises(AttributeError, match="immutable"):
        policy._canonical_json = "{}"


@pytest.mark.parametrize(
    "change, message",
    [
        (lambda p: p.update(extra=True), "unknown extra"),
        (lambda p: p.update(schema="brix.host-query-policy@2"), "unsupported host query policy"),
        (lambda p: p["profile"].update(profile_id="brix.host-scope.partition@2"), "unsupported host-scope"),
        (lambda p: p["query_limits"].update(max_eval_work=10_000_001), "may not exceed"),
        (lambda p: p["profile"]["global_relations"].append("catalogue::catalogue_entries"), "both partitioned and global"),
    ],
)
def test_host_policy_rejects_invalid_shape_and_widened_authority(change, message):
    policy = sample_policy()
    change(policy)
    with pytest.raises(ValueError, match=message):
        HostQueryPolicy(policy)


def test_named_query_requires_a_host_policy_locally():
    client = object.__new__(BrixClient)
    client._host_query_policy = None
    with pytest.raises(BrixError, match=r"requires BrixClient\(host_query_policy=\.\.\.\)"):
        client.world_query_named("world", "catalogue")


def test_trusted_named_query_is_tenant_scoped(brix_bin, examples_dir, tmp_path):
    world_dir = tmp_path / "catalogue-world"
    program = tmp_path / "catalogue.brix"
    program.write_text(
        """
rel input catalogue_entries: { id: Str, tenant_id: Str, kind: Str } key id
query catalogue(kind: Str) = select { id: c.id, kind: c.kind }
  from c in catalogue_entries where c.kind == kind
""".strip()
    )
    policy = HostQueryPolicy(sample_policy())

    with BrixClient(brix_bin=brix_bin, cwd=examples_dir, host_query_policy=policy) as client:
        assert client.world_init(world_dir, str(program))["ok"] is True
        seeded = client.world_batch(
            world_dir,
            {
                "schema": "brix.world.batch@1",
                "expected_base_revision": 0,
                "idempotency_key": "python-host-query-seed",
                "operations": [
                    {
                        "op": "upsert",
                        "relation": "catalogue::catalogue_entries",
                        "key": "a-1",
                        "tuple": {"id": "a-1", "tenant_id": "tenant-a", "kind": "open"},
                    },
                    {
                        "op": "upsert",
                        "relation": "catalogue::catalogue_entries",
                        "key": "b-1",
                        "tuple": {"id": "b-1", "tenant_id": "tenant-b", "kind": "open"},
                    },
                ],
            },
        )
        assert seeded["ok"] is True

        page = client.world_query_named(
            world_dir, "catalogue", arguments={"kind": "open"}, rev=1, limit=10
        )

    assert page["ok"] is True
    assert page["revision"] == 1
    assert page["count"] == 1
    assert [row["id"] for row in page["rows"]] == ["a-1"]
