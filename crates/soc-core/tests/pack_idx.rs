//! Sorted fanout-addressed per-pack `.idx`: lookup equivalence, legacy packs,
//! crash-safe publish, fail-closed corruption, and open cost flat in node count.

use brix_canon::{Digest, Domain};
use soc_core::store::{FileNodeStore, NodeStore};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

struct TestDir(PathBuf);
impl TestDir {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "brix-pack-idx-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create dir");
        Self(path)
    }
    fn path(&self) -> &Path {
        &self.0
    }
    fn objects(&self) -> PathBuf {
        self.0.join("objects")
    }
    fn with_ext(&self, ext: &str) -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = fs::read_dir(self.objects())
            .expect("read objects")
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some(ext))
            .collect();
        found.sort();
        found
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

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn chain_next(previous: Digest, record: &[u8; 80]) -> Digest {
    let mut bytes = [0; 112];
    bytes[..32].copy_from_slice(previous.as_bytes());
    bytes[32..].copy_from_slice(record);
    Digest::of(Domain::Value, &bytes)
}

/// Write a pack in the pre-idx v1 framing (no sidecar), bodies in the order given.
fn write_legacy_pack(path: &Path, items: &[(Digest, Vec<u8>)]) {
    let mut out = std::io::BufWriter::new(fs::File::create(path).expect("create pack"));
    out.write_all(b"BRIXPK01").unwrap();
    let mut records = Vec::new();
    let mut offset = 8u64;
    for (digest, body) in items {
        out.write_all(body).unwrap();
        let mut rec = [0u8; 80];
        rec[..32].copy_from_slice(digest.as_bytes());
        rec[32..40].copy_from_slice(&offset.to_be_bytes());
        rec[40..48].copy_from_slice(&(body.len() as u64).to_be_bytes());
        rec[48..].copy_from_slice(Digest::of(Domain::Value, body).as_bytes());
        records.push(rec);
        offset += body.len() as u64;
    }
    let mut chain = Digest::of(Domain::Value, b"brix:pack:index:v1");
    for rec in &records {
        out.write_all(rec).unwrap();
        chain = chain_next(chain, rec);
    }
    out.write_all(&offset.to_be_bytes()).unwrap();
    out.write_all(&(records.len() as u64).to_be_bytes())
        .unwrap();
    out.write_all(chain.as_bytes()).unwrap();
    out.write_all(b"BRIXEND1").unwrap();
    out.flush().unwrap();
}

#[test]
fn randomized_lookup_matches_model_across_packs_and_reopen() {
    let dir = TestDir::new("random");
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut model: BTreeMap<Digest, Vec<u8>> = BTreeMap::new();
    let mut store = FileNodeStore::new(dir.path()).expect("open");
    for round in 0..12u64 {
        let n = 1 + rng.next() % 400;
        for _ in 0..n {
            // Deliberately re-use some keys so dedupe across packs is exercised.
            let key = if rng.next().is_multiple_of(4) {
                rng.next() % 200
            } else {
                rng.next()
            };
            let body = format!("body-{key}").into_bytes();
            let mut s = store.clone();
            s.put_node(dig(key), body.clone());
            model.insert(dig(key), body);
        }
        store.flush().expect("flush");
        if round % 4 == 3 {
            store = FileNodeStore::new(dir.path()).expect("reopen");
            store.verify_indexes().expect("idx audit");
        }
    }
    store = FileNodeStore::new(dir.path()).expect("final reopen");
    store.set_cache_capacity(0);
    for (digest, body) in &model {
        assert!(store.contains(digest));
        assert_eq!(store.get_node(digest).as_ref(), Some(body));
    }
    for miss in 0..2000u64 {
        let d = Digest::of(
            Domain::Value,
            &[b"miss".as_slice(), &miss.to_be_bytes()].concat(),
        );
        assert!(!store.contains(&d));
        assert_eq!(store.get_node(&d), None);
    }
    assert_eq!(store.node_count(), model.len());
}

#[test]
fn legacy_pack_without_idx_and_loose_objects_remain_readable() {
    let dir = TestDir::new("legacy");
    fs::create_dir_all(dir.objects()).unwrap();
    let items: Vec<(Digest, Vec<u8>)> = (0..500u64)
        .map(|i| (dig(i), format!("legacy-{i}").into_bytes()))
        .collect();
    write_legacy_pack(&dir.objects().join("pack-1-1-1.pack"), &items);
    // Loose legacy object.
    let loose = dig(10_000);
    let hex = loose.to_hex();
    fs::create_dir_all(dir.objects().join(&hex[..2])).unwrap();
    fs::write(
        dir.objects()
            .join(&hex[..2])
            .join(format!("{}.bin", &hex[2..])),
        b"loose",
    )
    .unwrap();

    assert!(dir.with_ext("idx").is_empty());
    let store = FileNodeStore::new(dir.path()).expect("open legacy");
    assert_eq!(dir.with_ext("idx").len(), 1, "idx built once");
    for (d, b) in &items {
        assert_eq!(store.get_node(d).as_ref(), Some(b));
    }
    assert_eq!(store.get_node(&loose), Some(b"loose".to_vec()));
    store.verify_indexes().expect("built idx matches pack");

    // Second open reuses the idx: no pack-index scan.
    let reopened = FileNodeStore::new(dir.path()).expect("reopen");
    let read = reopened.io_stats().bytes_read;
    assert!(read < 3000, "reopen must not rescan the pack index: {read}");

    // New writes coexist with the legacy pack.
    let mut writer = reopened.clone();
    writer.put_node(dig(99_999), b"new".to_vec());
    writer.put_node(dig(3), b"dup-ignored".to_vec());
    writer.flush().expect("flush");
    assert_eq!(writer.get_node(&dig(99_999)), Some(b"new".to_vec()));
    assert_eq!(writer.get_node(&dig(3)), Some(b"legacy-3".to_vec()));
    assert_eq!(dir.with_ext("pack").len(), 2);
    assert_eq!(dir.with_ext("idx").len(), 2);
}

