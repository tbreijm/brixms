//! Hand-built programs settled by the oracle, snapshotted with `insta`.
//!
//! These are the G1 evidence that the boring evaluator produces the Part III
//! kernel semantics: positive recursion, stratified `without`, masks + the
//! Part III §6 phase rule, per-kind `KeyConflict`, `?` error edges, strict
//! constraints, snapshot-isolated transactions, and `Support`/`Claim`
//! provenance answering a `why`-style query.

use brix_oracle::dsl::*;
use brix_oracle::dump::render;
use brix_oracle::eval::settle;
use brix_oracle::program::{BinOp, Expr, Program, RelKind, RelationDef, Severity};
use brix_oracle::row::Extent;
use brix_oracle::store::{CommitError, Store};
use brix_oracle::txn::Transaction;
use brix_oracle::value::Value;
use std::collections::BTreeMap;

/// A partial fn used for the error-edge test: risk is `100 - due`, but a
/// past-due order (`due == 0`) is a validation failure (`Err`), which the
/// evaluator turns into a sealed `RuleError` (Part III §9) rather than
/// aborting the whole rule.
fn risk_model(args: &[Value]) -> Result<Value, Value> {
    match args[0] {
        Value::Nat(0) => Err(Value::Str("PastDue".to_string())),
        Value::Nat(due) => Ok(Value::Nat(100u64.saturating_sub(due))),
        _ => Err(Value::Str("MissingValue".to_string())),
    }
}

fn empty_ground() -> BTreeMap<String, Extent> {
    BTreeMap::new()
}

// ---------------------------------------------------------------------------
// 1. Positive recursion within one phase: transitive reachability.
// ---------------------------------------------------------------------------

#[test]
fn positive_recursion_reachability() {
    // rel Link { src, dst } key(src, dst)      -- ground
    // rel Reach { src, dst } key(src, dst)     -- derived
    // derive Base:  Reach(src: x, dst: y) from { Link(src: x, dst: y) }
    // derive Trans: Reach(src: x, dst: z) from { Reach(src: x, dst: y)
    //                                            Link(src: y, dst: z) }
    let program = Program::new()
        .with_relation(RelationDef::ground(
            "Link",
            &["src", "dst"],
            &["src", "dst"],
        ))
        .with_relation(RelationDef::derived(
            "Reach",
            &["src", "dst"],
            &["src", "dst"],
        ))
        .with_rule(rule(
            "Base",
            "Reach",
            &[("src", var("x")), ("dst", var("y"))],
            vec![edge("Link", &[("src", var("x")), ("dst", var("y"))])],
        ))
        .with_rule(rule(
            "Trans",
            "Reach",
            &[("src", var("x")), ("dst", var("z"))],
            vec![
                edge("Reach", &[("src", var("x")), ("dst", var("y"))]),
                edge("Link", &[("src", var("y")), ("dst", var("z"))]),
            ],
        ));
    program.validate().unwrap();
    let phases = brix_oracle::phase::infer_phases(&program).unwrap();
    // Base and Trans are one positive-recursion SCC → a single phase.
    assert_eq!(phases.len(), 1, "one recursive phase");

    // Ground chain a -> b -> c -> d.
    let mut link = Extent::new();
    for (s, d) in [("a", "b"), ("b", "c"), ("c", "d")] {
        let r = row(&[("src", Value::Str(s.into())), ("dst", Value::Str(d.into()))]);
        link.insert(brix_oracle::row::row_key(&r), edge_record(r));
    }
    let ground = BTreeMap::from([("Link".to_string(), link)]);

    let settled = settle(&program, &phases, &ground, &empty_ground(), 1);
    // Reach = transitive closure: 3 + 2 + 1 = 6 pairs.
    assert_eq!(settled.extent("Reach").unwrap().len(), 6);
    insta::assert_snapshot!("reachability", render(&settled));
}

// ---------------------------------------------------------------------------
// 2. Masks + the Part III §6 phase rule, and stratified `without`.
// ---------------------------------------------------------------------------

