//! R0 Phase 2 M4: restore-on-open.
//!
//! Persisted network state is a cache of the journal's consequences. These tests hold the
//! restored network to the replayed one (roots, settlements, derived relations, explanations),
//! step by step over a seeded fuzz, and check the failure policy: a damaged or inconsistent
//! `@3` world is a hard typed error, a stale validity key or a legacy revision replays, and
//! `open_replay` always recovers a world whose base facts are intact.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use brix_canon::Digest;
use brix_kb::world::network_state::{Loaded, NetworkStateHeader, OperatorRoots};
use brix_kb::world::state_store::{StateStore, SubRoot};
use brix_kb::world::{
    CrashPoint, OpenMode, TupleRecord, WorldBatch, WorldBatchOp, WorldError, WorldKey,
    WorldNetwork, WorldPaths, WorldRevision, WorldSession, REVISION_SCHEMA_V2, REVISION_SCHEMA_V3,
};
use brix_lower::module_graph::{LinkedProgram, ModuleGraph, ModuleLoaderLimits};
use proptest::prelude::*;

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

fn link(src: &str) -> LinkedProgram {
    let s = sources(src);
    let loader = |name: &str| s.get(name).cloned();
    ModuleGraph::load("root", &loader, ModuleLoaderLimits::default())
        .unwrap()
        .link()
        .unwrap()
}

