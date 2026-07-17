//! `spec/errata/0001-entity-key-conflict-and-ensure-idempotency.md` → I.5.
//!
//! Part III §8 names four sub-cases (Ground `rel`, `state rel`, `event
//! rel`, Derived) but not `entity`, even though every `entity` declares
//! `key(...)`. The ruling: treat `Entity` uniformly like `Derived` for
//! key-conflict purposes, regardless of whether a candidate row originated
//! from a transaction's `ensure` or from a rule's head. This fixture
//! exercises the specific case the erratum newly covers: a rule-derived
//! candidate disagreeing with a transaction-ensured one for the *same*
//! entity key. That's deliberately distinct from two `ensure`s disagreeing
//! within *one* transaction — `txn.rs`'s `EntityFieldConflict` already
//! rejects that at the transaction layer (Part III §8's ground-conflict
//! sub-case); this fixture's conflict can only be caught at settlement, by
//! `eval.rs::refresh_live` grouping the ground-ensured and rule-derived
//! candidates together (see its doc comment).

use brix_oracle::dsl::*;
use brix_oracle::program::{Program, RelationDef};
use brix_oracle::txn::Transaction;
use brix_oracle::value::Value;

use crate::fixture::Fixture;

pub fn program() -> Program {
    Program::new()
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
        ))
}

pub fn fixture() -> Fixture {
    // One entity key ("w1") with two disagreeing producers: a direct
    // `ensure` (ground-like) and a `RawLabel` fact that drives `FromRaw` to
    // derive a competing `Widget` row for the same key.
    let tx = Transaction::new(b"conf-0001-entity-key-conflict".to_vec())
        .ensure(
            "Widget",
            row(&[
                ("code", Value::Str("w1".into())),
                ("label", Value::Str("A".into())),
            ]),
        )
        .assert(
            "RawLabel",
            row(&[
                ("code", Value::Str("w1".into())),
                ("label", Value::Str("B".into())),
            ]),
        );
    Fixture {
        id: "errata-0001-entity-key-conflict-and-ensure-idempotency",
        appendix_i: "I.5",
        program: program(),
        stream: vec![tx],
    }
}
