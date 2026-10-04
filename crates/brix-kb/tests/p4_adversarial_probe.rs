//! Adversarial probes for P4 (ADR-0046 §4 P4 exit criteria & validation matrix).
//!
//! Evaluates the maintained operator network, truth maintenance system (TMS),
//! multi-support derivation tracking, candidate frontier re-deliberation,
//! zero-one aggregation transitions, and differential equivalence against
//! an independent full-recompute scratch oracle.
//!
//! Real, empirical measurements are printed for verifiable audit trail.

use std::collections::BTreeMap;

use brix_kb::world::{TupleRecord, WorldBatch, WorldBatchOp, WorldKey, WorldNetwork};
use brix_lower::module_graph::{ModuleGraph, ModuleLoaderLimits};

/// Helper to compile a WorldNetwork from module sources.
fn make_network(sources: &[(&str, &str)]) -> WorldNetwork {
    let mut map = BTreeMap::new();
    for (name, src) in sources {
        map.insert(name.to_string(), src.to_string());
    }
    let loader = |name: &str| map.get(name).cloned();
    let graph = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default())
        .expect("module graph load failed");
    let linked = graph.link().expect("module graph link failed");
    WorldNetwork::from_program(&linked).expect("relational lowering and network creation failed")
}

/// Helper to construct an Upsert batch op from key and string fields.
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

/// Helper to construct a Remove batch op from key.
fn remove_op(rel: &str, key_num: u64) -> WorldBatchOp {
    WorldBatchOp::Remove {
        relation: rel.to_string(),
        key: WorldKey::from_u64(key_num),
    }
}

/// Helper to apply a list of operations at a specific revision seq.
fn apply_batch(network: &mut WorldNetwork, seq: u64, id: &str, ops: Vec<WorldBatchOp>) {
    let batch = WorldBatch::new(seq, id, ops);
    network
        .apply_batch(&batch)
        .expect("batch application failed");
}

// ============================================================================
// Probe 1: Empty-to-nonempty join and reverse
// ============================================================================
#[test]
fn p01_empty_to_nonempty_join_and_reverse() {
    let src = r#"
rel input order: { id: Str, sku: Str, qty: Int } key id
rel input backorder: { id: Str, sku: Str, available: Int } key id

rel derived matched =
    select { order_id: o.id, sku: o.sku, available: b.available }
    from o in order, b in backorder
    where o.sku == b.sku
"#;

    let mut network = make_network(&[("root", src)]);

    // 1. Initial empty state: both relations empty, join output empty
    let initial = network
        .get_derived_tuples("root::matched")
        .unwrap_or_default();
    assert_eq!(
        initial.len(),
        0,
        "join on empty relations must produce 0 tuples"
    );

    // 2. Add an order (no matching backorder yet): must remain empty
    apply_batch(
        &mut network,
        0,
        "order-1",
        vec![upsert_op(
            "root::order",
            1,
            &[("id", "1"), ("sku", "SKU-A"), ("qty", "5")],
        )],
    );
    let after_order = network
        .get_derived_tuples("root::matched")
        .unwrap_or_default();
    assert_eq!(
        after_order.len(),
        0,
        "half-populated join with absent match must remain empty"
    );

    // 3. Add matching backorder: activation from 0 -> 1
    apply_batch(
        &mut network,
        1,
        "bo-1",
        vec![upsert_op(
            "root::backorder",
            101,
            &[("id", "101"), ("sku", "SKU-A"), ("available", "10")],
        )],
    );
    let after_bo = network
        .get_derived_tuples("root::matched")
        .unwrap_or_default();
    assert_eq!(
        after_bo.len(),
        1,
        "empty-to-nonempty join activation must produce exactly 1 matching tuple"
    );
    assert_eq!(after_bo[0].get_str("order_id"), Some("1"));
    assert_eq!(after_bo[0].get_str("sku"), Some("SKU-A"));
    assert_eq!(after_bo[0].get_str("available"), Some("10"));

    // 4. Add multiple matches: order 2 also with SKU-A (1 -> 2)
    apply_batch(
        &mut network,
        2,
        "order-2",
        vec![upsert_op(
            "root::order",
            2,
            &[("id", "2"), ("sku", "SKU-A"), ("qty", "3")],
        )],
    );
    let after_order2 = network
        .get_derived_tuples("root::matched")
        .unwrap_or_default();
    assert_eq!(
        after_order2.len(),
        2,
        "second matching order must expand join to 2 tuples"
    );

    // Add second matching backorder: fanout to 2x2 = 4
    apply_batch(
        &mut network,
        3,
        "bo-2",
        vec![upsert_op(
            "root::backorder",
            102,
            &[("id", "102"), ("sku", "SKU-A"), ("available", "20")],
        )],
    );
    let after_bo2 = network
        .get_derived_tuples("root::matched")
        .unwrap_or_default();
    assert_eq!(
        after_bo2.len(),
        4,
        "two orders matching two backorders must produce 4 join results"
    );

    // 5. Retract one match (order 2): shrinks 4 -> 2
    apply_batch(
        &mut network,
        4,
        "retract-order-2",
        vec![remove_op("root::order", 2)],
    );
    let after_retract_order2 = network
        .get_derived_tuples("root::matched")
        .unwrap_or_default();
    assert_eq!(
        after_retract_order2.len(),
        2,
        "retracting order 2 must cleanly retract both of its join matches (leaving 2)"
    );
    for tuple in &after_retract_order2 {
        assert_eq!(
            tuple.get_str("order_id"),
            Some("1"),
            "only order 1 matches should survive"
        );
    }

    // 6. Retract all remaining matches: shrinks 2 -> 0 (nonempty-to-empty reverse)
    apply_batch(
        &mut network,
        5,
        "retract-all",
        vec![
            remove_op("root::order", 1),
            remove_op("root::backorder", 101),
            remove_op("root::backorder", 102),
        ],
    );
    let final_tuples = network
        .get_derived_tuples("root::matched")
        .unwrap_or_default();
    assert_eq!(
        final_tuples.len(),
        0,
        "retracting all inputs must cleanly restore join to completely empty state"
    );

    // Oracle differential check
    network
        .verify_differential_correctness()
        .expect("p01 differential equivalence");
    println!("PROBE p01 PASSED: empty->nonempty (0->1->2->4) and reverse (4->2->0) verified");
}

