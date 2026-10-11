//! The plain (store-less) `TrieMap` operations must never silently treat an unhydrated lazy
//! node as empty (R0 M3): `get` used to return `None`, `iter` skipped the subtree and
//! `remove` was a no-op. They now fail loudly; the `*_with_store` operations are the only
//! way to read through lazy nodes.

use std::panic::{catch_unwind, AssertUnwindSafe};

use soc_core::store::{MemoryNodeStore, TrieMap};

fn lazy_map() -> (TrieMap<u64, u64>, MemoryNodeStore) {
    let mut map: TrieMap<u64, u64> = TrieMap::new();
    for i in 0..64u64 {
        map = map.insert(i, i * 10);
    }
    let mut store = MemoryNodeStore::new();
    assert!(map.persist_to_store(&mut store) > 0);
    (
        TrieMap::from_root_digest(map.root_digest(), map.len()),
        store,
    )
}

fn panics<R>(f: impl FnOnce() -> R) -> bool {
    catch_unwind(AssertUnwindSafe(f)).is_err()
}

#[test]
fn plain_ops_on_a_lazy_root_fail_loudly() {
    let (lazy, _store) = lazy_map();
    assert!(panics(|| lazy.get(&3)));
    assert!(panics(|| lazy.contains_key(&3)));
    assert!(panics(|| lazy.remove(&3)));
    assert!(panics(|| lazy.insert(100, 1)));
    assert!(panics(|| lazy.iter().len()));
}

#[test]
fn plain_ops_reaching_a_lazy_child_fail_loudly() {
    // Hydrate only the path to one key: siblings stay lazy.
    let (lazy, store) = lazy_map();
    let (hydrated, _) = lazy.insert_with_store(7, 70, &store).unwrap();
    assert!(
        panics(|| hydrated.iter().len()),
        "iterating a partially hydrated trie must not skip lazy subtrees"
    );
}

#[test]
fn store_aware_ops_read_through_lazy_nodes() {
    let (lazy, store) = lazy_map();
    for i in 0..64u64 {
        assert_eq!(lazy.get_with_store(&i, &store).unwrap(), Some(i * 10));
    }
    assert_eq!(lazy.get_with_store(&999, &store).unwrap(), None);
    let (removed, _) = lazy.remove_with_store(&5, &store).unwrap();
    assert_eq!(removed.len(), 63);
    assert_eq!(removed.get_with_store(&5, &store).unwrap(), None);
    let (rows, _) = lazy.iter_page_with_store(None, usize::MAX, &store).unwrap();
    assert_eq!(rows.len(), 64);
}

#[test]
fn fully_in_memory_tries_are_unaffected() {
    let mut map: TrieMap<u64, u64> = TrieMap::new();
    for i in 0..32u64 {
        map = map.insert(i, i);
    }
    assert_eq!(map.get(&4), Some(&4));
    assert_eq!(map.iter().len(), 32);
    assert_eq!(map.remove(&4).len(), 31);
}
