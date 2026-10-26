"""Tests for world operations and pinned queries in BrixClient."""

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from brix import BrixClient  # noqa: E402


@pytest.fixture
def client(brix_bin, examples_dir):
    with BrixClient(brix_bin=brix_bin, cwd=examples_dir) as c:
        yield c


def test_world_pinned_query(client, tmp_path):
    world_dir = tmp_path / "world"
    prog = Path(__file__).resolve().parents[3] / "examples" / "world-linked" / "main.brix"

    # 1. world_init
    init_res = client.world_init(world_dir, str(prog))
    assert init_res["ok"] is True

    # 2. world_batch rev 1
    batch1 = {
        "schema": "brix.world.batch@1",
        "expected_base_revision": 0,
        "idempotency_key": "py-seed-1",
        "operations": [
            {
                "op": "upsert",
                "relation": "policy::orders",
                "key": "order-1",
                "tuple": {"id": "order-1", "customer": "Ada", "sku": "sku-1", "qty": 2, "express": "yes"},
            },
            {
                "op": "upsert",
                "relation": "policy::orders",
                "key": "order-2",
                "tuple": {"id": "order-2", "customer": "Bob", "sku": "sku-2", "qty": 5, "express": "no"},
            },
        ],
    }
    b1_res = client.world_batch(world_dir, batch1)
    assert b1_res["ok"] is True
    assert b1_res["revision"] == 1
    rev1_digest = b1_res["revision_digest"]

    # 3. Query at rev 1
    q1 = client.world_query(world_dir, "policy::orders", rev=1, limit=1)
    assert q1["ok"] is True
    assert q1["revision"] == 1
    assert q1["revision_digest"] == rev1_digest
    assert q1["count"] == 1
    assert q1["has_more"] is True
    cursor1 = q1["next_cursor"]

    # 4. Commit rev 2
    batch2 = {
        "schema": "brix.world.batch@1",
        "expected_base_revision": 1,
        "idempotency_key": "py-seed-2",
        "operations": [
            {
                "op": "upsert",
                "relation": "policy::orders",
                "key": "order-3",
                "tuple": {"id": "order-3", "customer": "Carl", "sku": "sku-3", "qty": 1, "express": "yes"},
            }
        ],
    }
    b2_res = client.world_batch(world_dir, batch2)
    assert b2_res["ok"] is True
    assert b2_res["revision"] == 2

    # 5. Resuming cursor1 without rev must fail against HEAD (rev 2)
    stale = client.world_query(world_dir, "policy::orders", cursor=cursor1)
    assert stale["ok"] is False
    assert stale["status"] == "invalid-cursor"

    # 6. Resuming cursor1 with rev=1 completes pagination at rev 1
    q2 = client.world_query(world_dir, "policy::orders", rev=1, cursor=cursor1)
    assert q2["ok"] is True
    assert q2["revision"] == 1
    assert q2["count"] == 1
    assert q2["has_more"] is False

    # 7. Expect digest: correct succeeds, wrong fails
    q_good = client.world_query(world_dir, "policy::orders", rev=1, expect_digest=rev1_digest)
    assert q_good["ok"] is True

    wrong_digest = "0" * 64
    q_bad = client.world_query(world_dir, "policy::orders", rev=1, expect_digest=wrong_digest)
    assert q_bad["ok"] is False
    assert q_bad["status"] == "revision-digest-mismatch"