// ============================================================================
// Probe 2: Duplicate derivations and multiple supports
// ============================================================================
#[test]
fn p02_duplicate_derivations_and_multiple_supports() {
    // An equijoin where multiple tag rows join to the same customer, projecting
    // only customer_id to a Distinct relation.
    let src = r#"
rel input customer: { id: Str, name: Str } key id
rel input tag: { id: Str, customer: Str, reason: Str } key id

rel derived priority_customer =
    select { customer: c.id }
    from c in customer, t in tag
    where c.id == t.customer

decide priority_dispatch for p in priority_customer {
    propose expedite priority 1 when true = "expedite_shipment"
}
"#;

    let mut network = make_network(&[("root", src)]);

    // Seed customer C1
    apply_batch(
        &mut network,
        0,
        "seed-customer",
        vec![upsert_op(
            "root::customer",
            1,
            &[("id", "C1"), ("name", "Alice")],
        )],
    );

    // Add first tag T1 (Path 1 derivation of { customer: "C1" })
    apply_batch(
        &mut network,
        1,
        "tag-1",
        vec![upsert_op(
            "root::tag",
            101,
            &[("id", "T1"), ("customer", "C1"), ("reason", "VIP")],
        )],
    );

    let derived_1 = network
        .get_derived_tuples("root::priority_customer")
        .unwrap();
    assert_eq!(derived_1.len(), 1, "first derivation path asserts C1");
    let cands_1 = network
        .get_candidates("root::priority_dispatch", "C1")
        .unwrap();
    assert!(
        cands_1.contains_key("expedite"),
        "frontier holds expedite candidate"
    );
    assert_eq!(
        cands_1["expedite"].supports.len(),
        1,
        "expedite has 1 support"
    );

    // Add second tag T2 (Path 2 duplicate derivation of { customer: "C1" })
    apply_batch(
        &mut network,
        2,
        "tag-2",
        vec![upsert_op(
            "root::tag",
            102,
            &[("id", "T2"), ("customer", "C1"), ("reason", "HighValue")],
        )],
    );

    let derived_2 = network
        .get_derived_tuples("root::priority_customer")
        .unwrap();
    assert_eq!(
        derived_2.len(),
        1,
        "distinct view deduplicates identical tuple C1"
    );
    let state = network.current_state();
    let distinct_op_supports = state
        .distinct_supports
        .values()
        .next()
        .expect("distinct supports");
    assert_eq!(
        distinct_op_supports[&derived_2[0]].len(),
        2,
        "distinct operator tracks exactly 2 derivation supports for tuple C1"
    );
    let cands_2 = network
        .get_candidates("root::priority_dispatch", "C1")
        .unwrap();
    assert!(
        cands_2.contains_key("expedite"),
        "frontier holds expedite candidate"
    );

    // RETRACT PATH 1: remove tag T1
    apply_batch(
        &mut network,
        3,
        "retract-tag-1",
        vec![remove_op("root::tag", 101)],
    );

    // Tuple MUST survive in distinct view and candidate frontier!
    let derived_after_first_retract = network
        .get_derived_tuples("root::priority_customer")
        .unwrap();
    assert_eq!(
        derived_after_first_retract.len(),
        1,
        "tuple C1 MUST survive in distinct view when 1 of 2 supports is retracted"
    );
    let state_after_retract_1 = network.current_state();
    let distinct_supports_after_1 = state_after_retract_1
        .distinct_supports
        .values()
        .next()
        .unwrap();
    assert_eq!(
        distinct_supports_after_1[&derived_after_first_retract[0]].len(),
        1,
        "distinct operator tracks exactly 1 remaining support after 1 path retracted"
    );
    let cands_after_first_retract = network
        .get_candidates("root::priority_dispatch", "C1")
        .unwrap();
    assert!(
        cands_after_first_retract.contains_key("expedite"),
        "candidate expedite MUST survive in frontier when 1 of 2 supports is retracted"
    );

    // RETRACT PATH 2: remove tag T2 (last support removal)
    apply_batch(
        &mut network,
        4,
        "retract-tag-2",
        vec![remove_op("root::tag", 102)],
    );

    // Tuple MUST be cleanly retracted from distinct view and candidate frontier!
    let derived_final = network
        .get_derived_tuples("root::priority_customer")
        .unwrap();
    assert_eq!(
        derived_final.len(),
        0,
        "tuple C1 MUST be cleanly retracted from distinct view when all supports are removed"
    );
    let cands_final = network.get_candidates("root::priority_dispatch", "C1");
    assert!(
        cands_final.is_none(),
        "candidate expedite MUST be cleanly retracted from frontier when all supports are removed"
    );

    let settlement_final = network.get_settlement("root::priority_dispatch", "C1");
    assert!(
        settlement_final.is_none(),
        "settlement MUST be None after all supports are retracted"
    );

    // Differential equivalence
    network
        .verify_differential_correctness()
        .expect("p02 differential equivalence");
    println!("PROBE p02 PASSED: duplicate derivation survival and clean last-support retraction verified");
}

