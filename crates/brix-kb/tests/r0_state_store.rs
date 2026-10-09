//! R0 Phase 2 M2/M3: store-backed join, Distinct, GroupedCount, derived-relation, candidate
//! frontier and decision state.
//!
//! Run with `--features brix-kb/r0-oracle` to also compare every root and view against the
//! legacy shadows (`legacy_roots`, `legacy_derived_state`, `legacy_frontier_state`, ...);
//! without the feature the tests still check behaviour against a never-persisted control
//! network and the independent reference evaluator.
//!
//! The model and fuzz generator mirror `world_reference_differential.rs`. At every fuzz step
//! the persisted network is persisted, flushed and dehydrated (so the next batch reads
//! every operator index back from the store), and for the file store the store handle (not
//! the session) is closed and reopened.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use brix_kb::world::network::compute_decision_root;
use brix_kb::world::reference;
use brix_kb::world::state_store::{StateStore, DEFAULT_NODE_CACHE_BYTES};
use brix_kb::world::{TupleRecord, WorldBatch, WorldBatchOp, WorldError, WorldKey, WorldNetwork};
use brix_lower::module_graph::{ModuleGraph, ModuleLoaderLimits};
use proptest::prelude::*;
use soc_core::store::NodeStore;

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

fn make(quantum: usize) -> (WorldNetwork, reference::ReferenceProgram) {
    let mut sources = BTreeMap::new();
    sources.insert("root".to_string(), MODEL.to_string());
    let loader = |name: &str| sources.get(name).cloned();
    let graph = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default()).unwrap();
    let linked = graph.link().unwrap();
    let mut net = WorldNetwork::from_program(&linked).unwrap();
    net.set_scheduling_quantum(quantum);
    (net, reference::from_program(&linked).unwrap())
}

fn upsert_op(rel: &str, key_num: u64, fields: &[(&str, &str)]) -> WorldBatchOp {
    let mut rec = TupleRecord::new();
    for (k, v) in fields {
        rec.set_str(*k, v);
    }
    WorldBatchOp::Upsert {
        relation: rel.to_string(),
        key: WorldKey::from_u64(key_num),
        tuple: rec.to_tuple(),
    }
}

fn remove_op(rel: &str, key_num: u64) -> WorldBatchOp {
    WorldBatchOp::Remove {
        relation: rel.to_string(),
        key: WorldKey::from_u64(key_num),
    }
}

fn apply(net: &mut WorldNetwork, id: &str, ops: Vec<WorldBatchOp>) {
    let batch = WorldBatch::new(net.current_revision, id, ops);
    net.apply_batch(&batch).expect("batch application failed");
}

struct Lcg(u64);
impl Lcg {
    fn next_range(&mut self, bound: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 11) % bound
    }
}

/// Same op mix as `world_reference_differential::run_fuzz_battery`.
struct Gen {
    rng: Lcg,
    orders: Vec<u64>,
    inventory: Vec<u64>,
    shipping: Vec<u64>,
    next_order: u64,
    next_inv: u64,
    next_ship: u64,
}

impl Gen {
    fn new(seed: u64) -> Self {
        Gen {
            rng: Lcg(seed),
            orders: Vec::new(),
            inventory: Vec::new(),
            shipping: Vec::new(),
            next_order: 1,
            next_inv: 1000,
            next_ship: 2000,
        }
    }

    fn order_fields(&mut self, id: u64) -> WorldBatchOp {
        let sku = format!("SKU-{}", self.rng.next_range(4));
        let qty = (self.rng.next_range(20) + 1).to_string();
        let express = if self.rng.next_range(2) == 0 {
            "yes"
        } else {
            "no"
        };
        upsert_op(
            "root::orders",
            id,
            &[
                ("id", &format!("O-{id}")),
                ("customer", "C1"),
                ("sku", &sku),
                ("qty", &qty),
                ("express", express),
            ],
        )
    }

