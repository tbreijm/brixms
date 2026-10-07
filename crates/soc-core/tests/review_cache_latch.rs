use std::fs;

use brix_canon::{Digest, Domain};
use soc_core::store::{FileNodeStore, NodeStore};

#[test]
fn cached_node_is_rejected_after_store_latches_a_read_error() {
    let dir = std::env::temp_dir().join(format!("review_cache_latch_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let a = b"node-a".to_vec();
    let b = b"node-b".to_vec();
    let da = Digest::of(Domain::Value, &a);
    let db = Digest::of(Domain::Value, &b);
    let mut store = FileNodeStore::new(&dir).unwrap();
    store.put_node(da, a.clone());
    store.put_node(db, b.clone());
    store.flush().unwrap();

    assert_eq!(store.get_node(&da), Some(a)); // warm cache
    let pack = fs::read_dir(store.objects_dir())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut bytes = fs::read(&pack).unwrap();
    bytes[8 + 6] ^= 0xff; // corrupt B's body after A
    fs::write(&pack, bytes).unwrap();
    assert_eq!(store.get_node(&db), None); // checksum error latches the store
    assert_eq!(
        store.get_node(&da),
        None,
        "cache must not bypass latched store error"
    );
}
