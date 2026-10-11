//! Regression tests for the adversarial review of alpha.5 (H1, M1-M4, L1-L6).
//!
//! Each fails on commit 67d2910 and passes with the fixes; see
//! `docs/planning/r0-phase2-design.md`, section "Review fixes (alpha.5)".

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use brix_canon::{Digest, Domain};
use brix_kb::world::gc::{gc_world, verify_store, GcOptions};
use brix_kb::world::idempotency::{idempotency_trie_key, IdempotencyIndex, IdempotencySidecar};
use brix_kb::world::session::{CommittedBatchInfo, WorldLockGuard};
use brix_kb::world::settlement_checkpoint::SettlementCheckpoint;
use brix_kb::world::types::WorldTuple;
use brix_kb::world::{
    OpenPolicy, TupleRecord, WorldBatch, WorldBatchOp, WorldError, WorldKey, WorldPaths,
    WorldRevision, WorldSession, REVISION_SCHEMA_V2,
};
use soc_core::store::{FileNodeStore, NodeStore, TrieMap};

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
        let p = std::env::temp_dir().join(format!("brix_r0_rf_{name}_{}", std::process::id()));
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

// ---- shared helpers ----

fn settlements_of(s: &WorldSession) -> String {
    format!("{:?}", s.all_settlements().unwrap())
}

/// The truth: a fresh replay of the base facts with the settlement checkpoint removed.
fn replayed_settlements(dir: &Path) -> String {
    let _ = fs::remove_file(dir.join("settlements.ckpt"));
    settlements_of(&WorldSession::open_replay(dir).unwrap())
}

/// Digests the node `digest` embeds that name nodes present in the store.
fn children_of(dir: &Path, digest: &Digest) -> Vec<Digest> {
    let store = FileNodeStore::new(dir).unwrap();
    let bytes = store.get_node(digest).unwrap().unwrap();
    let mut out = Vec::new();
    for i in 0..bytes.len().saturating_sub(33) {
        if bytes[i] == 0x01 && bytes[i + 1] == 0x20 {
            let mut raw = [0u8; 32];
            raw.copy_from_slice(&bytes[i + 2..i + 34]);
            let d = Digest::from_bytes(raw);
            if store.try_contains(&d).unwrap() && !out.contains(&d) {
                out.push(d);
            }
        }
    }
    out
}

/// Flip one byte in the stored body of `digest` (found through the pack's record table, so the
/// pack framing stays valid and only the body checksum fails).
fn corrupt_node_body(dir: &Path, digest: &Digest) {
    for item in fs::read_dir(dir.join("objects")).unwrap().flatten() {
        let path = item.path();
        if path.extension().and_then(|e| e.to_str()) != Some("pack") {
            continue;
        }
        let mut bytes = fs::read(&path).unwrap();
        let needle = digest.as_bytes();
        let mut p = 0;
        while p + 80 <= bytes.len() {
            if &bytes[p..p + 32] == needle {
                let off = u64::from_be_bytes(bytes[p + 32..p + 40].try_into().unwrap()) as usize;
                let len = u64::from_be_bytes(bytes[p + 40..p + 48].try_into().unwrap()) as usize;
                if off >= 8 && len > 0 && off.checked_add(len).is_some_and(|e| e <= p) {
                    let body = &bytes[off..off + len];
                    if Digest::of(Domain::Value, body).as_bytes() == &bytes[p + 48..p + 80] {
                        bytes[off + len / 2] ^= 0xff;
                        fs::write(&path, bytes).unwrap();
                        return;
                    }
                }
            }
            p += 1;
        }
    }
    panic!("node {} not found in any pack", digest.to_hex());
}