fn pricing_program() -> Program {
    // entity Order { key ref }
    // rel BaseAmount { order, amount } key(order)          -- ground
    // state rel ManualPrice { order, amount } key(order)   -- ground/state
    // rel ComputedPrice { order, amount } key(order)       -- derived
    // rel EffectivePrice { order, amount } key(order)      -- derived
    // rel Unpriced { order } key(order)                    -- derived (negation)
    // derive Compute: ComputedPrice(order: o, amount: a)
    //                 from { BaseAmount(order: o, amount: a) }
    // derive Override: mask(price) by manual
    //                  from { price @ ComputedPrice(order: o)
    //                         manual @ ManualPrice(order: o) }
    // derive FromComputed: EffectivePrice(order: o, amount: a)
    //                      from { ComputedPrice(order: o, amount: a) }
    // derive FromManual:   EffectivePrice(order: o, amount: a)
    //                      from { ManualPrice(order: o, amount: a) }
    // derive Waiting: Unpriced(order: o)
    //                 from { BaseAmount(order: o) without { EffectivePrice(order: o) } }
    Program::new()
        .with_relation(RelationDef::entity("Order", &["ref"], &["ref"]))
        .with_relation(RelationDef::ground(
            "BaseAmount",
            &["order", "amount"],
            &["order"],
        ))
        .with_relation(RelationDef::state(
            "ManualPrice",
            &["order", "amount"],
            &["order"],
        ))
        .with_relation(RelationDef::derived(
            "ComputedPrice",
            &["order", "amount"],
            &["order"],
        ))
        .with_relation(RelationDef::derived(
            "EffectivePrice",
            &["order", "amount"],
            &["order"],
        ))
        .with_relation(RelationDef::derived("Unpriced", &["order"], &["order"]))
        .with_rule(rule(
            "Compute",
            "ComputedPrice",
            &[("order", var("o")), ("amount", var("a"))],
            vec![edge(
                "BaseAmount",
                &[("order", var("o")), ("amount", var("a"))],
            )],
        ))
        .with_rule(mask_rule(
            "Override",
            "ComputedPrice",
            "price",
            "manual",
            vec![
                edge_bind("price", "ComputedPrice", &[("order", var("o"))]),
                edge_bind("manual", "ManualPrice", &[("order", var("o"))]),
            ],
        ))
        .with_rule(rule(
            "FromComputed",
            "EffectivePrice",
            &[("order", var("o")), ("amount", var("a"))],
            vec![edge(
                "ComputedPrice",
                &[("order", var("o")), ("amount", var("a"))],
            )],
        ))
        .with_rule(rule(
            "FromManual",
            "EffectivePrice",
            &[("order", var("o")), ("amount", var("a"))],
            vec![edge(
                "ManualPrice",
                &[("order", var("o")), ("amount", var("a"))],
            )],
        ))
        .with_rule(rule(
            "Waiting",
            "Unpriced",
            &[("order", var("o"))],
            vec![
                edge("BaseAmount", &[("order", var("o"))]),
                without(vec![edge("EffectivePrice", &[("order", var("o"))])]),
            ],
        ))
}

