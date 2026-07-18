//! The compiled-engine [`Engine`] implementation (#11) — runs a [`Fixture`]
//! against `brix-rt::engine::Store` directly, in-process. No codegen, no
//! `cargo build`, no subprocess: `brix_rt::engine::Store` is a generic
//! interpreter over a `Program` *value* (confirmed by reading
//! `crates/brixc/src/emit/native.rs`, which lowers checked Core IR into
//! exactly that kind of value, not bespoke per-program source), so the
//! translated program can be driven the same way `OracleEngine` drives the
//! oracle's own `Store`. This is what makes a fast differential fuzzer
//! (tracked as a follow-up, not built in this pass) possible at all.

use brix_canon::{Digest, Domain};
use brix_rt::engine::Store;

use crate::fixture::{Engine, Fixture, RevDump, RunOutput};
use crate::translate;

/// Runs a [`Fixture`] against `brix-rt`. Fail-closed, matching
/// `OracleEngine`: a program that doesn't translate/phase-assign, or a
/// transaction that doesn't commit cleanly, is a hard panic — a fixture bug,
/// not a fact worth swallowing.
pub struct CompiledEngine;

impl Engine for CompiledEngine {
    fn run(&self, fixture: &Fixture) -> RunOutput {
        let program = translate::program(&fixture.program);
        let mut store = Store::new(program);

        let mut per_revision = Vec::with_capacity(fixture.stream.len());
        for (i, txn) in fixture.stream.iter().enumerate() {
            let native_txn = translate::transaction(txn);
            store.commit(&native_txn).unwrap_or_else(|e| {
                panic!(
                    "fixture `{}` transaction {i} must commit cleanly on the compiled engine: {e}",
                    fixture.id
                )
            });
            let bytes = store.current_dump();
            let digest = Digest::of(Domain::Value, &bytes);
            per_revision.push(RevDump { digest, bytes });
        }

        RunOutput { per_revision }
    }
}