fn head_seq(dir: &Path) -> u64 {
    fs::read_to_string(dir.join("HEAD"))
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

/// Rewrite HEAD as a pre-M4 `@2` record (no restorable state), as `r0_restore` does.
fn forge_legacy_head(dir: &Path) {
    let p = WorldPaths::new(dir);
    let seq = head_seq(dir);
    let v = serde_json::from_slice(&fs::read(p.revision_file(seq)).unwrap()).unwrap();
    let mut r = WorldRevision::from_json(&v).unwrap();
    r.schema = REVISION_SCHEMA_V2.to_string();
    r.network_state_root = None;
    r.revision_digest = r.compute_digest_for_record();
    fs::write(
        p.revision_file(seq),
        serde_json::to_vec_pretty(&r.to_json()).unwrap(),
    )
    .unwrap();
    fs::write(
        p.head(),
        format!("{} {}\n", seq, r.revision_digest.to_hex()),
    )
    .unwrap();
}

fn no_refresh() -> OpenPolicy {
    OpenPolicy {
        force_replay: false,
        refresh_after_replay: false,
    }
}

fn keyed_world(dir: &TempDir, keys: u64) -> WorldSession {
    let mut s = create(dir);
    commit(
        &mut s,
        "base",
        vec![stock(10, 0, 1_000_000), ship(20, 0, "Air", 2)],
    );
    for i in 0..keys {
        commit(
            &mut s,
            &format!("k{i}"),
            vec![order(100 + i, 0, 1, i % 2 == 0)],
        );
    }
    s
}

// ---- H1: a session open across a GC ----

/// A session left open across an external GC commits a state that recreates nodes GC deleted;
/// its caches vouch for them, so they were never re-written.
#[test]
fn stale_session_commit_after_gc_leaves_dangling_nodes() {
    let dir = TempDir::new("h1_stale");
    let mut s = create(&dir);
    commit(
        &mut s,
        "a",
        vec![order(1, 0, 3, true), stock(10, 0, 5), ship(20, 0, "Air", 2)],
    );
    commit(&mut s, "b", vec![order(2, 0, 1, false)]);
    let _ = s.all_settlements().unwrap();
    commit(&mut s, "c", vec![remove("orders", 2)]);
    let _ = s.all_settlements().unwrap();
    commit(&mut s, "d", vec![order(2, 0, 1, false)]);
    commit(&mut s, "e", vec![remove("orders", 2)]);
    gc_world(dir.path(), &GcOptions::default()).unwrap();
    commit(&mut s, "f", vec![order(2, 0, 1, false)]);
    drop(s);
    let s = WorldSession::open(dir.path()).expect("reopen after stale-session commit");
    s.verify_state().expect("verify_state");
    verify_store(dir.path()).unwrap();
    assert_eq!(settlements_of(&s), replayed_settlements(dir.path()));
}

#[test]
fn gc_in_another_process_does_not_poison_an_open_session() {
    if let Some(d) = std::env::var_os("BRIX_RF_GC_DIR") {
        // Child: the "other process".
        gc_world(Path::new(&d), &GcOptions::default()).unwrap();
        return;
    }
    let dir = TempDir::new("h1_child");
    let mut s = create(&dir);
    commit(
        &mut s,
        "a",
        vec![order(1, 0, 3, true), stock(10, 0, 5), ship(20, 0, "Air", 2)],
    );
    for i in 0..6u64 {
        commit(&mut s, &format!("c{i}"), vec![order(2, 0, 1, i % 2 == 0)]);
        // Warm every cache of the session.
        let _ = s.all_settlements().unwrap();
        let _ = s.get("root::orders", &WorldKey::from_u64(2)).unwrap();
    }
    commit(&mut s, "x", vec![remove("orders", 2)]);
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "gc_in_another_process_does_not_poison_an_open_session",
            "--nocapture",
        ])
        .env("BRIX_RF_GC_DIR", dir.path())
        .status()
        .unwrap();
    assert!(status.success());
    // Re-create exactly what GC deleted, several times over.
    for i in 0..3u64 {
        commit(&mut s, &format!("y{i}"), vec![order(2, 0, 1, i % 2 == 0)]);
        commit(&mut s, &format!("z{i}"), vec![remove("orders", 2)]);
    }
    commit(&mut s, "last", vec![order(2, 0, 1, true)]);
    drop(s);
    let s = WorldSession::open(dir.path()).unwrap();
    s.verify_state().unwrap();
    verify_store(dir.path()).unwrap();
    assert_eq!(settlements_of(&s), replayed_settlements(dir.path()));
}

