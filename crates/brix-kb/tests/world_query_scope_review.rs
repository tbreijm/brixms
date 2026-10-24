//! Versioned cursors cannot turn off scope validation by clearing a field.
use brix_canon::{Digest, Domain};
use brix_kb::world::types::QueryCursor;
use brix_kb::world::{
    RelationDecl, TupleRecord, WorldBatch, WorldBatchOp, WorldKey, WorldManifest, WorldSession,
};

#[test]
fn versioned_cursor_with_empty_world_cannot_bypass_other_scope_checks() {
    let dir = std::env::temp_dir().join(format!(
        "brix-query-scope-review-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut world = WorldSession::create(
        &dir,
        WorldManifest::new(
            "actual-world",
            "2026-10-23",
            Digest::of(Domain::Value, b"query-scope-review"),
            vec![RelationDecl::new("rows", vec!["id".into()], vec![], vec![])],
        ),
    )
    .unwrap();
    let ops = (0..3)
        .map(|n| {
            let key = format!("row-{n}");
            let mut tuple = TupleRecord::new();
            tuple.set_str("id", &key);
            WorldBatchOp::Upsert {
                relation: "rows".into(),
                key: WorldKey::from_str(&key),
                tuple: tuple.to_tuple(),
            }
        })
        .collect();
    world.apply_batch(WorldBatch::new(0, "seed", ops)).unwrap();
    let first = world.query_page("rows", None, 1).unwrap();
    let mut cursor = QueryCursor::from_token(first.next_cursor.as_ref().unwrap()).unwrap();
    cursor.world_id.clear();
    cursor.relation = "another-relation".into();
    cursor.revision = Digest::from_bytes([0; 32]);
    let bad = cursor.to_token();
    let live = world.query_page("rows", Some(&bad), 1);
    let historical = world
        .pin_revision(1)
        .unwrap()
        .query_page("rows", Some(&bad), 1);
    drop(world);
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(
        live.is_err(),
        "live query accepted a versioned cursor with absent world and wrong relation/revision"
    );
    assert!(
        historical.is_err(),
        "snapshot accepted a versioned cursor with absent world and wrong relation/revision"
    );
}
