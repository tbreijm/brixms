#![allow(unsafe_code)]
//! Measurement (not a gate): heap bytes per path-copied node of a hydrated trie, and the
//! size of `Node<WorldKey, WorldTuple>`. Run with
//! `cargo test -p brix-kb --test lane_a_node_heap -- --nocapture --ignored`.

use brix_kb::world::{WorldKey, WorldTuple};
use soc_core::store::{FileNodeStore, Node, TrieMap};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counting;
static LIVE: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        LIVE.fetch_add(l.size(), Ordering::Relaxed);
        System.alloc(l)
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        LIVE.fetch_sub(l.size(), Ordering::Relaxed);
        System.dealloc(p, l)
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        if n >= l.size() {
            LIVE.fetch_add(n - l.size(), Ordering::Relaxed);
        } else {
            LIVE.fetch_sub(l.size() - n, Ordering::Relaxed);
        }
        System.realloc(p, l, n)
    }
}

#[global_allocator]
static A: Counting = Counting;

fn key(i: u64) -> WorldKey {
    WorldKey::from_str(&format!("orders:order-{i:09}"))
}
fn tuple(i: u64) -> WorldTuple {
    WorldTuple::new(format!("sku-{:05}|qty={}|pad-pad-pad-pad-pad", i % 997, i % 13).into_bytes())
}

#[test]
#[ignore]
fn heap_bytes_per_path_copied_node() {
    println!(
        "size_of Node<WorldKey,WorldTuple> = {}",
        std::mem::size_of::<Node<WorldKey, WorldTuple>>()
    );
    let dir = std::env::temp_dir().join(format!("lane-a-heap-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut store = FileNodeStore::new(&dir).unwrap();
    let mut t = TrieMap::<WorldKey, WorldTuple>::new();
    for i in 0..200_000u64 {
        t = t.insert(key(i), tuple(i));
    }
    t.persist_to_store(&mut store);
    store.flush().unwrap();
    let root = t.root_digest();
    let len = t.len();
    drop(t);
    let mut cold = TrieMap::<WorldKey, WorldTuple>::from_root_digest(root, len);
    let before = LIVE.load(Ordering::Relaxed);
    for j in 0..20_000u64 {
        let i = (j * 7919) % 200_000;
        cold = cold
            .insert_with_store(key(i), tuple(i + 1), &store)
            .unwrap()
            .0;
    }
    let after = LIVE.load(Ordering::Relaxed);
    let live = after - before;
    let nodes = cold.persist_to_store(&mut store);
    println!(
        "hydrated heap {live} B for {nodes} path-copied nodes = {:.0} B/node (encoded avg ~150-500 B)",
        live as f64 / nodes as f64
    );
    let _ = std::fs::remove_dir_all(&dir);
}
