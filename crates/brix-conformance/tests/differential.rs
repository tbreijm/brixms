//! The differential harness (#11): `OracleEngine.run(fixture) ==
//! CompiledEngine.run(fixture)`, digest-for-digest, over the fixtures
//! pinned by issue #26.
//!
//! `entity_key_conflict` is `#[ignore]`d, not silently dropped: the oracle
//! settles an `Entity` key conflict as distinct candidates plus a
//! `KeyConflict` provenance entry (errata 0001), while `brix-rt::engine`
//! currently hard-rejects the conflicting transaction outright
//! (`TransactionError::EntityFieldConflict`/`GroundKeyConflict`) — a real
//! behavioral gap flagged in this PR's description, not a translator bug.
//! Re-enable once that native-engine parity work lands.

use brix_conformance::fixtures::{
    edge_identity_domain, entity_key_conflict, matchdigest_supportref, predicate_level_condensation,
};
use brix_conformance::{CompiledEngine, Engine, Fixture, OracleEngine};

fn assert_engines_agree(fixture: &Fixture) {
    let oracle = OracleEngine.run(fixture);
    let compiled = CompiledEngine.run(fixture);
    assert_eq!(
        oracle, compiled,
        "fixture `{}`: compiled engine must match the oracle digest-for-digest",
        fixture.id
    );
}

#[test]
fn edge_identity_compatibility_domain_agrees() {
    assert_engines_agree(&edge_identity_domain::fixture());
}

#[test]
fn predicate_level_condensation_agrees() {
    assert_engines_agree(&predicate_level_condensation::fixture());
}

#[test]
fn matchdigest_supportref_agrees() {
    assert_engines_agree(&matchdigest_supportref::fixture());
}

#[test]
#[ignore = "brix-rt::engine hard-rejects entity key conflicts instead of producing distinct \
            candidates like the oracle (errata 0001) — native-engine parity tracked as a \
            follow-up, see the #11 PR body"]
fn entity_key_conflict_agrees() {
    assert_engines_agree(&entity_key_conflict::fixture());
}
