//! Differential testing between `TrieMap` and `BTreeMap` reference oracle (ADR-0046, P1).
//!
//! Verifies:
//! 1. Differential equivalence: any sequence of inserts, overwrites, and removals
//!    produces identical contents and lookups compared to `std::collections::BTreeMap`.
//! 2. Canonical root invariant: equal maps from arbitrary insertion orderings produce
//!    byte-for-byte identical root digests.
//! 3. Adversarial key patterns (dense prefixes, sparse powers of two, monotonic runs).
//! 4. Hash collision handling with `ModuloHasher`.

use std::collections::BTreeMap;

use proptest::prelude::*;
use soc_core::store::{ModuloHasher, TrieMap};

#[derive(Clone, Debug)]
enum Op {
    Insert(u64, u64),
    Remove(u64),
}

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0u64..200, 0u64..10_000).prop_map(|(k, v)| Op::Insert(k, v)),
        (0u64..200).prop_map(Op::Remove),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn trie_map_matches_btree_map_under_arbitrary_operations(ops in prop::collection::vec(op_strategy(), 1..150)) {
        let mut trie: TrieMap<u64, u64> = TrieMap::new();
        let mut btree: BTreeMap<u64, u64> = BTreeMap::new();

        for op in &ops {
            match op {
                Op::Insert(k, v) => {
                    trie = trie.insert(*k, *v);
                    btree.insert(*k, *v);
                }
                Op::Remove(k) => {
                    trie = trie.remove(k);
                    btree.remove(k);
                }
            }

            prop_assert_eq!(trie.len(), btree.len());
            prop_assert_eq!(trie.is_empty(), btree.is_empty());
        }

        // Verify all keys match oracle
        for (k, v) in &btree {
            prop_assert_eq!(trie.get(k), Some(v));
        }

        // Verify absent keys
        for probe in 0..250u64 {
            if !btree.contains_key(&probe) {
                prop_assert_eq!(trie.get(&probe), None);
            }
        }
    }

    #[test]
    fn canonical_root_digest_is_independent_of_insertion_order(
        mut entries in prop::collection::vec((0u64..500, 0u64..10_000), 10..80)
    ) {
        // Remove duplicates from input to ensure equal final bindings
        entries.sort_by_key(|e| e.0);
        entries.dedup_by_key(|e| e.0);

        // Map 1: insert in forward order
        let mut m1: TrieMap<u64, u64> = TrieMap::new();
        for (k, v) in &entries {
            m1 = m1.insert(*k, *v);
        }

        // Map 2: insert in reverse order
        let mut m2: TrieMap<u64, u64> = TrieMap::new();
        for (k, v) in entries.iter().rev() {
            m2 = m2.insert(*k, *v);
        }

        // Map 3: insert in odd-even interleaved order
        let mut m3: TrieMap<u64, u64> = TrieMap::new();
        for (k, v) in entries.iter().step_by(2) {
            m3 = m3.insert(*k, *v);
        }
        for (k, v) in entries.iter().skip(1).step_by(2) {
            m3 = m3.insert(*k, *v);
        }

        prop_assert_eq!(m1.root_digest(), m2.root_digest());
        prop_assert_eq!(m1.root_digest(), m3.root_digest());
        prop_assert_eq!(&m1, &m2);
        prop_assert_eq!(&m1, &m3);
    }
}

#[test]
fn adversarial_key_distribution_dense_prefixes() {
    let mut trie: TrieMap<String, u64> = TrieMap::new();
    let mut btree: BTreeMap<String, u64> = BTreeMap::new();

    // 100 keys sharing a 40-character common prefix
    let prefix = "corporate.division.dept.group.project.team.";
    for i in 0..100u64 {
        let key = format!("{prefix}{i:05}");
        trie = trie.insert(key.clone(), i * 11);
        btree.insert(key, i * 11);
    }

    assert_eq!(trie.len(), 100);
    for (k, v) in &btree {
        assert_eq!(trie.get(k), Some(v));
    }

    // Remove half of them
    for i in (0..100u64).step_by(2) {
        let key = format!("{prefix}{i:05}");
        trie = trie.remove(&key);
        btree.remove(&key);
    }

    assert_eq!(trie.len(), 50);
    for (k, v) in &btree {
        assert_eq!(trie.get(k), Some(v));
    }
}

#[test]
fn adversarial_key_distribution_sparse_powers_of_two() {
    let mut trie: TrieMap<u64, u64> = TrieMap::new();
    let mut btree: BTreeMap<u64, u64> = BTreeMap::new();

    for shift in 0..63 {
        let key = 1u64 << shift;
        trie = trie.insert(key, shift as u64);
        btree.insert(key, shift as u64);
    }

    assert_eq!(trie.len(), 63);
    for (k, v) in &btree {
        assert_eq!(trie.get(k), Some(v));
    }
}

#[test]
fn collision_bucket_differential_under_modulo_hasher() {
    // Modulo 3 hasher forces keys to collide into 3 distinct hash buckets
    let hasher = ModuloHasher::new(3);
    let mut trie: TrieMap<u64, u64, ModuloHasher> = TrieMap::with_hasher(hasher);
    let mut btree: BTreeMap<u64, u64> = BTreeMap::new();

    for i in 0..60u64 {
        trie = trie.insert(i, i * 100);
        btree.insert(i, i * 100);
    }

    assert_eq!(trie.len(), 60);
    for (k, v) in &btree {
        assert_eq!(trie.get(k), Some(v));
    }

    // Remove 20 keys
    for i in 10..30u64 {
        trie = trie.remove(&i);
        btree.remove(&i);
    }

    assert_eq!(trie.len(), 40);
    for (k, v) in &btree {
        assert_eq!(trie.get(k), Some(v));
    }
}
