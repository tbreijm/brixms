use brix_canon::{Digest, Domain};
use soc_core::store::{FileNodeStore, MemoryNodeStore, NodeStore, TrieMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

struct TestDir(PathBuf);

impl TestDir {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "brix-batched-durability-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create test directory");
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

#[test]
fn test_cheap_existence_check_uses_in_memory_index_without_reading_body() {
    let dir = TestDir::new("cheap-exists");
    let mut store = FileNodeStore::new(dir.path()).expect("create store");
    let map: TrieMap<u64, u64> = (0..50).fold(TrieMap::new(), |m, k| m.insert(k, k * 10));
    map.persist_to_store(&mut store);
    store.flush().expect("flush pack");

    let root_digest = map.root_digest();

    // Reopen store to simulate cold session with loaded pack index
    let reopened = FileNodeStore::new(dir.path()).expect("reopen store");
    let before_io = reopened.io_stats();

    // Contains on root must be true and perform ZERO body reads
    assert!(reopened.contains(&root_digest));
    let after_io = reopened.io_stats();
    assert_eq!(
        after_io.reads, before_io.reads,
        "contains must not increment read count"
    );

    // Non-existent digest
    let bogus = Digest::of(Domain::Value, b"nonexistent");
    assert!(!reopened.contains(&bogus));
}

#[test]
fn test_crash_recovery_ignores_unfinalized_temp_packs() {
    let dir = TestDir::new("crash-tmp");
    let mut store = FileNodeStore::new(dir.path()).expect("create store");
    let d1 = Digest::of(Domain::Value, b"item1");
    store.put_node(d1, b"item1-payload".to_vec());

    // Plant an unfinalized .tmp pack file simulating an interrupted process
    let tmp_path = dir
        .path()
        .join("objects")
        .join("pack-99999-interrupted.tmp");
    fs::write(&tmp_path, b"incomplete-pack-data").expect("write tmp pack");

    // Do NOT call flush() on store (drop it, simulating crash)
    drop(store);

    // Reopen: must succeed, ignoring the .tmp file
    let mut reopened = FileNodeStore::new(dir.path()).expect("reopen must ignore tmp files");
    assert!(
        !reopened.contains(&d1),
        "unflushed write must not be visible"
    );

    // Now write and flush a real node
    let d2 = Digest::of(Domain::Value, b"item2");
    store_put(&mut reopened, d2, b"item2-payload".to_vec());
    reopened.flush().expect("flush real pack");

    let reopened2 = FileNodeStore::new(dir.path()).expect("second reopen");
    assert!(reopened2.contains(&d2));
    assert_eq!(reopened2.get_node(&d2), Some(b"item2-payload".to_vec()));
}

#[test]
fn cloned_pending_reader_survives_concurrent_flush_rename() {
    let dir = TestDir::new("clone-flush-race");
    let mut store = FileNodeStore::new(dir.path()).expect("create store");
    let digest = Digest::of(Domain::Value, b"pending-race-node");
    let payload = b"pending-race-payload".to_vec();
    store_put(&mut store, digest, payload.clone());

    let reader = store.clone();
    let barrier = Arc::new(Barrier::new(2));
    let reader_barrier = barrier.clone();
    let expected_payload = payload.clone();
    let read_thread = thread::spawn(move || {
        reader_barrier.wait();
        for _ in 0..10_000 {
            assert_eq!(reader.get_node(&digest), Some(expected_payload.clone()));
        }
        reader
    });

    barrier.wait();
    store.flush().expect("flush while clone reads");
    let reader = read_thread.join().expect("reader thread");
    assert_eq!(reader.get_node(&digest), Some(payload));
    assert!(reader.contains(&digest));
}

fn store_put(store: &mut FileNodeStore, digest: Digest, bytes: Vec<u8>) {
    NodeStore::put_node(store, digest, bytes);
}

#[test]
fn test_corrupt_or_truncated_pack_files_are_rejected() {
    let dir = TestDir::new("corrupt-reject");
    let mut store = FileNodeStore::new(dir.path()).expect("create store");
    for i in 0..10u64 {
        let d = Digest::of(Domain::Value, &i.to_be_bytes());
        store_put(&mut store, d, format!("val-{i}").into_bytes());
    }
    store.flush().expect("flush");
    drop(store);

    // Find the published pack
    let pack_path = fs::read_dir(dir.path().join("objects"))
        .expect("read objects")
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().and_then(|s| s.to_str()) == Some("pack"))
        .expect("must find pack");

    let original_bytes = fs::read(&pack_path).expect("read pack");
    assert!(original_bytes.len() > 100);

    // Test A: Truncate pack by 20 bytes (mutilates footer)
    {
        let mut truncated = original_bytes.clone();
        truncated.truncate(truncated.len() - 20);
        fs::write(&pack_path, &truncated).expect("write truncated pack");
        assert!(
            FileNodeStore::new(dir.path()).is_err(),
            "truncated pack must be rejected"
        );
    }

    // Test B: Corrupt end magic (last 8 bytes)
    {
        let mut corrupted = original_bytes.clone();
        let len = corrupted.len();
        corrupted[len - 8..].copy_from_slice(b"BADMAGIC");
        fs::write(&pack_path, &corrupted).expect("write bad magic pack");
        assert!(
            FileNodeStore::new(dir.path()).is_err(),
            "bad magic must be rejected"
        );
    }

    // Test C: Corrupt index chain checksum in footer (bytes len-40..len-8)
    {
        let mut corrupted = original_bytes.clone();
        let len = corrupted.len();
        corrupted[len - 25] ^= 0xFF; // flip bit in checksum
        fs::write(&pack_path, &corrupted).expect("write bad checksum pack");
        assert!(
            FileNodeStore::new(dir.path()).is_err(),
            "checksum mismatch must be rejected"
        );
    }

    // Test D: Restore original bytes and confirm clean open
    fs::write(&pack_path, &original_bytes).expect("restore pack");
    assert!(
        FileNodeStore::new(dir.path()).is_ok(),
        "restored pack must open cleanly"
    );
}

