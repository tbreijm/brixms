//! The conformance fixture format (Ring0_Build_Plan §1.10): `(program,
//! txn-stream, expected canonical settled dump per revision)`, with an ID
//! mapped to its Appendix I conformance category.
//!
//! Rust-defined for v0 — `brix-ir -> Program` lowering isn't wired into
//! this crate, so a hand-built `brix_oracle::program::Program` mirrors
//! `crates/brix-oracle/tests/settle.rs`'s existing style rather than
//! inventing an on-disk fixture DSL/parser.
//!
//! The [`Engine`] seam is what lets #11 drop in a compiled-engine runner
//! later without reshaping anything here: `OracleEngine` (this crate) is
//! the first and, for now, only implementation; #11 adds a `CompiledEngine`
//! and asserts `oracle.run(f) == compiled.run(f)` digest-for-digest per
//! revision — the whole differential harness.

use brix_canon::Digest;
use brix_oracle::program::Program;
use brix_oracle::txn::Transaction;

/// One conformance fixture: a program, an ordered transaction stream, and
/// the Appendix I category it pins.
pub struct Fixture {
    /// Stable identifier, e.g. `"errata-0001-entity-key-conflict"`.
    pub id: &'static str,
    /// The Appendix I conformance category this fixture exercises, e.g.
    /// `"I.5"`.
    pub appendix_i: &'static str,
    pub program: Program,
    /// Ordered revisions — each entry is committed in turn.
    pub stream: Vec<Transaction>,
}

/// One committed revision's canonical dump.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RevDump {
    pub digest: Digest,
    pub bytes: Vec<u8>,
}

/// A fixture run's output: one [`RevDump`] per committed revision, in
/// stream order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunOutput {
    pub per_revision: Vec<RevDump>,
}

/// Something that can run a [`Fixture`] to a [`RunOutput`]. `OracleEngine`
/// (this crate) is the first implementation; a future `CompiledEngine`
/// (#11) is the second — differential conformance is then just
/// `oracle.run(f) == compiled.run(f)`.
pub trait Engine {
    fn run(&self, fixture: &Fixture) -> RunOutput;
}
