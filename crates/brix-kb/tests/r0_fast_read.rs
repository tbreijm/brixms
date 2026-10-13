//! R0 alpha.5 lane B: the settlement checkpoint and the persisted idempotency index.
//!
//! Both are caches (ADR-0046 s5): whatever state they are in (absent, stale, torn, corrupt,
//! foreign, forged, crashed mid-write), the observable result must equal the one the
//! authoritative path (the candidate frontier; the receipt scan) gives.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use brix_canon::Digest;
use brix_kb::world::settlement_checkpoint::SettlementCheckpoint;
use brix_kb::world::{
    CrashPoint, OpenPolicy, TupleRecord, WorldBatch, WorldBatchOp, WorldError, WorldKey,
    WorldPaths, WorldSession,
};

const MODEL: &str = r#"
rel input orders: { id: Str, customer: Str, sku: Str, qty: Int, express: Str } key id
rel input inventory: { id: Str, sku: Str, available: Int } key id
rel input shipping: { id: Str, sku: Str, carrier: Str, lead_days: Int } key id

rel derived fulfillment =
    select { order_id: o.id, customer: o.customer, sku: o.sku, qty: o.qty, express: o.express, available: i.available, carrier: s.carrier, lead_days: s.lead_days }
    from o in orders, i in inventory, s in shipping
    where o.sku == i.sku && o.sku == s.sku && i.available >= 0

decide dispatch for f in fulfillment per order_id {
    propose ship_express priority 10 when f.express == "yes" && f.available >= f.qty = "air_express"
    propose ship_standard priority 20 when f.express != "yes" && f.available >= f.qty = "ground_standard"
    propose backorder priority 50 when f.available < f.qty = "backorder_hold"
}
"#;

type Settlements = BTreeMap<String, BTreeMap<String, brix_kb::world::SettledDecision>>;

struct TempDir(PathBuf);
impl TempDir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("brix_r0_fast_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        TempDir(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn create(dir: &TempDir) -> WorldSession {
    let sources = BTreeMap::from([("root".to_string(), MODEL.to_string())]);
    WorldSession::from_program_with_sources(dir.path(), "root", &sources).unwrap()
}

fn upsert(rel: &str, n: u64, fields: &[(&str, &str)]) -> WorldBatchOp {
    let mut r = TupleRecord::new();
    for (k, v) in fields {
        r.set_str(*k, v);
    }
    WorldBatchOp::Upsert {
        relation: format!("root::{rel}"),
        key: WorldKey::from_u64(n),
        tuple: r.to_tuple(),
    }
}

fn order(n: u64, sku: u64, qty: u64, express: bool) -> WorldBatchOp {
    upsert(
        "orders",
        n,
        &[
            ("id", &format!("O-{n}")),
            ("customer", "C1"),
            ("sku", &format!("SKU-{sku}")),
            ("qty", &qty.to_string()),
            ("express", if express { "yes" } else { "no" }),
        ],
    )
}

fn stock(n: u64, sku: u64, available: u64) -> WorldBatchOp {
    upsert(
        "inventory",
        n,
        &[
            ("id", &format!("INV-{n}")),
            ("sku", &format!("SKU-{sku}")),
            ("available", &available.to_string()),
        ],
    )
}

fn ship(n: u64, sku: u64) -> WorldBatchOp {
    upsert(
        "shipping",
        n,
        &[
            ("id", &format!("SHIP-{n}")),
            ("sku", &format!("SKU-{sku}")),
            ("carrier", "Air"),
            ("lead_days", "2"),
        ],
    )
}

fn commit(s: &mut WorldSession, key: &str, ops: Vec<WorldBatchOp>) {
    s.apply_batch(WorldBatch::new(s.current_revision(), key, ops))
        .unwrap();
}

struct Lcg(u64);
impl Lcg {
    fn next(&mut self, bound: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) % bound
    }
}