// ============================================================================
// Probe 3: Last support removal and multi-hop retraction
// ============================================================================
#[test]
fn p03_last_support_removal_and_multi_hop_retraction() {
    // 3-hop acyclic pipeline:
    // Input A (order) -> Derived B (valid_orders) -> Derived C (ready_queue) -> Decide Proposal
    let src = r#"
rel input order: { id: Str, qty: Int, status: Str } key id

rel derived valid_orders =
    select { id: o.id, qty: o.qty, status: o.status }
    from o in order
    where o.qty > 0

rel derived ready_queue =
    select { id: v.id, qty: v.qty }
    from v in valid_orders
    where v.status == "approved"

decide dispatch for r in ready_queue {
    propose ship priority 5 when true = "ship_packet"
}
"#;

    let mut network = make_network(&[("root", src)]);

    // Hop 0: Assert Input A
    apply_batch(
        &mut network,
        0,
        "assert-order-999",
        vec![upsert_op(
            "root::order",
            999,
            &[("id", "ORD-999"), ("qty", "10"), ("status", "approved")],
        )],
    );

    // Verify propagation through Hop 1 (B)
    let b_tuples = network.get_derived_tuples("root::valid_orders").unwrap();
    assert_eq!(
        b_tuples.len(),
        1,
        "Hop 1 (valid_orders) must contain ORD-999"
    );

    // Verify propagation through Hop 2 (C)
    let c_tuples = network.get_derived_tuples("root::ready_queue").unwrap();
    assert_eq!(
        c_tuples.len(),
        1,
        "Hop 2 (ready_queue) must contain ORD-999"
    );

    // Verify propagation through Hop 3 (Decide Proposal)
    let candidate = network.get_candidates("root::dispatch", "ORD-999");
    assert!(
        candidate.is_some(),
        "Hop 3 (Decide) must hold candidate for ORD-999"
    );
    let winner = network.get_settlement("root::dispatch", "ORD-999");
    assert_eq!(
        winner.as_ref().map(|w| w.candidate_name.as_str()),
        Some("ship"),
        "settled winner must be ship"
    );

    // Now RETRACT Input A: must cascade cleanly through B, C, and remove candidate
    apply_batch(
        &mut network,
        1,
        "retract-order-999",
        vec![remove_op("root::order", 999)],
    );

    let b_after = network.get_derived_tuples("root::valid_orders").unwrap();
    assert_eq!(b_after.len(), 0, "cascade: B (valid_orders) must be empty");

    let c_after = network.get_derived_tuples("root::ready_queue").unwrap();
    assert_eq!(c_after.len(), 0, "cascade: C (ready_queue) must be empty");

    let cands_after = network.get_candidates("root::dispatch", "ORD-999");
    assert!(
        cands_after.is_none(),
        "cascade: candidate frontier must be empty for ORD-999"
    );

    let winner_after = network.get_settlement("root::dispatch", "ORD-999");
    assert!(
        winner_after.is_none(),
        "cascade: settlement must be None (quiescence)"
    );

    // Differential equivalence
    network
        .verify_differential_correctness()
        .expect("p03 differential equivalence");
    println!("PROBE p03 PASSED: 3-hop cascade retraction A -> B -> C -> Decide verified");
}