#[test]
fn crash_leftovers_and_damaged_idx_are_rebuilt_from_pack() {
    let dir = TestDir::new("crash");
    let mut store = FileNodeStore::new(dir.path()).expect("open");
    for i in 0..300u64 {
        store.put_node(dig(i), format!("v{i}").into_bytes());
    }
    store.flush().expect("flush");
    drop(store);
    let idx = dir.with_ext("idx").remove(0);
    let good = fs::read(&idx).unwrap();

    let check = |label: &str| {
        let s = FileNodeStore::new(dir.path()).unwrap_or_else(|e| panic!("{label}: {e}"));
        for i in 0..300u64 {
            assert_eq!(
                s.get_node(&dig(i)),
                Some(format!("v{i}").into_bytes()),
                "{label}"
            );
        }
        s.verify_indexes()
            .unwrap_or_else(|e| panic!("{label}: {e}"));
        assert_eq!(
            fs::read(&idx).unwrap(),
            good,
            "{label}: rebuilt identically"
        );
    };

    // Crash after pack rename, before idx rename: pack present, idx absent,
    // temp idx left behind.
    fs::remove_file(&idx).unwrap();
    fs::write(dir.objects().join("pack-0-0-0.123.4.tmp"), b"partial").unwrap();
    check("missing idx");
    // Torn / zero-length / garbage / bit-flipped-header idx files.
    fs::write(&idx, &good[..good.len() / 2]).unwrap();
    check("truncated idx");
    fs::write(&idx, b"").unwrap();
    check("empty idx");
    let mut bad = good.clone();
    bad[20] ^= 1; // pack chain binding
    fs::write(&idx, &bad).unwrap();
    check("wrong chain binding");
    let mut bad = good.clone();
    bad[60] ^= 0xFF; // fanout
    fs::write(&idx, &bad).unwrap();
    check("bad fanout");
    // Stale idx from a different pack is rejected by the chain binding.
    let other = TestDir::new("crash-other");
    let mut o = FileNodeStore::new(other.path()).unwrap();
    for i in 1000..1300u64 {
        o.put_node(dig(i), format!("w{i}").into_bytes());
    }
    o.flush().unwrap();
    fs::copy(other.with_ext("idx").remove(0), &idx).unwrap();
    check("idx of another pack");
}

#[test]
fn corrupted_idx_records_fail_closed() {
    let dir = TestDir::new("corrupt");
    let mut store = FileNodeStore::new(dir.path()).expect("open");
    for i in 0..100u64 {
        store.put_node(dig(i), format!("v{i}").into_bytes());
    }
    store.flush().unwrap();
    drop(store);
    let idx = dir.with_ext("idx").remove(0);
    let good = fs::read(&idx).unwrap();
    let first = 1072usize; // first sorted record

    // Flipped checksum byte inside a record: header intact, so open succeeds;
    // the audit rejects it, and the body checksum rejects the read.
    let mut bad = good.clone();
    bad[first + 50] ^= 1;
    fs::write(&idx, &bad).unwrap();
    let s = FileNodeStore::new(dir.path()).expect("header still valid");
    assert!(s.verify_indexes().is_err());
    let victim = Digest::from_bytes(good[first..first + 32].try_into().unwrap());
    assert_eq!(s.get_node(&victim), None);

    // Offset pointing outside the pack: lookup errors, never reads elsewhere.
    let mut bad = good.clone();
    bad[first + 32..first + 40].copy_from_slice(&u64::MAX.to_be_bytes());
    fs::write(&idx, &bad).unwrap();
    let s = FileNodeStore::new(dir.path()).expect("header still valid");
    assert!(s.verify_indexes().is_err());
    assert_eq!(s.get_node(&victim), None);
    assert!(!s.contains(&victim), "latched after idx fault");
}

fn flat_open(nodes: u64) -> (u64, std::time::Duration) {
    let dir = TestDir::new(&format!("flat-{nodes}"));
    fs::create_dir_all(dir.objects()).unwrap();
    // Constant 1-byte body keeps the pack small; offsets stay distinct.
    let items: Vec<(Digest, Vec<u8>)> = (0..nodes).map(|i| (dig(i), vec![7u8])).collect();
    write_legacy_pack(&dir.objects().join("pack-1-1-1.pack"), &items);
    drop(items);
    drop(FileNodeStore::new(dir.path()).expect("first open builds idx"));
    let start = Instant::now();
    let store = FileNodeStore::new(dir.path()).expect("warm open");
    let elapsed = start.elapsed();
    let bytes = store.io_stats().bytes_read;
    // Lookups still work at scale.
    assert_eq!(store.get_node(&dig(nodes / 2)), Some(vec![7u8]));
    assert!(!store.contains(&dig(nodes + 1)));
    (bytes, elapsed)
}

#[test]
fn open_cost_is_flat_between_10k_and_1m_nodes() {
    let (small_bytes, small_time) = flat_open(10_000);
    let (large_bytes, large_time) = flat_open(1_000_000);
    eprintln!("open 10k: {small_bytes} B {small_time:?}; open 1M: {large_bytes} B {large_time:?}");
    assert_eq!(
        small_bytes, large_bytes,
        "open I/O must not grow with nodes"
    );
    assert!(
        large_time < std::time::Duration::from_millis(250),
        "open of 1M-node store took {large_time:?}"
    );
}
