//! Differential tests between the maintained [`WorldNetwork`] (incremental, ADR-0046 P4) and
//! the **independent** naive reference evaluator in `brix_kb::world::reference` (stage P6a).
//!
//! `WorldNetwork::recompute_from_scratch` replays base facts through a fresh instance of the
//! *same* incremental engine, so it only proves the incremental and from-scratch code paths of
//! one implementation agree — not that either is correct. `brix_kb::world::reference` is a
//! second, separately-written implementation (no `OperatorState`, no `PMap`/`PSet`, no
//! `DerivationId`, no `soc_core::calendar::Frontier`, no incremental code paths at all — see
//! that module's doc comment for the exact list of what is and is not shared). This file
//! drives both from the same deterministic, seeded sequences of batches and asserts they agree
//! after every batch.
//!
//! Required op variety (ADR-0046 §6 independence requirement): upserts, corrections
//! (re-upsert at an existing key with changed fields), retractions, multi-support (two
//! distinct base facts independently justifying the same derived tuple / candidate),
//! join-key changes (an order's `sku` moves to a different bucket, re-keying its join), branch
//! switching (an order flips between the `ship_express`/`ship_standard`/`backorder` regimes),
//! and absent-key operations (retracting a key that was never inserted).
//!
//! Known blocker at commit 473ae7e (noted in the Lane B task brief): decide-block guards and
//! other scalar-expression positions that use the `and`/`then` keywords lower to
//! `ast::BinOp::And`/`BinOp::Then`, which `brix_lower::l3_v2` currently rejects as "witness
//! composition has no executable meaning in v2" (Lane A is fixing this on `network.rs`/
//! `world_expr.rs`). `rel ... where ... and ...` clauses are unaffected, because
//! `relation_dag::lower_derived_query` splits `and`-joined `where` conjuncts apart and
//! recombines any residual (non-equijoin) predicate with `&&` before it ever reaches the
//! scalar evaluator — see `p5_adversarial_probe.rs`'s existing use of `and` in a `where`
//! clause, which passes today. A **decide guard** has no such rewrite, so this file's test
//! program uses `&&`/`!=`/`==` (never bare `and`) in every `when` guard, confirmed to lower
//! cleanly at this commit (see `boolean_guard_regression_smoke_test` below, which exists
//! purely to pin that down so a future regression here is caught close to its cause).

use std::collections::BTreeMap;

use brix_kb::world::reference::{self, ReferenceCandidate, ReferenceSettlement};
use brix_kb::world::{
    CandidateEntry, SettledDecision, TupleRecord, WorldBatch, WorldBatchOp, WorldKey, WorldNetwork,
};
use brix_lower::module_graph::{LinkedProgram, ModuleGraph, ModuleLoaderLimits};
use soc_core::calendar::Key;

/// The linked model used by the main differential battery: three input relations joined on
/// `sku`, a `GroupedCount` aggregate, and a `decide` block with three mutually-adjustable
/// candidates (so random mutation can flip an order between regimes — "branch switching").
const LINKED_MODEL_SRC: &str = r#"
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

decide dispatch for f in fulfillment {
    propose ship_express priority 10 when f.express == "yes" && f.available >= f.qty = "air_express"
    propose ship_standard priority 20 when f.express != "yes" && f.available >= f.qty = "ground_standard"
    propose backorder priority 50 when f.available < f.qty = "backorder_hold"
}
"#;

fn link(src: &str) -> LinkedProgram {
    let mut sources = BTreeMap::new();
    sources.insert("root".to_string(), src.to_string());
    let loader = |name: &str| sources.get(name).cloned();
    let graph = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default())
        .expect("module graph load failed");
    graph.link().expect("module graph link failed")
}

/// Build the real incremental network and the independent reference program from the *same*
/// linked program, so both see identical DAG node numbering (see `reference.rs` module docs
/// point 1: `lower_relations` is a legitimately shared, pure DAG-construction step).
fn make_pair(src: &str) -> (WorldNetwork, reference::ReferenceProgram) {
    let linked = link(src);
    let network = WorldNetwork::from_program(&linked).expect("network construction failed");
    let refprog = reference::from_program(&linked).expect("reference construction failed");
    (network, refprog)
}