// ---- L1: --keep-revisions overflow ----

#[test]
fn keep_revisions_huge_does_not_drop_history() {
    let dir = TempDir::new("l1_keephuge");
    let mut s = create(&dir);
    for i in 0..5u64 {
        commit(
            &mut s,
            &format!("k{i}"),
            vec![
                order(i, 0, 1, i % 2 == 0),
                stock(100 + i, 0, 5),
                ship(200 + i, 0, "Air", 2),
            ],
        );
    }
    drop(s);
    let p = dir.path().to_path_buf();
    let r = std::panic::catch_unwind(move || {
        gc_world(
            &p,
            &GcOptions {
                keep_revisions: Some(u64::MAX),
                ..Default::default()
            },
        )
    });
    match r {
        Err(_) => panic!("gc panicked on keep_revisions = u64::MAX (overflow)"),
        Ok(Ok(rep)) => assert_eq!(rep.revisions_retained, rep.revisions_total, "{rep:?}"),
        Ok(Err(e)) => panic!("{e:?}"),
    }
}

// ---- M1: settlement checkpoint authenticity ----

fn bulk_world(dir: &TempDir) -> (WorldSession, String) {
    let mut s = create(dir);
    let mut ops = vec![stock(10_000, 0, 1_000_000), ship(20_000, 0, "Air", 2)];
    for i in 0..400u64 {
        ops.push(order(i, 0, 1, i % 2 == 0));
    }
    commit(&mut s, "bulk", ops);
    let truth = settlements_of(&s);
    (s, truth)
}

fn forge_one_entry(path: &Path) -> SettlementCheckpoint {
    let mut ck = SettlementCheckpoint::decode(&fs::read(path).unwrap()).unwrap();
    let (_, ents) = ck.settlements.iter_mut().next().unwrap();
    let (_, d) = ents.iter_mut().nth(1).unwrap();
    d.candidate_name = format!("{}_FORGED", d.candidate_name);
    ck
}

/// A checkpoint whose header is intact but with one non-sampled entry changed (trailer
/// recomputed by `encode`) is served as truth.
#[test]
fn forged_checkpoint_entry_changes_output() {
    let dir = TempDir::new("m1_forged");
    let (s, truth) = bulk_world(&dir);
    let path = s.paths.settlement_checkpoint_file();
    let ck = forge_one_entry(&path);
    fs::write(&path, ck.encode()).unwrap();
    drop(s);
    let s2 = WorldSession::open(dir.path()).unwrap();
    let (got, src) = s2.all_settlements_with_source().unwrap();
    assert_eq!(
        format!("{got:?}"),
        truth,
        "forged checkpoint served (source {src})"
    );
    assert_ne!(src, "checkpoint");
}

/// Even a forgery that carries the genuine `auth` bytes and a recomputed trailer is a miss.
#[test]
fn forged_checkpoint_with_copied_auth_is_rejected() {
    let dir = TempDir::new("m1_copied_auth");
    let (s, truth) = bulk_world(&dir);
    let path = s.paths.settlement_checkpoint_file();
    let genuine = fs::read(&path).unwrap();
    let ck = forge_one_entry(&path);
    let mut bytes = ck.encode();
    // Header layout: magic 8, seq 8, two digests 64, root flag + root 33, then auth.
    bytes[113..145].copy_from_slice(&genuine[113..145]);
    let n = bytes.len() - 32;
    let digest = Digest::of(Domain::Value, &bytes[..n]);
    bytes[n..].copy_from_slice(digest.as_bytes());
    assert!(
        SettlementCheckpoint::decode(&bytes).is_ok(),
        "forgery must be well formed"
    );
    fs::write(&path, bytes).unwrap();
    drop(s);
    let s2 = WorldSession::open(dir.path()).unwrap();
    let (got, src) = s2.all_settlements_with_source().unwrap();
    assert_eq!(format!("{got:?}"), truth, "source {src}");
    assert_ne!(src, "checkpoint");
}

