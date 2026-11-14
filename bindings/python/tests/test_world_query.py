"""Tests for world operations and pinned queries in BrixClient."""

import json
import sys
from decimal import Decimal
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


def test_world_composite_decision_keys_and_evidence(client, tmp_path):
    world_dir = tmp_path / "world-composite"
    prog_content = """config Decision = Allowed(Int) | Denied

rel input limits: { tenant: Str, id: Str, amount: Int } key id
rel input requests: { tenant: Str, id: Str, requested: Int } key id

rel derived pending = select {
    tenant: r.tenant,
    id: r.id,
    requested: r.requested,
    limit: l.amount
} from r in requests, l in limits
where r.tenant == l.tenant && r.id == l.id

decide grant for p in pending per (tenant, id) {
    propose allow priority 1 when p.requested <= p.limit = Decision::Allowed(p.requested)
    propose deny priority 2 when true = Decision::Denied
}
"""
    prog_path = tmp_path / "composite.brix"
    prog_path.write_text(prog_content)

    init_res = client.world_init(world_dir, str(prog_path))
    assert init_res["ok"] is True
    manifest_digest = init_res["manifest_digest"]
    program_digest = init_res["program_digest"]
    batch = {
        "schema": "brix.world.batch@1",
        "expected_base_revision": 0,
        "idempotency_key": "seed-comp",
        "operations": [
            {
                "op": "upsert",
                "relation": "composite::limits",
                "key": "a::bc",
                "tuple": {"tenant": "a", "id": "bc", "amount": 100},
            },
            {
                "op": "upsert",
                "relation": "composite::requests",
                "key": "a::bc",
                "tuple": {"tenant": "a", "id": "bc", "requested": 50},
            },
            {
                "op": "upsert",
                "relation": "composite::limits",
                "key": "ab::c",
                "tuple": {"tenant": "ab", "id": "c", "amount": 100},
            },
            {
                "op": "upsert",
                "relation": "composite::requests",
                "key": "ab::c",
                "tuple": {"tenant": "ab", "id": "c", "requested": 150},
            },
        ],
    }
    b_res = client.world_batch(world_dir, batch)
    assert b_res["ok"] is True

    rev1_digest = b_res["revision_digest"]

    # 1. Query decisions using tuple/list entity
    dec_a_bc = client.world_decisions(world_dir, "composite::grant", entity=["a", "bc"])
    assert dec_a_bc["ok"] is True
    s_a_bc = dec_a_bc["settlements"]["composite::grant"]["[\"a\",\"bc\"]"]
    assert s_a_bc["candidate_name"] == "allow"
    assert s_a_bc["entity_key"] == ["a", "bc"]

    dec_ab_c = client.world_decisions(world_dir, "composite::grant", entity=["ab", "c"])
    assert dec_ab_c["ok"] is True
    s_ab_c = dec_ab_c["settlements"]["composite::grant"]["[\"ab\",\"c\"]"]
    assert s_ab_c["candidate_name"] == "deny"
    assert s_ab_c["entity_key"] == ["ab", "c"]

    # 2. Explain decision with composite entity
    expl = client.world_explain(world_dir, ["a", "bc"], decide="composite::grant")
    assert expl["ok"] is True
    assert expl["explanation"]["winning_candidate"] == "allow"
    assert expl["explanation"]["entity_key"] == ["a", "bc"]

    # 3. Decision evidence round trip with composite entity
    evidence_res = client.get_decision_evidence(world_dir, decide="composite::grant", key=["a", "bc"], rev=1)
    assert evidence_res["ok"] is True

    ver = client.verify_decision_evidence(
        evidence_res["evidence"],
        world_manifest_digest=manifest_digest,
        program_digest=program_digest,
        revision=1,
        revision_digest=rev1_digest,
        decide_name="composite::grant",
        entity_key=["a", "bc"],
        candidate="allow",
        value_digest=evidence_res["target"]["value_digest"],
    )
    assert ver["ok"] is True