// ============================================================================
// Probe 4: Branch switching and frontier re-deliberation
// ============================================================================
#[test]
fn p04_branch_switching_and_frontier_re_deliberation() {
    let src = r#"
rel input order: { id: Str, express: Str, status: Str } key id

rel derived active_orders =
    select { id: o.id, express: o.express, status: o.status }
    from o in order

decide fulfillment for o in active_orders {
    propose expedite priority 10 when o.express == "yes" = "overnight"
    propose standard priority 20 when o.status == "open" = "ground"
}
"#;

    let mut network = make_network(&[("root", src)]);

    // 1. Initial fact: express = "yes", status = "open"
    // Both guards are true. Priority 10 (expedite) < Priority 20 (standard).
    apply_batch(
        &mut network,
        0,
        "init-order-1",
        vec![upsert_op(
            "root::order",
            1,
            &[("id", "ORD-1"), ("express", "yes"), ("status", "open")],
        )],
    );

    let winner_1 = network
        .get_settlement("root::fulfillment", "ORD-1")
        .expect("winner 1");
    assert_eq!(winner_1.candidate_name, "expedite");
    assert_eq!(winner_1.priority, 10);

    // 2. Change fact: express flips from "yes" to "no"
    // Expedite guard flips from true to false!
    apply_batch(
        &mut network,
        1,
        "flip-express-guard",
        vec![upsert_op(
            "root::order",
            1,
            &[("id", "ORD-1"), ("express", "no"), ("status", "open")],
        )],
    );

    // The winner MUST re-deliberate to standard!
    let winner_2 = network
        .get_settlement("root::fulfillment", "ORD-1")
        .expect("winner 2");
    assert_eq!(
        winner_2.candidate_name, "standard",
        "flipping expedite guard must cause frontier to re-deliberate to standard"
    );
    assert_eq!(winner_2.priority, 20);

    // Check that expedite is no longer in candidate frontier
    let cands_2 = network
        .get_candidates("root::fulfillment", "ORD-1")
        .unwrap();
    assert!(
        !cands_2.contains_key("expedite"),
        "retracted candidate must not remain in frontier"
    );
    assert!(cands_2.contains_key("standard"));

    // 3. Change fact: status flips from "open" to "cancelled"
    // Standard guard flips from true to false!
    apply_batch(
        &mut network,
        2,
        "flip-status-guard",
        vec![upsert_op(
            "root::order",
            1,
            &[("id", "ORD-1"), ("express", "no"), ("status", "cancelled")],
        )],
    );

    // Frontier must re-deliberate to None (no winner / quiescence)
    let winner_3 = network.get_settlement("root::fulfillment", "ORD-1");
    assert!(
        winner_3.is_none(),
        "when all guards are false, settlement must be None (quiescence)"
    );

    // 4. Test canonical settlement tie-breaking with equal priority
    let tie_src = r#"
rel input item: { id: Str } key id
rel derived items = select { id: i.id } from i in item
decide tie_choice for i in items {
    propose cand_alpha priority 50 when true = "alpha"
    propose cand_beta priority 50 when true = "beta"
}
"#;
    let mut tie_net = make_network(&[("root", tie_src)]);
    apply_batch(
        &mut tie_net,
        0,
        "seed-item",
        vec![upsert_op("root::item", 1, &[("id", "ITEM-1")])],
    );

    let tie_winner = tie_net
        .get_settlement("root::tie_choice", "ITEM-1")
        .expect("tie winner");
    // Verify canonical settlement discipline: winner has least Key (tiebreak digest)
    let cands = tie_net
        .get_candidates("root::tie_choice", "ITEM-1")
        .unwrap();
    let key_alpha = cands["cand_alpha"].calendar_key;
    let key_beta = cands["cand_beta"].calendar_key;
    let expected_winner = if key_alpha < key_beta {
        "cand_alpha"
    } else {
        "cand_beta"
    };
    assert_eq!(
        tie_winner.candidate_name, expected_winner,
        "canonical tiebreak must pick candidate with least Key"
    );

    network
        .verify_differential_correctness()
        .expect("p04 differential equivalence");
    tie_net
        .verify_differential_correctness()
        .expect("tie_net differential equivalence");
    println!("PROBE p04 PASSED: branch switching, guard flipping, re-deliberation, and canonical tie-break verified");
}

