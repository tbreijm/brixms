//! Lane A cache behaviour: the raw node cache is byte-bounded, bulk scans do not populate it
//! or the decoded cache with leaves, and base tries dehydrated to (root, len) read back the
//! same root and entries through a store that has a decoded cache.

use brix_canon::{Digest, Domain};
use soc_core::store::{FileNodeStore, NodeStore, TrieMap};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

struct TestDir(PathBuf);
impl TestDir {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "brix-cache-budget-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create dir");
        Self(path)
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn raw_cache_is_byte_bounded_and_scan_reads_are_not_admitted() {
    let dir = TestDir::new("raw");
    let mut store = FileNodeStore::new(&dir.0).unwrap();
    let digests: Vec<Digest> = (0..200u32)
        .map(|i| {
            let mut bytes = vec![i as u8; 100];
            bytes.extend_from_slice(&i.to_be_bytes());
            let d = Digest::of(Domain::Value, &bytes);
            store.put_node(d, bytes);
            d
        })
        .collect();
    store.flush().unwrap();
    store.set_cache_budget_bytes(10 * (104 + 128));
    for d in &digests {
        store.get_node(d).unwrap();
    }
    assert!(store.cached_nodes_count() <= 10);
    assert!(store.cached_nodes_count() > 0);
    // Scan reads return the data but never admit.
    store.set_cache_budget_bytes(0);
    assert_eq!(store.cached_nodes_count(), 0);
    store.set_cache_budget_bytes(1000 * 232);
    for d in &digests {
        assert!(store.get_node_scan(d).unwrap().is_some());
    }
    assert_eq!(store.cached_nodes_count(), 0, "scan admitted nodes");
}

#[test]
fn scan_does_not_fill_decoded_cache_with_leaves_and_dehydrated_trie_reads_same() {
    let dir = TestDir::new("decoded");
    let mut store = FileNodeStore::new(&dir.0).unwrap();
    store.set_decoded_cache_capacity(64 << 20);
    let mut trie = TrieMap::<u64, u64>::new();
    for i in 0..2000u64 {
        trie = trie.insert(i, i * 7);
    }
    trie.persist_to_store(&mut store);
    store.flush().unwrap();
    let root = trie.root_digest();
    let cold = TrieMap::<u64, u64>::from_root_digest(root, trie.len());
    assert_eq!(cold.root_digest(), root);
    let used_after_persist = store.decoded_cache().unwrap().used_bytes();
    // Empty the decoded cache, then scan.
    store.set_decoded_cache_capacity(0);
    store.set_decoded_cache_capacity(64 << 20);
    let (all, _) = cold.iter_page_with_store(None, usize::MAX, &store).unwrap();
    assert_eq!(all.len(), 2000);
    let used = store.decoded_cache().unwrap().used_bytes();
    assert!(
        used < used_after_persist,
        "scan admitted leaves: {used} vs write-through {used_after_persist}"
    );
    for i in (0..2000u64).step_by(97) {
        assert_eq!(cold.get_with_store(&i, &store).unwrap(), Some(i * 7));
    }
    // Writing through a dehydrated trie reaches the same root as the hydrated one.
    let a = trie.insert(5000, 1);
    let (b, _) = cold.insert_with_store(5000, 1, &store).unwrap();
    assert_eq!(a.root_digest(), b.root_digest());
}