/// Deterministic mixed batch: order upserts/removals and stock changes over 4 SKUs.
fn fuzz_batch(rng: &mut Lcg) -> Vec<WorldBatchOp> {
    let mut ops = Vec::new();
    let mut touched = std::collections::BTreeSet::new();
    for _ in 0..=rng.next(4) {
        let n = rng.next(40);
        if !touched.insert(("o", n)) {
            continue;
        }
        if rng.next(5) == 0 {
            ops.push(WorldBatchOp::Remove {
                relation: "root::orders".into(),
                key: WorldKey::from_u64(n),
            });
        } else {
            ops.push(order(n, rng.next(4), 1 + rng.next(9), rng.next(2) == 0));
        }
    }
    if rng.next(3) == 0 {
        let n = rng.next(4);
        if touched.insert(("i", n)) {
            ops.push(stock(n, n, rng.next(10)));
        }
    }
    ops
}

/// A world with decisions: base inventory/shipping for 4 SKUs, then `batches` fuzz batches.
fn seeded(dir: &TempDir, batches: usize, seed: u64) -> WorldSession {
    let mut s = create(dir);
    let mut ops = Vec::new();
    for sku in 0..4 {
        ops.push(stock(sku, sku, 5));
        ops.push(ship(sku, sku));
    }
    commit(&mut s, "seed", ops);
    let mut rng = Lcg(seed);
    for b in 0..batches {
        let ops = fuzz_batch(&mut rng);
        if !ops.is_empty() {
            commit(&mut s, &format!("fz-{b}"), ops);
        }
    }
    s
}

fn paths(dir: &TempDir) -> WorldPaths {
    WorldPaths::new(dir.path())
}

fn replay_settlements(dir: &TempDir) -> Settlements {
    WorldSession::open_replay(dir.path())
        .unwrap()
        .network
        .as_ref()
        .unwrap()
        .all_settlements()
        .unwrap()
}

// =======================================================================================
// Settlement checkpoint
// =======================================================================================

#[test]
fn checkpoint_equals_frontier_and_replay() {
    for seed in 1..=4u64 {
        let dir = TempDir::new(&format!("ck_eq_{seed}"));
        let s = seeded(&dir, 25, seed);
        s.close().unwrap();
        let want = replay_settlements(&dir);
        assert!(!want.is_empty());

        let s = WorldSession::open(dir.path()).unwrap();
        let (first, src) = s.all_settlements_with_source().unwrap();
        assert_eq!(src, "frontier", "no checkpoint yet");
        assert_eq!(first, want);
        assert!(paths(&dir).settlement_checkpoint_file().exists());

        let s = WorldSession::open(dir.path()).unwrap();
        let (second, src) = s.all_settlements_with_source().unwrap();
        assert_eq!(src, "checkpoint");
        assert_eq!(second, want);
        assert_eq!(
            s.verify_settlement_checkpoint().unwrap(),
            Some(want.values().map(|m| m.len()).sum())
        );
    }
}

#[test]
fn checkpoint_tail_applies_deltas_exactly() {
    let dir = TempDir::new("ck_tail");
    let mut s = seeded(&dir, 10, 7);
    s.all_settlements().unwrap(); // writes the checkpoint at this revision
    let ck_seq = s.current_revision();
    let mut rng = Lcg(99);
    let mut saw_tail = 0;
    for round in 0..12 {
        let ops = fuzz_batch(&mut rng);
        if ops.is_empty() {
            continue;
        }
        commit(&mut s, &format!("t-{round}"), ops);
        s.close().unwrap();
        let reopened = WorldSession::open(dir.path()).unwrap();
        let (got, src) = reopened.all_settlements_with_source().unwrap();
        assert_eq!(got, replay_settlements(&dir), "round {round}");
        if reopened.current_revision() > ck_seq {
            saw_tail += 1;
            assert_eq!(src, "checkpoint+tail", "round {round}");
        }
        s = reopened;
    }
    assert!(saw_tail > 5);
}