#[test]
fn mask_phase_rule_and_negation() {
    let program = pricing_program();
    program.validate().unwrap();
    let phases = brix_oracle::phase::infer_phases(&program).unwrap();

    // Phase rule (Part III §6): Override (mask producer of ComputedPrice)
    // sits strictly above Compute (producer) and strictly below FromComputed
    // (ordinary live read of ComputedPrice). Assert that ordering holds.
    let phase_of = |rule_id: &str| {
        phases
            .iter()
            .position(|p| p.rules.iter().any(|r| r == rule_id))
            .unwrap()
    };
    assert!(phase_of("Compute") < phase_of("Override"));
    assert!(phase_of("Override") < phase_of("FromComputed"));

    // Two orders sharing one BaseAmount source; ord-2 also has a ManualPrice
    // (so ComputedPrice for ord-2 is masked and EffectivePrice comes from
    // the manual override), ord-1 has none (EffectivePrice from computed).
    let ord1 = Value::Node(
        program.relations["Order"].node_id(&row(&[("ref", Value::Str("ord-1".into()))])),
    );
    let ord2 = Value::Node(
        program.relations["Order"].node_id(&row(&[("ref", Value::Str("ord-2".into()))])),
    );

    let mut base = Extent::new();
    for (o, amt) in [(&ord1, 100i64), (&ord2, 200)] {
        let r = row(&[("order", o.clone()), ("amount", Value::Int(amt))]);
        base.insert(brix_oracle::row::row_key(&r), edge_record(r));
    }
    let mut manual = Extent::new();
    let mr = row(&[("order", ord2.clone()), ("amount", Value::Int(95))]);
    manual.insert(brix_oracle::row::row_key(&mr), edge_record(mr));

    let ground = BTreeMap::from([
        ("BaseAmount".to_string(), base),
        ("ManualPrice".to_string(), manual),
    ]);

    let settled = settle(&program, &phases, &ground, &empty_ground(), 1);

    // ord-1 effective = 100 (computed), ord-2 effective = 95 (manual).
    // ComputedPrice for ord-2 is masked out of the live view.
    let computed = settled.extent("ComputedPrice").unwrap();
    assert_eq!(computed.len(), 1, "ord-2 ComputedPrice masked");
    let effective = settled.extent("EffectivePrice").unwrap();
    assert_eq!(effective.len(), 2);
    // Nobody is Unpriced; both have an EffectivePrice.
    assert!(settled.extent("Unpriced").unwrap().is_empty());
    // One Masked edge recorded.
    assert_eq!(settled.provenance.masked.len(), 1);

    insta::assert_snapshot!("pricing_masked", render(&settled));
}

// ---------------------------------------------------------------------------
// 3. Per-kind KeyConflict on a derived relation (Part III §8).
// ---------------------------------------------------------------------------

#[test]
fn derived_key_conflict_withholds_value() {
    // Two rules derive ComputedPrice for the same order with different
    // amounts → sealed KeyConflict, no ordinary live value for that key.
    let program = Program::new()
        .with_relation(RelationDef::ground("A", &["order", "amount"], &["order"]))
        .with_relation(RelationDef::ground("B", &["order", "amount"], &["order"]))
        .with_relation(RelationDef::derived(
            "ComputedPrice",
            &["order", "amount"],
            &["order"],
        ))
        .with_rule(rule(
            "FromA",
            "ComputedPrice",
            &[("order", var("o")), ("amount", var("a"))],
            vec![edge("A", &[("order", var("o")), ("amount", var("a"))])],
        ))
        .with_rule(rule(
            "FromB",
            "ComputedPrice",
            &[("order", var("o")), ("amount", var("a"))],
            vec![edge("B", &[("order", var("o")), ("amount", var("a"))])],
        ));
    program.validate().unwrap();
    let phases = brix_oracle::phase::infer_phases(&program).unwrap();

    let o = Value::Str("ord-1".into());
    let mut a = Extent::new();
    let ar = row(&[("order", o.clone()), ("amount", Value::Int(100))]);
    a.insert(brix_oracle::row::row_key(&ar), edge_record(ar));
    let mut b = Extent::new();
    let br = row(&[("order", o.clone()), ("amount", Value::Int(200))]);
    b.insert(brix_oracle::row::row_key(&br), edge_record(br));
    let ground = BTreeMap::from([("A".to_string(), a), ("B".to_string(), b)]);

    let settled = settle(&program, &phases, &ground, &empty_ground(), 1);
    // No ordinary live value under the conflicted key.
    assert!(settled.extent("ComputedPrice").unwrap().is_empty());
    // Exactly one KeyConflict with two candidates.
    assert_eq!(settled.provenance.key_conflicts.len(), 1);
    assert_eq!(settled.provenance.key_conflicts[0].candidates.len(), 2);

    insta::assert_snapshot!("key_conflict", render(&settled));
}

