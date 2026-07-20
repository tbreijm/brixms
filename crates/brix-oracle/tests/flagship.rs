//! Ring 0 G1 acceptance test (issue #24): the real flagship program —
//! parsed, checked, and lowered by the real frontend, not hand-built via
//! `dsl.rs` — settles end-to-end on the oracle, and `why` answers from its
//! provenance. Per `spec/Ring0_Build_Plan.md §1.6`: "Exit G1: the flagship
//! parses, checks, and runs end-to-end on the oracle; `why` answers from
//! oracle provenance."
//!
//! Two things this test deliberately does NOT do (see the issue #24 plan
//! for the reasoning, restated briefly here):
//! - It does not execute `KeyClientsAtRisk` (the flagship's one `query`) —
//!   `brix_oracle::program::Program` has no query concept at all. This
//!   test demonstrates the query's *logic* directly against the settled
//!   extents instead (see `key_clients_at_risk` below).
//! - No `driver`/`scenario` data reaches the real frontend (both are
//!   wholesale skipped by `brixc`'s lowering today), so the driving
//!   transaction stream below is hand-authored, mirroring
//!   `crates/brix-oracle/tests/settle.rs`'s own style — not replayed from
//!   the flagship's own `RushWeek` scenario.

use brix_ast::parse_file;
use brix_oracle::dsl::row;
use brix_oracle::dump::render;
use brix_oracle::frontend::{program_from_source, FnLibrary, KindTable};
use brix_oracle::program::RelKind;
use brix_oracle::row::Row;
use brix_oracle::store::{CommitError, Store};
use brix_oracle::txn::Transaction;
use brix_oracle::value::Value;

const FLAGSHIP_SRC: &str =
    include_str!("../../brix-ast/tests/fixtures/spec/0001-part-i-the-flagship-program.brix");

/// Every relation the flagship's rules/constraints reference whose real
/// `RelKind` isn't mechanically recoverable from `Lowered` (see the
/// `frontend` module doc) — `Derived` relations (`Unassigned`, `Move`,
/// `ComputedPrice`, `EffectivePrice`, `LateRisk`, `AssignOrder.request`,
/// `NotifyOps.request`) are inferred automatically and need no entry here.
fn kinds() -> KindTable {
    let mut k = KindTable::new();
    for entity in ["Location", "Client", "Vehicle", "Tariff", "Order"] {
        k.insert(entity.to_string(), RelKind::Entity);
    }
    for state in ["OrderStatus", "TariffRate", "ManualPrice"] {
        k.insert(state.to_string(), RelKind::State);
    }
    k.insert("Delivered".to_string(), RelKind::Event);
    k.insert("Distance".to_string(), RelKind::Ground);
    k.insert("brix.sim.Now".to_string(), RelKind::Ground);
    // Protocol outcomes are asserted directly here, standing in for a
    // driver's answer (no driver reaches the real frontend today).
    k.insert("AssignOrder.Chosen".to_string(), RelKind::Ground);
    k.insert("AssignOrder.NoCapacity".to_string(), RelKind::Ground);
    k
}

// `surcharge` is no longer hand-transcribed here: it is compiled from its
// BrixMS source and executes via `Program::fn_defs` (issue #47 Slice 1.5), so
// the `FnLibrary` only needs to supply `riskModel` (still deferred).

/// `riskModel(due, now) -> Result<Probability, ValidationError>`:
/// `let remaining = due - now; if remaining <= 0 hours { 1.0 } else {
/// clamp(1.0 - remaining / 24 hours, 0.0, 1.0) }` — hand-transcribed from
/// the flagship source, same reasoning as `surcharge`. `Instant` values
/// are whole hours here; `Probability` is basis points (×10000).
///
/// Integer-floor fixed-point (issue #47 Part 2 ruling): truncating `i128`
/// division, no float arithmetic — matches `brix-rt::engine::builtin_partial`
/// bit-for-bit (previously this hand-reg used f64 round-half, disagreeing
/// with the runtime by 1 bp at `remaining = 8`: 6667 vs 6666).
fn risk_model(args: &[Value]) -> Result<Value, Value> {
    let due = args[0].as_i128().expect("riskModel: non-numeric due");
    let now = args[1].as_i128().expect("riskModel: non-numeric now");
    let remaining = due - now;
    let risk = if remaining <= 0 {
        10_000i64
    } else {
        (((24 - remaining).clamp(0, 24) * 10_000) / 24) as i64
    };
    Ok(Value::Int(risk))
}

fn fn_library() -> FnLibrary {
    FnLibrary::new().with_partial_fn("riskModel", risk_model)
}

fn node_ref(program: &brix_oracle::program::Program, rel: &str, key_row: Row) -> Value {
    Value::Node(program.relations[rel].node_id(&key_row))
}