// ============================================================================
// Probe 5: Absent key insertion and range stability
// ============================================================================
#[test]
fn p05_absent_key_insertion_and_range_stability() {
    let src = r#"
rel input order: { id: Str, sku: Str } key id
rel input warehouse: { id: Str, sku: Str, loc: Str } key id

rel derived fulfillment =
    select { order_id: o.id, sku: o.sku, loc: w.loc }
    from o in order, w in warehouse
    where o.sku == w.sku

decide dispatch for f in fulfillment {
    propose ship priority 1 when true = f.loc
}
"#;

    let mut network = make_network(&[("root", src)]);

    // 1. Seed target key: SKU-TARGET
    apply_batch(
        &mut network,
        0,
        "seed-target",
        vec![
            upsert_op(
                "root::order",
                101,
                &[("id", "ORD-101"), ("sku", "SKU-TARGET")],
            ),
            upsert_op(
                "root::warehouse",
                501,
                &[("id", "WH-501"), ("sku", "SKU-TARGET"), ("loc", "Aisle-42")],
            ),
        ],
    );

    let state_before = network.current_state();
    let winner_before = network
        .get_settlement("root::dispatch", "ORD-101")
        .expect("winner before");
    assert_eq!(winner_before.candidate_name, "ship");

    // 2. Insert unindexed / non-matching keys:
    // An order with SKU-ABSENT-1 and warehouse with SKU-ABSENT-2
    let report = network
        .apply_ops(&[
            upsert_op(
                "root::order",
                900,
                &[("id", "ORD-900"), ("sku", "SKU-ABSENT-1")],
            ),
            upsert_op(
                "root::warehouse",
                901,
                &[("id", "WH-901"), ("sku", "SKU-ABSENT-2"), ("loc", "Bay-Z")],
            ),
        ])
        .expect("absent ops applied");

    // Absent keys have 0 matching join rows -> 0 derived tuples inserted/retracted!
    assert_eq!(
        report.derived_tuples_inserted, 0,
        "inserting non-matching keys must produce 0 joined tuples"
    );
    assert_eq!(
        report.derived_tuples_retracted, 0,
        "inserting non-matching keys must retract 0 joined tuples"
    );
    assert_eq!(
        report.candidates_inserted, 0,
        "unmatched keys must not insert candidate proposals"
    );

    // Target decisions, supports, and relations must be 100% UNMUTATED
    let winner_after = network
        .get_settlement("root::dispatch", "ORD-101")
        .expect("winner after");
    assert_eq!(
        winner_after, winner_before,
        "target decision must be completely unaffected"
    );

    let fulfillment_tuples = network.get_derived_tuples("root::fulfillment").unwrap();
    assert_eq!(
        fulfillment_tuples.len(),
        1,
        "fulfillment table size must be unchanged"
    );
    assert_eq!(fulfillment_tuples[0].get_str("order_id"), Some("ORD-101"));

    let state_after = network.current_state();
    assert_eq!(
        state_after.distinct_supports, state_before.distinct_supports,
        "supports for existing relations must remain byte-identical"
    );

    network
        .verify_differential_correctness()
        .expect("p05 differential equivalence");
    println!("PROBE p05 PASSED: absent key insertion does not mutate unrelated joins, supports, or candidate decisions");
}