/// The tail path (checkpoint behind HEAD, deltas applied) must not trust a forged base either.
#[test]
fn forged_checkpoint_behind_head_is_rejected() {
    let dir = TempDir::new("m1_tail");
    let (mut s, truth) = bulk_world(&dir);
    let path = s.paths.settlement_checkpoint_file();
    let ck = forge_one_entry(&path);
    fs::write(&path, ck.encode()).unwrap();
    // A commit that decides nothing new: the tail is empty of decision changes.
    commit(&mut s, "more", vec![stock(11, 7, 3)]);
    drop(s);
    // (`open_replay` reads the checkpoint too, so the truth is the pre-forgery answer: the
    // second commit changes no decision.)
    let s2 = WorldSession::open(dir.path()).unwrap();
    assert_eq!(settlements_of(&s2), truth);
}

/// Without the per-world secret nothing can be authenticated: a miss, rebuilt on the spot.
#[test]
fn a_lost_checkpoint_key_is_a_miss_not_an_error() {
    let dir = TempDir::new("m1_nokey");
    let (s, truth) = bulk_world(&dir);
    drop(s);
    fs::remove_file(dir.path().join("checkpoint.key")).unwrap();
    let s2 = WorldSession::open(dir.path()).unwrap();
    let (got, src) = s2.all_settlements_with_source().unwrap();
    assert_eq!(format!("{got:?}"), truth);
    assert_eq!(src, "frontier");
    drop(s2);
    let s3 = WorldSession::open(dir.path()).unwrap();
    assert_eq!(s3.all_settlements_with_source().unwrap().1, "checkpoint");
}

// ---- M2: torn receipt vs the idempotency scan ----

/// A torn receipt hides its revision record from the idempotency scan.
#[test]
fn torn_receipt_loses_idempotency_key_on_scan() {
    let dir = TempDir::new("m2_receipt");
    let mut s = create(&dir);
    let b1 = WorldBatch::new(s.current_revision, "k1", vec![order(1, 0, 3, true)]);
    s.apply_batch(b1).unwrap();
    commit(&mut s, "k2", vec![order(2, 0, 3, true)]);
    drop(s);
    let _ = fs::remove_file(dir.path().join("idempotency.json"));
    fs::write(
        dir.path().join("receipts").join("1.json"),
        b"{\"seq\": 1, \"idempo",
    )
    .unwrap();
    let s = WorldSession::open(dir.path()).unwrap();
    assert!(
        s.idempotency_lookup("k1").unwrap().is_some(),
        "k1 lost from the index"
    );
    assert!(s.idempotency_lookup("k2").unwrap().is_some());
}

/// When a key cannot be recovered at all, the incomplete scan must not be persisted.
#[test]
fn a_lossy_scan_is_never_persisted() {
    let dir = TempDir::new("m2_lossy");
    let mut s = create(&dir);
    commit(&mut s, "k1", vec![order(1, 0, 3, true)]);
    commit(&mut s, "k2", vec![order(2, 0, 3, true)]);
    drop(s);
    let _ = fs::remove_file(dir.path().join("idempotency.json"));
    fs::write(
        dir.path().join("receipts").join("1.json"),
        b"{\"seq\": 1, \"idempo",
    )
    .unwrap();
    fs::write(dir.path().join("revisions").join("1.json"), b"garbage").unwrap();
    let s = WorldSession::open(dir.path()).unwrap();
    assert!(s.idempotency_lookup("k2").unwrap().is_some());
    assert!(
        !dir.path().join("idempotency.json").exists(),
        "a sidecar built from a scan that lost a key was persisted"
    );
}

// ---- M3: damage to the idempotency cache never blocks commits ----

