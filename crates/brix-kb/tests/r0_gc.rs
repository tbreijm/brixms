//! R0 M6: garbage collection and pack compaction (`gc_world`).
//!
//! GC must be invisible: every read a world supports before it (HEAD, restored network,
//! historical revisions, decision evidence) answers identically afterwards, whether the run
//! completes or crashes at any injected point, and it must actually shrink the store.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use brix_kb::world::gc::{gc_world, verify_store, GcCrashPoint, GcOptions};
use brix_kb::world::{
    OpenMode, TupleRecord, WorldBatch, WorldBatchOp, WorldError, WorldKey, WorldSession,
};

const MODEL: &str = r#"
rel input orders: { id: Str, customer: Str, sku: Str, qty: Int, express: Str } key id
rel input inventory: { id: Str, sku: Str, available: Int } key id
rel input shipping: { id: Str, sku: Str, carrier: Str, lead_days: Int } key id

rel derived fulfillment =
    select { order_id: o.id, customer: o.customer, sku: o.sku, qty: o.qty, express: o.express, available: i.available, carrier: s.carrier, lead_days: s.lead_days }
    from o in orders, i in inventory, s in shipping
    where o.sku == i.sku && o.sku == s.sku && i.available >= 0

rel derived sku_order_counts =
    select { sku: f.sku, order_count: count() }
    from f in fulfillment
    group by f.sku

decide dispatch for f in fulfillment per order_id {
    propose ship_express priority 10 when f.express == "yes" && f.available >= f.qty = "air_express"
    propose ship_standard priority 20 when f.express != "yes" && f.available >= f.qty = "ground_standard"
    propose backorder priority 50 when f.available < f.qty = "backorder_hold"
}
"#;

fn sources(src: &str) -> BTreeMap<String, String> {
    BTreeMap::from([("root".to_string(), src.to_string())])
}

struct TempDir(PathBuf);
impl TempDir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("brix_r0_gc_{name}_{}", std::process::id()));
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
    WorldSession::from_program_with_sources(dir.path(), "root", &sources(MODEL)).unwrap()
}

fn rec(fields: &[(&str, &str)]) -> brix_kb::world::WorldTuple {
    let mut r = TupleRecord::new();
    for (k, v) in fields {
        r.set_str(*k, v);
    }
    r.to_tuple()
}

fn upsert(rel: &str, n: u64, fields: &[(&str, &str)]) -> WorldBatchOp {
    WorldBatchOp::Upsert {
        relation: format!("root::{rel}"),
        key: WorldKey::from_u64(n),
        tuple: rec(fields),
    }
}