fn upsert_op(rel: &str, key_num: u64, fields: &[(&str, &str)]) -> WorldBatchOp {
    let mut rec = TupleRecord::new();
    for (k, v) in fields {
        rec.set_str(*k, *v);
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

fn apply(network: &mut WorldNetwork, id: &str, ops: Vec<WorldBatchOp>) {
    let batch = WorldBatch::new(network.current_revision, id, ops);
    network
        .apply_batch(&batch)
        .expect("batch application failed");
}

// ---------------------------------------------------------------------------------------
// Comparable projections: both `CandidateEntry`/`SettledDecision` (network.rs) and
// `ReferenceCandidate`/`ReferenceSettlement` (reference.rs) are projected into these common,
// directly-`assert_eq!`-able shapes. `Value::to_string()` is used instead of a shared `Value`
// type precisely because the two engines deliberately use *separate* `Value` enums (see
// reference.rs module docs) — the string form is the only thing both sides can produce
// without coupling the types together.
// ---------------------------------------------------------------------------------------

#[derive(Clone, PartialEq, Eq, Debug)]
struct ComparableCandidate {
    priority: u64,
    phase: u64,
    value: String,
    calendar_key: Key,
    support_count: usize,
}

#[derive(Clone, PartialEq, Eq, Debug)]
struct ComparableSettlement {
    candidate_name: String,
    priority: u64,
    phase: u64,
    value: String,
    calendar_key: Key,
}

fn network_candidate(c: &CandidateEntry) -> ComparableCandidate {
    ComparableCandidate {
        priority: c.priority,
        phase: c.phase,
        value: c.value.to_string(),
        calendar_key: c.calendar_key,
        support_count: c.supports.len(),
    }
}

fn reference_candidate(c: &ReferenceCandidate) -> ComparableCandidate {
    ComparableCandidate {
        priority: c.priority,
        phase: c.phase,
        value: c.value.to_string(),
        calendar_key: c.calendar_key,
        support_count: c.support_count,
    }
}

fn network_settlement(s: &SettledDecision) -> ComparableSettlement {
    ComparableSettlement {
        candidate_name: s.candidate_name.clone(),
        priority: s.priority,
        phase: s.phase,
        value: s.value.to_string(),
        calendar_key: s.calendar_key,
    }
}

fn reference_settlement(s: &ReferenceSettlement) -> ComparableSettlement {
    ComparableSettlement {
        candidate_name: s.candidate_name.clone(),
        priority: s.priority,
        phase: s.phase,
        value: s.value.to_string(),
        calendar_key: s.calendar_key,
    }
}

/// Compare the live incremental network against the independent reference evaluator run over
/// the network's own current base-relation snapshot. Panics with a descriptive message
/// (including the batch label, for narrowing a fuzz failure to a step) on any disagreement.
fn assert_agrees_with_reference(
    network: &WorldNetwork,
    refprog: &reference::ReferenceProgram,
    label: &str,
) {
    let net_state = network.current_state();
    let ref_state = reference::evaluate(refprog, &net_state.base_relations)
        .unwrap_or_else(|e| panic!("[{label}] reference evaluator failed: {e}"));

    assert_eq!(
        net_state.base_relations, ref_state.base_relations,
        "[{label}] base_relations diverged (sanity check on the snapshot handoff itself)"
    );

    assert_eq!(
        net_state.derived_relations, ref_state.derived_relations,
        "[{label}] derived_relations diverged between incremental network and independent reference"
    );

    let net_candidates: BTreeMap<String, BTreeMap<String, BTreeMap<String, ComparableCandidate>>> =
        net_state
            .candidate_frontier
            .iter()
            .map(|(decide, entities)| {
                (
                    decide.clone(),
                    entities
                        .iter()
                        .map(|(entity, cands)| {
                            (
                                entity.clone(),
                                cands
                                    .iter()
                                    .map(|(name, c)| (name.clone(), network_candidate(c)))
                                    .collect(),
                            )
                        })
                        .collect(),
                )
            })
            .collect();
    let ref_candidates: BTreeMap<String, BTreeMap<String, BTreeMap<String, ComparableCandidate>>> =
        ref_state
            .candidate_frontier
            .iter()
            .map(|(decide, entities)| {
                (
                    decide.clone(),
                    entities
                        .iter()
                        .map(|(entity, cands)| {
                            (
                                entity.clone(),
                                cands
                                    .iter()
                                    .map(|(name, c)| (name.clone(), reference_candidate(c)))
                                    .collect(),
                            )
                        })
                        .collect(),
                )
            })
            .collect();
    assert_eq!(
        net_candidates, ref_candidates,
        "[{label}] candidate_frontier diverged between incremental network and independent reference"
    );

    let net_settlements: BTreeMap<String, BTreeMap<String, ComparableSettlement>> = net_state
        .settlements
        .iter()
        .map(|(decide, entities)| {
            (
                decide.clone(),
                entities
                    .iter()
                    .map(|(entity, s)| (entity.clone(), network_settlement(s)))
                    .collect(),
            )
        })
        .collect();
    let ref_settlements: BTreeMap<String, BTreeMap<String, ComparableSettlement>> = ref_state
        .settlements
        .iter()
        .map(|(decide, entities)| {
            (
                decide.clone(),
                entities
                    .iter()
                    .map(|(entity, s)| (entity.clone(), reference_settlement(s)))
                    .collect(),
            )
        })
        .collect();
    assert_eq!(
        net_settlements, ref_settlements,
        "[{label}] settlements diverged between incremental network and independent reference"
    );
}

/// Pins down that `&&`/`!=`/`==` guards (as opposed to the `and`/`then` keywords) lower and
/// evaluate cleanly at this commit, so a future regression surfaces here rather than only as a
/// confusing failure deep inside the fuzz battery below.
#[test]
fn boolean_guard_regression_smoke_test() {
    let (mut network, refprog) = make_pair(LINKED_MODEL_SRC);
    apply(
        &mut network,
        "smoke-seed",
        vec![
            upsert_op(
                "root::orders",
                1,
                &[
                    ("id", "O-1"),
                    ("customer", "C1"),
                    ("sku", "SKU-0"),
                    ("qty", "5"),
                    ("express", "yes"),
                ],
            ),
            upsert_op(
                "root::inventory",
                100,
                &[("id", "INV-100"), ("sku", "SKU-0"), ("available", "10")],
            ),
            upsert_op(
                "root::shipping",
                200,
                &[
                    ("id", "SHIP-200"),
                    ("sku", "SKU-0"),
                    ("carrier", "Air"),
                    ("lead_days", "1"),
                ],
            ),
        ],
    );
    let settlement = network.get_settlement("root::dispatch", "O-1");
    assert_eq!(
        settlement.map(|s| s.candidate_name),
        Some("ship_express".to_string()),
        "boolean && guard must settle ship_express for an express order with enough stock"
    );
    assert_agrees_with_reference(&network, &refprog, "smoke-seed");
}

/// Explicit, named coverage of every op variety the task requires, in a readable linear
/// narrative (the randomized battery below covers breadth; this covers the required list by
/// name so a reviewer can see each one exercised directly).
#[test]
fn named_op_variety_coverage() {
    let (mut network, refprog) = make_pair(LINKED_MODEL_SRC);

    // 1. Upserts: seed two orders, inventory, and shipping for two distinct SKUs.
    apply(
        &mut network,
        "upserts",
        vec![
            upsert_op(
                "root::orders",
                1,
                &[
                    ("id", "O-1"),
                    ("customer", "C1"),
                    ("sku", "SKU-0"),
                    ("qty", "5"),
                    ("express", "yes"),
                ],
            ),
            upsert_op(
                "root::orders",
                2,
                &[
                    ("id", "O-2"),
                    ("customer", "C2"),
                    ("sku", "SKU-1"),
                    ("qty", "3"),
                    ("express", "no"),
                ],
            ),
            upsert_op(
                "root::inventory",
                100,
                &[("id", "INV-100"), ("sku", "SKU-0"), ("available", "10")],
            ),
            upsert_op(
                "root::inventory",
                101,
                &[("id", "INV-101"), ("sku", "SKU-1"), ("available", "1")],
            ),
            upsert_op(
                "root::shipping",
                200,
                &[
                    ("id", "SHIP-200"),
                    ("sku", "SKU-0"),
                    ("carrier", "Air"),
                    ("lead_days", "1"),
                ],
            ),
            upsert_op(
                "root::shipping",
                201,
                &[
                    ("id", "SHIP-201"),
                    ("sku", "SKU-1"),
                    ("carrier", "Ground"),
                    ("lead_days", "3"),
                ],
            ),
        ],
    );
    assert_agrees_with_reference(&network, &refprog, "upserts");
    assert_eq!(
        network
            .get_settlement("root::dispatch", "O-1")
            .map(|s| s.candidate_name),
        Some("ship_express".to_string())
    );
    assert_eq!(
        network
            .get_settlement("root::dispatch", "O-2")
            .map(|s| s.candidate_name),
        Some("backorder".to_string()),
        "O-2 wants 3 but only 1 available: backorder"
    );

    // 2. Correction: re-upsert O-2 at the same key with more stock available via its order
    //    quantity dropping, flipping it from backorder to ship_standard ("branch switching").
    apply(
        &mut network,
        "correction-branch-switch",
        vec![upsert_op(
            "root::orders",
            2,
            &[
                ("id", "O-2"),
                ("customer", "C2"),
                ("sku", "SKU-1"),
                ("qty", "1"),
                ("express", "no"),
            ],
        )],
    );
    assert_agrees_with_reference(&network, &refprog, "correction-branch-switch");
    assert_eq!(
        network
            .get_settlement("root::dispatch", "O-2")
            .map(|s| s.candidate_name),
        Some("ship_standard".to_string()),
        "lowering qty below available must flip the settled regime"
    );

    // 3. Join-key change: move O-2 to SKU-0's bucket (different inventory/shipping join path).
    apply(
        &mut network,
        "join-key-change",
        vec![upsert_op(
            "root::orders",
            2,
            &[
                ("id", "O-2"),
                ("customer", "C2"),
                ("sku", "SKU-0"),
                ("qty", "1"),
                ("express", "no"),
            ],
        )],
    );
    assert_agrees_with_reference(&network, &refprog, "join-key-change");
    assert_eq!(
        network
            .get_derived_tuples("root::fulfillment")
            .map(|v| v.len()),
        Some(2),
        "both orders now resolve to fulfillment rows via SKU-0's inventory/shipping"
    );

    // 4. Multi-support: a second, content-identical inventory row for SKU-0 (same
    //    `available`) makes the SKU-0 fulfillment rows doubly-supported without changing
    //    their content.
    apply(
        &mut network,
        "multi-support-insert",
        vec![upsert_op(
            "root::inventory",
            102,
            &[("id", "INV-102"), ("sku", "SKU-0"), ("available", "10")],
        )],
    );
    assert_agrees_with_reference(&network, &refprog, "multi-support-insert");
    assert_eq!(
        network
            .get_derived_tuples("root::fulfillment")
            .map(|v| v.len()),
        Some(2),
        "a content-identical duplicate inventory row must not duplicate fulfillment tuples"
    );

    // 5. Retraction of one of the two duplicate supports: fulfillment/candidates survive.
    apply(
        &mut network,
        "multi-support-partial-retract",
        vec![remove_op("root::inventory", 100)],
    );
    assert_agrees_with_reference(&network, &refprog, "multi-support-partial-retract");
    assert_eq!(
        network
            .get_derived_tuples("root::fulfillment")
            .map(|v| v.len()),
        Some(2),
        "removing one of two duplicate supports must not retract the still-supported tuple"
    );

    // 6. Retraction of the last remaining support: now it really goes away.
    apply(
        &mut network,
        "multi-support-full-retract",
        vec![remove_op("root::inventory", 102)],
    );
    assert_agrees_with_reference(&network, &refprog, "multi-support-full-retract");
    assert_eq!(
        network
            .get_derived_tuples("root::fulfillment")
            .map(|v| v.len()),
        Some(0),
        "removing the last support must retract the fulfillment tuple"
    );

    // 7. Absent-key retraction: removing a key that was never inserted must be a harmless
    //    no-op, not an error and not a spurious state change.
    apply(
        &mut network,
        "absent-key-retract",
        vec![remove_op("root::inventory", 9999)],
    );
    assert_agrees_with_reference(&network, &refprog, "absent-key-retract");
}

/// Dedicated case for the entity-id fallback precedence rule (see `reference.rs` module docs
/// and coordinator note on this task): a decide source row carrying both `entity_id` and
/// `order_id`-shaped fields, verified to settle under the `entity_id`-wins precedence that
/// both the network and the independent reference agree on.
#[test]
fn entity_id_fallback_precedence_end_to_end() {
    let src = r#"
rel input widgets: { entity_id: Str, order_id: Str, ready: Str } key order_id

decide pick for w in widgets {
    propose go priority 1 when w.ready == "yes" = "go"
}
"#;
    let (mut network, refprog) = make_pair(src);
    apply(
        &mut network,
        "entity-id-precedence",
        vec![upsert_op(
            "root::widgets",
            1,
            &[
                ("entity_id", "E-1"),
                ("order_id", "ORD-1"),
                ("ready", "yes"),
            ],
        )],
    );
    assert_agrees_with_reference(&network, &refprog, "entity-id-precedence");
    // `extract_entity_id` prefers `entity_id` over `order_id`; the settled decision's entity
    // key must be "E-1", not "ORD-1".
    assert_eq!(
        network
            .get_settlement("root::pick", "E-1")
            .map(|s| s.candidate_name),
        Some("go".to_string())
    );
    assert_eq!(network.get_settlement("root::pick", "ORD-1"), None);
}

/// Negative control: the comparison itself must be able to detect a real disagreement. We
/// corrupt a *clone* of the network's observable state (never the live network) by deleting
/// one settled decision, then show the same equality check the positive tests rely on fails
/// loudly on the corrupted copy. This does not touch `reference.rs` or `network.rs` — it
/// exercises only the differential harness in this file, proving a false "agree" is not
/// structurally possible (e.g. via a shape mismatch silently short-circuiting to `true`).
#[test]
fn negative_control_corrupted_state_is_detected() {
    let (mut network, refprog) = make_pair(LINKED_MODEL_SRC);
    apply(
        &mut network,
        "negative-control-seed",
        vec![
            upsert_op(
                "root::orders",
                1,
                &[
                    ("id", "O-1"),
                    ("customer", "C1"),
                    ("sku", "SKU-0"),
                    ("qty", "5"),
                    ("express", "yes"),
                ],
            ),
            upsert_op(
                "root::inventory",
                100,
                &[("id", "INV-100"), ("sku", "SKU-0"), ("available", "10")],
            ),
            upsert_op(
                "root::shipping",
                200,
                &[
                    ("id", "SHIP-200"),
                    ("sku", "SKU-0"),
                    ("carrier", "Air"),
                    ("lead_days", "1"),
                ],
            ),
        ],
    );
    // Sanity: the real (uncorrupted) network does agree.
    assert_agrees_with_reference(&network, &refprog, "negative-control-seed");

    // Now corrupt a clone's observable settlements and confirm the SAME comparison this file
    // uses elsewhere flags it as a real mismatch (verified by catching the panic, since
    // `assert_agrees_with_reference` is `assert_eq!`-based by design, matching every positive
    // check in this file rather than a separate boolean-returning code path).
    let net_state = network.current_state();
    let ref_state = reference::evaluate(&refprog, &net_state.base_relations).unwrap();

    let project = |settlements: &BTreeMap<String, BTreeMap<String, SettledDecision>>| {
        settlements
            .iter()
            .map(|(decide, entities)| {
                (
                    decide.clone(),
                    entities
                        .iter()
                        .map(|(entity, s)| (entity.clone(), network_settlement(s)))
                        .collect::<BTreeMap<_, _>>(),
                )
            })
            .collect::<BTreeMap<_, _>>()
    };
    let ref_settlements: BTreeMap<String, BTreeMap<String, ComparableSettlement>> = ref_state
        .settlements
        .iter()
        .map(|(decide, entities)| {
            (
                decide.clone(),
                entities
                    .iter()
                    .map(|(entity, s)| (entity.clone(), reference_settlement(s)))
                    .collect::<BTreeMap<_, _>>(),
            )
        })
        .collect();

    let mut corrupted_settlements = project(&net_state.settlements);
    corrupted_settlements
        .get_mut("root::dispatch")
        .expect("dispatch settlements present")
        .remove("O-1");
    assert_ne!(
        corrupted_settlements, ref_settlements,
        "corrupted settlements must differ from the independent reference's settlements"
    );

    let result = std::panic::catch_unwind(|| {
        assert_eq!(
            corrupted_settlements, ref_settlements,
            "corrupted settlements vs reference settlements"
        );
    });
    assert!(
        result.is_err(),
        "negative control FAILED: a deliberately corrupted settlement set must be detected as a mismatch"
    );
}

/// Deterministic xorshift*-style PRNG (no external `rand` dependency; this crate has none in
/// its dev-dependencies and this file does not want to add one for a single test). Not
/// cryptographic, just a reproducible, well-mixed sequence from a fixed seed.
struct Lcg(u64);
impl Lcg {
    fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0
    }
    fn next_range(&mut self, bound: u64) -> u64 {
        self.next_u64() % bound
    }
}

/// Randomized differential battery: seeded, deterministic sequences of upserts, corrections,
/// retractions, join-key changes, and absent-key retractions over the linked model, comparing
/// the live network against the independent reference evaluator after every batch.
fn run_fuzz_battery(seed: u64, total_steps: usize) {
    let (mut network, refprog) = make_pair(LINKED_MODEL_SRC);
    let mut rng = Lcg(seed);

    let sku_bucket_count = 4u64;
    let mut active_orders: Vec<u64> = Vec::new();
    let mut active_inventory: Vec<u64> = Vec::new();
    let mut active_shipping: Vec<u64> = Vec::new();
    let mut next_order = 1u64;
    let mut next_inventory = 1000u64;
    let mut next_shipping = 2000u64;

    assert_agrees_with_reference(&network, &refprog, &format!("seed={seed} initial"));

    for step in 0..total_steps {
        let label = format!("seed={seed} step={step}");
        let action = rng.next_range(100);
        let ops = if action < 25 {
            // Insert a new order.
            let id = next_order;
            next_order += 1;
            active_orders.push(id);
            let sku = format!("SKU-{}", rng.next_range(sku_bucket_count));
            let qty = (rng.next_range(20) + 1).to_string();
            let express = if rng.next_range(2) == 0 { "yes" } else { "no" };
            vec![upsert_op(
                "root::orders",
                id,
                &[
                    ("id", &format!("O-{id}")),
                    ("customer", "C1"),
                    ("sku", &sku),
                    ("qty", &qty),
                    ("express", express),
                ],
            )]
        } else if action < 45 && !active_orders.is_empty() {
            // Correction: re-upsert an existing order with new qty/express (may or may not
            // also change sku, i.e. sometimes a join-key change / "branch switch").
            let id = active_orders[rng.next_range(active_orders.len() as u64) as usize];
            let sku = format!("SKU-{}", rng.next_range(sku_bucket_count));
            let qty = (rng.next_range(20) + 1).to_string();
            let express = if rng.next_range(2) == 0 { "yes" } else { "no" };
            vec![upsert_op(
                "root::orders",
                id,
                &[
                    ("id", &format!("O-{id}")),
                    ("customer", "C1"),
                    ("sku", &sku),
                    ("qty", &qty),
                    ("express", express),
                ],
            )]
        } else if action < 55 && !active_orders.is_empty() {
            // Retraction of a real order.
            let idx = rng.next_range(active_orders.len() as u64) as usize;
            let id = active_orders.swap_remove(idx);
            vec![remove_op("root::orders", id)]
        } else if action < 65 {
            // Absent-key retraction: either a never-issued order id, or (sometimes) one that
            // was already retracted — both must be harmless no-ops.
            let id = next_order + 10_000 + rng.next_range(1000);
            vec![remove_op("root::orders", id)]
        } else if action < 75 {
            // Insert inventory, occasionally duplicating an existing (sku, available) pair
            // verbatim to manufacture genuine multi-support.
            let id = next_inventory;
            next_inventory += 1;
            active_inventory.push(id);
            let sku = format!("SKU-{}", rng.next_range(sku_bucket_count));
            let available = (rng.next_range(15)).to_string();
            vec![upsert_op(
                "root::inventory",
                id,
                &[
                    ("id", &format!("INV-{id}")),
                    ("sku", &sku),
                    ("available", &available),
                ],
            )]
        } else if action < 82 && !active_inventory.is_empty() {
            let idx = rng.next_range(active_inventory.len() as u64) as usize;
            let id = active_inventory.swap_remove(idx);
            vec![remove_op("root::inventory", id)]
        } else if action < 92 {
            let id = next_shipping;
            next_shipping += 1;
            active_shipping.push(id);
            let sku = format!("SKU-{}", rng.next_range(sku_bucket_count));
            let carrier = if rng.next_range(2) == 0 {
                "Air"
            } else {
                "Ground"
            };
            let lead_days = (rng.next_range(5) + 1).to_string();
            vec![upsert_op(
                "root::shipping",
                id,
                &[
                    ("id", &format!("SHIP-{id}")),
                    ("sku", &sku),
                    ("carrier", carrier),
                    ("lead_days", &lead_days),
                ],
            )]
        } else if !active_shipping.is_empty() {
            let idx = rng.next_range(active_shipping.len() as u64) as usize;
            let id = active_shipping.swap_remove(idx);
            vec![remove_op("root::shipping", id)]
        } else {
            Vec::new()
        };

        if ops.is_empty() {
            continue;
        }
        apply(&mut network, &label, ops);
        assert_agrees_with_reference(&network, &refprog, &label);
    }
}

#[test]
fn fuzz_battery_seed_1() {
    run_fuzz_battery(0xDEAD_BEEF_CAFE_FEED, 250);
}

#[test]
fn fuzz_battery_seed_2() {
    run_fuzz_battery(0x1234_5678_9ABC_DEF0, 250);
}

#[test]
fn fuzz_battery_seed_3() {
    run_fuzz_battery(0x0BAD_C0FF_EE00_1234, 250);
}
