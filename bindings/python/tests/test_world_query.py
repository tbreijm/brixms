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