// ============================================================================
// Probe 6: Grouped count zero-one transitions
// ============================================================================
#[test]
fn p06_grouped_count_zero_one_transitions() {
    let src = r#"
rel input order: { id: Str, customer: Str } key id

rel derived customer_order_count =
    select { customer: o.customer, count: count() }
    from o in order
    group by o.customer
"#;

    let mut network = make_network(&[("root", src)]);

    // Initial state: 0 rows
    let initial = network
        .get_derived_tuples("root::customer_order_count")
        .unwrap_or_default();
    assert_eq!(
        initial.len(),
        0,
        "initial group count relation must be empty"
    );

    // Step 1: Transition 0 -> 1 (group row created)
    apply_batch(
        &mut network,
        0,
        "cust-first-order",
        vec![upsert_op(
            "root::order",
            1,
            &[("id", "O1"), ("customer", "CUST-DELTA")],
        )],
    );

    let step1 = network
        .get_derived_tuples("root::customer_order_count")
        .unwrap();
    assert_eq!(step1.len(), 1, "0 -> 1: group row must be created");
    assert_eq!(step1[0].get_str("customer"), Some("CUST-DELTA"));
    assert_eq!(step1[0].get_str("count"), Some("1"), "count must be 1");

    // Step 2: Transition 1 -> 2 (group row updated to 2)
    apply_batch(
        &mut network,
        1,
        "cust-second-order",
        vec![upsert_op(
            "root::order",
            2,
            &[("id", "O2"), ("customer", "CUST-DELTA")],
        )],
    );

    let step2 = network
        .get_derived_tuples("root::customer_order_count")
        .unwrap();
    assert_eq!(
        step2.len(),
        1,
        "1 -> 2: relation must still have exactly 1 group row"
    );
    assert_eq!(step2[0].get_str("customer"), Some("CUST-DELTA"));
    assert_eq!(
        step2[0].get_str("count"),
        Some("2"),
        "count must be updated to 2"
    );

    // Step 3: Transition 2 -> 1 (group row updated to 1)
    apply_batch(
        &mut network,
        2,
        "cust-retract-second",
        vec![remove_op("root::order", 2)],
    );

    let step3 = network
        .get_derived_tuples("root::customer_order_count")
        .unwrap();
    assert_eq!(
        step3.len(),
        1,
        "2 -> 1: relation must still have exactly 1 group row"
    );
    assert_eq!(step3[0].get_str("customer"), Some("CUST-DELTA"));
    assert_eq!(
        step3[0].get_str("count"),
        Some("1"),
        "count must be updated back to 1"
    );

    // Step 4: Transition 1 -> 0 (group row removed, NOT leaving count=0 row)
    apply_batch(
        &mut network,
        3,
        "cust-retract-first",
        vec![remove_op("root::order", 1)],
    );

    let step4 = network
        .get_derived_tuples("root::customer_order_count")
        .unwrap();
    assert_eq!(
        step4.len(),
        0,
        "1 -> 0: group row MUST be completely removed, NOT left as count=0"
    );

    // Adversarial verification: verify no phantom row with count=0 exists in state
    for tuple in step4 {
        assert_ne!(
            tuple.get_str("customer"),
            Some("CUST-DELTA"),
            "ADVERSARIAL DEFECT: found residual group row with count={:?}",
            tuple.get_str("count")
        );
    }

    network
        .verify_differential_correctness()
        .expect("p06 differential equivalence");
    println!("PROBE p06 PASSED: grouped count 0->1, 1->2, 2->1, 1->0 clean transitions verified");
}

