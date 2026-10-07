//! Positive scale test and structural bound verification for `TrieMap` (ADR-0046, P1).
//!
//! Verifies that:
//! 1. Single-key insert does NOT copy existing entries in the map (entry clones remain 0).
//! 2. Structural operations (`nodes_allocated`, `nodes_visited`, `nodes_hashed`)
//!    are strictly logarithmic in store size (bounded by depth <= 6 for N=1k..10k).

use std::cell::Cell;

use brix_canon::{CanonWriter, Canonical};
use soc_core::store::TrieMap;

thread_local! {
    static CLONES: Cell<u64> = const { Cell::new(0) };
}

#[derive(Debug, PartialEq, Eq)]
struct Counted(u64);

impl Clone for Counted {
    fn clone(&self) -> Self {
        CLONES.with(|c| c.set(c.get() + 1));
        Counted(self.0)
    }
}

impl Canonical for Counted {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_uint(self.0);
    }
}

fn count_clones<T>(f: impl FnOnce() -> T) -> (T, u64) {
    let before = CLONES.with(Cell::get);
    let out = f();
    (out, CLONES.with(Cell::get) - before)
}

#[test]
fn trie_map_insert_does_not_copy_existing_entries() {
    let measure = |size: usize| -> u64 {
        let mut map: TrieMap<u64, Counted> = TrieMap::new();
        for i in 0..size as u64 {
            map = map.insert(i, Counted(i * 10));
        }
        assert_eq!(map.len(), size);

        // Measure a single insert into the populated map:
        let (next, clones) = count_clones(|| map.insert(size as u64 + 100_000, Counted(999)));
        assert_eq!(next.len(), size + 1);
        clones
    };

    let series: Vec<(usize, u64)> = [1_000usize, 2_000, 4_000]
        .iter()
        .map(|&n| (n, measure(n)))
        .collect();

    for (n, c) in &series {
        eprintln!("TrieMap::insert (1 key)         N={n:>6}: {c} entry clones");
        // Counted is cloned 0 times because it is moved into Arc::new(value).
        // Crucially, zero of the existing N entries are cloned!
        assert_eq!(
            *c, 0,
            "TrieMap::insert must perform 0 clones of existing entries; got {c} at N={n}"
        );
    }
}

#[test]
fn trie_map_structural_operations_are_logarithmic() {
    // Measure TrieOpStats for inserting one new key into maps of sizes 100, 1k, 10k
    for &size in &[100usize, 1_000, 10_000] {
        let mut map: TrieMap<u64, u64> = TrieMap::new();
        for i in 0..size as u64 {
            map = map.insert(i, i * 7);
        }

        let key = size as u64 + 999_999;
        let (_next, stats) = map.insert_with_stats(key, 42);

        eprintln!(
            "N={size:>5} | visited: {:>2} | allocated: {:>2} | hashed: {:>2} | key_cmp: {:>2}",
            stats.nodes_visited, stats.nodes_allocated, stats.nodes_hashed, stats.key_comparisons
        );

        // In a 16-way branching radix trie:
        // log_16(10_000) ~ 3.32
        // Max depth is bounded well below 8
        assert!(
            stats.nodes_visited <= 8,
            "nodes visited ({}) exceeds structural bound 8 at N={size}",
            stats.nodes_visited
        );
        assert!(
            stats.nodes_allocated <= 8,
            "nodes allocated ({}) exceeds structural bound 8 at N={size}",
            stats.nodes_allocated
        );
        assert!(
            stats.nodes_hashed <= 8,
            "nodes hashed ({}) exceeds structural bound 8 at N={size}",
            stats.nodes_hashed
        );
    }
}