def test_world_decision_evidence_and_parent_anchored_collect(client, tmp_path):
    world_dir = tmp_path / "atlas_world"
    prog_file = tmp_path / "atlas_collect.brix"
    prog_file.write_text(
        """
config Plan = AskBack(List<Str>) | Done
config CertifiedAnswer = { path: Plan, caller: Str, release_id: Str, grants: List<Str> }

fn no_slots(): List<Str> = []

rel input parents: { parent: Str, caller: Str, release_id: Str } key parent
rel input parent_grants: { id: Str, parent: Str, grant: Str, ord: Int } key id
rel input slots: { id: Str, parent: Str, slot: Str, ord: Int } key id

rel derived nonempty_slots = select {
  parent: s.parent,
  missing: collect(s.slot, s.ord, 16)
} from s in slots group by s.parent

rel derived collected_grants = select {
  parent: g.parent,
  grants: collect(g.grant, g.ord, 16)
} from g in parent_grants group by g.parent

rel derived nonempty_context = select {
  parent: s.parent,
  missing: s.missing,
  caller: p.caller,
  release_id: p.release_id,
  grants: g.grants
} from s in nonempty_slots, p in parents, g in collected_grants
where s.parent == p.parent && s.parent == g.parent

rel derived empty_context = select {
  parent: p.parent,
  missing: no_slots(),
  caller: p.caller,
  release_id: p.release_id,
  grants: g.grants
} from p in parents, g in collected_grants
where p.parent == g.parent && not exists s in slots where s.parent == p.parent

rel derived parent_context = union nonempty_context, empty_context key parent

decide plan for g in parent_context per parent {
  propose ask priority 1 when true = CertifiedAnswer {
    path: Plan::AskBack(g.missing),
    caller: g.caller,
    release_id: g.release_id,
    grants: g.grants
  }
}
""".strip()
    )

    init_res = client.world_init(world_dir, str(prog_file))
    assert init_res["ok"] is True
    manifest_digest = init_res["manifest_digest"]
    program_digest = init_res["program_digest"]

    b1 = client.world_batch(
        world_dir,
        {
            "schema": "brix.world.batch@1",
            "expected_base_revision": 0,
            "idempotency_key": "collect-seed-1",
            "operations": [
                {
                    "op": "upsert",
                    "relation": "atlas_collect::parents",
                    "key": "q1",
                    "tuple": {
                        "parent": "q1",
                        "caller": "service:atlas",
                        "release_id": "atlas-2026.10",
                    },
                },
                {
                    "op": "upsert",
                    "relation": "atlas_collect::parent_grants",
                    "key": "g1",
                    "tuple": {"id": "g1", "parent": "q1", "grant": "read:atlas", "ord": 1},
                },
                {
                    "op": "upsert",
                    "relation": "atlas_collect::slots",
                    "key": "s1",
                    "tuple": {"id": "s1", "parent": "q1", "slot": "volume", "ord": 1},
                },
                {
                    "op": "upsert",
                    "relation": "atlas_collect::slots",
                    "key": "s2",
                    "tuple": {"id": "s2", "parent": "q1", "slot": "customer-grain", "ord": 2},
                },
            ],
        },
    )
    assert b1["ok"] is True
    rev1_digest = b1["revision_digest"]

    ev_res = client.get_decision_evidence(
        world_dir,
        decide="atlas_collect::plan",
        key="q1",
        rev=1,
        caller="service:atlas",
        grants=["read:atlas"],
    )
    assert ev_res["ok"] is True
    evidence = ev_res["evidence"]
    assert evidence["bundle"] is not None
    expected_value_digest = ev_res["target"]["value_digest"]

    ver_ok = client.verify_decision_evidence(
        evidence,
        world_manifest_digest=manifest_digest,
        program_digest=program_digest,
        revision=1,
        revision_digest=rev1_digest,
        decide_name="atlas_collect::plan",
        entity_key="q1",
        candidate="ask",
        value_digest=expected_value_digest,
        caller="service:atlas",
        release="atlas-2026.10",
        grants=["read:atlas"],
    )
    assert ver_ok["ok"] is True
    assert ver_ok["receipt"]["authority"] == "runtime_check"

    # Retracting all children preserves parent settlement with empty list []
    b2 = client.world_batch(
        world_dir,
        {
            "schema": "brix.world.batch@1",
            "expected_base_revision": 1,
            "idempotency_key": "collect-retract-all",
            "operations": [
                {"op": "remove", "relation": "atlas_collect::slots", "key": "s1"},
                {"op": "remove", "relation": "atlas_collect::slots", "key": "s2"},
            ],
        },
    )
    assert b2["ok"] is True

    # Historical evidence request at rev 1 after advancing to rev 2 must still verify
    ev_hist = client.get_decision_evidence(
        world_dir,
        decide="atlas_collect::plan",
        key="q1",
        rev=1,
        caller="service:atlas",
        grants=["read:atlas"],
    )
    assert ev_hist["ok"] is True
    ver_hist = client.verify_decision_evidence(
        ev_hist["evidence"],
        world_manifest_digest=manifest_digest,
        program_digest=program_digest,
        revision=1,
        revision_digest=rev1_digest,
        decide_name="atlas_collect::plan",
        entity_key="q1",
        candidate="ask",
        value_digest=expected_value_digest,
        caller="service:atlas",
        release="atlas-2026.10",
        grants=["read:atlas"],
    )
    assert ver_hist["ok"] is True
