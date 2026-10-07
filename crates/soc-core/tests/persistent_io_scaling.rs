use brix_canon::Digest;
use soc_core::store::{CanonHasher, FileNodeStore, KeyHasher, MemoryNodeStore, NodeStore, TrieMap};
use std::cell::Cell;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const HASH_DEPTH_PATH_GATE: u64 = 64 * 16;

#[derive(Default)]
struct IoCounts {
    reads: Cell<u64>,
    bytes_read: Cell<u64>,
    writes: Cell<u64>,
    bytes_written: Cell<u64>,
}

#[derive(Default)]
struct CountingMemoryStore {
    inner: MemoryNodeStore,
    counts: IoCounts,
}

impl CountingMemoryStore {
    fn reset_counts(&self) {
        self.counts.reads.set(0);
        self.counts.bytes_read.set(0);
        self.counts.writes.set(0);
        self.counts.bytes_written.set(0);
    }
}

impl NodeStore for CountingMemoryStore {
    fn get_node(&self, digest: &Digest) -> Option<Vec<u8>> {
        self.counts.reads.set(self.counts.reads.get() + 1);
        let bytes = self.inner.get_node(digest)?;
        self.counts
            .bytes_read
            .set(self.counts.bytes_read.get() + bytes.len() as u64);
        Some(bytes)
    }

    fn put_node(&mut self, digest: Digest, bytes: Vec<u8>) {
        self.counts.writes.set(self.counts.writes.get() + 1);
        self.counts
            .bytes_written
            .set(self.counts.bytes_written.get() + bytes.len() as u64);
        self.inner.put_node(digest, bytes);
    }
}

struct TestDir(PathBuf);

impl TestDir {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "brix-persistent-io-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create test directory");
        Self(path)
    }

    fn path(&self) -> &PathBuf {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn seeded_map(count: u64) -> TrieMap<u64, u64> {
    (0..count).fold(TrieMap::new(), |map, key| map.insert(key, key * 3))
}

fn assert_path_gate(reads: u64, bytes: u64) {
    assert!(reads <= HASH_DEPTH_PATH_GATE, "path read {reads} nodes");
    assert!(bytes > 0, "path read should account for node bytes");
}

#[test]
fn warm_and_lazy_cold_point_paths_stay_within_hash_depth_bound_as_world_grows() {
    let mut measurements = Vec::new();
    for size in [1_000u64, 10_000] {
        let map = seeded_map(size);
        let (_, warm_stats) = map.insert_with_stats(size + 1, 7);
        assert!(
            warm_stats.nodes_visited <= 64 * 16,
            "warm lookup at {size} entries visited {} nodes",
            warm_stats.nodes_visited
        );

        let mut store = CountingMemoryStore::default();
        map.persist_to_store(&mut store);
        store.reset_counts();
        let cold: TrieMap<u64, u64> = TrieMap::from_root_digest(map.root_digest(), map.len());
        let (cold_updated, cold_stats) = cold
            .insert_with_store(size - 2, 7, &store)
            .expect("cold path update");
        assert!(cold_stats.nodes_visited <= 64 * 16);
        cold_updated.persist_to_store(&mut store);
        assert_path_gate(store.counts.reads.get(), store.counts.bytes_read.get());
        assert!(store.counts.writes.get() <= HASH_DEPTH_PATH_GATE);
        assert!(store.counts.bytes_written.get() > 0);
        measurements.push((size, warm_stats.nodes_visited, store.counts.reads.get()));
    }

    assert!(measurements[1].1 <= 64 * 16);
    assert!(measurements[1].2 <= 64 * 16);
}

#[test]
fn late_page_reads_are_bounded_and_full_scan_is_a_growing_negative_control() {
    let size = 10_000u64;
    let map = seeded_map(size);
    let mut store = CountingMemoryStore::default();
    map.persist_to_store(&mut store);
    let cold: TrieMap<u64, u64> = TrieMap::from_root_digest(map.root_digest(), map.len());

    let mut ranked_keys: Vec<_> = (0..size)
        .map(|key| (CanonHasher.hash_key(&key), key))
        .collect();
    ranked_keys.sort();
    let (cursor_hash, cursor_key) = ranked_keys[ranked_keys.len() * 9 / 10];

    store.reset_counts();
    let (page, next) = cold
        .iter_page_with_store(Some((&cursor_hash, &cursor_key)), 8, &store)
        .unwrap();
    assert_eq!(page.len(), 8);
    assert!(next.is_some());
    let late_page_reads = store.counts.reads.get();
    assert_path_gate(late_page_reads, store.counts.bytes_read.get());
    assert!(
        late_page_reads < size / 2,
        "late page read {late_page_reads} nodes"
    );

    let mut scan_counts = Vec::new();
    for scan_size in [1_000u64, 10_000] {
        let scan_map = seeded_map(scan_size);
        let mut scan_store = CountingMemoryStore::default();
        scan_map.persist_to_store(&mut scan_store);
        scan_store.reset_counts();
        let scan: TrieMap<u64, u64> =
            TrieMap::from_root_digest(scan_map.root_digest(), scan_map.len());
        let (all, cursor) = scan
            .iter_page_with_store(None, scan_size as usize, &scan_store)
            .unwrap();
        assert_eq!(all.len(), scan_size as usize);
        assert!(cursor.is_none());
        scan_counts.push(scan_store.counts.reads.get());
    }
    assert!(
        scan_counts[1] > scan_counts[0],
        "full scan should grow with world size"
    );
    assert!(
        scan_counts[1] > HASH_DEPTH_PATH_GATE,
        "negative control should exceed the point-path gate"
    );
}

#[test]
fn file_store_flush_syncs_only_new_objects_once() {
    let dir = TestDir::new("flush");
    let mut store = FileNodeStore::new(dir.path()).expect("create file node store");
    let map = seeded_map(128);
    let expected_new_files = map.persist_to_store(&mut store) as u64;
    assert!(expected_new_files > 0);

    let before_flush = store.io_stats();
    assert_eq!(before_flush.writes, expected_new_files);
    store.flush().expect("flush new objects");
    let after_flush = store.io_stats();
    assert_eq!(after_flush.files_synced, 1);
    assert!(after_flush.physical_writes < expected_new_files);
    assert_eq!(after_flush.directories_synced, 2);
    assert!(after_flush.directories_synced > 0);
    assert!(after_flush.directories_synced <= expected_new_files + 1);

    store.flush().expect("second flush has no pending objects");
    assert_eq!(store.io_stats(), after_flush);
}
