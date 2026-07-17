//! The oracle-side [`Engine`] implementation — the reference authority's
//! half of the differential harness (#11 adds the compiled-engine half).

use brix_oracle::dump::{dump_bytes, dump_digest};
use brix_oracle::store::Store;

use crate::fixture::{Engine, Fixture, RevDump, RunOutput};

/// Runs a [`Fixture`] against `brix-oracle`. Fail-closed: a fixture whose
/// program does not phase-assign, or whose transaction stream does not
/// commit cleanly, is a hard panic (surfaced as a test failure) — never a
/// silent skip. A conformance fixture that can't run is a fixture bug, not
/// a fact worth swallowing.
pub struct OracleEngine;

impl Engine for OracleEngine {
    fn run(&self, fixture: &Fixture) -> RunOutput {
        let mut store = Store::new(fixture.program.clone()).unwrap_or_else(|e| {
            panic!(
                "fixture `{}` program must phase-assign cleanly: {e}",
                fixture.id
            )
        });

        let mut per_revision = Vec::with_capacity(fixture.stream.len());
        for (i, txn) in fixture.stream.iter().enumerate() {
            let settled = store.commit(txn).unwrap_or_else(|e| {
                panic!(
                    "fixture `{}` transaction {i} must commit cleanly: {e}",
                    fixture.id
                )
            });
            per_revision.push(RevDump {
                digest: dump_digest(settled),
                bytes: dump_bytes(settled),
            });
        }

        RunOutput { per_revision }
    }
}