// ============================================================================
// Probe 7: Differential oracle fuzz
// ============================================================================
#[test]
fn p07_differential_oracle_fuzz() {
    let src = r#"
rel input order: { id: Str, customer: Str, sku: Str, status: Str } key id
rel input inventory: { id: Str, sku: Str, available: Int } key id

rel derived fulfillment =
    select { order_id: o.id, customer: o.customer, sku: o.sku, available: i.available }
    from o in order, i in inventory
    where o.sku == i.sku and o.status == "pending"

rel derived sku_summary =
    select { sku: o.sku, count: count() }
    from o in order
    group by o.sku

decide dispatch for f in fulfillment {
    propose expedite priority 10 when f.available > 5 = "fast_track"
    propose standard priority 30 when f.available <= 5 = "standard_queue"
}
"#;

    let mut network = make_network(&[("root", src)]);

    // Deterministic LCG pseudo-random generator
    let mut rng_state = 0x5EED_CAFE_1234_5678u64;
    let mut next_rand = || -> u64 {
        rng_state = rng_state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        rng_state
    };

    let skus = ["SKU-A", "SKU-B", "SKU-C", "SKU-D"];
    let statuses = ["pending", "shipped", "hold", "cancelled"];
    let customers = ["CUST-1", "CUST-2", "CUST-3"];

    let mut active_order_keys: Vec<u64> = Vec::new();
    let mut active_inv_keys: Vec<u64> = Vec::new();
    let mut next_order_id = 1u64;
    let mut next_inv_id = 1001u64;

    let total_mutations = 75; // Bounded in 50-100 range per ADR-0046 P4 exit criteria

    println!("Starting differential oracle fuzz: {total_mutations} mixed mutations...");

    for step in 0..total_mutations {
        let action = next_rand() % 100;
        let mut ops = Vec::new();

        if action < 45 || (active_order_keys.is_empty() && active_inv_keys.is_empty()) {
            // INSERT: 45% probability
            if next_rand() % 2 == 0 {
                let id = next_order_id;
                next_order_id += 1;
                active_order_keys.push(id);
                let sku = skus[(next_rand() as usize) % skus.len()];
                let status = statuses[(next_rand() as usize) % statuses.len()];
                let cust = customers[(next_rand() as usize) % customers.len()];
                ops.push(upsert_op(
                    "root::order",
                    id,
                    &[
                        ("id", &format!("O-{id}")),
                        ("customer", cust),
                        ("sku", sku),
                        ("status", status),
                    ],
                ));
            } else {
                let id = next_inv_id;
                next_inv_id += 1;
                active_inv_keys.push(id);
                let sku = skus[(next_rand() as usize) % skus.len()];
                let avail = format!("{}", (next_rand() % 20));
                ops.push(upsert_op(
                    "root::inventory",
                    id,
                    &[
                        ("id", &format!("INV-{id}")),
                        ("sku", sku),
                        ("available", &avail),
                    ],
                ));
            }
        } else if action < 75 {
            // UPDATE: 30% probability
            if !active_order_keys.is_empty() && (next_rand() % 2 == 0 || active_inv_keys.is_empty())
            {
                let idx = (next_rand() as usize) % active_order_keys.len();
                let id = active_order_keys[idx];
                let sku = skus[(next_rand() as usize) % skus.len()];
                let status = statuses[(next_rand() as usize) % statuses.len()];
                let cust = customers[(next_rand() as usize) % customers.len()];
                ops.push(upsert_op(
                    "root::order",
                    id,
                    &[
                        ("id", &format!("O-{id}")),
                        ("customer", cust),
                        ("sku", sku),
                        ("status", status),
                    ],
                ));
            } else if !active_inv_keys.is_empty() {
                let idx = (next_rand() as usize) % active_inv_keys.len();
                let id = active_inv_keys[idx];
                let sku = skus[(next_rand() as usize) % skus.len()];
                let avail = format!("{}", (next_rand() % 20));
                ops.push(upsert_op(
                    "root::inventory",
                    id,
                    &[
                        ("id", &format!("INV-{id}")),
                        ("sku", sku),
                        ("available", &avail),
                    ],
                ));
            }
        } else {
            // DELETE: 25% probability
            if !active_order_keys.is_empty() && (next_rand() % 2 == 0 || active_inv_keys.is_empty())
            {
                let idx = (next_rand() as usize) % active_order_keys.len();
                let id = active_order_keys.swap_remove(idx);
                ops.push(remove_op("root::order", id));
            } else if !active_inv_keys.is_empty() {
                let idx = (next_rand() as usize) % active_inv_keys.len();
                let id = active_inv_keys.swap_remove(idx);
                ops.push(remove_op("root::inventory", id));
            }
        }

        if !ops.is_empty() {
            let batch = WorldBatch::new(network.current_revision, &format!("fuzz-{step}"), ops);
            network.apply_batch(&batch).expect("fuzz batch failed");

            // Verify 100% equivalence between incremental state and scratch recomputation
            network
                .verify_differential_correctness()
                .unwrap_or_else(|e| {
                    panic!("Differential oracle equivalence failure at mutation {step}: {e}");
                });
        }
    }

    println!("PROBE p07 PASSED: {total_mutations} mixed mutations verified 100% equivalent to scratch oracle");
}