#[test]
fn a_damaged_deep_idempotency_node_does_not_block_commits() {
    let dir = TempDir::new("m3_deep");
    let mut s = create(&dir);
    commit(
        &mut s,
        "base",
        vec![stock(10, 0, 1_000_000), ship(20, 0, "Air", 2)],
    );
    let mut k3 = None;
    for i in 0..40u64 {
        let b = WorldBatch::new(
            s.current_revision,
            format!("k{i}"),
            vec![order(100 + i, 0, 1, i % 2 == 0)],
        );
        if i == 3 {
            k3 = Some(b.clone());
        }
        s.apply_batch(b).unwrap();
    }
    drop(s);
    let sc = IdempotencySidecar::read(&WorldPaths::new(dir.path()).idempotency_file()).unwrap();
    let kids = children_of(dir.path(), &sc.root);
    assert!(!kids.is_empty());
    for k in &kids {
        corrupt_node_body(dir.path(), k);
    }
    let mut s = WorldSession::open(dir.path()).expect("open with a damaged idempotency cache");
    commit(&mut s, "after-damage", vec![order(500, 0, 1, true)]);
    // Existing keys still answer from the receipts.
    let replay = s.apply_batch(k3.unwrap());
    assert!(
        matches!(&replay, Ok(r) if r.is_idempotent_replay),
        "replay of an old key failed: {replay:?}"
    );
    commit(&mut s, "after-damage-2", vec![order(501, 0, 1, true)]);
    drop(s);
    // And the next open and commit work too.
    let mut s = WorldSession::open(dir.path()).unwrap();
    commit(&mut s, "after-reopen", vec![order(502, 0, 1, true)]);
    s.verify_state().unwrap();
}

// ---- M4: readers' cache writes stay out of a writer's way ----

#[test]
fn a_readers_refresh_does_not_make_a_concurrent_commit_fail() {
    for round in 0..4u64 {
        let dir = TempDir::new(&format!("m4_reader_{round}"));
        let mut s = create(&dir);
        let mut ops = vec![stock(10_000, 0, 1_000_000), ship(20_000, 0, "Air", 2)];
        for i in 0..1500u64 {
            ops.push(order(i, 0, 1, i % 2 == 0));
        }
        commit(&mut s, "bulk", ops);
        drop(s);
        forge_legacy_head(dir.path());
        let _ = fs::remove_file(dir.path().join("state-cache.json"));
        // The writer opens first (replaying, never refreshing), like `world batch` would.
        let mut w = WorldSession::open_with_policy(dir.path(), no_refresh()).unwrap();
        let path = dir.path().to_path_buf();
        let reader = std::thread::spawn(move || {
            let r = WorldSession::open(&path).unwrap();
            r.state_refreshed
        });
        // Wait until the reader holds the writer lock (it is refreshing its caches), then
        // commit at once. A reader that does its heavy work outside the lock is never seen
        // holding it for long, and a commit rides out its brief holds.
        let started = Instant::now();
        loop {
            match WorldLockGuard::acquire(dir.path()) {
                Err(WorldError::WorldLocked(_)) => break,
                Ok(g) => drop(g),
                Err(e) => panic!("{e:?}"),
            }
            if started.elapsed().as_secs() > 60 || reader.is_finished() {
                break;
            }
            std::thread::yield_now();
        }
        let res = w.apply_batch(WorldBatch::new(
            w.current_revision,
            format!("w{round}"),
            vec![order(5000, 0, 1, true)],
        ));
        let _ = reader.join().unwrap();
        assert!(
            res.is_ok(),
            "round {round}: commit beside a reader refresh failed: {res:?}"
        );
    }
}

#[test]
fn a_commit_waits_briefly_for_a_short_lock_and_fails_typed_for_a_long_one() {
    let dir = TempDir::new("m4_wait");
    let mut s = create(&dir);
    commit(&mut s, "a", vec![order(1, 0, 1, true)]);
    let root = dir.path().to_path_buf();
    let holder = std::thread::spawn(move || {
        let g = WorldLockGuard::acquire(&root).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(120));
        drop(g);
    });
    std::thread::sleep(std::time::Duration::from_millis(30));
    let b = WorldBatch::new(s.current_revision, "b", vec![order(2, 0, 1, true)]);
    s.apply_batch(b)
        .expect("a short lock must not fail a commit");
    holder.join().unwrap();
    let g = WorldLockGuard::acquire(dir.path()).unwrap();
    let b = WorldBatch::new(s.current_revision, "c", vec![order(3, 0, 1, true)]);
    assert!(matches!(s.apply_batch(b), Err(WorldError::WorldLocked(_))));
    drop(g);
}

