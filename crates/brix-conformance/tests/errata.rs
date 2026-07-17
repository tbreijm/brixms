//! One test per pinned erratum fixture (issue #26): an `insta` snapshot of
//! each committed revision's rendered dump (human-reviewable), a digest
//! stability assertion (two independent `OracleEngine` runs over the same
//! fixture must produce byte-identical `RevDump`s — the property the
//! `determinism` CI gate checks by running the whole suite twice), and one
//! structural assertion specific to the erratum each fixture pins.

use brix_conformance::fixtures::{
    edge_identity_domain, entity_key_conflict, matchdigest_supportref, predicate_level_condensation,
};
use brix_conformance::{Engine, Fixture, OracleEngine};
use brix_oracle::dump::render;
use brix_oracle::eval::Settled;
use brix_oracle::store::Store;

/// Two independent `OracleEngine` runs over structurally identical fixtures
/// (same program, same stream, built fresh each time) must produce
/// byte-identical output.
fn assert_deterministic(a: &Fixture, b: &Fixture) {
    let out_a = OracleEngine.run(a);
    let out_b = OracleEngine.run(b);
    assert_eq!(
        out_a, out_b,
        "fixture `{}` must be deterministic across independent runs",
        a.id
    );
}

/// Drive `fixture`'s stream through a fresh `Store`, returning one cloned
/// `Settled` per committed revision — for erratum-specific structural
/// assertions and snapshot rendering that go beyond `RunOutput`'s digests.
fn settle_all(fixture: &Fixture) -> Vec<Settled> {
    let mut store = Store::new(fixture.program.clone()).unwrap_or_else(|e| {
        panic!(
            "fixture `{}` program must phase-assign cleanly: {e}",
            fixture.id
        )
    });
    fixture
        .stream
        .iter()
        .enumerate()
        .map(|(i, txn)| {
            store
                .commit(txn)
                .unwrap_or_else(|e| {
                    panic!(
                        "fixture `{}` transaction {i} must commit cleanly: {e}",
                        fixture.id
                    )
                })
                .clone()
        })
        .collect()
}

#[test]
fn errata_0002_predicate_level_condensation() {
    let fixture = predicate_level_condensation::fixture();
    assert_deterministic(&fixture, &predicate_level_condensation::fixture());

    // The erratum's core claim: Base/Trans condense into one phase despite
    // no direct Trans -> Base edge.
    let store = Store::new(fixture.program.clone()).unwrap();
    assert_eq!(
        store.phases().len(),
        1,
        "Base and Trans must settle in one phase (errata 0002)"
    );

    let revisions = settle_all(&fixture);
    assert_eq!(revisions.len(), 1);
    // Transitive closure over a->b->c->d: 3 + 2 + 1 = 6 pairs.
    assert_eq!(revisions[0].extent("Reach").unwrap().len(), 6);
    insta::assert_snapshot!(
        "errata_0002_predicate_level_condensation_rev1",
        render(&revisions[0])
    );
}

#[test]
fn errata_0001_entity_key_conflict() {
    let fixture = entity_key_conflict::fixture();
    assert_deterministic(&fixture, &entity_key_conflict::fixture());

    let revisions = settle_all(&fixture);
    assert_eq!(revisions.len(), 1);
    let settled = &revisions[0];

    // A rule-derived Widget(w1, "B") and a transaction-ensured Widget(w1,
    // "A") must surface as one KeyConflict with 2 candidates, per errata
    // 0001 (Entity relations join Derived under Part III §8's key-conflict
    // grouping) -- never a silent winner.
    assert_eq!(settled.provenance.key_conflicts.len(), 1);
    assert_eq!(settled.provenance.key_conflicts[0].relation, "Widget");
    assert_eq!(settled.provenance.key_conflicts[0].candidates.len(), 2);
    assert!(
        settled.extent("Widget").unwrap().is_empty(),
        "the conflicted key must not appear in Widget's live extent"
    );

    insta::assert_snapshot!("errata_0001_entity_key_conflict_rev1", render(settled));
}

#[test]
fn errata_0001_matchdigest_supportref_determinism() {
    let fixture = matchdigest_supportref::fixture();
    assert_deterministic(&fixture, &matchdigest_supportref::fixture());

    let revisions = settle_all(&fixture);
    assert_eq!(revisions.len(), 2);

    // Revision 1: both SourceA and SourceB support Flagged("x1") -- two
    // independent matches deriving identical content, not a conflict.
    let flagged_rev1 = revisions[0].extent("Flagged").unwrap();
    assert_eq!(flagged_rev1.len(), 1);
    let rev1_supports = flagged_rev1.values().next().unwrap().supports.len();
    assert_eq!(rev1_supports, 2, "Flagged(x1) must have two supports");

    // Revision 2: retracting SourceA's claim removes one support; the row
    // stays live via FromB alone (errata 0001: "shared supports removed in
    // any order converge", Appendix I.3).
    let flagged_rev2 = revisions[1].extent("Flagged").unwrap();
    assert_eq!(
        flagged_rev2.len(),
        1,
        "Flagged(x1) must survive losing one of its two supports"
    );
    let rev2_supports = flagged_rev2.values().next().unwrap().supports.len();
    assert_eq!(rev2_supports, 1);

    insta::assert_snapshot!(
        "errata_0001_matchdigest_supportref_rev1",
        render(&revisions[0])
    );
    insta::assert_snapshot!(
        "errata_0001_matchdigest_supportref_rev2",
        render(&revisions[1])
    );
}

#[test]
fn errata_0002_edge_identity_compatibility_domain() {
    let fixture = edge_identity_domain::fixture();
    assert_deterministic(&fixture, &edge_identity_domain::fixture());

    // The substantive claim: two relations with an identical role shape
    // and identical row content never collide on identity, because the
    // relation name is folded into the hashed payload ahead of the role
    // tuple (errata 0002; already implemented in
    // `RelationDef::edge_id`).
    let program = edge_identity_domain::program();
    let row = edge_identity_domain::matching_row();
    let edge_id_a = program.relations["A"].edge_id(&row);
    let edge_id_b = program.relations["B"].edge_id(&row);
    assert_ne!(
        edge_id_a, edge_id_b,
        "identical rows in different relations must not collide"
    );

    let revisions = settle_all(&fixture);
    assert_eq!(revisions.len(), 1);
    assert_eq!(revisions[0].extent("A").unwrap().len(), 1);
    assert_eq!(revisions[0].extent("B").unwrap().len(), 1);

    insta::assert_snapshot!(
        "errata_0002_edge_identity_compatibility_domain_rev1",
        render(&revisions[0])
    );
}
