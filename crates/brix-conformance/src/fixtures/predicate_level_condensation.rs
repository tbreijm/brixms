//! `spec/errata/0002-appendix-f-predicate-level-condensation.md` → I.1 / I.4.
//!
//! Lifted almost directly from the erratum's own worked example (transitive
//! closure): `Base`/`Trans` both derive `Reach`, but only `Trans` reads
//! `Reach` — a naive rule-granular Tarjan puts them in two separate SCCs
//! (`Base → Trans`, no edge back), which would split one relation's
//! completion boundary across two phases. The ruling: condensation is
//! predicate-granular for tuple production, so `Base`/`Trans` must settle
//! in one phase. This is exactly `brix-oracle/tests/settle.rs`'s
//! `positive_recursion_reachability` program, reused here as a pinned
//! conformance fixture.

use brix_oracle::dsl::*;
use brix_oracle::program::{Program, RelationDef};
use brix_oracle::txn::Transaction;
use brix_oracle::value::Value;

use crate::fixture::Fixture;

pub fn program() -> Program {
    Program::new()
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
        ))
}

pub fn fixture() -> Fixture {
    let mut tx = Transaction::new(b"conf-0002-predicate-level-condensation".to_vec());
    for (s, d) in [("a", "b"), ("b", "c"), ("c", "d")] {
        tx = tx.assert(
            "Link",
            row(&[("src", Value::Str(s.into())), ("dst", Value::Str(d.into()))]),
        );
    }
    Fixture {
        id: "errata-0002-appendix-f-predicate-level-condensation",
        appendix_i: "I.1 / I.4",
        program: program(),
        stream: vec![tx],
    }
}