// ---- L3: verify_after proves reachability ----

#[test]
fn verify_after_detects_an_unreachable_idempotency_trie() {
    let dir = TempDir::new("l3_verify");
    drop(keyed_world(&dir, 3));
    let paths = WorldPaths::new(dir.path());
    let sc = IdempotencySidecar::read(&paths.idempotency_file()).unwrap();
    // A trie whose root is stored in the world but whose children are not.
    for attempt in 0..64u32 {
        let scratch = TempDir::new(&format!("l3_scratch_{attempt}"));
        fs::create_dir_all(scratch.path()).unwrap();
        let sp = WorldPaths::new(scratch.path());
        let mut store = FileNodeStore::new(scratch.path()).unwrap();
        let mut idx = IdempotencyIndex::empty();
        for i in 0..6u32 {
            idx.record(
                &format!("a{attempt}-{i}"),
                CommittedBatchInfo {
                    seq: 1,
                    revision_digest: sc.revision_digest,
                    batch_digest: None,
                },
                &store,
            );
        }
        idx.stage(&mut store);
        store.flush().unwrap();
        idx.publish(&sp, sc.revision_seq, sc.revision_digest)
            .unwrap();
        let fake = IdempotencySidecar::read(&sp.idempotency_file()).unwrap();
        let root_bytes = store.get_node(&fake.root).unwrap().unwrap();
        let mut world_store = FileNodeStore::new(dir.path()).unwrap();
        world_store.put_new_node(fake.root, root_bytes);
        world_store.flush().unwrap();
        drop(world_store);
        fake.write(&paths).unwrap();
        // The cache is only usable if the open lands on the sidecar (the probe key's path
        // does not cross a missing child); otherwise try another shape.
        let probe = WorldSession::open_with_policy(dir.path(), no_refresh()).unwrap();
        if probe.idempotency.source.label() != "sidecar" {
            continue;
        }
        drop(probe);
        let r = gc_world(dir.path(), &GcOptions::default());
        assert!(
            r.is_err(),
            "verify_after accepted an idempotency trie with unreadable nodes: {r:?}"
        );
        return;
    }
    panic!("could not build a usable damaged sidecar");
}

// ---- L4: error classes ----

#[test]
fn gc_reports_damage_as_corrupt_state() {
    let dir = TempDir::new("l4_gcerr");
    drop(keyed_world(&dir, 5));
    let paths = WorldPaths::new(dir.path());
    let sc = IdempotencySidecar::read(&paths.idempotency_file()).unwrap();
    corrupt_node_body(dir.path(), &sc.root);
    let err = gc_world(dir.path(), &GcOptions::default()).err().unwrap();
    assert!(matches!(err, WorldError::CorruptState(_)), "{err:?}");
}

#[test]
fn compacted_history_is_a_typed_error_not_damage() {
    let dir = TempDir::new("l4_compacted");
    drop(keyed_world(&dir, 8));
    gc_world(
        dir.path(),
        &GcOptions {
            keep_revisions: Some(2),
            ..Default::default()
        },
    )
    .unwrap();
    let s = WorldSession::open(dir.path()).unwrap();
    assert!(matches!(
        s.pin_revision(3),
        Err(WorldError::RevisionCompacted(3))
    ));
    s.pin_revision(0).unwrap();
    s.pin_revision(s.current_revision()).unwrap();
}