#[test]
fn body_corruption_is_latched_and_semantic_digest_is_still_verified() {
    let dir = TestDir::new("body-corrupt");
    let mut store = FileNodeStore::new(dir.path()).expect("create store");
    let digest = Digest::of(Domain::Value, b"node-address");
    store_put(&mut store, digest, b"node-body".to_vec());
    store.flush().expect("flush");
    drop(store);

    let pack_path = fs::read_dir(dir.path().join("objects"))
        .expect("read objects")
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().and_then(|s| s.to_str()) == Some("pack"))
        .expect("pack exists");
    let mut bytes = fs::read(&pack_path).expect("read pack");
    bytes[8] ^= 1;
    fs::write(&pack_path, bytes).expect("corrupt body");

    let reopened = FileNodeStore::new(dir.path()).expect("index framing stays valid");
    assert!(reopened.contains(&digest));
    assert_eq!(reopened.get_node(&digest), None);
    assert!(
        !reopened.contains(&digest),
        "checksum failure latches the handle"
    );

    // Generic NodeStore callers may store arbitrary bytes under an arbitrary
    // key. Trie hydration separately enforces that serialized nodes match the
    // expected Merkle digest.
    let semantic_dir = TestDir::new("semantic-digest");
    let mut semantic_store = FileNodeStore::new(semantic_dir.path()).expect("create store");
    let map: TrieMap<u64, u64> = TrieMap::new().insert(7, 11);
    let mut memory = MemoryNodeStore::new();
    map.persist_to_store(&mut memory);
    let node_bytes = memory
        .get_node(&map.root_digest())
        .expect("root node bytes");
    let wrong_digest = Digest::of(Domain::Value, b"not-the-root-digest");
    store_put(&mut semantic_store, wrong_digest, node_bytes);
    semantic_store.flush().expect("flush");
    let cold: TrieMap<u64, u64> = TrieMap::from_root_digest(wrong_digest, 1);
    assert!(cold.get_with_store(&0, &semantic_store).is_err());
}

#[test]
fn duplicate_digest_across_packs_is_rejected_on_open() {
    let dir = TestDir::new("duplicate-pack-digest");
    let mut first = FileNodeStore::new(dir.path()).expect("open first writer");
    let mut second = FileNodeStore::new(dir.path()).expect("open second writer");
    let digest = Digest::of(Domain::Value, b"shared-digest");

    store_put(&mut first, digest, b"same object".to_vec());
    store_put(&mut second, digest, b"same object".to_vec());
    first.flush().expect("publish first pack");
    second.flush().expect("publish second pack");

    assert!(
        FileNodeStore::new(dir.path()).is_err(),
        "ambiguous duplicate digest entries must not depend on directory order"
    );
}