#[test]
fn long_tail_refreshes_the_checkpoint() {
    let dir = TempDir::new("ck_refresh");
    let mut s = seeded(&dir, 3, 3);
    s.all_settlements().unwrap();
    for i in 0..40u64 {
        commit(
            &mut s,
            &format!("x-{i}"),
            vec![order(100 + i, i % 4, 2, false)],
        );
    }
    s.close().unwrap();
    let s = WorldSession::open(dir.path()).unwrap();
    let (a, src) = s.all_settlements_with_source().unwrap();
    assert_eq!(src, "checkpoint+tail");
    assert_eq!(a, replay_settlements(&dir));
    // The read refreshed it: now at HEAD.
    let s = WorldSession::open(dir.path()).unwrap();
    assert_eq!(s.all_settlements_with_source().unwrap().1, "checkpoint");
}

/// Every way the file can be damaged is a miss with identical output, and the next read
/// repairs it.
#[test]
fn damaged_checkpoint_falls_back_identically() {
    let dir = TempDir::new("ck_damage");
    let s = seeded(&dir, 15, 11);
    s.close().unwrap();
    let want = replay_settlements(&dir);
    WorldSession::open(dir.path())
        .unwrap()
        .all_settlements()
        .unwrap();
    let file = paths(&dir).settlement_checkpoint_file();
    let good = fs::read(&file).unwrap();
    assert!(good.len() > 200);

    let mut cases: Vec<(String, Vec<u8>)> = vec![
        ("empty".into(), vec![]),
        ("garbage".into(), vec![7u8; 500]),
        ("truncated".into(), good[..good.len() / 2].to_vec()),
        ("truncated trailer".into(), good[..good.len() - 1].to_vec()),
        ("extended".into(), [good.clone(), vec![0]].concat()),
    ];
    for pos in [
        0,
        9,
        20,
        60,
        85,
        115,
        130,
        good.len() / 2,
        good.len() - 40,
        good.len() - 1,
    ] {
        let mut b = good.clone();
        b[pos] ^= 0x40;
        cases.push((format!("flip@{pos}"), b));
    }
    for (name, bytes) in cases {
        fs::write(&file, &bytes).unwrap();
        let s = WorldSession::open(dir.path()).unwrap();
        let (got, src) = s.all_settlements_with_source().unwrap();
        assert_eq!(src, "frontier", "{name}");
        assert_eq!(got, want, "{name}");
        // Repaired by the fallback read.
        assert_eq!(fs::read(&file).unwrap(), good, "{name}: repaired");
    }
}

/// A checkpoint with a valid trailer but wrong content or binding: the binding checks and
/// the spot probes reject it on read; a full verify rejects forged content too.
#[test]
fn forged_and_foreign_checkpoints_are_rejected() {
    let dir = TempDir::new("ck_forged");
    let s = seeded(&dir, 15, 21);
    s.close().unwrap();
    let want = replay_settlements(&dir);
    let session = WorldSession::open(dir.path()).unwrap();
    session.all_settlements().unwrap();
    let file = paths(&dir).settlement_checkpoint_file();
    let good = SettlementCheckpoint::decode(&fs::read(&file).unwrap()).unwrap();

    let reject = |label: &str, ck: SettlementCheckpoint| {
        fs::write(&file, ck.encode()).unwrap();
        let s = WorldSession::open_with_policy(
            dir.path(),
            OpenPolicy {
                force_replay: false,
                refresh_after_replay: false,
            },
        )
        .unwrap();
        let (got, src) = s.all_settlements_with_source().unwrap();
        assert_eq!(src, "frontier", "{label}");
        assert_eq!(got, want, "{label}");
    };
    let mut c = good.clone();
    c.revision_digest = Digest::from_bytes([9; 32]);
    reject("wrong revision digest", c);
    let mut c = good.clone();
    c.validity_key_digest = Digest::from_bytes([9; 32]);
    reject("foreign validity key", c);
    let mut c = good.clone();
    c.decision_root = Some(Digest::from_bytes([9; 32]));
    reject("wrong decision root", c);
    let mut c = good.clone();
    c.revision_seq += 5;
    reject("from the future", c);
    // One decision changed (all probes cannot miss it with <= 64 entries: stride 1).
    let mut c = good.clone();
    let (decide, entities) = c.settlements.iter_mut().next().unwrap();
    let _ = decide;
    let (_, d) = entities.iter_mut().next().unwrap();
    d.candidate_name = "forged".into();
    reject("forged decision", c.clone());
    // One decision dropped: count mismatch.
    let mut c = good.clone();
    let entities = c.settlements.values_mut().next().unwrap();
    let k = entities.keys().next().unwrap().clone();
    entities.remove(&k);
    reject("dropped decision", c);

    // A full verify recomputes the root from the content, so it catches forged content
    // whatever the probes sampled.
    let mut c = good.clone();
    let entities = c.settlements.values_mut().next().unwrap();
    entities.values_mut().last().unwrap().candidate_name = "forged".into();
    fs::write(&file, c.encode()).unwrap();
    let s = WorldSession::open(dir.path()).unwrap();
    assert!(s.verify_settlement_checkpoint().is_err());
}