/// An undecodable value in a trie that otherwise reads fine is corrupt state, not a
/// `NetworkError`.
#[test]
fn a_corrupt_idempotency_entry_is_corrupt_state() {
    let dir = TempDir::new("l4_entry");
    let s = keyed_world(&dir, 2);
    let mut store = FileNodeStore::new(dir.path()).unwrap();
    let bad: TrieMap<WorldKey, WorldTuple> =
        TrieMap::new().insert(idempotency_trie_key("bad"), WorldTuple::new(vec![1, 2, 3]));
    bad.persist_to_store(&mut store);
    store.flush().unwrap();
    let sc = IdempotencySidecar {
        revision_seq: s.current_revision,
        revision_digest: s.current_revision_digest.unwrap(),
        root: bad.root_digest(),
        len: 1,
    };
    sc.write(&s.paths).unwrap();
    let idx = IdempotencyIndex::load(&s.paths, &store, s.current_revision, sc.revision_digest);
    let err = idx.get("bad", &store).err().unwrap();
    assert!(matches!(err, WorldError::CorruptState(_)), "{err:?}");
}

// ---- L5 ----

#[test]
fn gc_counts_unique_dead_nodes_and_sweeps_checkpoint_leftovers() {
    let dir = TempDir::new("l5_counts");
    let mut s = keyed_world(&dir, 6);
    for i in 0..4u64 {
        commit(&mut s, &format!("flip{i}"), vec![remove("orders", 100)]);
        commit(&mut s, &format!("flop{i}"), vec![order(100, 0, 1, true)]);
    }
    drop(s);
    // The same (unreachable) node written by two handles lands in two packs.
    let dup = b"duplicated dead node".to_vec();
    let dup_digest = Digest::of(Domain::Value, &dup);
    for _ in 0..2 {
        let mut st = FileNodeStore::new(dir.path()).unwrap();
        st.put_new_node(dup_digest, dup.clone());
        st.flush().unwrap();
    }
    // Distinct digests across all idx files, computed independently.
    let mut seen = std::collections::BTreeSet::new();
    let mut entries = 0u64;
    for item in fs::read_dir(dir.path().join("objects")).unwrap().flatten() {
        let p = item.path();
        if p.extension().and_then(|e| e.to_str()) == Some("idx") {
            let b = fs::read(&p).unwrap();
            let header = 8 + 8 + 32 + 256 * 4;
            let n = (b.len() - header - 32) / 80;
            for i in 0..n {
                let at = header + i * 80;
                seen.insert(b[at..at + 32].to_vec());
                entries += 1;
            }
        }
    }
    let stale = dir.path().join("settlements.tmp.99999");
    fs::write(&stale, b"leftover").unwrap();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    fs::OpenOptions::new()
        .write(true)
        .open(&stale)
        .unwrap()
        .set_modified(old)
        .unwrap();
    let fresh = dir.path().join("settlements.tmp.99998");
    fs::write(&fresh, b"live reader").unwrap();
    assert!(entries > seen.len() as u64, "fixture must hold a duplicate");
    let r = gc_world(dir.path(), &GcOptions::default()).unwrap();
    assert_eq!(
        r.nodes_before,
        seen.len() as u64,
        "entries {entries}: {r:?}"
    );
    assert_eq!(r.dead_nodes, r.nodes_before - r.live_nodes);
    assert!(!stale.exists(), "old leftover not swept");
    assert!(
        fresh.exists(),
        "a recent tmp (maybe a live reader) must be kept"
    );
}

// ---- L6: a seq-0 sidecar is checked ----

#[test]
fn a_genesis_sidecar_with_a_wrong_digest_is_not_trusted() {
    let dir = TempDir::new("l6_genesis");
    drop(keyed_world(&dir, 3));
    let paths = WorldPaths::new(dir.path());
    let sc = IdempotencySidecar::read(&paths.idempotency_file()).unwrap();
    IdempotencySidecar {
        revision_seq: 0,
        revision_digest: Digest::of(Domain::Value, b"not genesis"),
        root: sc.root,
        len: sc.len,
    }
    .write(&paths)
    .unwrap();
    let s = WorldSession::open_with_policy(dir.path(), no_refresh()).unwrap();
    assert_eq!(s.idempotency.source.label(), "scan");
    assert!(s.idempotency_lookup("k1").unwrap().is_some());
}