fn remove(rel: &str, n: u64) -> WorldBatchOp {
    WorldBatchOp::Remove {
        relation: format!("root::{rel}"),
        key: WorldKey::from_u64(n),
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

fn ship(n: u64, sku: u64, carrier: &str, days: u64) -> WorldBatchOp {
    upsert(
        "shipping",
        n,
        &[
            ("id", &format!("SHIP-{n}")),
            ("sku", &format!("SKU-{sku}")),
            ("carrier", carrier),
            ("lead_days", &days.to_string()),
        ],
    )
}

fn commit(s: &mut WorldSession, key: &str, ops: Vec<WorldBatchOp>) {
    let b = WorldBatch::new(s.current_revision, key, ops);
    s.apply_batch(b).unwrap();
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

fn fuzz_ops(rng: &mut Lcg, next: &mut [u64; 3]) -> Vec<WorldBatchOp> {
    let mut ops = Vec::new();
    for _ in 0..=rng.next(3) {
        let sku = rng.next(4);
        match rng.next(100) {
            0..=24 => {
                next[0] += 1;
                ops.push(order(next[0], sku, rng.next(20) + 1, rng.next(2) == 0));
            }
            25..=44 => {
                // Correction (and join-key change) of an existing order.
                let n = rng.next(next[0] + 1).max(1);
                ops.push(order(n, sku, rng.next(20) + 1, rng.next(2) == 0));
            }
            45..=54 => ops.push(remove("orders", rng.next(next[0] + 1).max(1))),
            55..=64 => ops.push(remove("orders", 50_000 + rng.next(100))),
            65..=77 => {
                next[1] += 1;
                ops.push(stock(1000 + next[1], sku, rng.next(15)));
            }
            78..=83 => ops.push(remove("inventory", 1000 + rng.next(next[1] + 1))),
            84..=93 => {
                next[2] += 1;
                ops.push(ship(
                    2000 + next[2],
                    sku,
                    if rng.next(2) == 0 { "Air" } else { "Ground" },
                    rng.next(5) + 1,
                ));
            }
            _ => ops.push(remove("shipping", 2000 + rng.next(next[2] + 1))),
        }
    }
    // A batch names a key at most once.
    let mut seen = std::collections::BTreeSet::new();
    ops.retain(|op| seen.insert((op.relation().to_string(), op.key().clone())));
    ops
}

fn dir_bytes(dir: &Path) -> u64 {
    let mut total = 0;
    for e in fs::read_dir(dir.join("objects")).unwrap().flatten() {
        let m = e.metadata().unwrap();
        if m.is_file() {
            total += m.len();
        }
    }
    total
}

fn files_with_ext(dir: &Path, ext: &str) -> Vec<PathBuf> {
    fs::read_dir(dir.join("objects"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some(ext))
        .collect()
}

/// Everything a reader can learn about the world, at HEAD and at every revision.
fn fingerprint(dir: &Path) -> String {
    let s = WorldSession::open(dir).unwrap();
    assert_eq!(s.open_mode, OpenMode::Restored);
    s.verify_state().unwrap();
    let mut out = format!("{:?}\n", s.all_settlements().unwrap());
    for seq in 0..=s.current_revision() {
        let snap = s.pin_revision(seq).unwrap();
        for rel in ["root::orders", "root::inventory", "root::shipping"] {
            let mut cursor: Option<String> = None;
            loop {
                let page = snap.query_page(rel, cursor.as_deref(), 100).unwrap();
                out.push_str(&format!("{seq} {rel} {:?}\n", page.entries));
                if !page.has_more {
                    break;
                }
                cursor = page.next_cursor;
            }
        }
        if seq > 0 {
            out.push_str(&format!("delta {seq} {:?}\n", s.decision_delta(seq)));
            out.push_str(&format!(
                "hist {seq} {:?}\n",
                s.historical_settlement(seq, None, "O-1").unwrap()
            ));
        }
    }
    out
}

fn history(dir: &TempDir, steps: usize) -> WorldSession {
    let mut s = create(dir);
    let mut rng = Lcg(0xFEED_FACE_1234_5678);
    let mut next = [0u64; 3];
    commit(
        &mut s,
        "b0",
        vec![order(1, 0, 3, true), stock(10, 0, 5), ship(20, 0, "Air", 2)],
    );
    for step in 0..steps {
        let ops = fuzz_ops(&mut rng, &mut next);
        commit(&mut s, &format!("g{step}"), ops);
    }
    s
}

#[test]
fn gc_is_invisible_and_shrinks_the_store() {
    let dir = TempDir::new("invisible");
    drop(history(&dir, 40));
    let want = fingerprint(dir.path());
    let before = dir_bytes(dir.path());

    let dry = gc_world(
        dir.path(),
        &GcOptions {
            dry_run: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(dir_bytes(dir.path()), before, "dry run must not write");
    assert!(dry.dead_nodes > 0 && dry.live_nodes > 0, "{dry:?}");

    let r = gc_world(dir.path(), &GcOptions::default()).unwrap();
    assert_eq!(r.live_nodes, dry.live_nodes);
    assert_eq!(r.revisions_retained, r.revisions_total);
    assert!(r.bytes_after < r.bytes_before, "{r:?}");
    assert_eq!(r.packs_after, 1);
    assert_eq!(dir_bytes(dir.path()), r.bytes_after);
    assert_eq!(fingerprint(dir.path()), want);

    // The retained nodes are exactly the live set: a second run frees nothing.
    let r2 = gc_world(dir.path(), &GcOptions::default()).unwrap();
    assert_eq!(r2.dead_nodes, 0, "{r2:?}");
    assert_eq!(r2.live_nodes, r.live_nodes);
    assert_eq!(r2.bytes_after, r.bytes_after);

    // The world keeps working: commits after GC, restore, and replay equivalence.
    let mut s = WorldSession::open(dir.path()).unwrap();
    commit(
        &mut s,
        "after-gc",
        vec![order(900, 1, 2, true), stock(901, 1, 4)],
    );
    let live = format!("{:?}", s.all_settlements().unwrap());
    let reopened = WorldSession::open(dir.path()).unwrap();
    assert_eq!(reopened.open_mode, OpenMode::Restored);
    assert_eq!(format!("{:?}", reopened.all_settlements().unwrap()), live);
    let replayed = WorldSession::open_replay(dir.path()).unwrap();
    assert_eq!(format!("{:?}", replayed.all_settlements().unwrap()), live);
    reopened.verify_state().unwrap();
    verify_store(dir.path()).unwrap();
}

#[test]
fn multiple_output_packs_and_a_tiny_target_still_work() {
    let dir = TempDir::new("multipack");
    drop(history(&dir, 25));
    let want = fingerprint(dir.path());
    let r = gc_world(
        dir.path(),
        &GcOptions {
            pack_target_bytes: Some(4096),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(r.packs_after > 1, "{r:?}");
    assert_eq!(fingerprint(dir.path()), want);
    let r2 = gc_world(dir.path(), &GcOptions::default()).unwrap();
    assert_eq!(r2.packs_after, 1);
    assert_eq!(fingerprint(dir.path()), want);
}

#[test]
fn a_held_writer_lock_refuses_gc() {
    let dir = TempDir::new("locked");
    drop(history(&dir, 3));
    let guard = brix_kb::world::session::WorldLockGuard::acquire(dir.path()).unwrap();
    let err = gc_world(dir.path(), &GcOptions::default()).err().unwrap();
    assert!(matches!(err, WorldError::WorldLocked(_)), "{err:?}");
    drop(guard);
    gc_world(dir.path(), &GcOptions::default()).unwrap();
}

#[test]
fn keep_revisions_drops_old_history_but_not_head_or_genesis() {
    let dir = TempDir::new("keep");
    drop(history(&dir, 20));
    let head_before = {
        let s = WorldSession::open(dir.path()).unwrap();
        format!("{:?}", s.all_settlements().unwrap())
    };
    let all = gc_world(
        dir.path(),
        &GcOptions {
            dry_run: true,
            ..Default::default()
        },
    )
    .unwrap();
    let r = gc_world(
        dir.path(),
        &GcOptions {
            keep_revisions: Some(2),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(r.revisions_retained < r.revisions_total, "{r:?}");
    assert!(r.live_nodes < all.live_nodes, "{r:?} vs {all:?}");
    let s = WorldSession::open(dir.path()).unwrap();
    assert_eq!(s.open_mode, OpenMode::Restored);
    assert_eq!(format!("{:?}", s.all_settlements().unwrap()), head_before);
    s.verify_state().unwrap();
    let head = s.current_revision();
    // Genesis, HEAD and the newest revisions stay readable.
    for seq in [0, head - 1, head] {
        let snap = s.pin_revision(seq).unwrap();
        snap.query_page("root::orders", None, 10).unwrap();
    }
    // A dropped revision reads as a typed error (history removed on purpose, not damage),
    // never as silently empty data.
    assert!(matches!(
        s.pin_revision(3),
        Err(WorldError::RevisionCompacted(3))
    ));
}

#[test]
fn a_crash_at_every_point_leaves_a_working_store() {
    for (i, point) in [
        GcCrashPoint::AfterFirstNewPack,
        GcCrashPoint::BeforeFinalFlush,
        GcCrashPoint::AfterCopyBeforeRetire,
        GcCrashPoint::MidRetire,
        GcCrashPoint::AfterRetire,
    ]
    .into_iter()
    .enumerate()
    {
        let dir = TempDir::new(&format!("crash{i}"));
        drop(history(&dir, 12));
        // Several packs, so MidRetire leaves a genuine mix of old packs.
        {
            let mut s = WorldSession::open(dir.path()).unwrap();
            commit(&mut s, "extra", vec![order(700, 2, 1, false)]);
        }
        let want = fingerprint(dir.path());
        let err = gc_world(
            dir.path(),
            &GcOptions {
                crash: Some(point),
                pack_target_bytes: Some(2048),
                ..Default::default()
            },
        )
        .err()
        .unwrap_or_else(|| panic!("{point:?} must fail"));
        assert!(format!("{err}").contains("injected gc crash"), "{err}");
        // Whatever was left behind opens and answers identically.
        assert_eq!(fingerprint(dir.path()), want, "after crash at {point:?}");
        verify_store(dir.path()).unwrap();
        let s = WorldSession::open_replay(dir.path()).unwrap();
        s.verify_state().unwrap();
        drop(s);
        // A later GC finishes the job and removes every leftover.
        let r = gc_world(dir.path(), &GcOptions::default()).unwrap();
        assert_eq!(r.packs_after, 1, "{point:?}");
        assert!(files_with_ext(dir.path(), "tmp").is_empty(), "{point:?}");
        assert_eq!(fingerprint(dir.path()), want, "after recovery {point:?}");
    }
}

#[test]
fn verify_store_reports_pack_damage_as_corrupt_state() {
    let dir = TempDir::new("verify_store");
    drop(history(&dir, 4));
    let ok = verify_store(dir.path()).unwrap();
    assert!(ok.packs > 0 && ok.nodes > 0);
    // Flip one body byte; framing and index stay intact, only the body checksum catches it.
    let pack = files_with_ext(dir.path(), "pack")[0].clone();
    let mut bytes = fs::read(&pack).unwrap();
    bytes[8 + 3] ^= 0x40;
    fs::write(&pack, bytes).unwrap();
    let err = verify_store(dir.path()).err().unwrap();
    assert!(matches!(err, WorldError::CorruptState(_)), "{err:?}");
    // GC refuses to launder damage into a fresh pack.
    assert!(gc_world(dir.path(), &GcOptions::default()).is_err());
}

#[test]
fn gc_keeps_the_idempotency_index_root_alive() {
    let dir = TempDir::new("idem_root");
    let mut s = history(&dir, 12);
    // Make sure the sidecar for HEAD exists (it is written best-effort after a commit).
    let _ = s.persist_idempotency_index();
    drop(s);
    gc_world(dir.path(), &GcOptions::default()).unwrap();
    // With the receipts gone only the persisted index can still answer; a GC that dropped
    // its trie nodes would degrade the open to a (here empty) scan.
    let paths = brix_kb::world::WorldPaths::new(dir.path());
    for e in fs::read_dir(paths.receipts_dir()).unwrap() {
        fs::remove_file(e.unwrap().path()).unwrap();
    }
    let s = WorldSession::open(dir.path()).unwrap();
    assert_eq!(s.idempotency.source.label(), "sidecar");
    for k in ["b0", "g0", "g11"] {
        assert!(s.idempotency_lookup(k).unwrap().is_some(), "{k}");
    }
}