// ============================================================================
// Probe 8: Real empirical measurements (printed for nocapture)
// ============================================================================
#[test]
fn p08_measure_real_p4_numbers() {
    let src = r#"
rel input order: { id: Str, customer: Str, sku: Str, qty: Int } key id
rel input warehouse: { id: Str, sku: Str, loc: Str } key id

rel derived fulfillment =
    select { order_id: o.id, customer: o.customer, sku: o.sku, qty: o.qty, loc: w.loc }
    from o in order, w in warehouse
    where o.sku == w.sku

rel derived order_counts =
    select { sku: o.sku, count: count() }
    from o in order
    group by o.sku

decide shipping for f in fulfillment {
    propose expedite priority 10 when f.qty > 5 = "air"
    propose ground priority 20 when f.qty <= 5 = "truck"
}
"#;

    let mut network = make_network(&[("root", src)]);

    // Measure seed of 100 orders and 20 warehouses
    let mut seed_ops = Vec::new();
    for i in 0..100u64 {
        let sku = format!("SKU-{}", i % 10);
        seed_ops.push(upsert_op(
            "root::order",
            i,
            &[
                ("id", &format!("O-{i}")),
                ("customer", "C1"),
                ("sku", &sku),
                ("qty", &format!("{}", i % 15)),
            ],
        ));
    }
    for w in 0..20u64 {
        let sku = format!("SKU-{}", w % 10);
        seed_ops.push(upsert_op(
            "root::warehouse",
            1000 + w,
            &[
                ("id", &format!("W-{w}")),
                ("sku", &sku),
                ("loc", &format!("Bay-{w}")),
            ],
        ));
    }

    let seed_report = network
        .apply_batch(&WorldBatch::new(0, "seed-100", seed_ops))
        .expect("seed");

    println!("P4-MEASURED seed: ops={}, intermediate_deltas={}, derived_inserted={}, candidates_inserted={}",
        seed_report.ops_applied,
        seed_report.intermediate_deltas_count,
        seed_report.derived_tuples_inserted,
        seed_report.candidates_inserted
    );

    // Measure 1-key targeted edit (1 order update)
    let edit_report = network
        .apply_batch(&WorldBatch::new(
            1,
            "edit-1",
            vec![upsert_op(
                "root::order",
                42,
                &[
                    ("id", "O-42"),
                    ("customer", "C1"),
                    ("sku", "SKU-2"),
                    ("qty", "20"),
                ],
            )],
        ))
        .expect("edit");

    println!(
        "P4-MEASURED 1-key edit: ops={}, intermediate_deltas={}, settlements_updated={}",
        edit_report.ops_applied,
        edit_report.intermediate_deltas_count,
        edit_report.settlements.len()
    );

    // Measure absent key insertion (0 matches in join fulfillment)
    let fulfillment_before = network
        .get_derived_tuples("root::fulfillment")
        .unwrap()
        .len();
    let absent_report = network
        .apply_batch(&WorldBatch::new(
            2,
            "absent-1",
            vec![upsert_op(
                "root::order",
                9999,
                &[
                    ("id", "O-ABSENT"),
                    ("customer", "C1"),
                    ("sku", "SKU-NONE"),
                    ("qty", "1"),
                ],
            )],
        ))
        .expect("absent");

    let fulfillment_after = network
        .get_derived_tuples("root::fulfillment")
        .unwrap()
        .len();
    assert_eq!(
        fulfillment_after, fulfillment_before,
        "absent key produced zero join matches in fulfillment"
    );

    println!(
        "P4-MEASURED absent-key edit: ops={}, derived_inserted={} (1 group row), join_matches=0",
        absent_report.ops_applied, absent_report.derived_tuples_inserted
    );

    network
        .verify_differential_correctness()
        .expect("p08 differential equivalence");
}
