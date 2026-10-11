//! The in-memory sparse sample of a pack's sorted idx narrows each lookup to a small window of
//! records. It is derived data: it must never hide a stored node or invent an absent one.

use brix_canon::{Digest, Domain};
use soc_core::store::{FileNodeStore, NodeStore};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

struct TestDir(PathBuf);
impl TestDir {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "brix-pack-sparse-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create dir");
        Self(path)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn dig(i: u64) -> Digest {
    Digest::of(Domain::Value, &i.to_be_bytes())
}

/// A digest that sorts next to `d` and shares its 8-byte sample prefix.
fn neighbour(d: &Digest) -> Digest {
    let mut b = *d.as_bytes();
    b[31] ^= 0x01;
    Digest::from_bytes(b)
}

/// Packs far larger than the sample stride: the table built at flush (same process) and the
/// one built lazily after a reopen must both find every stored digest and no absent one.
#[test]
fn sparse_windows_find_every_node_in_large_packs() {
    let dir = TestDir::new("large");
    let store = FileNodeStore::new(dir.path()).expect("open");
    let mut stored: Vec<Digest> = Vec::new();
    for pack in 0..3u64 {
        for i in 0..6000u64 {
            let d = dig(pack * 1_000_000 + i);
            let mut s = store.clone();
            s.put_node(d, format!("n-{pack}-{i}").into_bytes());
            stored.push(d);
        }
        store.flush().expect("flush");
    }
    let absent: Vec<Digest> = stored.iter().step_by(7).map(neighbour).collect();
    let check = |store: &FileNodeStore| {
        store.set_cache_capacity(0);
        for (n, d) in stored.iter().enumerate() {
            assert!(store.contains(d), "stored node {n} not found");
            assert!(
                store.get_node(d).unwrap().is_some(),
                "stored node {n} unreadable"
            );
        }
        for d in &absent {
            assert!(!store.contains(d), "absent neighbour reported present");
            assert_eq!(store.get_node(d).unwrap(), None);
        }
    };
    check(&store);
    let reopened = FileNodeStore::new(dir.path()).expect("reopen");
    check(&reopened);
    reopened.verify_indexes().expect("idx audit");
}

/// Sample boundaries: the smallest and largest digests of a pack, and a pack smaller than one
/// stride.
#[test]
fn sparse_windows_handle_pack_edges() {
    let dir = TestDir::new("edges");
    let store = FileNodeStore::new(dir.path()).expect("open");
    let small: Vec<Digest> = (0..5u64).map(|i| dig(900_000 + i)).collect();
    for d in &small {
        let mut s = store.clone();
        s.put_node(*d, b"small".to_vec());
    }
    store.flush().expect("flush small pack");
    let mut all: Vec<Digest> = (0..2000u64).map(dig).collect();
    for d in &all {
        let mut s = store.clone();
        s.put_node(*d, b"big".to_vec());
    }
    store.flush().expect("flush big pack");
    all.extend(small);
    all.sort();
    let (lowest, highest) = (all[0], *all.last().expect("non-empty"));
    for s in [
        store.clone(),
        FileNodeStore::new(dir.path()).expect("reopen"),
    ] {
        s.set_cache_capacity(0);
        for d in &all {
            assert!(s.contains(d));
        }
        assert!(s.contains(&lowest) && s.contains(&highest));
        for probe in [Digest::from_bytes([0; 32]), Digest::from_bytes([0xff; 32])] {
            assert!(!s.contains(&probe));
        }
        // Enough probes that the lazily opened packs build their tables mid-run.
        for round in 0..600u64 {
            assert!(!s.contains(&neighbour(&dig(round))));
        }
    }
}