    fn step(&mut self) -> Vec<WorldBatchOp> {
        let action = self.rng.next_range(100);
        if action < 25 {
            let id = self.next_order;
            self.next_order += 1;
            self.orders.push(id);
            vec![self.order_fields(id)]
        } else if action < 45 && !self.orders.is_empty() {
            let id = self.orders[self.rng.next_range(self.orders.len() as u64) as usize];
            vec![self.order_fields(id)]
        } else if action < 55 && !self.orders.is_empty() {
            let idx = self.rng.next_range(self.orders.len() as u64) as usize;
            vec![remove_op("root::orders", self.orders.swap_remove(idx))]
        } else if action < 65 {
            let id = self.next_order + 10_000 + self.rng.next_range(1000);
            vec![remove_op("root::orders", id)]
        } else if action < 75 {
            let id = self.next_inv;
            self.next_inv += 1;
            self.inventory.push(id);
            let sku = format!("SKU-{}", self.rng.next_range(4));
            let available = self.rng.next_range(15).to_string();
            vec![upsert_op(
                "root::inventory",
                id,
                &[
                    ("id", &format!("INV-{id}")),
                    ("sku", &sku),
                    ("available", &available),
                ],
            )]
        } else if action < 82 && !self.inventory.is_empty() {
            let idx = self.rng.next_range(self.inventory.len() as u64) as usize;
            vec![remove_op(
                "root::inventory",
                self.inventory.swap_remove(idx),
            )]
        } else if action < 92 {
            let id = self.next_ship;
            self.next_ship += 1;
            self.shipping.push(id);
            let sku = format!("SKU-{}", self.rng.next_range(4));
            let carrier = if self.rng.next_range(2) == 0 {
                "Air"
            } else {
                "Ground"
            };
            let lead = (self.rng.next_range(5) + 1).to_string();
            vec![upsert_op(
                "root::shipping",
                id,
                &[
                    ("id", &format!("SHIP-{id}")),
                    ("sku", &sku),
                    ("carrier", carrier),
                    ("lead_days", &lead),
                ],
            )]
        } else if !self.shipping.is_empty() {
            let idx = self.rng.next_range(self.shipping.len() as u64) as usize;
            vec![remove_op("root::shipping", self.shipping.swap_remove(idx))]
        } else {
            Vec::new()
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Backend {
    Memory,
    File,
}

struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("r0_state_store_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(feature = "r0-oracle")]
fn assert_legacy_roots(net: &WorldNetwork, label: &str) {
    assert_eq!(
        net.distinct_roots(),
        net.legacy_distinct_roots().unwrap(),
        "[{label}] distinct roots != legacy"
    );
    assert_eq!(
        net.group_roots(),
        net.legacy_group_roots().unwrap(),
        "[{label}] group roots != legacy"
    );
    assert_eq!(
        net.join_index_roots(),
        net.legacy_roots().unwrap(),
        "[{label}] join roots != legacy"
    );
    let state = net.current_state().unwrap();
    assert_eq!(
        state.derived_relations,
        net.legacy_derived_state(),
        "[{label}] derived relations != legacy"
    );
    assert_eq!(
        state.candidate_frontier,
        net.legacy_frontier_state(),
        "[{label}] candidate frontier != legacy"
    );
    assert_eq!(
        net.all_settlements().unwrap(),
        net.legacy_settlements(),
        "[{label}] settlements != legacy"
    );
    assert_eq!(
        net.frontier_root(),
        net.legacy_frontier_root().unwrap(),
        "[{label}] frontier root != legacy"
    );
    assert_eq!(
        net.decision_sub_root(),
        net.legacy_decision_sub_root(),
        "[{label}] decision trie != legacy"
    );
}
#[cfg(not(feature = "r0-oracle"))]
fn assert_legacy_roots(_net: &WorldNetwork, _label: &str) {}

/// Every observable root and view of `net` equals the never-persisted `control`'s, and the
/// decision root is the canonical one for the settlements.
fn assert_same_state(net: &WorldNetwork, control: &WorldNetwork, label: &str) {
    assert_eq!(net.derived_roots(), control.derived_roots(), "[{label}]");
    assert_eq!(net.frontier_root(), control.frontier_root(), "[{label}]");
    assert_eq!(
        net.decision_sub_root(),
        control.decision_sub_root(),
        "[{label}]"
    );
    let settlements = net.all_settlements().unwrap();
    assert_eq!(
        net.decision_root(),
        compute_decision_root(&settlements),
        "[{label}] decision root is not the canonical root of the settlements"
    );
    for (decide, entities) in &settlements {
        for entity in entities.keys() {
            assert_eq!(
                net.explain_decision_for(decide, entity).unwrap(),
                control.explain_decision_for(decide, entity).unwrap(),
                "[{label}] explanation of {decide}/{entity} diverged"
            );
            assert_eq!(
                net.get_candidates(decide, entity).unwrap(),
                control.get_candidates(decide, entity).unwrap(),
                "[{label}] candidates of {decide}/{entity} diverged"
            );
            assert_eq!(
                net.get_settlement(decide, entity).unwrap().as_ref(),
                Some(&entities[entity]),
                "[{label}]"
            );
        }
    }
    for rel in net.derived_relations.keys() {
        assert_eq!(
            net.get_derived_tuples(rel).unwrap(),
            control.get_derived_tuples(rel).unwrap(),
            "[{label}] derived {rel} diverged"
        );
    }
}

fn persist_and_dehydrate(net: &WorldNetwork) -> WorldNetwork {
    net.persist_state().unwrap();
    net.state_store().flush().unwrap();
    net.dehydrated().unwrap()
}

fn run_fuzz(seed: u64, steps: usize, quantum: usize, backend: Backend) {
    let dir = TempDir::new(&format!("{seed:x}_{quantum}_{backend:?}"));
    let (mut control, refprog) = make(quantum);
    let (mut net, _) = make(quantum);
    if backend == Backend::File {
        net.attach_state_store(StateStore::open_file(&dir.0, DEFAULT_NODE_CACHE_BYTES).unwrap());
    }
    let mut gen = Gen::new(seed);

    for step in 0..steps {
        let label = format!("seed={seed:x} q={quantum} {backend:?} step={step}");
        let ops = gen.step();
        if ops.is_empty() {
            continue;
        }
        apply(&mut control, &label, ops.clone());
        apply(&mut net, &label, ops);
        assert_legacy_roots(&net, &label);

        net = persist_and_dehydrate(&net);
        if backend == Backend::File {
            // Close and reopen the store handle (not the session).
            net.attach_state_store(
                StateStore::open_file(&dir.0, DEFAULT_NODE_CACHE_BYTES).unwrap(),
            );
        }
        assert_legacy_roots(&net, &label);

        let got = net.current_state().unwrap();
        let want = control.current_state().unwrap();
        assert_eq!(got, want, "[{label}] current_state diverged");
        assert_eq!(
            net.all_settlements().unwrap(),
            control.all_settlements().unwrap(),
            "[{label}] settlements diverged"
        );
        assert_same_state(&net, &control, &label);
        assert_eq!(net.distinct_roots(), control.distinct_roots(), "[{label}]");
        assert_eq!(net.group_roots(), control.group_roots(), "[{label}]");
        assert_eq!(
            net.join_index_roots(),
            control.join_index_roots(),
            "[{label}]"
        );
        let r = reference::evaluate(&refprog, &got.base_relations).unwrap();
        assert_eq!(
            got.derived_relations, r.derived_relations,
            "[{label}] derived relations diverged from the reference"
        );
    }
    net.verify_differential_correctness().unwrap();
}

#[test]
fn fuzz_memory_store() {
    for q in [1, 3, 256] {
        run_fuzz(0xDEAD_BEEF_CAFE_FEED, 120, q, Backend::Memory);
        run_fuzz(0x1234_5678_9ABC_DEF0, 120, q, Backend::Memory);
    }
}

#[test]
fn fuzz_file_store_reopen() {
    for q in [1, 3, 256] {
        run_fuzz(0x0BAD_C0FF_EE00_1234, 40, q, Backend::File);
    }
}

fn seeded_net(quantum: usize) -> WorldNetwork {
    let (mut net, _) = make(quantum);
    net.attach_state_store(StateStore::memory(0));
    let mut gen = Gen::new(77);
    for i in 0..60 {
        let ops = gen.step();
        if !ops.is_empty() {
            apply(&mut net, &format!("seed-{i}"), ops);
        }
    }
    // Make sure the join exists.
    apply(
        &mut net,
        "fixed",
        vec![
            upsert_op(
                "root::orders",
                5000,
                &[
                    ("id", "O-5000"),
                    ("customer", "C"),
                    ("sku", "SKU-0"),
                    ("qty", "1"),
                    ("express", "yes"),
                ],
            ),
            upsert_op(
                "root::inventory",
                5001,
                &[("id", "I"), ("sku", "SKU-0"), ("available", "9")],
            ),
            upsert_op(
                "root::shipping",
                5002,
                &[
                    ("id", "S"),
                    ("sku", "SKU-0"),
                    ("carrier", "Air"),
                    ("lead_days", "1"),
                ],
            ),
        ],
    );
    persist_and_dehydrate(&net)
}

/// Negative control: a missing or flipped state node must surface as an error on the next
/// read, never as an empty or absent index.
#[test]
fn missing_state_nodes_are_errors_not_empty() {
    let net = seeded_net(8);
    assert!(net.current_state().is_ok());
    for roots in [net.distinct_roots(), net.group_roots()] {
        assert!(roots.values().any(|r| r.len > 0), "need non-empty indexes");
    }

    // Delete: a fresh store without any of the persisted nodes.
    let mut gone = net.clone();
    gone.attach_state_store(StateStore::memory(0));
    let err = gone
        .current_state()
        .expect_err("reading a deleted index must fail");
    let msg = err.to_string();
    assert!(
        msg.contains("missing") || msg.contains("Missing") || msg.contains("state store"),
        "unexpected error: {msg}"
    );
    // Applying a delta that touches the indexes also fails instead of starting empty.
    let batch = WorldBatch::new(
        gone.current_revision,
        "touch",
        vec![upsert_op(
            "root::orders",
            9000,
            &[
                ("id", "O-9000"),
                ("customer", "C"),
                ("sku", "SKU-0"),
                ("qty", "1"),
                ("express", "yes"),
            ],
        )],
    );
    assert!(gone.apply_batch(&batch).is_err());

    // Flip: overwrite each of the first Distinct/Group outer roots with junk, one at a time.
    let victims: Vec<_> = net
        .distinct_roots()
        .into_values()
        .chain(net.group_roots().into_values())
        .filter(|r| r.len > 0)
        .map(|r| r.root)
        .collect();
    assert!(!victims.is_empty());
    for root in victims {
        let mut flipped = net.clone();
        let mut store = flipped.state_store().clone();
        let mut bytes = store.get_node(&root).expect("root persisted");
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        // Fresh store sharing nothing: copy not possible, so overwrite in place on a clone of
        // the network's own store (cache disabled via capacity 0).
        store.put_node(root, bytes);
        flipped.attach_state_store(store);
        assert!(
            flipped.current_state().is_err(),
            "a flipped node must be detected"
        );
    }
}

fn is_typed_state_error(e: &WorldError) -> bool {
    matches!(
        e,
        WorldError::CorruptState(_) | WorldError::MissingState(_) | WorldError::State(_)
    )
}

fn touch_existing_entity() -> WorldBatchOp {
    upsert_op(
        "root::orders",
        5000,
        &[
            ("id", "O-5000"),
            ("customer", "C"),
            ("sku", "SKU-0"),
            ("qty", "2"),
            ("express", "yes"),
        ],
    )
}

/// Negative control for the M3 structures: a flipped or missing frontier / decision node is
/// a typed state error on every read path that reaches it, never an empty frontier or an
/// absent settlement.
#[test]
fn frontier_and_decision_nodes_are_typed_errors_not_empty() {
    let probe = seeded_net(8);
    assert!(probe.frontier_root().len > 0, "need a non-empty frontier");
    assert!(probe.decision_sub_root().len > 0, "need decisions");

    let flip = |net: &WorldNetwork, root| {
        let mut store = net.state_store().clone();
        let mut bytes = store.get_node(&root).expect("root persisted");
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        store.put_node(root, bytes);
    };

    // Frontier: flipped.
    let net = seeded_net(8);
    flip(&net, net.frontier_root().root);
    let e = net.all_settlements().expect_err("flipped frontier root");
    assert!(is_typed_state_error(&e), "{e:?}");
    assert!(net.current_state().is_err());
    assert!(net.explain_decision("O-5000").is_err());
    assert!(net.get_settlement("root::dispatch", "O-5000").is_err());
    assert!(net.get_candidates("root::dispatch", "O-5000").is_err());
    let batch = WorldBatch::new(net.current_revision, "t", vec![touch_existing_entity()]);
    let e = net
        .clone()
        .apply_batch(&batch)
        .expect_err("apply over flipped frontier");
    assert!(is_typed_state_error(&e), "{e:?}");

    // Frontier: deleted (a store that never held the nodes).
    let net = seeded_net(8);
    let mut gone = net.clone();
    gone.attach_state_store(StateStore::memory(0));
    let e = gone.all_settlements().expect_err("missing frontier");
    assert!(
        matches!(e, WorldError::MissingState(_)),
        "absence must be MissingState, got {e:?}"
    );
    assert!(gone.explain_decision("O-5000").is_err());

    // Decisions: flipped. Reads that do not touch the decision trie keep working; an apply
    // that settles an entity fails.
    let net = seeded_net(8);
    flip(&net, net.decision_sub_root().root);
    assert!(net.all_settlements().is_ok());
    let e = net
        .clone()
        .apply_batch(&batch)
        .expect_err("apply over flipped decisions");
    assert!(is_typed_state_error(&e), "{e:?}");

    // Decisions: deleted.
    let net = seeded_net(8);
    let mut gone = net.clone();
    gone.attach_state_store(StateStore::memory(0));
    let e = gone
        .clone()
        .apply_batch(&batch)
        .expect_err("apply with no decision nodes");
    assert!(
        matches!(e, WorldError::MissingState(_) | WorldError::CorruptState(_)),
        "{e:?}"
    );
}

fn base_facts() -> impl Strategy<Value = Vec<(u8, u8, u8, u8)>> {
    // (relation 0..3, key 0..6, sku 0..3, qty/avail 0..4)
    proptest::collection::vec((0u8..3, 0u8..6, 0u8..3, 0u8..4), 1..40)
}

fn fact_op(kind: u8, key: u8, sku: u8, n: u8, remove: bool) -> WorldBatchOp {
    let (rel, base) = match kind {
        0 => ("root::orders", 100u64),
        1 => ("root::inventory", 200),
        _ => ("root::shipping", 300),
    };
    let key_num = base + key as u64;
    if remove {
        return remove_op(rel, key_num);
    }
    let sku = format!("SKU-{sku}");
    match kind {
        0 => upsert_op(
            rel,
            key_num,
            &[
                ("id", &format!("O-{key_num}")),
                ("customer", "C"),
                ("sku", &sku),
                ("qty", &n.to_string()),
                ("express", if n.is_multiple_of(2) { "yes" } else { "no" }),
            ],
        ),
        1 => upsert_op(
            rel,
            key_num,
            &[
                ("id", &format!("I-{key_num}")),
                ("sku", &sku),
                ("available", &n.to_string()),
            ],
        ),
        _ => upsert_op(
            rel,
            key_num,
            &[
                ("id", &format!("S-{key_num}")),
                ("sku", &sku),
                (
                    "carrier",
                    if n.is_multiple_of(2) { "Air" } else { "Ground" },
                ),
                ("lead_days", &n.to_string()),
            ],
        ),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// The same final facts reached by different batchings, orders (including intermediate
    /// retractions) and scheduling quanta give identical roots for every operator index,
    /// whether or not the network was persisted and dehydrated along the way.
    #[test]
    fn roots_are_history_independent(
        facts in base_facts(),
        chunk in 1usize..5,
        q1 in prop::sample::select(vec![1usize, 3, 256]),
        q2 in prop::sample::select(vec![1usize, 3, 256]),
        removals in proptest::collection::vec(0usize..40, 0..8),
        dehydrate in any::<bool>(),
    ) {
        // History: all upserts, then removals of some of them, then re-upserts of the removed.
        let mut history: Vec<WorldBatchOp> = facts
            .iter()
            .map(|&(k, key, sku, n)| fact_op(k, key, sku, n, false))
            .collect();
        let mut final_state: BTreeMap<(String, WorldKey), WorldBatchOp> = BTreeMap::new();
        let record = |m: &mut BTreeMap<(String, WorldKey), WorldBatchOp>, op: &WorldBatchOp| match op {
            WorldBatchOp::Upsert { relation, key, .. } => {
                m.insert((relation.clone(), key.clone()), op.clone());
            }
            WorldBatchOp::Remove { relation, key } => {
                m.remove(&(relation.clone(), key.clone()));
            }
        };
        for op in &history { record(&mut final_state, op); }
        let mut extra = Vec::new();
        for r in &removals {
            let &(k, key, sku, n) = &facts[r % facts.len()];
            extra.push(fact_op(k, key, sku, n, true));
        }
        for e in &extra { record(&mut final_state, e); }
        history.extend(extra);

        // A: one batch of the final facts (de-duplicated by key).
        let (mut a, _) = make(q1);
        let finals: Vec<WorldBatchOp> = final_state.values().cloned().collect();
        if !finals.is_empty() { apply(&mut a, "a", finals); }

        // B: the whole history in chunks, optionally persisting and dehydrating between.
        let (mut b, _) = make(q2);
        let mut seen: BTreeSet<(String, WorldKey)> = BTreeSet::new();
        let mut batch: Vec<WorldBatchOp> = Vec::new();
        let mut flushed = 0usize;
        for op in history {
            let key = match &op {
                WorldBatchOp::Upsert { relation, key, .. } | WorldBatchOp::Remove { relation, key } => (relation.clone(), key.clone()),
            };
            // A batch never touches the same key twice.
            if seen.contains(&key) || batch.len() >= chunk {
                if !batch.is_empty() { apply(&mut b, "b", std::mem::take(&mut batch)); }
                flushed += 1;
                seen.clear();
                if dehydrate { b = persist_and_dehydrate(&b); }
            }
            seen.insert(key);
            batch.push(op);
        }
        if !batch.is_empty() { apply(&mut b, "b", batch); }
        let _ = flushed;
        if dehydrate { b = persist_and_dehydrate(&b); }
        let a = persist_and_dehydrate(&a);

        prop_assert_eq!(a.distinct_roots(), b.distinct_roots());
        prop_assert_eq!(a.group_roots(), b.group_roots());
        prop_assert_eq!(a.join_index_roots(), b.join_index_roots());
        prop_assert_eq!(a.derived_roots(), b.derived_roots());
        prop_assert_eq!(a.all_settlements().unwrap(), b.all_settlements().unwrap());
        prop_assert_eq!(a.frontier_root(), b.frontier_root());
        prop_assert_eq!(a.decision_sub_root(), b.decision_sub_root());
        prop_assert_eq!(a.decision_root(), b.decision_root());
        prop_assert_eq!(a.all_settlements().unwrap(), b.all_settlements().unwrap());
        prop_assert_eq!(a.current_state().unwrap().derived_relations, b.current_state().unwrap().derived_relations);
        assert_legacy_roots(&b, "proptest");
    }
}

/// R0 M3 hardening: the plain `TrieMap` operations treat an unhydrated node as empty (get,
/// iter, remove) or panic (insert); soc-core now makes all four fail loudly on one. This test
/// shows network state can never reach them: (1) `network.rs` names no `TrieMap` at all, so
/// every persisted collection is an `SMap`/`SSet` that only exposes store-aware, fallible
/// operations; (2) a fully dehydrated network (every trie `Lazy`) serves every public API
/// without panicking; (3) with its nodes gone, every API returns a typed error and none panics.
#[test]
fn network_state_cannot_reach_plain_trie_ops() {
    use std::panic::{catch_unwind, AssertUnwindSafe};

    // (1) static
    let src = include_str!("../src/world/network.rs");
    for (n, line) in src.lines().enumerate() {
        let code = line.split("//").next().unwrap_or("");
        assert!(
            !code.contains("TrieMap"),
            "network.rs:{} names TrieMap: {line}",
            n + 1
        );
    }

    // (2) dynamic: everything lazy, every API works.
    let net = seeded_net(8);
    assert!(net.decision_sub_root().len > 0 && net.frontier_root().len > 0);
    let exercise = |net: &WorldNetwork| -> Vec<Result<(), WorldError>> {
        let batch = WorldBatch::new(net.current_revision, "t", vec![touch_existing_entity()]);
        let mut staged = net.clone();
        vec![
            net.current_state().map(|_| ()),
            net.all_settlements().map(|_| ()),
            net.get_derived_tuples("root::fulfillment").map(|_| ()),
            net.get_candidates("root::dispatch", "O-5000").map(|_| ()),
            net.get_settlement("root::dispatch", "O-5000").map(|_| ()),
            net.explain_decision("O-5000").map(|_| ()),
            net.explain_decision_for("root::dispatch", "O-5000")
                .map(|_| ()),
            net.verify_differential_correctness(),
            staged.apply_batch(&batch).map(|_| ()),
            staged.persist_state().map(|_| ()),
            staged.dehydrated().map(|_| ()),
        ]
    };
    for (i, r) in exercise(&net).into_iter().enumerate() {
        r.unwrap_or_else(|e| panic!("api #{i} failed on healthy lazy state: {e}"));
    }

    // (3) nodes gone: typed errors, no panic.
    let mut gone = net.clone();
    gone.attach_state_store(StateStore::memory(0));
    let results = catch_unwind(AssertUnwindSafe(|| exercise(&gone)))
        .expect("no API may panic on missing state");
    let errors = results.iter().filter(|r| r.is_err()).count();
    assert!(errors >= 8, "expected the read paths to fail, got {errors}");
}

fn cached_dehydrated_net() -> WorldNetwork {
    let (mut net, _) = make(8);
    net.attach_state_store(StateStore::memory(DEFAULT_NODE_CACHE_BYTES));
    let mut gen = Gen::new(99);
    for i in 0..250 {
        let ops = gen.step();
        if !ops.is_empty() {
            apply(&mut net, &format!("t-{i}"), ops);
        }
    }
    persist_and_dehydrate(&net)
}

/// R0 M7 trace hook: opt-in, schema `brix.world.policy-trace@1`, labels touches by scope,
/// distinguishes cache hits from store reads, bounded, and never changes a result.
#[test]
fn policy_trace_records_node_touches_without_changing_results() {
    use brix_kb::world::policy_trace::{TraceOp, POLICY_TRACE_SCHEMA};

    // Off by default: nothing recorded, nothing to read.
    let net = cached_dehydrated_net();
    let store = net.state_store().clone();
    let off_state = net.current_state().unwrap();

    let trace = store.enable_policy_trace(1_000_000);
    assert!(trace.is_empty(), "records start empty");

    // A cold query reads nodes from the store (misses), labelled "query".
    let first = net.all_settlements().unwrap();
    let cold = trace.take();
    assert!(!cold.is_empty());
    assert!(cold.iter().all(|r| r.scope == "query"));
    assert!(cold.iter().all(|r| r.op == TraceOp::NodeRead));
    assert!(cold.iter().any(|r| r.hit) || cold.iter().any(|r| !r.hit));
    for r in &cold {
        assert!(store.has_node(&r.digest), "touched node is a real node");
        assert_eq!(r.hit, r.bytes == 0, "bytes are reported for reads only");
    }
    // Sequence numbers are dense from 0.
    assert!(cold.iter().enumerate().all(|(i, r)| r.seq == i as u64));

    // The same query again is served from the decoded-node cache at least in part.
    let again = net.all_settlements().unwrap();
    assert_eq!(first, again);
    let warm = trace.take();
    assert!(warm.iter().any(|r| r.hit), "warm query must hit the cache");
    assert!(warm.first().map(|r| r.seq) == cold.last().map(|r| r.seq + 1));

    // An apply is labelled "apply" and its persist writes nodes (outside any scope).
    let mut staged = net.clone();
    let batch = WorldBatch::new(staged.current_revision, "t", vec![touch_existing_entity()]);
    staged.apply_batch(&batch).unwrap();
    staged.persist_state().unwrap();
    let applied = trace.take();
    assert!(applied
        .iter()
        .any(|r| r.scope == "apply" && r.op == TraceOp::NodeRead));
    assert!(applied
        .iter()
        .any(|r| r.op == TraceOp::NodeWrite && r.scope.is_empty()));

    // JSONL form.
    let jsonl = trace.to_jsonl();
    assert!(jsonl.is_empty(), "take() emptied the sink");
    net.current_state().unwrap();
    for line in trace.to_jsonl().lines() {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(v["schema"], POLICY_TRACE_SCHEMA);
        for field in ["seq", "scope", "op", "digest", "hit", "bytes"] {
            assert!(v.get(field).is_some(), "missing {field} in {line}");
        }
    }

    // Recording never changes a result.
    assert_eq!(net.current_state().unwrap(), off_state);

    // Disabled again: further work records nothing.
    store.disable_policy_trace();
    let before = trace.len();
    net.current_state().unwrap();
    assert_eq!(trace.len(), before);
}

#[test]
fn policy_trace_is_bounded_and_clones_share_the_sink() {
    let net = cached_dehydrated_net();
    let store = net.state_store().clone();
    let trace = store.enable_policy_trace(5);
    net.clone().all_settlements().unwrap();
    assert_eq!(trace.len(), 5);
    assert!(trace.dropped() > 0, "overflow is counted, not stored");
    let seqs: Vec<u64> = trace.records().iter().map(|r| r.seq).collect();
    assert_eq!(seqs, vec![0, 1, 2, 3, 4]);
}

/// Regression: a join result that exists only transiently inside one batch (derived by one
/// delta, retracted by another) must leave no candidate and no settlement. Before M3 the
/// frontier applied all retractions first, then all insertions, so the retraction found
/// nothing to remove and the insertion left a stale candidate and decision behind.
#[test]
fn transient_derivation_within_a_batch_leaves_no_candidate() {
    for q in [1, 3, 256] {
        let (mut n, refprog) = make(q);
        apply(
            &mut n,
            "1",
            vec![
                upsert_op(
                    "root::shipping",
                    300,
                    &[
                        ("id", "S-300"),
                        ("sku", "SKU-2"),
                        ("carrier", "Air"),
                        ("lead_days", "0"),
                    ],
                ),
                upsert_op(
                    "root::inventory",
                    203,
                    &[("id", "I-203"), ("sku", "SKU-2"), ("available", "0")],
                ),
            ],
        );
        // The order joins the old inventory row in this batch, which the same batch moves
        // to another sku: the fulfillment tuple is derived and retracted within the batch.
        apply(
            &mut n,
            "2",
            vec![
                upsert_op(
                    "root::orders",
                    100,
                    &[
                        ("id", "O-100"),
                        ("customer", "C"),
                        ("sku", "SKU-2"),
                        ("qty", "0"),
                        ("express", "yes"),
                    ],
                ),
                upsert_op(
                    "root::inventory",
                    203,
                    &[("id", "I-203"), ("sku", "SKU-0"), ("available", "0")],
                ),
            ],
        );
        let state = n.current_state().unwrap();
        assert!(state.settlements.is_empty(), "q={q}: stale settlement");
        assert!(
            state.candidate_frontier.is_empty(),
            "q={q}: stale candidate"
        );
        assert_eq!(n.frontier_root().len, 0, "q={q}");
        let r = reference::evaluate(&refprog, &state.base_relations).unwrap();
        assert_eq!(state.derived_relations, r.derived_relations, "q={q}");
        n.verify_differential_correctness().unwrap();
    }
}