// ---------------------------------------------------------------------------
// 3b. Per-kind KeyConflict on an Entity relation (errata 0001): a
//     transaction-ensured candidate disagreeing with a rule-derived one for
//     the same key must surface all distinct candidates, not collapse them
//     — `RelationDef::digest` hashes only key fields for `Entity` kind, so
//     naively reusing it to identify candidates would always collapse a
//     conflict group to one entry (every candidate shares the conflicted
//     key by construction). Regression test for that bug.
// ---------------------------------------------------------------------------

#[test]
fn entity_key_conflict_surfaces_distinct_candidates() {
    let program = Program::new()
        .with_relation(RelationDef::entity("Widget", &["code", "label"], &["code"]))
        .with_relation(RelationDef::ground(
            "RawLabel",
            &["code", "label"],
            &["code"],
        ))
        .with_rule(rule(
            "FromRaw",
            "Widget",
            &[("code", var("c")), ("label", var("l"))],
            vec![edge("RawLabel", &[("code", var("c")), ("label", var("l"))])],
        ));
    program.validate().unwrap();
    let phases = brix_oracle::phase::infer_phases(&program).unwrap();

    // A ground-ensured Widget("w1", "A") coexists with RawLabel("w1", "B"),
    // which drives FromRaw to derive a competing Widget("w1", "B") — two
    // disagreeing candidates under one entity key, from two different
    // sources (Part III §8 extended to Entity relations, errata 0001).
    let ensured = row(&[
        ("code", Value::Str("w1".into())),
        ("label", Value::Str("A".into())),
    ]);
    let mut widget = Extent::new();
    widget.insert(brix_oracle::row::row_key(&ensured), edge_record(ensured));

    let raw = row(&[
        ("code", Value::Str("w1".into())),
        ("label", Value::Str("B".into())),
    ]);
    let mut raw_label = Extent::new();
    raw_label.insert(brix_oracle::row::row_key(&raw), edge_record(raw));

    let ground = BTreeMap::from([
        ("Widget".to_string(), widget),
        ("RawLabel".to_string(), raw_label),
    ]);

    let settled = settle(&program, &phases, &ground, &empty_ground(), 1);
    assert!(
        settled.extent("Widget").unwrap().is_empty(),
        "no silent winner under the conflicted key"
    );
    assert_eq!(settled.provenance.key_conflicts.len(), 1);
    assert_eq!(
        settled.provenance.key_conflicts[0].candidates.len(),
        2,
        "both disagreeing candidates must be individually visible"
    );

    insta::assert_snapshot!("entity_key_conflict", render(&settled));
}

// ---------------------------------------------------------------------------
// 4. Error edges (Part III §9): `?` failure derives a RuleError, siblings
//    unaffected.
// ---------------------------------------------------------------------------

#[test]
fn partial_fn_failure_derives_rule_error() {
    // entity Order { key ref, due }
    // rel LateRisk { order, risk } key(order)   -- derived
    // derive Risk: LateRisk(order: o, risk: r)
    //              from { o @ Order(due: d), let r = riskModel(d)? }
    let program = Program::new()
        .with_relation(RelationDef::entity("Order", &["ref", "due"], &["ref"]))
        .with_relation(RelationDef::derived(
            "LateRisk",
            &["order", "risk"],
            &["order"],
        ))
        .with_partial_fn("riskModel", risk_model)
        .with_rule(rule(
            "Risk",
            "LateRisk",
            &[("order", var("o")), ("risk", var("r"))],
            vec![
                edge_bind("o", "Order", &[("due", var("d"))]),
                let_("r", try_call("riskModel", vec![evar("d")])),
            ],
        ));
    program.validate().unwrap();
    let phases = brix_oracle::phase::infer_phases(&program).unwrap();

    // Two orders: ord-ok (due 10 → risk 90) and ord-bad (due 0 → RuleError).
    let mut orders = Extent::new();
    for (r, due) in [("ord-ok", 10u64), ("ord-bad", 0)] {
        let row = row(&[("ref", Value::Str(r.into())), ("due", Value::Nat(due))]);
        orders.insert(brix_oracle::row::row_key(&row), edge_record(row));
    }
    let ground = BTreeMap::from([("Order".to_string(), orders)]);

    let settled = settle(&program, &phases, &ground, &empty_ground(), 1);
    // The good order produced a LateRisk; the bad one produced a RuleError
    // and contributed no LateRisk (sibling matches unaffected — Part III §9).
    assert_eq!(settled.extent("LateRisk").unwrap().len(), 1);
    assert_eq!(settled.provenance.rule_errors.len(), 1);
    assert_eq!(
        settled.provenance.rule_errors[0].error,
        Value::Str("PastDue".to_string())
    );

    insta::assert_snapshot!("rule_error", render(&settled));
}