/// An old checkpoint whose tail includes a revision whose delta is gone (compaction) is a
/// miss, not an error.
#[test]
fn compacted_tail_falls_back() {
    let dir = TempDir::new("ck_compact");
    let mut s = seeded(&dir, 4, 5);
    s.all_settlements().unwrap();
    commit(&mut s, "after-1", vec![order(60, 1, 2, true)]);
    commit(&mut s, "after-2", vec![order(61, 2, 3, false)]);
    let want = s.all_settlements().unwrap(); // frontier path of the live session? checkpoint+tail
    s.compact_history(s.current_revision() - 1).unwrap();
    s.close().unwrap();
    let s = WorldSession::open(dir.path()).unwrap();
    let (got, _) = s.all_settlements_with_source().unwrap();
    assert_eq!(got, want);
}

#[test]
fn stale_checkpoint_from_an_older_revision() {
    let dir = TempDir::new("ck_stale");
    let mut s = seeded(&dir, 6, 31);
    s.all_settlements().unwrap();
    let file = paths(&dir).settlement_checkpoint_file();
    let old = fs::read(&file).unwrap();
    let mut rng = Lcg(5);
    for i in 0..6 {
        let ops = fuzz_batch(&mut rng);
        if !ops.is_empty() {
            commit(&mut s, &format!("s-{i}"), ops);
        }
    }
    s.close().unwrap();
    // Put the older checkpoint back (as after a restore from backup).
    fs::write(&file, &old).unwrap();
    let s = WorldSession::open(dir.path()).unwrap();
    let (got, src) = s.all_settlements_with_source().unwrap();
    assert_eq!(src, "checkpoint+tail");
    assert_eq!(got, replay_settlements(&dir));
}

#[test]
fn checkpoint_crash_point_leaves_no_visible_state() {
    let dir = TempDir::new("ck_crash");
    let mut s = seeded(&dir, 8, 41);
    s.set_crash_point(Some(CrashPoint::CheckpointAfterTmpBeforeRename));
    match s.refresh_settlement_checkpoint() {
        Err(WorldError::InjectedCrash(CrashPoint::CheckpointAfterTmpBeforeRename)) => {}
        other => panic!("expected injected crash, got {other:?}"),
    }
    s.close().unwrap();
    assert!(!paths(&dir).settlement_checkpoint_file().exists());
    let s = WorldSession::open(dir.path()).unwrap();
    let (got, src) = s.all_settlements_with_source().unwrap();
    assert_eq!(src, "frontier");
    assert_eq!(got, replay_settlements(&dir));
}

#[test]
fn explicit_refresh_and_open_replay_never_writes() {
    let dir = TempDir::new("ck_explicit");
    let s = seeded(&dir, 8, 51);
    s.close().unwrap();
    let r = WorldSession::open_replay(dir.path()).unwrap();
    r.all_settlements().unwrap();
    assert!(!paths(&dir).settlement_checkpoint_file().exists());
    let mut s = WorldSession::open(dir.path()).unwrap();
    let n = s.refresh_settlement_checkpoint().unwrap();
    assert!(n > 0);
    assert_eq!(s.verify_settlement_checkpoint().unwrap(), Some(n));
}