#[test]
fn test_roots_come_out_identical_whatever_batch_split_or_storage_layout() {
    let items: Vec<(u64, String)> = (0..500u64)
        .map(|i| (i, format!("record-payload-{i}")))
        .collect();

    // 1. In-memory baseline: all in one pass
    let mut map_mem = TrieMap::new();
    for (k, v) in &items {
        map_mem = map_mem.insert(*k, v.clone());
    }
    let expected_root = map_mem.root_digest();

    // 2. In-memory: 10 batches of 50
    let mut map_batched = TrieMap::new();
    for chunk in items.chunks(50) {
        for (k, v) in chunk {
            map_batched = map_batched.insert(*k, v.clone());
        }
    }
    assert_eq!(
        map_batched.root_digest(),
        expected_root,
        "in-memory batch split matches"
    );

    // 3. FileNodeStore: single batch of 500
    let dir_single = TestDir::new("pack-single");
    let mut store_single = FileNodeStore::new(dir_single.path()).expect("store single");
    let mut map_file_single = TrieMap::new();
    for (k, v) in &items {
        map_file_single = map_file_single.insert(*k, v.clone());
    }
    map_file_single.persist_to_store(&mut store_single);
    store_single.flush().expect("flush single");
    assert_eq!(
        map_file_single.root_digest(),
        expected_root,
        "file pack single batch matches root"
    );

    // 4. FileNodeStore: 25 separate batches of 20 with flush after each
    let dir_multi = TestDir::new("pack-multi");
    let mut store_multi = FileNodeStore::new(dir_multi.path()).expect("store multi");
    let mut map_file_multi = TrieMap::new();
    for chunk in items.chunks(20) {
        for (k, v) in chunk {
            map_file_multi = map_file_multi.insert(*k, v.clone());
        }
        map_file_multi.persist_to_store(&mut store_multi);
        store_multi.flush().expect("flush batch");
    }
    assert_eq!(
        map_file_multi.root_digest(),
        expected_root,
        "file pack multi batch matches root"
    );

    // 5. Simulated legacy layout: write nodes to objects/{xx}/{yy}.bin
    let dir_legacy = TestDir::new("legacy-layout");
    let objects_dir = dir_legacy.path().join("objects");
    fs::create_dir_all(&objects_dir).expect("create objects dir");
    let mut mem_store = MemoryNodeStore::new();
    map_mem.persist_to_store(&mut mem_store);

    // Save all nodes from memory store to legacy loose files
    let legacy_store = FileNodeStore::new(dir_legacy.path()).expect("create store");
    // Manually write one node to verify legacy read
    let hex = expected_root.to_hex();
    let prefix = &hex[..2];
    let rest = &hex[2..];
    let sub = objects_dir.join(prefix);
    fs::create_dir_all(&sub).expect("create prefix dir");
    let legacy_node_path = sub.join(format!("{rest}.bin"));
    if let Some(bytes) = mem_store.get_node(&expected_root) {
        fs::write(&legacy_node_path, &bytes).expect("write legacy node");
    }

    // Verify FileNodeStore can read the legacy file directly
    assert!(legacy_store.contains(&expected_root));
    assert!(legacy_store.get_node(&expected_root).is_some());
}

#[test]
fn test_legacy_one_file_per_object_layout_stays_readable_and_updatable() {
    let dir = TestDir::new("legacy-compat");
    let objects_dir = dir.path().join("objects");
    fs::create_dir_all(&objects_dir).expect("create objects dir");

    // Populate a legacy node: digest d_leg
    let d_leg = Digest::of(Domain::Value, b"legacy-node-content-12345");
    let leg_bytes = b"canonical-legacy-payload-data";
    let hex = d_leg.to_hex();
    let sub = objects_dir.join(&hex[..2]);
    fs::create_dir_all(&sub).expect("create prefix");
    fs::write(sub.join(format!("{}.bin", &hex[2..])), leg_bytes).expect("write legacy bin");

    // Open FileNodeStore
    let mut store = FileNodeStore::new(dir.path()).expect("open store");
    assert!(
        store.contains(&d_leg),
        "legacy node must be detected by contains"
    );
    assert_eq!(
        store.get_node(&d_leg),
        Some(leg_bytes.to_vec()),
        "legacy node bytes must be read back exactly"
    );

    // Now write a new node which should land in a pack
    let d_new = Digest::of(Domain::Value, b"new-node-content-67890");
    let new_bytes = b"new-pack-payload-data";
    store_put(&mut store, d_new, new_bytes.to_vec());
    store.flush().expect("flush pack");

    // Both legacy and new node must be available
    assert!(store.contains(&d_leg));
    assert_eq!(store.get_node(&d_leg), Some(leg_bytes.to_vec()));
    assert!(store.contains(&d_new));
    assert_eq!(store.get_node(&d_new), Some(new_bytes.to_vec()));

    // Reopen and check again
    let reopened = FileNodeStore::new(dir.path()).expect("reopen store");
    assert!(reopened.contains(&d_leg));
    assert_eq!(reopened.get_node(&d_leg), Some(leg_bytes.to_vec()));
    assert!(reopened.contains(&d_new));
    assert_eq!(reopened.get_node(&d_new), Some(new_bytes.to_vec()));
}
