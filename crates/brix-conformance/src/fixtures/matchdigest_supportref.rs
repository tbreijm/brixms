//! `spec/errata/0001-matchdigest-supportref-formula.md` → I.1 / I.3 (scoped).
//!
//! The erratum's ruled formula (`MatchDigest = Hash(canon(R) ++
//! canon(bindings))`, `SupportRef = Hash(edge ++ R ++ MatchDigest)`) is
//! attributed to `crates/brix-rt/src/ids.rs` — a crate that does not exist
//! in this repo yet, so there is no second implementation to cross-check
//! the oracle's own `env_digest`/`SupportRef` against byte-for-byte (that
//! cross-implementation compare is #11's differential harness). This
//! fixture instead pins what the oracle *can* demonstrate today, which is
//! the property the formula exists to guarantee: match/support digests are
//! deterministic, and a derived row supported by two independent rule
//! matches survives the retraction of either one's ground claim alone (I.3
//! — "shared supports removed in any order converge").
//!
//! Two ground sources feed one derived relation via two separate rules
//! that, for the same key, derive *identical* row content — genuine shared
//! support, not a `KeyConflict` (differing content would be one).

use brix_oracle::dsl::*;
use brix_oracle::program::{Program, RelationDef};
use brix_oracle::txn::Transaction;
use brix_oracle::value::Value;

use crate::fixture::Fixture;

pub fn program() -> Program {
    Program::new()
        .with_relation(RelationDef::ground("SourceA", &["code"], &["code"]))
        .with_relation(RelationDef::ground("SourceB", &["code"], &["code"]))
        .with_relation(RelationDef::derived("Flagged", &["code"], &["code"]))
        .with_rule(rule(
            "FromA",
            "Flagged",
            &[("code", var("c"))],
            vec![edge("SourceA", &[("code", var("c"))])],
        ))
        .with_rule(rule(
            "FromB",
            "Flagged",
            &[("code", var("c"))],
            vec![edge("SourceB", &[("code", var("c"))])],
        ))
}

/// Revision 1: assert both sources for the same key — `Flagged("x1")` gets
/// two independent supports (`FromA`, `FromB`). Revision 2: retract the
/// `SourceA` claim from revision 1 — `Flagged("x1")` must stay live (still
/// supported by `FromB`), with one fewer support.
pub fn fixture() -> Fixture {
    let rev1 = Transaction::new(b"conf-0001-matchdigest-rev1".to_vec())
        .assert("SourceA", row(&[("code", Value::Str("x1".into()))]))
        .assert("SourceB", row(&[("code", Value::Str("x1".into()))]));
    let source_a_claim = rev1.claim_id(0);

    let rev2 =
        Transaction::new(b"conf-0001-matchdigest-rev2".to_vec()).retract("SourceA", source_a_claim);

    Fixture {
        id: "errata-0001-matchdigest-supportref-formula",
        appendix_i: "I.1 / I.3",
        program: program(),
        stream: vec![rev1, rev2],
    }
}