#[test]
fn settlements_page_covers_everything_once() {
    let dir = TempDir::new("ck_page");
    let s = seeded(&dir, 20, 61);
    s.close().unwrap();
    let s = WorldSession::open(dir.path()).unwrap();
    let want = s.all_settlements().unwrap();
    let mut got: Settlements = BTreeMap::new();
    let mut cursor = None;
    let mut pages = 0;
    loop {
        let (rows, next) = s.settlements_page(cursor.as_ref(), 7).unwrap();
        for (decide, entity, d) in rows {
            assert!(got.entry(decide).or_default().insert(entity, d).is_none());
        }
        pages += 1;
        match next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert!(pages > 1);
    assert_eq!(got, want);
}

// =======================================================================================
// Idempotency index
// =======================================================================================

fn lookups(s: &WorldSession, keys: &[String]) -> Vec<Option<(u64, Digest, Option<Digest>)>> {
    keys.iter()
        .map(|k| {
            s.idempotency_lookup(k)
                .unwrap()
                .map(|i| (i.seq, i.revision_digest, i.batch_digest))
        })
        .collect()
}

fn keys_of(n: usize) -> Vec<String> {
    let mut k: Vec<String> = (0..n).map(|i| format!("k-{i}")).collect();
    k.push("never-used".into());
    k.push("genesis".into());
    k
}

fn many(dir: &TempDir, n: usize) -> WorldSession {
    let mut s = create(dir);
    for i in 0..n {
        commit(
            &mut s,
            &format!("k-{i}"),
            vec![stock(i as u64, i as u64 % 4, 3)],
        );
    }
    s
}

fn quiet() -> OpenPolicy {
    OpenPolicy {
        force_replay: false,
        refresh_after_replay: false,
    }
}

#[test]
fn index_agrees_with_scan_and_old_worlds_upgrade() {
    let dir = TempDir::new("idem_scan");
    let s = many(&dir, 40);
    s.close().unwrap();
    let keys = keys_of(40);

    let with_sidecar = WorldSession::open_with_policy(dir.path(), quiet()).unwrap();
    assert_eq!(with_sidecar.idempotency.source.label(), "sidecar");
    let a = lookups(&with_sidecar, &keys);
    assert!(a[..40].iter().all(Option::is_some));
    assert_eq!(a[40], None);
    assert_eq!(a[41], None);

    // An old world: no sidecar. The quiet open scans and writes nothing...
    fs::remove_file(paths(&dir).idempotency_file()).unwrap();
    let scanned = WorldSession::open_with_policy(dir.path(), quiet()).unwrap();
    assert_eq!(scanned.idempotency.source.label(), "scan");
    assert_eq!(lookups(&scanned, &keys), a);
    assert!(!paths(&dir).idempotency_file().exists());
    // ...the default open upgrades it, and the next open reads the index.
    let upgraded = WorldSession::open(dir.path()).unwrap();
    assert_eq!(upgraded.idempotency.source.label(), "scan");
    assert_eq!(lookups(&upgraded, &keys), a);
    let again = WorldSession::open(dir.path()).unwrap();
    assert_eq!(again.idempotency.source.label(), "sidecar");
    assert_eq!(lookups(&again, &keys), a);

    // Revision-only world (no receipts at all): still the same answers.
    for e in fs::read_dir(paths(&dir).receipts_dir()).unwrap() {
        fs::remove_file(e.unwrap().path()).unwrap();
    }
    fs::remove_file(paths(&dir).idempotency_file()).unwrap();
    let revs = WorldSession::open_with_policy(dir.path(), quiet()).unwrap();
    assert_eq!(revs.idempotency.source.label(), "scan");
    assert_eq!(lookups(&revs, &keys), a);
}

#[test]
fn idempotent_replay_and_conflict_semantics_hold_for_every_source() {
    let dir = TempDir::new("idem_sem");
    let s = many(&dir, 12);
    s.close().unwrap();
    for scan in [false, true] {
        if scan {
            let _ = fs::remove_file(paths(&dir).idempotency_file());
        }
        let mut s = WorldSession::open_with_policy(dir.path(), quiet()).unwrap();
        let head = s.current_revision();
        // Same key, same payload: replay of the original receipt.
        let r = s
            .apply_batch(WorldBatch::new(5, "k-5", vec![stock(5, 5 % 4, 3)]))
            .unwrap();
        assert!(r.is_idempotent_replay);
        assert_eq!(r.revision_seq, 6);
        assert_eq!(s.current_revision(), head);
        // Same key, different payload: conflict.
        let err = s
            .apply_batch(WorldBatch::new(head, "k-5", vec![stock(5, 0, 99)]))
            .unwrap_err();
        assert!(
            matches!(err, WorldError::IdempotencyConflict { .. }),
            "{err:?}"
        );
    }
}

#[test]
fn stale_sidecar_is_caught_up_from_receipts_and_persisted() {
    let dir = TempDir::new("idem_tail");
    let mut s = many(&dir, 6);
    let old = fs::read(paths(&dir).idempotency_file()).unwrap();
    for i in 6..12 {
        commit(&mut s, &format!("k-{i}"), vec![stock(i, i % 4, 3)]);
    }
    s.close().unwrap();
    fs::write(paths(&dir).idempotency_file(), &old).unwrap();
    let keys = keys_of(12);
    let truth = {
        let _ = fs::remove_file(paths(&dir).idempotency_file());
        let t = WorldSession::open_with_policy(dir.path(), quiet()).unwrap();
        lookups(&t, &keys)
    };
    fs::write(paths(&dir).idempotency_file(), &old).unwrap();
    let mut s = WorldSession::open_with_policy(dir.path(), quiet()).unwrap();
    assert_eq!(s.idempotency.source.label(), "sidecar+tail");
    assert_eq!(lookups(&s, &keys), truth);
    // The next commit persists the whole thing, tail keys included.
    commit(&mut s, "k-12", vec![stock(12, 0, 3)]);
    s.close().unwrap();
    let s = WorldSession::open_with_policy(dir.path(), quiet()).unwrap();
    assert_eq!(s.idempotency.source.label(), "sidecar");
    let got = lookups(&s, &keys);
    assert_eq!(&got[..12], &truth[..12]);
    assert!(s.idempotency_lookup("k-12").unwrap().is_some());
}

#[test]
fn unusable_sidecars_fall_back_to_the_scan() {
    let dir = TempDir::new("idem_bad");
    let s = many(&dir, 10);
    s.close().unwrap();
    let keys = keys_of(10);
    let file = paths(&dir).idempotency_file();
    let good = fs::read(&file).unwrap();
    let truth = {
        let s = WorldSession::open_with_policy(dir.path(), quiet()).unwrap();
        lookups(&s, &keys)
    };
    let v: serde_json::Value = serde_json::from_slice(&good).unwrap();
    let with = |f: &dyn Fn(&mut serde_json::Value)| {
        let mut v = v.clone();
        f(&mut v);
        serde_json::to_vec(&v).unwrap()
    };
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("garbage", b"not json".to_vec()),
        ("empty", vec![]),
        ("other schema", with(&|v| v["schema"] = "x".into())),
        (
            "wrong head digest",
            with(&|v| v["revision_digest"] = Digest::from_bytes([1; 32]).to_hex().into()),
        ),
        (
            "root not in store",
            with(&|v| v["root"] = Digest::from_bytes([2; 32]).to_hex().into()),
        ),
        ("from the future", with(&|v| v["revision_seq"] = 999.into())),
        (
            "older, digest not matching its receipt",
            with(&|v| {
                v["revision_seq"] = 4.into();
                v["revision_digest"] = Digest::from_bytes([3; 32]).to_hex().into();
            }),
        ),
    ];
    for (name, bytes) in cases {
        fs::write(&file, bytes).unwrap();
        let s = WorldSession::open_with_policy(dir.path(), quiet()).unwrap();
        assert_eq!(s.idempotency.source.label(), "scan", "{name}");
        assert_eq!(lookups(&s, &keys), truth, "{name}");
    }
}