def test_review_entity_key_identity_and_numeric_evidence_roundtrip(client, tmp_path):
    world_dir = tmp_path / "world-key-identity"
    program = tmp_path / "key-identity.brix"
    program.write_text(
        """rel input rows: { id: Str, n: F64, d: Decimal } key id
decide choose for r in rows per (id, n, d) {
    propose yes priority 1 when true = 1
}
decide choose_scalar for r in rows per id {
    propose yes priority 1 when true = 1
}
"""
    )
    init = client.world_init(world_dir, str(program))
    assert init["ok"] is True
    batch = client.world_batch(
        world_dir,
        {
            "schema": "brix.world.batch@1",
            "expected_base_revision": 0,
            "idempotency_key": "identity-seed",
            "operations": [
                {
                    "op": "upsert",
                    "relation": "key-identity::rows",
                    "key": "[\"a\", \"b\"]",
                    "tuple": {"id": "[\"a\", \"b\"]", "n": 0.5, "d": "0.50"},
                },
                {
                    "op": "upsert",
                    "relation": "key-identity::rows",
                    "key": '["a","b"]',
                    "tuple": {"id": '["a","b"]', "n": 0.5, "d": "0.50"},
                },
                {
                    "op": "upsert",
                    "relation": "key-identity::rows",
                    "key": "numeric",
                    "tuple": {"id": "numeric", "n": 0.5, "d": "0.50"},
                },
            ],
        },
    )
    assert batch["ok"] is True

    spaced = '["a", "b"]'
    exact = client.world_decisions(world_dir, "key-identity::choose_scalar", entity=spaced)
    assert exact["ok"] is True
    assert list(exact["settlements"]["key-identity::choose_scalar"]) == [spaced]
    compact = client.world_decisions(
        world_dir, "key-identity::choose_scalar", entity='["a","b"]'
    )
    assert compact["ok"] is True
    assert list(compact["settlements"]["key-identity::choose_scalar"]) == ['["a","b"]']
    explanation = client.world_explain(world_dir, spaced, decide="key-identity::choose_scalar")
    assert explanation["ok"] is True
    assert explanation["explanation"]["entity_key"] == spaced

    evidence_file = tmp_path / "literal-str.evidence.json"
    evidence = client.get_decision_evidence(
        world_dir,
        decide="key-identity::choose_scalar",
        key=spaced,
        rev=1,
        out=evidence_file,
    )
    assert evidence["ok"] is True
    saved_evidence = json.loads(evidence_file.read_text())
    assert saved_evidence["target"]["entity_key"] == evidence["target"]["entity_key"] == spaced
    assert (
        saved_evidence["settlement"]["entity_key"]
        == evidence["settlement"]["entity_key"]
        == spaced
    )
    verified = client.verify_decision_evidence(
        evidence_file,
        world_manifest_digest=init["manifest_digest"],
        program_digest=init["program_digest"],
        revision=1,
        revision_digest=batch["revision_digest"],
        decide_name="key-identity::choose_scalar",
        entity_key=spaced,
        candidate="yes",
        value_digest=evidence["target"]["value_digest"],
    )
    assert verified["ok"] is True

    numeric_key = ["numeric", 0.5, Decimal("0.500")]
    numeric_evidence = client.get_decision_evidence(
        world_dir,
        decide="key-identity::choose",
        key=numeric_key,
        rev=1,
    )
    assert numeric_evidence["ok"] is True
    numeric_verify = client.verify_decision_evidence(
        numeric_evidence["evidence"],
        world_manifest_digest=init["manifest_digest"],
        program_digest=init["program_digest"],
        revision=1,
        revision_digest=batch["revision_digest"],
        decide_name="key-identity::choose",
        entity_key=numeric_key,
        candidate="yes",
        value_digest=numeric_evidence["target"]["value_digest"],
    )
    assert numeric_verify["ok"] is True
    wrong_key = client.verify_decision_evidence(
        numeric_evidence["evidence"],
        world_manifest_digest=init["manifest_digest"],
        program_digest=init["program_digest"],
        revision=1,
        revision_digest=batch["revision_digest"],
        decide_name="key-identity::choose",
        entity_key=["numeric", 0.75, Decimal("0.500")],
        candidate="yes",
        value_digest=numeric_evidence["target"]["value_digest"],
    )
    assert wrong_key["ok"] is False
    wrong_type = client.verify_decision_evidence(
        numeric_evidence["evidence"],
        world_manifest_digest=init["manifest_digest"],
        program_digest=init["program_digest"],
        revision=1,
        revision_digest=batch["revision_digest"],
        decide_name="key-identity::choose",
        entity_key=["numeric", "0.5", Decimal("0.500")],
        candidate="yes",
        value_digest=numeric_evidence["target"]["value_digest"],
    )
    assert wrong_type["ok"] is False