fn tier(name: &str, ordinal: u32) -> Value {
    Value::Enum {
        ty: "Tier".into(),
        ordinal,
        name: name.into(),
    }
}
fn vehicle_class(name: &str, ordinal: u32) -> Value {
    Value::Enum {
        ty: "VehicleClass".into(),
        ordinal,
        name: name.into(),
    }
}
fn status(name: &str, ordinal: u32) -> Value {
    Value::Enum {
        ty: "Status".into(),
        ordinal,
        name: name.into(),
    }
}

#[test]
fn flagship_settles_end_to_end_and_answers_why() {
    let (file, parse_diags) = parse_file(FLAGSHIP_SRC);
    assert!(!parse_diags.has_errors(), "flagship must parse cleanly");

    let lowered = brixc::lower_file(&file, &parse_diags);
    assert!(
        !lowered.has_errors(),
        "flagship must lower cleanly: {:#?}",
        lowered.diags
    );

    let program = program_from_source(&lowered.source, &lowered.resolver, &kinds(), fn_library())
        .expect("flagship must adapt cleanly onto the oracle's Program");
    program
        .validate()
        .expect("adapted flagship program must validate");

    let mut store = Store::new(program).expect("flagship must be well-stratified");

    // --- Revision 1: a hand-authored driving transaction ------------------
    let ams = node_ref(
        store.program(),
        "Location",
        row(&[("code", Value::Str("AMS".into()))]),
    );
    let rtm = node_ref(
        store.program(),
        "Location",
        row(&[("code", Value::Str("RTM".into()))]),
    );
    let acme = node_ref(
        store.program(),
        "Client",
        row(&[("code", Value::Str("ACME".into()))]),
    );
    let v01 = node_ref(
        store.program(),
        "Vehicle",
        row(&[("plate", Value::Str("V-01".into()))]),
    );
    let v02 = node_ref(
        store.program(),
        "Vehicle",
        row(&[("plate", Value::Str("V-02".into()))]),
    );
    let tariff_standard = node_ref(
        store.program(),
        "Tariff",
        row(&[("class", vehicle_class("Standard", 1))]),
    );
    let tariff_suv = node_ref(
        store.program(),
        "Tariff",
        row(&[("class", vehicle_class("SUV", 2))]),
    );
    let ord1 = node_ref(
        store.program(),
        "Order",
        row(&[("ref", Value::Str("ord-1".into()))]),
    );
    let ord2 = node_ref(
        store.program(),
        "Order",
        row(&[("ref", Value::Str("ord-2".into()))]),
    );
    let ord3 = node_ref(
        store.program(),
        "Order",
        row(&[("ref", Value::Str("ord-3".into()))]),
    );

    let tx1 = Transaction::new(b"flagship-rev1".to_vec())
        .ensure("Location", row(&[("code", Value::Str("AMS".into()))]))
        .ensure("Location", row(&[("code", Value::Str("RTM".into()))]))
        .ensure(
            "Client",
            row(&[
                ("code", Value::Str("ACME".into())),
                ("tier", tier("Key", 1)),
            ]),
        )
        .ensure(
            "Vehicle",
            row(&[
                ("plate", Value::Str("V-01".into())),
                ("class", vehicle_class("Standard", 1)),
                ("capacity", Value::Nat(2000)),
            ]),
        )
        .ensure(
            "Vehicle",
            row(&[
                ("plate", Value::Str("V-02".into())),
                ("class", vehicle_class("SUV", 2)),
                ("capacity", Value::Nat(3500)),
            ]),
        )
        .ensure("Tariff", row(&[("class", vehicle_class("Standard", 1))]))
        .ensure("Tariff", row(&[("class", vehicle_class("SUV", 2))]))
        .set(
            "TariffRate",
            row(&[
                ("tariff", tariff_standard.clone()),
                ("rate", Value::Int(120)),
            ]),
        )
        .set(
            "TariffRate",
            row(&[("tariff", tariff_suv.clone()), ("rate", Value::Int(165))]),
        )
        .assert(
            "Distance",
            row(&[
                ("from", ams.clone()),
                ("to", rtm.clone()),
                ("length", Value::Nat(78)),
            ]),
        )
        .ensure(
            "Order",
            row(&[
                ("ref", Value::Str("ord-1".into())),
                ("client", acme.clone()),
                ("from", ams.clone()),
                ("to", rtm.clone()),
                ("weight", Value::Nat(1500)),
                ("due", Value::Nat(20)),
            ]),
        )
        .ensure(
            "Order",
            row(&[
                ("ref", Value::Str("ord-2".into())),
                ("client", acme.clone()),
                ("from", ams.clone()),
                ("to", rtm.clone()),
                ("weight", Value::Nat(3000)),
                ("due", Value::Nat(100)),
            ]),
        )
        .ensure(
            "Order",
            row(&[
                ("ref", Value::Str("ord-3".into())),
                ("client", acme.clone()),
                ("from", ams.clone()),
                ("to", rtm.clone()),
                ("weight", Value::Nat(800)),
                ("due", Value::Nat(2)),
            ]),
        )
        .set(
            "OrderStatus",
            row(&[("order", ord1.clone()), ("value", status("Open", 0))]),
        )
        .set(
            "OrderStatus",
            row(&[("order", ord2.clone()), ("value", status("Open", 0))]),
        )
        .set(
            "OrderStatus",
            row(&[("order", ord3.clone()), ("value", status("Open", 0))]),
        )
        // ord-1/ord-2 are assigned, as if a driver had answered; ord-3 is
        // left pending, so it stays `Unassigned` (exercises Waiting).
        .assert(
            "AssignOrder.Chosen",
            row(&[("order", ord1.clone()), ("vehicle", v01.clone())]),
        )
        .assert(
            "AssignOrder.Chosen",
            row(&[("order", ord2.clone()), ("vehicle", v02.clone())]),
        )
        // Overrides ord-2's computed price (exercises Override/masking).
        .set(
            "ManualPrice",
            row(&[("order", ord2.clone()), ("amount", Value::Int(9_500))]),
        )
        // "now" for Risk — ord-3's due (2) is already past, so its risk is
        // maximal and Escalate fires.
        .assert("brix.sim.Now", row(&[("at", Value::Nat(12))]));

    let settled = store
        .commit(&tx1)
        .expect("revision 1 must commit cleanly")
        .clone();
    assert_eq!(settled.at_revision, 1);

    // Waiting/RequestAssignment: only the pending order is Unassigned.
    let unassigned = settled.extent("Unassigned").unwrap();
    assert_eq!(unassigned.len(), 1, "only ord-3 should still be Unassigned");

    // Assign: both assigned orders produced a Move.
    assert_eq!(settled.extent("Move").unwrap().len(), 2);

    // PriceOrder + Override: ComputedPrice(ord-2) is masked by the manual
    // override, so only ord-1's computed price stays live.
    assert_eq!(
        settled.extent("ComputedPrice").unwrap().len(),
        1,
        "ord-2's ComputedPrice must be masked"
    );
    assert_eq!(settled.provenance.masked.len(), 1);

    // FromComputed + FromManual: both assigned orders end up with an
    // EffectivePrice, from different rules.
    assert_eq!(settled.extent("EffectivePrice").unwrap().len(), 2);

    // Risk + Escalate: only the overdue, still-pending order escalates.
    assert_eq!(settled.extent("LateRisk").unwrap().len(), 1);
    assert_eq!(settled.extent("NotifyOps.request").unwrap().len(), 1);

    insta::assert_snapshot!("flagship_revision_1", render(&settled));

    // --- `why`: the literal G1 acceptance line -----------------------------
    let eff_row = row(&[("order", ord1.clone()), ("amount", Value::Int(120 * 78))]);
    let eff_id = store.program().relations["EffectivePrice"].digest(&eff_row);
    let supports = store.current().provenance.why(eff_id);
    assert_eq!(
        supports.len(),
        1,
        "EffectivePrice(ord-1) must have exactly one support"
    );
    assert_eq!(supports[0].rule, "FromComputed");

    // --- KeyClientsAtRisk, demonstrated directly (not executed as a query,
    // see the module doc): order/client/risk where the client is `Key`
    // tier and risk exceeds a threshold.
    let threshold = 8_000i64; // 0.8
    let key_clients_at_risk: Vec<_> = settled
        .extent("LateRisk")
        .unwrap()
        .values()
        .filter_map(|rec| {
            let risk = rec.row.get("risk")?.as_i128()? as i64;
            if risk <= threshold {
                return None;
            }
            Some(rec.row.get("order")?.clone())
        })
        .collect();
    assert_eq!(
        key_clients_at_risk.len(),
        1,
        "ord-3 crosses the risk threshold"
    );

    // --- Revision 2: a deliberate Capacity violation, expected to be
    // rejected atomically (Part IV §7) — proves the constraint machinery
    // holds on a real program, not just hand-built ones (settle.rs already
    // covers the hand-built case).
    let ord4 = node_ref(
        store.program(),
        "Order",
        row(&[("ref", Value::Str("ord-4".into()))]),
    );
    let tx2 = Transaction::new(b"flagship-rev2".to_vec())
        .ensure(
            "Order",
            row(&[
                ("ref", Value::Str("ord-4".into())),
                ("client", acme.clone()),
                ("from", ams.clone()),
                ("to", rtm.clone()),
                ("weight", Value::Nat(5_000)), // exceeds every vehicle's capacity
                ("due", Value::Nat(200)),
            ]),
        )
        .set(
            "OrderStatus",
            row(&[("order", ord4.clone()), ("value", status("Open", 0))]),
        )
        .assert(
            "AssignOrder.Chosen",
            row(&[("order", ord4), ("vehicle", v02)]),
        );

    let err = store.commit(&tx2).unwrap_err();
    assert!(matches!(
        err,
        CommitError::StrictViolation { at_revision: 2 }
    ));
    assert_eq!(
        store.current().at_revision,
        1,
        "snapshot isolation: the rejected transaction must not be observable"
    );
}
