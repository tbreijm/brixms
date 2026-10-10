//! Physical GC primitives of `FileNodeStore`: retiring packs, sweeping leftovers, legacy loose
//! objects and the full pack audit.

use std::fs;

use brix_canon::{Digest, Domain};
use soc_core::store::{FileNodeStore, NodeStore};

fn dir(name: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("pack_gc_{name}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

fn node(n: u32) -> (Digest, Vec<u8>) {
    let b = format!("node-{n}").into_bytes();
    (Digest::of(Domain::Value, &b), b)
}

#[test]
fn retire_packs_forgets_removed_nodes_and_keeps_the_rest() {
    let d = dir("retire");
    let mut s = FileNodeStore::new(&d).unwrap();
    let (a, ab) = node(1);
    let (b, bb) = node(2);
    s.put_node(a, ab.clone());
    s.flush().unwrap();
    s.put_node(b, bb.clone());
    s.flush().unwrap();
    assert!(s.contains(&a)); // warm `known`
    let packs = s.published_packs().unwrap();
    assert_eq!(packs.len(), 2);
    // Retire the pack holding `a` only.
    let first = packs[0].clone();
    let mut calls = 0;
    s.retire_packs(std::slice::from_ref(&first), &mut |_| {
        calls += 1;
        Ok(())
    })
    .unwrap();
    assert_eq!(calls, 1);
    assert_eq!(s.published_packs().unwrap().len(), 1);
    let survivors = [a, b].iter().filter(|x| s.try_contains(x).unwrap()).count();
    assert_eq!(survivors, 1, "exactly the other pack's node survives");
    assert!(!first.exists() && !first.with_extension("idx").exists());
    let reopened = FileNodeStore::new(&d).unwrap();
    assert_eq!(reopened.published_packs().unwrap().len(), 1);
}

#[test]
fn legacy_loose_objects_are_readable_and_retirable() {
    let d = dir("legacy");
    let (x, xb) = node(7);
    let hex = x.to_hex();
    let shard = d.join("objects").join(&hex[..2]);
    fs::create_dir_all(&shard).unwrap();
    fs::write(shard.join(format!("{}.bin", &hex[2..])), &xb).unwrap();
    let s = FileNodeStore::new(&d).unwrap();
    assert!(s.has_legacy_objects());
    assert_eq!(s.get_node(&x).unwrap(), Some(xb.clone()));
    assert!(s.try_contains(&x).unwrap());
    assert_eq!(s.verify_store().unwrap().legacy_nodes, 1);
    // GC copies live legacy nodes into a pack, then drops the loose files.
    let mut s = s;
    s.put_new_node(x, xb.clone());
    s.flush().unwrap();
    assert!(s.retire_legacy_objects().unwrap() > 0);
    assert!(!shard.exists());
    let reopened = FileNodeStore::new(&d).unwrap();
    assert!(!reopened.has_legacy_objects());
    assert_eq!(reopened.get_node(&x).unwrap(), Some(xb));
}

#[test]
fn sweep_removes_unpublished_leftovers_and_orphan_idx() {
    let d = dir("sweep");
    let mut s = FileNodeStore::new(&d).unwrap();
    let (a, ab) = node(1);
    s.put_node(a, ab);
    s.flush().unwrap();
    let objects = d.join("objects");
    fs::write(objects.join("pack-1-2-3.tmp"), b"junk").unwrap();
    fs::write(objects.join("pack-9-9-9.idx"), b"orphan").unwrap();
    assert_eq!(s.sweep_unpublished().unwrap(), 2);
    assert!(s.try_contains(&a).unwrap());
}

#[test]
fn verify_store_catches_a_flipped_body_byte() {
    let d = dir("audit");
    let mut s = FileNodeStore::new(&d).unwrap();
    for n in 0..20 {
        let (x, xb) = node(n);
        s.put_node(x, xb);
    }
    s.flush().unwrap();
    let r = s.verify_store().unwrap();
    assert_eq!((r.packs, r.nodes), (1, 20));
    let pack = s.published_packs().unwrap()[0].clone();
    let mut bytes = fs::read(&pack).unwrap();
    bytes[9] ^= 1;
    fs::write(&pack, bytes).unwrap();
    let e = s.verify_store().unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
}
