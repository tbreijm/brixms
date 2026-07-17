//! `spec/errata/0002-edge-identity-compatibility-domain.md` → I.2.
//!
//! `EdgeId = Hash(relation compatibility domain, canonical role tuple)`;
//! the ruling realizes "relation compatibility domain" as the relation's
//! stable fully-qualified name, folded into the hashed payload ahead of
//! the role tuple. `crates/brix-oracle/src/identity.rs::RelationDef::edge_id`
//! already implements exactly this (`w.write_tag(&self.name)` before
//! `row.canon_write(&mut w)`) — this fixture pins the substantive claim:
//! two relations with an otherwise-identical role shape must never collide
//! on identity just because a row's fields happen to match.

use brix_oracle::dsl::*;
use brix_oracle::program::{Program, RelationDef};
use brix_oracle::row::Row;
use brix_oracle::txn::Transaction;
use brix_oracle::value::Value;

use crate::fixture::Fixture;

pub fn program() -> Program {
    Program::new()
        .with_relation(RelationDef::ground("A", &["x"], &["x"]))
        .with_relation(RelationDef::ground("B", &["x"], &["x"]))
}

/// The row shape shared by both `A` and `B` — same fields, same value.
pub fn matching_row() -> Row {
    row(&[("x", Value::Int(42))])
}

pub fn fixture() -> Fixture {
    let tx = Transaction::new(b"conf-0002-edge-identity-domain".to_vec())
        .assert("A", matching_row())
        .assert("B", matching_row());
    Fixture {
        id: "errata-0002-edge-identity-compatibility-domain",
        appendix_i: "I.2",
        program: program(),
        stream: vec![tx],
    }
}