// ---------------------------------------------------------------------------
// 5. Transactions + strict constraint + snapshot isolation + `why`
//    (drives the full Store engine across revisions).
// ---------------------------------------------------------------------------

#[test]
fn transactions_constraint_and_why() {
    // Reuse the pricing program plus a strict constraint rejecting any
    // ComputedPrice above a cap.
    let program = pricing_program().with_constraint(constraint(
        "PriceCap",
        Severity::Strict,
        vec![
            edge(
                "ComputedPrice",
                &[("order", var("o")), ("amount", var("a"))],
            ),
            when(binop(BinOp::Gt, evar("a"), Expr::Const(Value::Int(1000)))),
        ],
    ));
    program.validate().unwrap();
    let mut store = Store::new(program).unwrap();

    // Revision 1: create an order and a base amount within the cap.
    let ord = order_ref(store.program(), "ord-1");
    let tx1 = Transaction::new(b"tx1".to_vec())
        .ensure("Order", row(&[("ref", Value::Str("ord-1".into()))]))
        .assert(
            "BaseAmount",
            row(&[("order", ord.clone()), ("amount", Value::Int(500))]),
        );
    let settled = store.commit(&tx1).unwrap();
    assert_eq!(settled.at_revision, 1);
    assert_eq!(settled.extent("EffectivePrice").unwrap().len(), 1);

    // `why EffectivePrice(order: ord-1)` — provenance answers from Support.
    let eff_row = row(&[("order", ord.clone()), ("amount", Value::Int(500))]);
    let eff_id = store.program().relations["EffectivePrice"].digest(&eff_row);
    let supports = store.current().provenance.why(eff_id);
    assert_eq!(supports.len(), 1);
    assert_eq!(supports[0].rule, "FromComputed");

    // Revision 2 attempt: a fresh order whose ComputedPrice exceeds the cap.
    // The strict constraint rejects the fully-settled candidate revision
    // atomically — the store stays at revision 1 (snapshot isolation:
    // Appendix I.11, the rejected txn is never observable).
    let ord2 = order_ref(store.program(), "ord-2");
    let tx_bad = Transaction::new(b"tx-bad".to_vec())
        .ensure("Order", row(&[("ref", Value::Str("ord-2".into()))]))
        .assert(
            "BaseAmount",
            row(&[("order", ord2.clone()), ("amount", Value::Int(9999))]),
        );
    let err = store.commit(&tx_bad).unwrap_err();
    assert!(matches!(
        err,
        CommitError::StrictViolation { at_revision: 2 }
    ));
    assert_eq!(
        store.current().at_revision,
        1,
        "snapshot isolation: rejected txn invisible"
    );

    insta::assert_snapshot!("txn_revision_1", render(store.current()));
}

// --- small test helpers -----------------------------------------------------

fn edge_record(row: brix_oracle::row::Row) -> brix_oracle::row::EdgeRecord {
    // A ground row with one synthetic claim so it counts as live.
    let mut rec = brix_oracle::row::EdgeRecord {
        row,
        ..Default::default()
    };
    rec.claims
        .insert(brix_canon::ClaimId::from_canon(b"test-claim"));
    rec
}

fn order_ref(program: &Program, r: &str) -> Value {
    let def = &program.relations["Order"];
    debug_assert_eq!(def.kind, RelKind::Entity);
    Value::Node(def.node_id(&row(&[("ref", Value::Str(r.into()))])))
}