/// A sidecar copied from another world at the same revision number is not this world's.
#[test]
fn foreign_sidecar_is_rejected() {
    let a = TempDir::new("idem_foreign_a");
    let b = TempDir::new("idem_foreign_b");
    let sa = many(&a, 5);
    sa.close().unwrap();
    let mut sb = create(&b);
    for i in 0..5 {
        commit(&mut sb, &format!("other-{i}"), vec![stock(i, 0, 1)]);
    }
    sb.close().unwrap();
    fs::copy(paths(&a).idempotency_file(), paths(&b).idempotency_file()).unwrap();
    let s = WorldSession::open_with_policy(b.path(), quiet()).unwrap();
    assert_eq!(s.idempotency.source.label(), "scan");
    assert!(s.idempotency_lookup("k-1").unwrap().is_none());
    assert!(s.idempotency_lookup("other-1").unwrap().is_some());
}

#[test]
fn crash_points_keep_idempotency_exact() {
    // Crash after the HEAD rename but before the sidecar: the key is committed and found
    // via the tail catch-up.
    let dir = TempDir::new("idem_crash_after");
    let mut s = many(&dir, 3);
    s.set_crash_point(Some(CrashPoint::AfterHeadRenameBeforeDirectoryFsync));
    let r = s.apply_batch(WorldBatch::new(3, "k-crash", vec![stock(50, 0, 1)]));
    assert!(matches!(r, Err(WorldError::InjectedCrash(_))));
    drop(s);
    let mut s = WorldSession::open_with_policy(dir.path(), quiet()).unwrap();
    assert_eq!(s.current_revision(), 4);
    assert_eq!(s.idempotency.source.label(), "sidecar+tail");
    let hit = s.idempotency_lookup("k-crash").unwrap().unwrap();
    assert_eq!(hit.seq, 4);
    let r = s
        .apply_batch(WorldBatch::new(3, "k-crash", vec![stock(50, 0, 1)]))
        .unwrap();
    assert!(r.is_idempotent_replay);

    // Crash before the HEAD rename: not committed, its receipt (seq > HEAD) is ignored, and
    // the retry commits for real.
    let dir = TempDir::new("idem_crash_before");
    let mut s = many(&dir, 3);
    s.set_crash_point(Some(CrashPoint::AfterRevisionFsyncBeforeHeadRename));
    let r = s.apply_batch(WorldBatch::new(3, "k-crash", vec![stock(50, 0, 1)]));
    assert!(matches!(r, Err(WorldError::InjectedCrash(_))));
    drop(s);
    let mut s = WorldSession::open_with_policy(dir.path(), quiet()).unwrap();
    assert_eq!(s.current_revision(), 3);
    assert!(s.idempotency_lookup("k-crash").unwrap().is_none());
    let r = s
        .apply_batch(WorldBatch::new(3, "k-crash", vec![stock(50, 0, 1)]))
        .unwrap();
    assert!(!r.is_idempotent_replay);
    assert_eq!(r.revision_seq, 4);
}

#[test]
fn open_does_not_scan_files_when_the_index_is_current() {
    // Observable proxy: delete every receipt; the index (not the scan) still answers.
    let dir = TempDir::new("idem_noscan");
    let s = many(&dir, 30);
    s.close().unwrap();
    for e in fs::read_dir(paths(&dir).receipts_dir()).unwrap() {
        fs::remove_file(e.unwrap().path()).unwrap();
    }
    let s = WorldSession::open_with_policy(dir.path(), quiet()).unwrap();
    assert_eq!(s.idempotency.source.label(), "sidecar");
    for k in 0..30 {
        assert!(s.idempotency_lookup(&format!("k-{k}")).unwrap().is_some());
    }
}