struct TempDir(PathBuf);
impl TempDir {
    fn new(name: &str) -> Self {
        let p = std::env::temp_dir().join(format!("brix_r0_restore_{name}_{}", std::process::id()));
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

/// A small fixed history that exercises joins, multi-support, grouping and decisions.
fn seed(s: &mut WorldSession) {
    commit(
        s,
        "b1",
        vec![
            order(1, 0, 3, true),
            order(2, 0, 9, false),
            order(3, 1, 2, false),
            stock(10, 0, 5),
            stock(11, 0, 5),
            stock(12, 1, 1),
            ship(20, 0, "Air", 2),
            ship(21, 1, "Ground", 4),
        ],
    );
}

// ---------------------------------------------------------------------------------------
// Observations
// ---------------------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
struct Obs {
    header: NetworkStateHeader,
    settlements: String,
    derived: BTreeMap<String, Vec<String>>,
    explanations: BTreeMap<(String, String), String>,
}

fn observe_net(net: &WorldNetwork, key: brix_kb::world::ValidityKey) -> Obs {
    let settlements = net.all_settlements().unwrap();
    let mut derived = BTreeMap::new();
    for (name, _) in net.derived_relations.iter() {
        let rows = net.get_derived_tuples(name).unwrap().unwrap_or_default();
        derived.insert(
            name.clone(),
            rows.iter().map(|r| format!("{r:?}")).collect(),
        );
    }
    let mut explanations = BTreeMap::new();
    for (decide, entities) in &settlements {
        for entity in entities.keys() {
            let e = net.explain_decision_for(decide, entity).unwrap();
            explanations.insert(
                (decide.clone(), entity.clone()),
                e.map(|e| e.to_json().to_string()).unwrap_or_default(),
            );
        }
    }
    Obs {
        header: net.state_header(key),
        settlements: format!("{settlements:?}"),
        derived,
        explanations,
    }
}

fn observe(s: &WorldSession) -> Obs {
    let net = s.network.as_ref().unwrap();
    observe_net(net, s.validity_key_for(net))
}

/// Observation of a network rebuilt from the session's base facts, reading no persisted
/// operator state.
fn observe_fresh(s: &WorldSession) -> Obs {
    let fresh = s.replay_network().unwrap();
    observe_net(&fresh, s.validity_key_for(&fresh))
}

fn assert_all_agree(dir: &Path, live: &WorldSession, label: &str) {
    let restored = WorldSession::open(dir).unwrap();
    assert_eq!(restored.open_mode, OpenMode::Restored, "[{label}]");
    assert_eq!(restored.tail_batches, 0);
    let replayed = WorldSession::open_replay(dir).unwrap();
    assert_eq!(replayed.open_mode, OpenMode::Replay, "[{label}]");
    assert_eq!(replayed.replay_reason, Some("forced"));
    let want = observe_fresh(live);
    assert_eq!(observe(live), want, "[{label}] live != fresh replay");
    assert_eq!(
        observe(&restored),
        want,
        "[{label}] restored != fresh replay"
    );
    assert_eq!(
        observe(&replayed),
        want,
        "[{label}] open_replay != fresh replay"
    );
    restored.verify_state().unwrap();
    replayed.verify_state().unwrap();
}

// ---------------------------------------------------------------------------------------
// Fuzz differential
// ---------------------------------------------------------------------------------------

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

fn run_fuzz(seed: u64, steps: usize) {
    let dir = TempDir::new(&format!("fuzz{seed}"));
    let mut live = create(&dir);
    let mut rng = Lcg(seed);
    let mut next = [0u64; 3];
    for step in 0..steps {
        let ops = fuzz_ops(&mut rng, &mut next);
        commit(&mut live, &format!("s{seed}-{step}"), ops);
        assert_all_agree(dir.path(), &live, &format!("seed={seed} step={step}"));
        // Keep going from the restored session now and then, so commits are also made on
        // networks that were never replayed.
        if step % 4 == 3 {
            live = WorldSession::open(dir.path()).unwrap();
            assert_eq!(live.open_mode, OpenMode::Restored);
        }
    }
}

#[test]
fn restore_equals_replay_at_every_fuzz_step_seed_1() {
    run_fuzz(0xDEAD_BEEF_CAFE_FEED, 60);
}

#[test]
fn restore_equals_replay_at_every_fuzz_step_seed_2() {
    run_fuzz(0x1234_5678_9ABC_DEF0, 60);
}

#[test]
fn restore_equals_replay_at_every_fuzz_step_seed_3() {
    run_fuzz(0x0BAD_C0FF_EE00_1234, 60);
}

// ---------------------------------------------------------------------------------------
// Forging helpers
// ---------------------------------------------------------------------------------------

fn head_seq(dir: &Path) -> u64 {
    fs::read_to_string(WorldPaths::new(dir).head())
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

fn read_head_revision(dir: &Path) -> WorldRevision {
    let p = WorldPaths::new(dir);
    let v = serde_json::from_slice(&fs::read(p.revision_file(head_seq(dir))).unwrap()).unwrap();
    WorldRevision::from_json(&v).unwrap()
}

/// Rewrite the HEAD revision record (and HEAD itself) with a re-sealed digest.
fn forge_head(dir: &Path, f: impl FnOnce(WorldRevision) -> WorldRevision) {
    let p = WorldPaths::new(dir);
    let rev = f(read_head_revision(dir));
    fs::write(
        p.revision_file(rev.seq),
        serde_json::to_vec_pretty(&rev.to_json()).unwrap(),
    )
    .unwrap();
    fs::write(
        p.head(),
        format!("{} {}\n", rev.seq, rev.revision_digest.to_hex()),
    )
    .unwrap();
}

fn header_of(dir: &Path) -> NetworkStateHeader {
    let rev = read_head_revision(dir);
    let store = StateStore::open_file(dir, 1 << 20).unwrap();
    NetworkStateHeader::read(&store, &rev.network_state_root.unwrap()).unwrap()
}

/// Replace the HEAD revision's header with `edit(header)` and re-seal the revision.
fn forge_header(dir: &Path, edit: impl FnOnce(&mut NetworkStateHeader)) {
    let mut h = header_of(dir);
    edit(&mut h);
    let store = StateStore::open_file(dir, 1 << 20).unwrap();
    let d = h.write(&store);
    store.flush().unwrap();
    forge_head(dir, |r| r.with_network_state_root(d));
}

fn all_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            all_files(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// Flip one byte in the middle of the (single) stored copy of `needle`.
fn flip_byte_in_store(dir: &Path, needle: &[u8]) {
    let mut files = Vec::new();
    all_files(&dir.join("objects"), &mut files);
    for f in files {
        let mut bytes = fs::read(&f).unwrap();
        if let Some(at) = bytes.windows(needle.len()).position(|w| w == needle) {
            bytes[at + needle.len() / 2] ^= 0x40;
            fs::write(&f, bytes).unwrap();
            return;
        }
    }
    panic!("node bytes not found in any store file");
}

fn seeded_world(name: &str) -> (TempDir, WorldSession) {
    let dir = TempDir::new(name);
    let mut s = create(&dir);
    seed(&mut s);
    commit(
        &mut s,
        "b2",
        vec![order(4, 1, 1, true), remove("orders", 2)],
    );
    (dir, s)
}

// ---------------------------------------------------------------------------------------
// Legacy and validity-key replay
// ---------------------------------------------------------------------------------------

#[test]
fn legacy_revision_replays_then_the_next_commit_is_restorable() {
    let (dir, mut live) = seeded_world("legacy");
    let want = observe_fresh(&live);
    drop(live);
    // Turn the HEAD record into a pre-M4 `@2` record.
    forge_head(dir.path(), |mut r| {
        r.schema = REVISION_SCHEMA_V2.to_string();
        r.network_state_root = None;
        r.revision_digest = r.compute_digest_for_record();
        r
    });
    let s = WorldSession::open(dir.path()).unwrap();
    assert_eq!(s.open_mode, OpenMode::Replay);
    assert_eq!(s.replay_reason, Some("legacy-revision"));
    assert_eq!(observe(&s), want);
    live = s;
    commit(&mut live, "b3", vec![order(5, 0, 1, false)]);
    assert_eq!(read_head_revision(dir.path()).schema, REVISION_SCHEMA_V3);
    let again = WorldSession::open(dir.path()).unwrap();
    assert_eq!(again.open_mode, OpenMode::Restored);
    assert_all_agree(dir.path(), &live, "after legacy upgrade");
}

#[test]
fn revision_3_without_a_state_root_is_a_hard_error() {
    let (dir, live) = seeded_world("noroot");
    drop(live);
    forge_head(dir.path(), |mut r| {
        r.network_state_root = None;
        r.revision_digest = r.compute_digest_for_record();
        r
    });
    assert!(matches!(
        WorldSession::open(dir.path()),
        Err(WorldError::InvalidSchema(_))
    ));
    // The deliberate rebuild path still opens it.
    assert!(WorldSession::open_replay(dir.path()).is_ok());
}

#[test]
fn a_validity_key_change_replays_instead_of_failing() {
    for tweak in 0..3 {
        let (dir, live) = seeded_world(&format!("validity{tweak}"));
        let want = observe_fresh(&live);
        drop(live);
        forge_header(dir.path(), |h| match tweak {
            0 => h.validity.lowering_version = "brix.world.net-lowering@0".into(),
            1 => h.validity.codec_version = "brix.world.state-value@1".into(),
            _ => h.validity.network_fingerprint = Digest::from_bytes([7; 32]),
        });
        let mut s = WorldSession::open(dir.path()).unwrap();
        assert_eq!(s.open_mode, OpenMode::Replay, "tweak {tweak}");
        assert_eq!(s.replay_reason, Some("validity-key"));
        assert_eq!(observe(&s), want);
        commit(&mut s, "after", vec![order(9, 2, 1, true)]);
        let r = WorldSession::open(dir.path()).unwrap();
        assert_eq!(r.open_mode, OpenMode::Restored, "tweak {tweak}");
    }
}

#[test]
fn a_header_of_another_schema_replays_it_is_not_corruption() {
    let (dir, live) = seeded_world("schema");
    drop(live);
    let store = StateStore::open_file(dir.path(), 1 << 20).unwrap();
    let mut w = brix_canon::CanonWriter::new();
    w.write_bytes(b"brix.world.network-state@2");
    w.write_uint(1);
    let bytes = w.finish();
    let d = brix_canon::Digest::of(brix_canon::Domain::State, &bytes);
    let mut s2 = store.clone();
    soc_core::store::NodeStore::put_node(&mut s2, d, bytes);
    store.flush().unwrap();
    forge_head(dir.path(), |r| r.with_network_state_root(d));
    let s = WorldSession::open(dir.path()).unwrap();
    assert_eq!(s.open_mode, OpenMode::Replay);
    assert_eq!(s.replay_reason, Some("schema"));
}

// ---------------------------------------------------------------------------------------
// Negative controls
// ---------------------------------------------------------------------------------------

#[test]
fn flipped_byte_in_the_header_is_a_hard_state_error_and_replay_recovers() {
    let (dir, live) = seeded_world("flip_header");
    drop(live);
    let rev = read_head_revision(dir.path());
    let bytes = header_of(dir.path()).encode();
    let _ = rev;
    flip_byte_in_store(dir.path(), &bytes);
    // The file store latches a failed read and reports everything afterwards as absent, so
    // pack-level damage is `MissingState` until `NodeStore::get_node` returns a `Result`
    // (deviation #2, M6). Hash mismatches the store does not mask are `CorruptState`
    // (unit-tested in `network_state`).
    let err = WorldSession::open(dir.path()).err().expect("must fail");
    assert!(
        matches!(
            err,
            WorldError::CorruptState(_) | WorldError::MissingState(_)
        ),
        "{err:?}"
    );
    let s = WorldSession::open_replay(dir.path()).unwrap();
    s.verify_state().unwrap();
}

#[test]
fn flipped_byte_in_a_trie_node_is_a_hard_state_error() {
    let (dir, live) = seeded_world("flip_node");
    drop(live);
    let h = header_of(dir.path());
    // The frontier root is probed at open; read its bytes back and damage them in place.
    let store = soc_core::store::FileNodeStore::new(dir.path()).unwrap();
    let node = soc_core::store::NodeStore::get_node(&store, &h.frontier.root).unwrap();
    drop(store);
    flip_byte_in_store(dir.path(), &node);
    let err = WorldSession::open(dir.path()).err().expect("must fail");
    assert!(
        matches!(
            err,
            WorldError::CorruptState(_) | WorldError::MissingState(_)
        ),
        "{err:?}"
    );
    // Base facts are intact, so the deliberate rebuild opens and reads them.
    let s = WorldSession::open_replay(dir.path()).unwrap();
    assert!(s
        .get("root::orders", &WorldKey::from_u64(1))
        .unwrap()
        .is_some());
}

#[test]
fn a_root_missing_from_the_store_is_missing_state_and_replay_recovers() {
    let (dir, live) = seeded_world("missing");
    let want = observe_fresh(&live);
    drop(live);
    forge_header(dir.path(), |h| {
        h.frontier = SubRoot {
            root: Digest::from_bytes([9; 32]),
            len: 3,
        }
    });
    let err = WorldSession::open(dir.path()).err().expect("must fail");
    assert!(matches!(err, WorldError::MissingState(_)), "{err:?}");
    let s = WorldSession::open_replay(dir.path()).unwrap();
    assert_eq!(observe(&s), want);
}

#[test]
fn a_missing_header_is_missing_state() {
    let (dir, live) = seeded_world("noheader");
    drop(live);
    forge_head(dir.path(), |r| {
        r.with_network_state_root(Digest::from_bytes([5; 32]))
    });
    let err = WorldSession::open(dir.path()).err().expect("must fail");
    assert!(matches!(err, WorldError::MissingState(_)), "{err:?}");
    assert!(WorldSession::open_replay(dir.path()).is_ok());
}

#[test]
fn a_tampered_revision_record_fails_its_digest() {
    let (dir, live) = seeded_world("tamper_rev");
    drop(live);
    let p = WorldPaths::new(dir.path());
    let seq = head_seq(dir.path());
    let mut v: serde_json::Value =
        serde_json::from_slice(&fs::read(p.revision_file(seq)).unwrap()).unwrap();
    v["network_state_root"] = Digest::from_bytes([1; 32]).to_hex().into();
    fs::write(p.revision_file(seq), serde_json::to_vec(&v).unwrap()).unwrap();
    assert!(WorldSession::open(dir.path()).is_err());
}

#[test]
fn a_decision_root_that_disagrees_with_the_revision_is_a_validity_mismatch() {
    let (dir, live) = seeded_world("decision");
    drop(live);
    forge_header(dir.path(), |h| {
        // A real, readable trie that is not the decision trie.
        h.decision = h.frontier;
    });
    let err = WorldSession::open(dir.path()).err().expect("must fail");
    assert!(matches!(err, WorldError::ValidityMismatch(_)), "{err:?}");
    assert!(WorldSession::open_replay(dir.path()).is_ok());
}

#[test]
fn a_header_for_a_different_program_is_a_validity_mismatch() {
    let (dir, live) = seeded_world("otherprog");
    drop(live);
    forge_header(dir.path(), |h| {
        h.validity.program_digest = Digest::from_bytes([3; 32])
    });
    let err = WorldSession::open(dir.path()).err().expect("must fail");
    assert!(matches!(err, WorldError::ValidityMismatch(_)), "{err:?}");
}

#[test]
fn an_operator_set_that_does_not_match_the_program_is_a_validity_mismatch() {
    let (dir, live) = seeded_world("opset");
    drop(live);
    forge_header(dir.path(), |h| {
        let first = *h.operators.keys().next().unwrap();
        h.operators.remove(&first);
    });
    let err = WorldSession::open(dir.path()).err().expect("must fail");
    assert!(matches!(err, WorldError::ValidityMismatch(_)), "{err:?}");
}

#[test]
fn verify_state_detects_a_restored_state_that_is_wrong_but_well_formed() {
    let (dir, live) = seeded_world("divergent");
    drop(live);
    forge_header(dir.path(), |h| {
        let mut swapped = false;
        for roots in h.operators.values_mut() {
            if let OperatorRoots::Join { left, right } = roots {
                if left != right {
                    std::mem::swap(left, right);
                    swapped = true;
                    break;
                }
            }
        }
        assert!(swapped, "need a join with distinct sides");
    });
    let s = WorldSession::open(dir.path()).unwrap();
    assert_eq!(s.open_mode, OpenMode::Restored);
    assert!(matches!(
        s.verify_state(),
        Err(WorldError::StateMismatch(_))
    ));
    assert!(WorldSession::open_replay(dir.path())
        .unwrap()
        .verify_state()
        .is_ok());
}

// ---------------------------------------------------------------------------------------
// from_program on an existing world
// ---------------------------------------------------------------------------------------

#[test]
fn from_program_on_an_existing_world_does_not_poison_the_cache() {
    let (dir, live) = seeded_world("fromprog");
    drop(live);
    // A different program (a changed decide priority) is refused outright.
    let changed = link(&MODEL.replace("priority 20", "priority 70"));
    assert!(WorldSession::from_program(dir.path(), &changed).is_err());
    // Nothing about the world changed: it still restores and verifies.
    let s = WorldSession::open(dir.path()).unwrap();
    assert_eq!(s.open_mode, OpenMode::Restored);
    s.verify_state().unwrap();
    drop(s);
    // The same program reuses the restored network without a second replay.
    let mut s = WorldSession::from_program(dir.path(), &link(MODEL)).unwrap();
    assert_eq!(s.open_mode, OpenMode::Restored);
    commit(&mut s, "b3", vec![order(6, 0, 2, true)]);
    drop(s);
    let r = WorldSession::open(dir.path()).unwrap();
    assert_eq!(r.open_mode, OpenMode::Restored);
    r.verify_state().unwrap();
    assert_eq!(
        WorldSession::open_replay(dir.path())
            .unwrap()
            .network_state_root,
        r.network_state_root
    );
}

#[test]
fn the_validity_fingerprint_follows_the_program() {
    let base = WorldNetwork::from_program(&link(MODEL)).unwrap();
    let same = WorldNetwork::from_program(&link(MODEL)).unwrap();
    assert_eq!(base.fingerprint(), same.fingerprint());
    for (from, to) in [
        ("priority 20", "priority 70"),
        ("i.available >= 0", "i.available >= 1"),
        ("group by f.sku", "group by f.carrier"),
        (
            "f.available >= f.qty = \"air_express\"",
            "f.available > f.qty = \"air_express\"",
        ),
        ("per order_id", "per customer"),
    ] {
        assert!(MODEL.contains(from), "{from}");
        let changed = WorldNetwork::from_program(&link(&MODEL.replace(from, to)));
        if let Ok(n) = changed {
            assert_ne!(n.fingerprint(), base.fingerprint(), "{from} -> {to}");
        }
    }
}

// ---------------------------------------------------------------------------------------
// Crash points
// ---------------------------------------------------------------------------------------

#[test]
fn every_crash_point_leaves_old_or_new_head_with_matching_roots() {
    use CrashPoint::*;
    for (i, point) in [
        BeforeObjectsFsync,
        AfterBaseObjectsBeforeNetworkObjects,
        AfterNetworkObjectsBeforeFlush,
        AfterObjectsFsyncBeforeRevisionFsync,
        AfterDecisionDeltaFsyncBeforeRevisionFsync,
        AfterRevisionFsyncBeforeHeadRename,
        AfterHeadRenameBeforeDirectoryFsync,
    ]
    .into_iter()
    .enumerate()
    {
        let dir = TempDir::new(&format!("crash{i}"));
        let mut s = create(&dir);
        seed(&mut s);
        let old = s.current_revision;
        s.set_crash_point(Some(point));
        let b = WorldBatch::new(
            s.current_revision,
            "crashing",
            vec![order(8, 0, 4, true), stock(13, 0, 9)],
        );
        assert!(matches!(
            s.apply_batch(b),
            Err(WorldError::InjectedCrash(p)) if p == point
        ));
        drop(s);
        let head = head_seq(dir.path());
        assert!(head == old || head == old + 1, "{point}: head {head}");
        let live = WorldSession::open(dir.path()).unwrap();
        assert_eq!(live.current_revision, head);
        assert_eq!(live.open_mode, OpenMode::Restored, "{point}");
        assert_all_agree(dir.path(), &live, &format!("{point}"));
        // The world is writable afterwards, whichever side of the crash it landed on.
        let mut live = live;
        commit(&mut live, "after-crash", vec![order(14, 1, 1, false)]);
        assert_all_agree(dir.path(), &live, &format!("{point} + commit"));
    }
}

// ---------------------------------------------------------------------------------------
// Receipts and idempotency
// ---------------------------------------------------------------------------------------

#[test]
fn idempotency_and_receipts_work_after_restore() {
    let (dir, live) = seeded_world("idem");
    drop(live);
    let mut s = WorldSession::open(dir.path()).unwrap();
    assert_eq!(s.open_mode, OpenMode::Restored);
    // "b2" was committed before the reopen.
    let replay = s
        .apply_batch(WorldBatch::new(
            1,
            "b2",
            vec![order(4, 1, 1, true), remove("orders", 2)],
        ))
        .unwrap();
    assert!(replay.is_idempotent_replay);
    assert_eq!(replay.revision_seq, 2);
    let conflict = s.apply_batch(WorldBatch::new(1, "b2", vec![order(99, 1, 1, true)]));
    assert!(matches!(
        conflict,
        Err(WorldError::IdempotencyConflict { .. })
    ));
    // A fresh key commits against the restored network and is itself replay-safe.
    let b = WorldBatch::new(2, "b3", vec![order(5, 0, 1, true)]);
    let first = s.apply_batch(b.clone()).unwrap();
    assert!(!first.is_idempotent_replay);
    let second = s.apply_batch(b).unwrap();
    assert!(second.is_idempotent_replay);
    assert_eq!(second.revision_digest, first.revision_digest);
    assert_all_agree(dir.path(), &s, "after idempotent commits");
}

// ---------------------------------------------------------------------------------------
// Roots do not depend on batching, order or quantum
// ---------------------------------------------------------------------------------------

fn final_roots(
    facts: &[(String, u64, brix_kb::world::WorldTuple)],
    junk: &[(String, u64, brix_kb::world::WorldTuple)],
    chunk: usize,
    quantum: usize,
    rotate: usize,
) -> Obs {
    let mut net = WorldNetwork::from_program(&link(MODEL))
        .unwrap()
        .with_scheduling_quantum(quantum);
    let to_op = |(rel, n, t): &(String, u64, brix_kb::world::WorldTuple)| WorldBatchOp::Upsert {
        relation: format!("root::{rel}"),
        key: WorldKey::from_u64(*n),
        tuple: t.clone(),
    };
    // Junk first, then the facts in a rotated order, then the junk retracted; each phase in
    // batches of `chunk` (a batch names a key once, and the phases never share a key).
    let mut rotated = facts.to_vec();
    if !rotated.is_empty() {
        let r = rotate % rotated.len();
        rotated.rotate_left(r);
    }
    let retract: Vec<WorldBatchOp> = junk
        .iter()
        .map(|(rel, n, _)| WorldBatchOp::Remove {
            relation: format!("root::{rel}"),
            key: WorldKey::from_u64(*n),
        })
        .collect();
    let phases = [
        junk.iter().map(to_op).collect::<Vec<_>>(),
        rotated.iter().map(to_op).collect(),
        retract,
    ];
    for phase in phases {
        for part in phase.chunks(chunk.max(1)) {
            let batch = WorldBatch::new(
                net.current_revision,
                format!("c{}", net.current_revision),
                part.to_vec(),
            );
            net.apply_batch(&batch).unwrap();
        }
    }
    observe_net(
        &net,
        brix_kb::world::ValidityKey::current(
            Digest::from_bytes([1; 32]),
            None,
            Digest::from_bytes([2; 32]),
        ),
    )
}

fn arb_fact() -> impl Strategy<Value = (String, u64, brix_kb::world::WorldTuple)> {
    (0u8..3, 0u64..6, 0u64..3, 1u64..9, any::<bool>()).prop_map(
        |(rel, n, sku, v, flag)| match rel {
            0 => (
                "orders".to_string(),
                n,
                rec(&[
                    ("id", &format!("O-{n}")),
                    ("customer", "C1"),
                    ("sku", &format!("SKU-{sku}")),
                    ("qty", &v.to_string()),
                    ("express", if flag { "yes" } else { "no" }),
                ]),
            ),
            1 => (
                "inventory".to_string(),
                n,
                rec(&[
                    ("id", &format!("INV-{n}")),
                    ("sku", &format!("SKU-{sku}")),
                    ("available", &v.to_string()),
                ]),
            ),
            _ => (
                "shipping".to_string(),
                n,
                rec(&[
                    ("id", &format!("SHIP-{n}")),
                    ("sku", &format!("SKU-{sku}")),
                    ("carrier", if flag { "Air" } else { "Ground" }),
                    ("lead_days", &v.to_string()),
                ]),
            ),
        },
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn roots_are_independent_of_batching_order_and_quantum(
        raw in proptest::collection::vec(arb_fact(), 1..14),
        raw_junk in proptest::collection::vec(arb_fact(), 0..5),
        chunk in 1usize..9,
        quantum in 1usize..12,
        rotate in 0usize..14,
    ) {
        // One fact per (relation, key): the final world is the same set however it is built.
        let mut by_key: BTreeMap<(String, u64), brix_kb::world::WorldTuple> = BTreeMap::new();
        for (rel, n, t) in raw {
            by_key.insert((rel, n), t);
        }
        let facts: Vec<_> = by_key.iter().map(|((r, n), t)| (r.clone(), *n, t.clone())).collect();
        // Junk uses keys the final world does not contain.
        let junk: Vec<_> = raw_junk
            .into_iter()
            .filter(|(r, n, _)| !by_key.contains_key(&(r.clone(), *n)))
            .map(|(r, n, t)| ((r, n), t))
            .collect::<BTreeMap<_, _>>()
            .into_iter()
            .map(|((r, n), t)| (r, n, t))
            .collect::<Vec<_>>();
        let baseline = final_roots(&facts, &[], facts.len().max(1), 1024, 0);
        let varied = final_roots(&facts, &junk, chunk, quantum, rotate);
        prop_assert_eq!(baseline, varied);
    }
}

#[test]
fn loaded_enum_is_reachable_from_the_public_api() {
    // Compile-time check that the staged loader is part of the public surface.
    let _ = |l: Loaded| matches!(l, Loaded::Stale(_));
}

// ---------------------------------------------------------------------------------------
// State-cache sidecar
// ---------------------------------------------------------------------------------------

use brix_kb::world::OpenPolicy;

/// A world whose HEAD is a pre-M4 `@2` record, with the observation of its true state.
fn legacy_world(name: &str) -> (TempDir, Obs) {
    let (dir, live) = seeded_world(name);
    let want = observe_fresh(&live);
    drop(live);
    forge_legacy_head(dir.path());
    (dir, want)
}

fn forge_legacy_head(dir: &Path) {
    forge_head(dir, |mut r| {
        r.schema = REVISION_SCHEMA_V2.to_string();
        r.network_state_root = None;
        r.revision_digest = r.compute_digest_for_record();
        r
    });
}

fn sidecar(dir: &Path) -> PathBuf {
    WorldPaths::new(dir).state_cache_file()
}

fn no_refresh() -> OpenPolicy {
    OpenPolicy {
        force_replay: false,
        refresh_after_replay: false,
    }
}

#[test]
fn replay_refreshes_the_sidecar_and_the_next_open_restores_from_it() {
    let (dir, want) = legacy_world("sc_basic");
    let first = WorldSession::open(dir.path()).unwrap();
    assert_eq!(first.open_mode, OpenMode::Replay);
    assert_eq!(first.replay_reason, Some("legacy-revision"));
    assert!(first.state_refreshed, "{:?}", first.state_refresh_error);
    assert!(sidecar(dir.path()).exists());
    assert_eq!(observe(&first), want);
    drop(first);

    let mut second = WorldSession::open(dir.path()).unwrap();
    assert_eq!(second.open_mode, OpenMode::Restored);
    assert_eq!(second.state_source, Some("sidecar"));
    assert_eq!(second.replay_reason, None);
    assert_eq!(observe(&second), want, "sidecar restore != replay");
    second.verify_state().unwrap();
    // The first commit after a refresh writes the delta only, and is a restorable @3.
    let receipt = second
        .apply_batch(WorldBatch::new(
            second.current_revision,
            "b3",
            vec![order(5, 0, 1, true)],
        ))
        .unwrap();
    assert!(receipt.objects_written < 100, "{}", receipt.objects_written);
    let third = WorldSession::open(dir.path()).unwrap();
    assert_eq!(third.open_mode, OpenMode::Restored);
    assert_eq!(third.state_source, Some("revision"));
    assert_all_agree(dir.path(), &third, "after sidecar restore + commit");
}

#[test]
fn a_validity_key_change_also_refreshes() {
    let (dir, live) = seeded_world("sc_key");
    let want = observe_fresh(&live);
    drop(live);
    forge_header(dir.path(), |h| {
        h.validity.lowering_version = "brix.world.net-lowering@0".into()
    });
    let first = WorldSession::open(dir.path()).unwrap();
    assert_eq!(first.replay_reason, Some("validity-key"));
    assert!(first.state_refreshed);
    drop(first);
    let second = WorldSession::open(dir.path()).unwrap();
    assert_eq!(second.state_source, Some("sidecar"));
    assert_eq!(observe(&second), want);
}

#[test]
fn a_stale_sidecar_is_ignored() {
    // HEAD advanced after the sidecar was written.
    let (dir, _) = legacy_world("sc_stale_head");
    drop(WorldSession::open(dir.path()).unwrap());
    let old = fs::read(sidecar(dir.path())).unwrap();
    let mut s = WorldSession::open(dir.path()).unwrap();
    commit(&mut s, "advance", vec![order(6, 1, 1, true)]);
    let want = observe_fresh(&s);
    drop(s);
    forge_legacy_head(dir.path());
    fs::write(sidecar(dir.path()), old).unwrap();
    let r = WorldSession::open(dir.path()).unwrap();
    assert_eq!(r.open_mode, OpenMode::Replay);
    assert_eq!(observe(&r), want);

    // Written under another validity key.
    let (dir, want) = legacy_world("sc_stale_key");
    drop(WorldSession::open(dir.path()).unwrap());
    let mut v: serde_json::Value =
        serde_json::from_slice(&fs::read(sidecar(dir.path())).unwrap()).unwrap();
    v["validity_key_digest"] = Digest::from_bytes([4; 32]).to_hex().into();
    fs::write(sidecar(dir.path()), serde_json::to_vec(&v).unwrap()).unwrap();
    // Replays; the refresh then rewrites a correct sidecar.
    let r = WorldSession::open(dir.path()).unwrap();
    assert_eq!(r.open_mode, OpenMode::Replay);
    assert_eq!(observe(&r), want);
    assert_eq!(
        WorldSession::open(dir.path()).unwrap().state_source,
        Some("sidecar")
    );
}

#[test]
fn an_unreadable_sidecar_is_a_cache_miss() {
    for garbage in [
        &b""[..],
        b"{",
        b"{\"schema\":\"brix.world.state-cache@1\"",
        b"[1,2]",
    ] {
        let (dir, want) = legacy_world("sc_garbage");
        drop(WorldSession::open(dir.path()).unwrap());
        fs::write(sidecar(dir.path()), garbage).unwrap();
        let r = WorldSession::open(dir.path()).unwrap();
        assert_eq!(r.open_mode, OpenMode::Replay);
        assert_eq!(observe(&r), want);
    }
}

#[test]
fn damage_behind_a_valid_sidecar_is_a_hard_error() {
    let (dir, _) = legacy_world("sc_damage");
    drop(WorldSession::open(dir.path()).unwrap());
    let cache = brix_kb::world::state_cache::StateCache::read(&sidecar(dir.path())).unwrap();
    let store = StateStore::open_file(dir.path(), 1 << 20).unwrap();
    let h = NetworkStateHeader::read(&store, &cache.network_state_root).unwrap();
    let node = soc_core::store::NodeStore::get_node(&store, &h.frontier.root).unwrap();
    drop(store);
    flip_byte_in_store(dir.path(), &node);
    let err = WorldSession::open(dir.path()).err().expect("must fail");
    assert!(
        matches!(
            err,
            WorldError::CorruptState(_) | WorldError::MissingState(_)
        ),
        "{err:?}"
    );
    assert!(WorldSession::open_replay(dir.path()).is_ok());
}

#[test]
fn a_held_lock_skips_the_refresh_and_open_still_succeeds() {
    let (dir, want) = legacy_world("sc_locked");
    let guard = brix_kb::world::session::WorldLockGuard::acquire(dir.path()).unwrap();
    let s = WorldSession::open(dir.path()).unwrap();
    assert_eq!(s.open_mode, OpenMode::Replay);
    assert!(!s.state_refreshed);
    assert!(s.state_refresh_error.is_some());
    assert!(!sidecar(dir.path()).exists());
    assert_eq!(observe(&s), want);
    drop(guard);
}

#[test]
fn forced_replay_never_writes_a_sidecar() {
    let (dir, _) = legacy_world("sc_forced");
    let s = WorldSession::open_replay(dir.path()).unwrap();
    assert!(!s.state_refreshed);
    assert!(!sidecar(dir.path()).exists());
    // Nor does a policy that opts out.
    let s = WorldSession::open_with_policy(dir.path(), no_refresh()).unwrap();
    assert_eq!(s.open_mode, OpenMode::Replay);
    assert!(!sidecar(dir.path()).exists());
}

#[test]
fn sidecar_crash_points_leave_a_consistent_cache() {
    for (i, point) in [
        CrashPoint::SidecarBeforeFlush,
        CrashPoint::SidecarAfterFlushBeforeRename,
        CrashPoint::SidecarAfterRename,
    ]
    .into_iter()
    .enumerate()
    {
        let (dir, want) = legacy_world(&format!("sc_crash_{i}"));
        let mut s = WorldSession::open_with_policy(dir.path(), no_refresh()).unwrap();
        s.set_crash_point(Some(point));
        assert!(matches!(
            s.refresh_state_cache(),
            Err(WorldError::InjectedCrash(p)) if p == point
        ));
        drop(s);
        let present = sidecar(dir.path()).exists();
        assert_eq!(present, point == CrashPoint::SidecarAfterRename, "{point}");
        // Whatever survived, the world opens to the true state.
        let r = WorldSession::open(dir.path()).unwrap();
        assert_eq!(observe(&r), want, "{point}");
        if present {
            assert_eq!(r.state_source, Some("sidecar"));
        }
    }
}

/// Every committed batch records where its time went; an idempotent replay commits nothing
/// and leaves the previous profile alone.
#[test]
fn commit_profile_records_ordered_phases_and_io() {
    let dir = TempDir::new("commit_profile");
    let mut s = create(&dir);
    assert!(s.last_commit_profile.is_none());
    let batch = WorldBatch::new(
        s.current_revision(),
        "profile-1",
        vec![stock(1, 1, 10), order(1, 1, 2, false)],
    );
    s.apply_batch(batch.clone()).unwrap();
    let profile = s.last_commit_profile.clone().expect("profile after commit");
    let names: Vec<&str> = profile.phases.iter().map(|(n, _)| *n).collect();
    assert_eq!(
        names,
        [
            "clone_network",
            "apply_ops",
            "decision_encode",
            "stage_base",
            "persist_base",
            "persist_state",
            "state_header",
            "flush",
            "publish",
            "dehydrate",
        ]
    );
    let sum: f64 = profile.phases.iter().map(|(_, ms)| ms).sum();
    assert!(profile.phases.iter().all(|(_, ms)| *ms >= 0.0));
    assert!(
        sum <= profile.total_ms + 1e-6,
        "{sum} > {}",
        profile.total_ms
    );

    let replay = s.apply_batch(batch).unwrap();
    assert!(replay.is_idempotent_replay);
    assert_eq!(s.last_commit_profile, Some(profile));
}
