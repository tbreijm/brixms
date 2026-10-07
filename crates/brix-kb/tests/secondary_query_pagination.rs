//! Secondary query continuations never silently truncate or change query scope.
use brix_canon::{Digest, Domain};
use brix_kb::world::session::SecondaryIndexCursor;
use brix_kb::world::{
    encode_secondary_key, RelationDecl, TupleRecord, WorldBatch, WorldBatchOp, WorldError,
    WorldKey, WorldManifest, WorldSession, WorldTuple,
};
use std::{collections::BTreeSet, fs, path::PathBuf, time::SystemTime};

struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(std::env::temp_dir().join(format!(
            "brix-index-page-{name}-{}-{nonce}",
            std::process::id()
        )))
    }
    fn session(&self, count: u64) -> WorldSession {
        let manifest = WorldManifest::new(
            "pagination",
            "2026-10-04T00:00:00Z",
            Digest::of(Domain::Value, b"pagination"),
            ["orders", "shipments"]
                .into_iter()
                .map(|name| {
                    RelationDecl::new(
                        name,
                        vec!["id".into()],
                        vec!["customer".into(), "carrier".into()],
                        vec!["customer".into(), "carrier".into()],
                    )
                })
                .collect(),
        );
        let mut session = WorldSession::create(&self.0, manifest).unwrap();
        let mut tuple = TupleRecord::new();
        tuple.set_str("customer", "alice");
        tuple.set_str("carrier", "alice");
        session
            .apply_batch(WorldBatch::new(
                0,
                "seed",
                (0..count)
                    .map(|id| WorldBatchOp::Upsert {
                        relation: "orders".into(),
                        key: WorldKey::from_u64(id),
                        tuple: tuple.to_tuple(),
                    })
                    .collect(),
            ))
            .unwrap();
        session
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn pages_reopen_and_convenience_collect_every_member() {
    let fixture = Fixture::new("all");
    let session = fixture.session(1030);
    let key = encode_secondary_key(b"alice");
    let expected: BTreeSet<_> = (0..1030).map(WorldKey::from_u64).collect();
    let all = session
        .query_secondary_index("orders", "customer", &key)
        .unwrap();
    assert_eq!(all.len(), 1030);
    assert_eq!(all.into_iter().collect::<BTreeSet<_>>(), expected);
    let first = session
        .query_secondary_index_page("orders", "customer", &key, None, 100)
        .unwrap();
    assert_eq!(first.keys.len(), 100);
    let token = first.next_cursor.unwrap().to_token();
    drop(session);
    let reopened = WorldSession::open(&fixture.0).unwrap();
    let snapshot = reopened.pin_revision(1).unwrap();
    let mut cursor = Some(SecondaryIndexCursor::from_token(&token).unwrap());
    let mut actual: BTreeSet<_> = first.keys.into_iter().collect();
    loop {
        let page = snapshot
            .query_secondary_index_page("orders", "customer", &key, cursor.as_ref(), 100)
            .unwrap();
        assert!(page.keys.len() <= 100);
        for member in page.keys {
            assert!(actual.insert(member), "duplicate membership");
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(actual, expected);
    assert_eq!(
        snapshot
            .query_secondary_index("orders", "customer", &key)
            .unwrap()
            .len(),
        1030
    );
}

#[test]
fn cursor_is_bound_to_revision_relation_index_and_value() {
    let fixture = Fixture::new("scope");
    let mut session = fixture.session(4);
    let key = encode_secondary_key(b"alice");
    let cursor = session
        .query_secondary_index_page("orders", "customer", &key, None, 1)
        .unwrap()
        .next_cursor
        .unwrap();
    for (relation, index, value) in [
        ("shipments", "customer", key.clone()),
        ("orders", "carrier", key.clone()),
        ("orders", "customer", encode_secondary_key(b"bob")),
    ] {
        assert!(matches!(
            session.query_secondary_index_page(relation, index, &value, Some(&cursor), 1),
            Err(WorldError::InvalidCursor(_))
        ));
    }
    let snapshot = session.pin_revision(1).unwrap();
    session
        .apply_batch(WorldBatch::new(
            1,
            "remove",
            vec![WorldBatchOp::Remove {
                relation: "orders".into(),
                key: WorldKey::from_u64(0),
            }],
        ))
        .unwrap();
    assert!(matches!(
        session.query_secondary_index_page("orders", "customer", &key, Some(&cursor), 1),
        Err(WorldError::InvalidCursor(_))
    ));
    assert!(snapshot
        .query_secondary_index_page("orders", "customer", &key, Some(&cursor), 1)
        .is_ok());
    let token = cursor.to_token();
    assert_eq!(SecondaryIndexCursor::from_token(&token).unwrap(), cursor);
    assert!(SecondaryIndexCursor::from_token(&(token + "00")).is_err());
    for malformed in ["", "garbage", "00", "éé"] {
        assert!(SecondaryIndexCursor::from_token(malformed).is_err());
    }
}

#[test]
fn exact_terminal_page_zero_limit_and_explicit_key_encoding() {
    let fixture = Fixture::new("limits");
    let session = fixture.session(4);
    let key = encode_secondary_key(b"alice");
    let page = session
        .query_secondary_index_page("orders", "customer", &key, None, 4)
        .unwrap();
    assert_eq!(page.keys.len(), 4);
    assert!(page.next_cursor.is_none());
    assert!(session
        .query_secondary_index_page("orders", "customer", &key, None, 0)
        .is_err());
    assert_eq!(
        session
            .query_secondary_index_page("orders", "customer", &key, None, usize::MAX)
            .unwrap()
            .keys
            .len(),
        4
    );
    // Raw bytes are not an alternate representation of the encoded key.
    assert!(session
        .query_secondary_index("orders", "customer", &WorldKey::new(b"alice".to_vec()))
        .unwrap()
        .is_empty());
    assert!(session
        .query_secondary_index_page(
            "orders",
            "customer",
            &encode_secondary_key(b"missing"),
            None,
            2
        )
        .unwrap()
        .keys
        .is_empty());
}

#[test]
fn malformed_or_missing_membership_roots_fail_closed() {
    let fixture = Fixture::new("corruption");
    let mut session = fixture.session(4);
    let key = encode_secondary_key(b"alice");
    for bytes in [vec![1; 31], vec![2; 33], vec![3; 32]] {
        let index = &session.secondary_indexes["orders:customer"];
        let (changed, _) = index
            .insert_with_store(key.clone(), WorldTuple::new(bytes), &session.node_store)
            .unwrap();
        session
            .secondary_indexes
            .insert("orders:customer".into(), changed);
        assert!(matches!(
            session.query_secondary_index("orders", "customer", &key),
            Err(WorldError::CorruptedObject(_) | WorldError::MissingObject(_))
        ));
        let mut snapshot = session.pin_revision(1).unwrap();
        snapshot.secondary_indexes = session.secondary_indexes.clone();
        assert!(matches!(
            snapshot.query_secondary_index_page("orders", "customer", &key, None, 1),
            Err(WorldError::CorruptedObject(_) | WorldError::MissingObject(_))
        ));
    }
}
