//! Physical pack framing only; semantic node bytes and Merkle roots are unchanged.
//!
//! v1: 8-byte magic, concatenated bodies, 80-byte index records in body order
//! (digest[32], offset u64 BE, length u64 BE, body checksum[32]), then a 56-byte
//! footer (index start u64 BE, record count u64 BE, index chain checksum[32],
//! end magic[8]). The chain starts at H("brix:pack:index:v1") and folds each
//! record as H(previous || record), using Digest/Domain::Value. Body checksums
//! likewise use Digest/Domain::Value; these are separate from semantic Merkle IDs.
//! Every offset must be contiguous, each body <=64 MiB, count <=2^32, pack <=2^48
//! bytes. The checksummed index and exact file-length equation reject incomplete
//! published packs. Body checksums are checked on every read; trie decoding also
//! verifies the semantic Merkle ID. Unpublished .tmp files are never indexed.
//!
//! Sorted sidecar index (`<pack>.idx`, derived data, pack format unchanged):
//! magic `BRIXIDX1`, record count u64 BE, the pack's footer chain checksum[32],
//! a 256-entry cumulative fanout table (u32 BE, by first digest byte), then
//! `count` 80-byte records in the same layout as the pack index but sorted by
//! digest, then a 32-byte chain checksum over those sorted records. The idx is
//! bound to its pack by count and chain checksum. Cold open reads the 56-byte
//! footer and the 1072-byte idx header of every pack (O(packs), independent of
//! node count); a missing, stale, or structurally damaged idx is rebuilt once
//! from the (fully validated) pack. Lookup consults the in-memory fanout, then
//! binary-searches the records with positional reads through a bounded cache
//! of open idx files; every hit is bounds-checked and bodies stay checksummed.
//! `verify_indexes` fully re-derives each idx from its pack for audits.
//! Index files are fsynced before the pack is renamed into place, but that
//! sync is not counted in `files_synced` (it is derived data).
//! Body buffering is fixed at 64 KiB, with one requested
//! body allocated per read. Clones share the index and pending writer.
//! Identical digests in several packs are tolerated (content-addressed).

use super::{DecodedNodeCache, NodeStore};
use brix_canon::{Digest, Domain};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const MAGIC: &[u8; 8] = b"BRIXPK01";
const END: &[u8; 8] = b"BRIXEND1";
const RECORD: u64 = 80;
const FOOTER: u64 = 56;
const MAX_BODY: u64 = 64 * 1024 * 1024;
const MAX_COUNT: u64 = 1 << 32;
const MAX_PACK: u64 = 1 << 48;
const IDX_MAGIC: &[u8; 8] = b"BRIXIDX1";
const IDX_HEADER: u64 = 8 + 8 + 32 + 256 * 4;
const IDX_CACHE_FILES: usize = 64;
const PACK_CACHE_FILES: usize = 32;
const BUCKET_READ: u64 = 256;
const KNOWN_CAP: usize = 1 << 18;

#[cfg(unix)]
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, buf, offset)
}

#[cfg(not(unix))]
fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    let mut file = file.try_clone()?;
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(buf)
}
static NONCE: AtomicU64 = AtomicU64::new(0);

/// Actual filesystem work, shared by cloned handles to one store.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StoreIoStats {
    /// Node read attempts (including legacy misses).
    pub reads: u64,
    /// Successfully read body bytes, plus pack metadata read at cold open.
    pub bytes_read: u64,
    /// Newly appended node records (the historical objects-written counter).
    pub writes: u64,
    /// Bytes actually written, including physical pack framing and indexes.
    pub bytes_written: u64,
    /// Successful underlying file write calls, after bounded buffering.
    pub physical_writes: u64,
    pub files_synced: u64,
    pub directories_synced: u64,
    /// In-memory node cache hits.
    pub cache_hits: u64,
    /// In-memory node cache misses requiring disk reads.
    pub cache_misses: u64,
    /// Nodes evicted from in-memory cache to bound memory.
    pub evictions: u64,
}

#[derive(Debug, Default)]
struct Counters {
    reads: AtomicU64,
    bytes_read: AtomicU64,
    writes: AtomicU64,
    bytes_written: AtomicU64,
    physical_writes: AtomicU64,
    files_synced: AtomicU64,
    directories_synced: AtomicU64,
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
    evictions: AtomicU64,
}

#[derive(Debug)]
struct CountedFile {
    file: File,
    io: Arc<Counters>,
}
impl Write for CountedFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let n = self.file.write(bytes)?;
        self.io.physical_writes.fetch_add(1, Ordering::Relaxed);
        self.io.bytes_written.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

#[derive(Clone, Debug)]
struct Entry {
    offset: u64,
    length: u64,
    checksum: Digest,
}
/// One published pack: metadata only; records live in the sorted idx file.
#[derive(Debug)]
struct PackRef {
    path: Arc<PathBuf>,
    idx_path: PathBuf,
    /// Offset of the pack's own index region (upper bound for bodies).
    start: u64,
    count: u64,
    fanout: Box<[u32; 256]>,
    /// In-memory membership filter over this pack's digests (no false negatives).
    /// Built for free at flush; for packs found at open it is built lazily once
    /// the pack has been probed `FILTER_AFTER_PROBES` times.
    filter: Option<Bloom>,
    /// In-memory sparse sample of the sorted idx (same lifetime as `filter`): lets a lookup
    /// read one small window of records instead of bisecting the idx with several reads.
    sparse: Option<Sparse>,
    probes: u32,
}

/// Records between two samples of a [`Sparse`] table.
const SPARSE_STRIDE: u64 = 32;

/// The first 8 digest bytes (big-endian) of every `SPARSE_STRIDE`-th record of a pack's sorted
/// idx. Derived data only: it bounds where a digest can be, never whether it is present, so a
/// stale or damaged table can at worst miss a node (and the idx itself stays authoritative).
#[derive(Debug, Default)]
struct Sparse {
    prefixes: Vec<u64>,
}

fn digest_prefix(digest: &Digest) -> u64 {
    u64::from_be_bytes(digest.as_bytes()[..8].try_into().expect("32-byte digest"))
}

impl Sparse {
    /// Record indexes `[lo, hi)` (within `[lo0, hi0)`) that can hold `digest` in a pack of
    /// `count` sorted records.
    fn window(&self, digest: &Digest, count: u64, lo0: u64, hi0: u64) -> (u64, u64) {
        let target = digest_prefix(digest);
        // Samples strictly below the target prefix: the last of them is a record below the
        // digest, so the digest lies after it. The first sample above the target prefix is a
        // record above the digest, so the digest lies before it.
        let below = self.prefixes.partition_point(|&p| p < target) as u64;
        let at_or_below = self.prefixes.partition_point(|&p| p <= target) as u64;
        let lo = if below == 0 {
            0
        } else {
            (below - 1) * SPARSE_STRIDE + 1
        };
        let hi = if at_or_below as usize >= self.prefixes.len() {
            count
        } else {
            at_or_below * SPARSE_STRIDE
        };
        (lo.max(lo0), hi.min(hi0))
    }
}

/// Probes of an opened pack (via binary search) before its filter is built.
const FILTER_AFTER_PROBES: u32 = 256;

/// Plain Bloom filter keyed directly on (already uniform) digest bytes.
#[derive(Debug)]
struct Bloom {
    bits: Vec<u64>,
    mask: u64,
}

impl Bloom {
    const K: u32 = 5;

    fn with_capacity(count: u64) -> Self {
        // 16 bits per entry, rounded up to a power of two (>= 64 bits).
        let nbits = (count.max(1) * 16).next_power_of_two().max(64);
        Self {
            bits: vec![0; (nbits / 64) as usize],
            mask: nbits - 1,
        }
    }

    fn hashes(digest: &Digest) -> (u64, u64) {
        let b = digest.as_bytes();
        let h1 = u64::from_le_bytes(b[8..16].try_into().expect("32-byte digest"));
        let h2 = u64::from_le_bytes(b[16..24].try_into().expect("32-byte digest")) | 1;
        (h1, h2)
    }

    fn insert(&mut self, digest: &Digest) {
        let (h1, h2) = Self::hashes(digest);
        for i in 0..Self::K {
            let bit = h1.wrapping_add(u64::from(i).wrapping_mul(h2)) & self.mask;
            self.bits[(bit >> 6) as usize] |= 1 << (bit & 63);
        }
    }

    fn may_contain(&self, digest: &Digest) -> bool {
        let (h1, h2) = Self::hashes(digest);
        (0..Self::K).all(|i| {
            let bit = h1.wrapping_add(u64::from(i).wrapping_mul(h2)) & self.mask;
            self.bits[(bit >> 6) as usize] & (1 << (bit & 63)) != 0
        })
    }
}

/// Read every digest of an idx file (sequential) into a filter and a sparse sample table.
fn build_filter(idx_path: &Path, count: u64) -> io::Result<(Bloom, Sparse)> {
    let mut file = File::open(idx_path)?;
    file.seek(SeekFrom::Start(IDX_HEADER))?;
    let mut reader = BufReader::with_capacity(256 * 1024, file);
    let mut bloom = Bloom::with_capacity(count);
    let mut sparse = Sparse::default();
    let mut record = [0; RECORD as usize];
    for i in 0..count {
        reader.read_exact(&mut record)?;
        let digest = digest_at(&record, 0);
        bloom.insert(&digest);
        if i % SPARSE_STRIDE == 0 {
            sparse.prefixes.push(digest_prefix(&digest));
        }
    }
    Ok((bloom, sparse))
}
#[derive(Debug)]
struct Pending {
    path: PathBuf,
    writer: BufWriter<CountedFile>,
    entries: Vec<(Digest, Entry)>,
    lookup: BTreeMap<Digest, usize>,
    end: u64,
}
#[derive(Debug)]
struct State {
    packs: Vec<PackRef>,
    /// Bounded LRU of open idx files, most recent last: (pack number, file).
    idx_files: Vec<(usize, File)>,
    /// Bounded cache of open pack files (positional reads, no shared offset).
    pack_files: Vec<(Arc<PathBuf>, Arc<File>)>,
    /// Digests recently confirmed present in a published pack (bounded; cleared when
    /// full). Packs are immutable and never removed while open, so a positive entry
    /// cannot go stale; it only saves repeated index lookups of shared subtrees.
    // Membership tests only (never iterated).
    #[allow(clippy::disallowed_types)]
    known: std::collections::HashSet<Digest>,
    pending: Option<Pending>,
    root_synced: bool,
    node_cache: BTreeMap<Digest, Vec<u8>>,
    cache_lru: std::collections::VecDeque<Digest>,
    max_cached_nodes: usize,
}

impl State {
    fn cache_insert(&mut self, digest: Digest, bytes: Vec<u8>, io: &Counters) {
        if self.node_cache.insert(digest, bytes).is_none() {
            self.cache_lru.push_back(digest);
        }
        while self.node_cache.len() > self.max_cached_nodes {
            let Some(old) = self.cache_lru.pop_front() else {
                break;
            };
            if self.node_cache.remove(&old).is_some() {
                io.evictions.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Shared open handle for a published pack (bounded cache, most recent last).
    fn pack_file(&mut self, path: &Arc<PathBuf>) -> io::Result<Arc<File>> {
        if let Some(i) = self
            .pack_files
            .iter()
            .position(|(p, _)| Arc::ptr_eq(p, path))
        {
            let item = self.pack_files.remove(i);
            let file = item.1.clone();
            self.pack_files.push(item);
            return Ok(file);
        }
        let file = Arc::new(File::open(path.as_ref())?);
        if self.pack_files.len() >= PACK_CACHE_FILES {
            self.pack_files.remove(0);
        }
        self.pack_files.push((path.clone(), file.clone()));
        Ok(file)
    }

    /// Locate a published node: fanout range, then binary search by positional
    /// reads. Every hit is bounds-checked against the owning pack.
    fn find(&mut self, digest: &Digest) -> io::Result<Option<(Arc<PathBuf>, Entry)>> {
        let first = digest.as_bytes()[0] as usize;
        for n in (0..self.packs.len()).rev() {
            let pack = &self.packs[n];
            let lo = if first == 0 {
                0
            } else {
                u64::from(pack.fanout[first - 1])
            };
            let hi = u64::from(pack.fanout[first]);
            if lo >= hi {
                continue;
            }
            let (path, start, idx_path) = (pack.path.clone(), pack.start, pack.idx_path.clone());
            if self.packs[n].filter.is_none() {
                let pack = &mut self.packs[n];
                pack.probes = pack.probes.saturating_add(1);
                if pack.probes > FILTER_AFTER_PROBES {
                    let (filter, sparse) = build_filter(&pack.idx_path, pack.count)?;
                    pack.filter = Some(filter);
                    pack.sparse = Some(sparse);
                }
            }
            if let Some(filter) = &self.packs[n].filter {
                if !filter.may_contain(digest) {
                    continue;
                }
            }
            let slot = match self.idx_files.iter().position(|(id, _)| *id == n) {
                Some(i) => i,
                None => {
                    if self.idx_files.len() >= IDX_CACHE_FILES {
                        self.idx_files.remove(0);
                    }
                    self.idx_files.push((n, File::open(&idx_path)?));
                    self.idx_files.len() - 1
                }
            };
            let last = self.idx_files.len() - 1;
            self.idx_files.swap(slot, last);
            let file = &self.idx_files[last].1;
            let (mut lo, mut hi) = (lo, hi);
            if let Some(sparse) = &self.packs[n].sparse {
                (lo, hi) = sparse.window(digest, self.packs[n].count, lo, hi);
                if lo >= hi {
                    continue;
                }
            }
            // Narrow large buckets with single-record positional reads, then read
            // the remaining (at most BUCKET_READ) records in one syscall.
            while hi - lo > BUCKET_READ {
                let mid = lo + (hi - lo) / 2;
                let mut digest_bytes = [0u8; 32];
                read_exact_at(file, &mut digest_bytes, IDX_HEADER + mid * RECORD)?;
                match Digest::from_bytes(digest_bytes).cmp(digest) {
                    std::cmp::Ordering::Less => lo = mid + 1,
                    std::cmp::Ordering::Greater => hi = mid,
                    std::cmp::Ordering::Equal => {
                        lo = mid;
                        hi = mid + 1;
                    }
                }
            }
            let mut bucket = vec![0u8; ((hi - lo) * RECORD) as usize];
            read_exact_at(file, &mut bucket, IDX_HEADER + lo * RECORD)?;
            let base = lo;
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                let at = ((mid - base) * RECORD) as usize;
                let record = &bucket[at..at + RECORD as usize];
                let found = digest_at(record, 0);
                match found.cmp(digest) {
                    std::cmp::Ordering::Less => lo = mid + 1,
                    std::cmp::Ordering::Greater => hi = mid,
                    std::cmp::Ordering::Equal => {
                        let entry = Entry {
                            offset: u64_at(record, 32),
                            length: u64_at(record, 40),
                            checksum: digest_at(record, 48),
                        };
                        if entry.offset < 8
                            || entry.length == 0
                            || entry.length > MAX_BODY
                            || entry
                                .offset
                                .checked_add(entry.length)
                                .is_none_or(|e| e > start)
                        {
                            return Err(invalid("idx record out of pack bounds"));
                        }
                        return Ok(Some((path, entry)));
                    }
                }
            }
        }
        Ok(None)
    }
}

impl Default for State {
    fn default() -> Self {
        Self {
            packs: Vec::new(),
            idx_files: Vec::new(),
            pack_files: Vec::new(),
            #[allow(clippy::disallowed_types)]
            known: std::collections::HashSet::new(),
            pending: None,
            root_synced: false,
            node_cache: BTreeMap::new(),
            cache_lru: std::collections::VecDeque::new(),
            max_cached_nodes: 65_536,
        }
    }
}

/// Durable, content-addressed node packs with legacy `objects/xx/*.bin` reads.
/// Call `flush` before publishing any root referencing new nodes. One successful
/// nonempty flush uses one file sync and one directory sync (plus the root
/// directory on the handle's first flush). Failed I/O permanently poisons the
/// shared handle. Reopen to recover; abandoned temporary packs are ignored.
/// Independent writers must be serialized by the caller (WorldSession's lock).
#[derive(Clone, Debug)]
pub struct FileNodeStore {
    objects_dir: PathBuf,
    state: Arc<Mutex<State>>,
    failed: Arc<AtomicBool>,
    io: Arc<Counters>,
    /// Legacy `objects/xx/` shard directories that existed at open (legacy files are
    /// read-only history; shards created after open are not consulted until reopen).
    legacy_shards: Arc<Vec<AtomicBool>>,
    /// Decoded-node cache for store-aware trie reads made directly through this handle
    /// (the base relations). Shared by clones; capacity 0 (disabled) until
    /// [`FileNodeStore::set_decoded_cache_capacity`].
    decoded: Arc<DecodedNodeCache>,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn chain_start() -> Digest {
    Digest::of(Domain::Value, b"brix:pack:index:v1")
}
fn chain_next(previous: Digest, record: &[u8; 80]) -> Digest {
    let mut bytes = [0; 112];
    bytes[..32].copy_from_slice(previous.as_bytes());
    bytes[32..].copy_from_slice(record);
    Digest::of(Domain::Value, &bytes)
}
fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_be_bytes(bytes[offset..offset + 8].try_into().expect("fixed framing"))
}
fn digest_at(bytes: &[u8], offset: usize) -> Digest {
    Digest::from_bytes(
        bytes[offset..offset + 32]
            .try_into()
            .expect("fixed framing"),
    )
}
fn encode_entry(digest: &Digest, entry: &Entry) -> [u8; 80] {
    let mut bytes = [0; 80];
    bytes[..32].copy_from_slice(digest.as_bytes());
    bytes[32..40].copy_from_slice(&entry.offset.to_be_bytes());
    bytes[40..48].copy_from_slice(&entry.length.to_be_bytes());
    bytes[48..].copy_from_slice(entry.checksum.as_bytes());
    bytes
}

impl FileNodeStore {
    pub fn new(root_dir: impl AsRef<Path>) -> io::Result<Self> {
        let objects_dir = root_dir.as_ref().join("objects");
        fs::create_dir_all(&objects_dir)?;
        let store = Self {
            objects_dir,
            state: Arc::new(Mutex::new(State::default())),
            failed: Arc::new(AtomicBool::new(false)),
            io: Arc::new(Counters::default()),
            legacy_shards: Arc::new((0..256).map(|_| AtomicBool::new(false)).collect()),
            decoded: Arc::new(DecodedNodeCache::new(0)),
        };
        let mut state = store
            .state
            .lock()
            .map_err(|_| invalid("poisoned pack index"))?;
        let mut packs = Vec::new();
        for item in fs::read_dir(&store.objects_dir)? {
            let path = item?.path();
            if path.extension().and_then(|s| s.to_str()) == Some("pack") {
                packs.push(path);
            } else if path.is_dir() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.len() == 2 {
                        if let Ok(byte) = u8::from_str_radix(name, 16) {
                            store.legacy_shards[byte as usize].store(true, Ordering::Relaxed);
                        }
                    }
                }
            }
        }
        packs.sort();
        for path in packs {
            let pack = store.open_pack(&path)?;
            state.packs.push(pack);
        }
        drop(state);
        Ok(store)
    }

    /// Read and validate only the pack footer; returns (index start, count, chain).
    fn read_footer(&self, file: &mut File) -> io::Result<(u64, u64, Digest)> {
        let size = file.metadata()?.len();
        if !(8 + RECORD + FOOTER..=MAX_PACK).contains(&size) {
            return Err(invalid("invalid pack size"));
        }
        let mut magic = [0; 8];
        file.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(invalid("invalid pack version"));
        }
        file.seek(SeekFrom::Start(size - FOOTER))?;
        let mut footer = [0; FOOTER as usize];
        file.read_exact(&mut footer)?;
        let start = u64_at(&footer, 0);
        let count = u64_at(&footer, 8);
        if &footer[48..] != END
            || count == 0
            || count > MAX_COUNT
            || start < 8
            || count
                .checked_mul(RECORD)
                .and_then(|n| start.checked_add(n))
                .and_then(|n| n.checked_add(FOOTER))
                != Some(size)
        {
            return Err(invalid("invalid pack footer"));
        }
        self.io.bytes_read.fetch_add(8 + FOOTER, Ordering::Relaxed);
        Ok((start, count, digest_at(&footer, 16)))
    }

    /// Full validation of a pack's own index; O(count). Used to (re)build an idx.
    fn scan_pack(&self, path: &Path) -> io::Result<(Vec<(Digest, Entry)>, Digest)> {
        let mut file = File::open(path)?;
        let (start, count, footer_chain) = self.read_footer(&mut file)?;
        let mut file = BufReader::new(file);
        file.seek(SeekFrom::Start(start))?;
        let mut chain = chain_start();
        let mut expected_offset = 8;
        let mut entries = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let mut record = [0; RECORD as usize];
            file.read_exact(&mut record)?;
            chain = chain_next(chain, &record);
            let entry = Entry {
                offset: u64_at(&record, 32),
                length: u64_at(&record, 40),
                checksum: digest_at(&record, 48),
            };
            if entry.offset != expected_offset || entry.length == 0 || entry.length > MAX_BODY {
                return Err(invalid("invalid pack record bounds"));
            }
            expected_offset = entry
                .offset
                .checked_add(entry.length)
                .filter(|end| *end <= start)
                .ok_or_else(|| invalid("pack body out of bounds"))?;
            entries.push((digest_at(&record, 0), entry));
        }
        if expected_offset != start || chain != footer_chain {
            return Err(invalid("pack index checksum mismatch"));
        }
        self.io
            .bytes_read
            .fetch_add(count * RECORD, Ordering::Relaxed);
        Ok((entries, footer_chain))
    }

    /// Open one published pack: footer + idx header only; rebuild idx if unusable.
    fn open_pack(&self, path: &Path) -> io::Result<PackRef> {
        let mut file = File::open(path)?;
        let (start, count, chain) = self.read_footer(&mut file)?;
        let idx_path = path.with_extension("idx");
        let fanout = match self.read_idx_header(&idx_path, count, &chain) {
            Ok(fanout) => fanout,
            Err(_) => {
                let (mut entries, chain) = self.scan_pack(path)?;
                let tmp = self.write_idx_tmp(&idx_path, &mut entries, &chain)?;
                fs::rename(&tmp, &idx_path)?;
                self.read_idx_header(&idx_path, count, &chain)?
            }
        };
        Ok(PackRef {
            path: Arc::new(path.to_path_buf()),
            idx_path,
            start,
            count,
            fanout,
            filter: None,
            sparse: None,
            probes: 0,
        })
    }

    fn read_idx_header(
        &self,
        idx_path: &Path,
        count: u64,
        chain: &Digest,
    ) -> io::Result<Box<[u32; 256]>> {
        let mut file = File::open(idx_path)?;
        let size = file.metadata()?.len();
        if count
            .checked_mul(RECORD)
            .and_then(|n| n.checked_add(IDX_HEADER + 32))
            != Some(size)
        {
            return Err(invalid("idx size mismatch"));
        }
        let mut header = vec![0; IDX_HEADER as usize];
        file.read_exact(&mut header)?;
        self.io.bytes_read.fetch_add(IDX_HEADER, Ordering::Relaxed);
        if &header[..8] != IDX_MAGIC
            || u64_at(&header, 8) != count
            || digest_at(&header, 16) != *chain
        {
            return Err(invalid("idx header does not match pack"));
        }
        let mut fanout = Box::new([0u32; 256]);
        let mut previous = 0u32;
        for (i, slot) in fanout.iter_mut().enumerate() {
            let at = 48 + i * 4;
            let v = u32::from_be_bytes(header[at..at + 4].try_into().expect("fixed framing"));
            if v < previous {
                return Err(invalid("idx fanout not monotone"));
            }
            *slot = v;
            previous = v;
        }
        if u64::from(previous) != count {
            return Err(invalid("idx fanout total mismatch"));
        }
        Ok(fanout)
    }

    /// Write a sorted idx to a unique temp file (fsynced); caller renames it.
    fn write_idx_tmp(
        &self,
        idx_path: &Path,
        entries: &mut [(Digest, Entry)],
        pack_chain: &Digest,
    ) -> io::Result<PathBuf> {
        entries.sort_by_key(|a| a.0);
        let mut fanout = [0u32; 256];
        for (digest, _) in entries.iter() {
            fanout[digest.as_bytes()[0] as usize] += 1;
        }
        let mut total = 0u32;
        for slot in &mut fanout {
            total += *slot;
            *slot = total;
        }
        let nonce = NONCE.fetch_add(1, Ordering::Relaxed);
        let tmp = idx_path.with_extension(format!("{}.{nonce}.tmp", std::process::id()));
        let file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        let mut writer = BufWriter::with_capacity(
            64 * 1024,
            CountedFile {
                file,
                io: self.io.clone(),
            },
        );
        writer.write_all(IDX_MAGIC)?;
        writer.write_all(&(entries.len() as u64).to_be_bytes())?;
        writer.write_all(pack_chain.as_bytes())?;
        for v in fanout {
            writer.write_all(&v.to_be_bytes())?;
        }
        let mut chain = chain_start();
        for (digest, entry) in entries.iter() {
            let record = encode_entry(digest, entry);
            writer.write_all(&record)?;
            chain = chain_next(chain, &record);
        }
        writer.write_all(chain.as_bytes())?;
        writer.flush()?;
        writer.get_ref().file.sync_all()?;
        Ok(tmp)
    }

    /// Audit: re-derive every idx from its pack (O(total nodes)) and require
    /// byte equality. Never called on the open or lookup paths.
    pub fn verify_indexes(&self) -> io::Result<()> {
        let state = self
            .state
            .lock()
            .map_err(|_| invalid("poisoned pack index"))?;
        for pack in &state.packs {
            let (mut entries, chain) = self.scan_pack(&pack.path)?;
            let tmp = self.write_idx_tmp(&pack.idx_path, &mut entries, &chain)?;
            let expected = fs::read(&tmp);
            let _ = fs::remove_file(&tmp);
            if expected? != fs::read(&pack.idx_path)? {
                return Err(invalid("idx does not match its pack"));
            }
        }
        Ok(())
    }

    pub fn io_stats(&self) -> StoreIoStats {
        StoreIoStats {
            reads: self.io.reads.load(Ordering::Relaxed),
            bytes_read: self.io.bytes_read.load(Ordering::Relaxed),
            writes: self.io.writes.load(Ordering::Relaxed),
            bytes_written: self.io.bytes_written.load(Ordering::Relaxed),
            physical_writes: self.io.physical_writes.load(Ordering::Relaxed),
            files_synced: self.io.files_synced.load(Ordering::Relaxed),
            directories_synced: self.io.directories_synced.load(Ordering::Relaxed),
            cache_hits: self.io.cache_hits.load(Ordering::Relaxed),
            cache_misses: self.io.cache_misses.load(Ordering::Relaxed),
            evictions: self.io.evictions.load(Ordering::Relaxed),
        }
    }

    /// Set maximum number of nodes cached in memory. Evicts excess nodes immediately.
    pub fn set_cache_capacity(&self, max_nodes: usize) {
        if let Ok(mut state) = self.state.lock() {
            state.max_cached_nodes = max_nodes;
            while state.node_cache.len() > state.max_cached_nodes {
                if let Some(old) = state.cache_lru.pop_front() {
                    state.node_cache.remove(&old);
                    self.io.evictions.fetch_add(1, Ordering::Relaxed);
                } else {
                    break;
                }
            }
        }
    }

    /// Bound the decoded-node cache used by store-aware reads through this handle (0
    /// disables it). Shared by all clones; evicts immediately when shrinking.
    pub fn set_decoded_cache_capacity(&self, cap_bytes: usize) {
        self.decoded.set_capacity(cap_bytes);
    }

    /// Number of nodes currently resident in the in-memory cache.
    pub fn cached_nodes_count(&self) -> usize {
        self.state.lock().map_or(0, |s| s.node_cache.len())
    }

    pub fn objects_dir(&self) -> &Path {
        &self.objects_dir
    }

    fn object_path(&self, digest: &Digest) -> PathBuf {
        let hex = digest.to_hex();
        self.objects_dir
            .join(&hex[..2])
            .join(format!("{}.bin", &hex[2..]))
    }

    fn legacy_shard_present(&self, digest: &Digest) -> bool {
        self.legacy_shards[digest.as_bytes()[0] as usize].load(Ordering::Relaxed)
    }

    fn legacy_contains(&self, digest: &Digest) -> io::Result<bool> {
        if !self.legacy_shard_present(digest) {
            return Ok(false);
        }
        match fs::metadata(self.object_path(digest)) {
            Ok(meta) if meta.is_file() => Ok(true),
            Ok(_) => Err(invalid("legacy node is not a file")),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(err),
        }
    }

    fn pending(&self) -> io::Result<Pending> {
        let nonce = NONCE.fetch_add(1, Ordering::Relaxed);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        let path = self
            .objects_dir
            .join(format!("pack-{}-{now}-{nonce}.tmp", std::process::id()));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let mut writer = BufWriter::with_capacity(
            64 * 1024,
            CountedFile {
                file,
                io: self.io.clone(),
            },
        );
        writer.write_all(MAGIC)?;
        Ok(Pending {
            path,
            writer,
            entries: Vec::new(),
            lookup: BTreeMap::new(),
            end: 8,
        })
    }

    fn check(&self) -> io::Result<()> {
        if self.failed.load(Ordering::SeqCst) {
            Err(io::Error::other("node I/O failed in FileNodeStore"))
        } else {
            Ok(())
        }
    }
    fn latch<T>(&self, result: io::Result<T>) -> io::Result<T> {
        if result.is_err() {
            self.failed.store(true, Ordering::SeqCst);
        }
        result
    }

    /// Diagnostic traversal; never used by point reads, writes, or publication.
    pub fn node_count(&self) -> usize {
        let state = self.state.lock().expect("poisoned pack index");
        let mut count = state.packs.iter().map(|p| p.count as usize).sum::<usize>()
            + state.pending.as_ref().map_or(0, |p| p.entries.len());
        if let Ok(dirs) = fs::read_dir(&self.objects_dir) {
            for dir in dirs.flatten() {
                if let Ok(files) = fs::read_dir(dir.path()) {
                    count += files
                        .flatten()
                        .filter(|f| f.path().extension().and_then(|s| s.to_str()) == Some("bin"))
                        .count();
                }
            }
        }
        count
    }

    /// Physical bytes on disk plus bytes still in the bounded pending buffer.
    pub fn total_bytes(&self) -> u64 {
        let state = self.state.lock().expect("poisoned pack index");
        let mut total = state
            .pending
            .as_ref()
            .map_or(0, |p| p.writer.buffer().len() as u64);
        if let Ok(items) = fs::read_dir(&self.objects_dir) {
            for item in items.flatten() {
                if let Ok(meta) = item.metadata() {
                    if meta.is_file() {
                        total += meta.len();
                    } else if let Ok(files) = fs::read_dir(item.path()) {
                        total += files
                            .flatten()
                            .filter_map(|f| f.metadata().ok())
                            .map(|m| m.len())
                            .sum::<u64>();
                    }
                }
            }
        }
        total
    }

    pub fn flush(&self) -> io::Result<()> {
        self.latch((|| {
            let mut state = self
                .state
                .lock()
                .map_err(|_| invalid("poisoned pack index"))?;
            self.check()?;
            let Some(pending) = state.pending.as_mut() else {
                return Ok(());
            };
            let mut chain = chain_start();
            for (digest, entry) in &pending.entries {
                let record = encode_entry(digest, entry);
                pending.writer.write_all(&record)?;
                chain = chain_next(chain, &record);
            }
            pending.writer.write_all(&pending.end.to_be_bytes())?;
            pending
                .writer
                .write_all(&(pending.entries.len() as u64).to_be_bytes())?;
            pending.writer.write_all(chain.as_bytes())?;
            pending.writer.write_all(END)?;
            pending.writer.flush()?;
            pending.writer.get_ref().file.sync_all()?;
            self.io.files_synced.fetch_add(1, Ordering::Relaxed);
            let published = pending.path.with_extension("pack");
            let idx_path = published.with_extension("idx");
            let mut sorted = pending.entries.clone();
            let idx_tmp = self.write_idx_tmp(&idx_path, &mut sorted, &chain)?;
            fs::rename(&pending.path, &published)?;
            fs::rename(&idx_tmp, &idx_path)?;
            File::open(&self.objects_dir)?.sync_all()?;
            self.io.directories_synced.fetch_add(1, Ordering::Relaxed);
            if !state.root_synced {
                File::open(self.objects_dir.parent().expect("objects parent"))?.sync_all()?;
                self.io.directories_synced.fetch_add(1, Ordering::Relaxed);
                state.root_synced = true;
            }
            let pending = state.pending.take().expect("pending pack");
            let mut fanout = Box::new([0u32; 256]);
            let mut filter = Bloom::with_capacity(pending.entries.len() as u64);
            for (digest, _) in &pending.entries {
                fanout[digest.as_bytes()[0] as usize] += 1;
                filter.insert(digest);
            }
            // `sorted` is the idx order (sorted in place by `write_idx_tmp`).
            let sparse = Sparse {
                prefixes: sorted
                    .iter()
                    .step_by(SPARSE_STRIDE as usize)
                    .map(|(d, _)| digest_prefix(d))
                    .collect(),
            };
            let mut total = 0u32;
            for slot in fanout.iter_mut() {
                total += *slot;
                *slot = total;
            }
            state.packs.push(PackRef {
                path: Arc::new(published),
                idx_path,
                start: pending.end,
                count: pending.entries.len() as u64,
                fanout,
                filter: Some(filter),
                sparse: Some(sparse),
                probes: 0,
            });
            Ok(())
        })())
    }

    fn read_entry(&self, file: &File, entry: &Entry) -> io::Result<Vec<u8>> {
        let mut bytes = vec![0; entry.length as usize];
        read_exact_at(file, &mut bytes, entry.offset)?;
        self.io
            .bytes_read
            .fetch_add(entry.length, Ordering::Relaxed);
        if Digest::of(Domain::Value, &bytes) != entry.checksum {
            return Err(invalid("pack body checksum mismatch"));
        }
        Ok(bytes)
    }
}

impl NodeStore for FileNodeStore {
    fn decoded_cache(&self) -> Option<&DecodedNodeCache> {
        Some(&self.decoded)
    }

    fn contains(&self, digest: &Digest) -> bool {
        self.latch((|| {
            let mut state = self
                .state
                .lock()
                .map_err(|_| invalid("poisoned pack index"))?;
            self.check()?;
            if state.known.contains(digest) {
                return Ok(true);
            }
            if state.find(digest)?.is_some() {
                if state.known.len() >= KNOWN_CAP {
                    state.known.clear();
                }
                state.known.insert(*digest);
                return Ok(true);
            }
            if state
                .pending
                .as_ref()
                .is_some_and(|p| p.lookup.contains_key(digest))
            {
                return Ok(true);
            }
            self.legacy_contains(digest)
        })())
        .unwrap_or(false)
    }

    fn get_node(&self, digest: &Digest) -> Option<Vec<u8>> {
        self.io.reads.fetch_add(1, Ordering::Relaxed);
        if self.check().is_err() {
            return None;
        }
        // 1. Check in-memory LRU cache
        if let Ok(state) = self.state.lock() {
            if let Some(bytes) = state.node_cache.get(digest) {
                self.io.cache_hits.fetch_add(1, Ordering::Relaxed);
                return Some(bytes.clone());
            }
        }
        self.io.cache_misses.fetch_add(1, Ordering::Relaxed);

        let result = self
            .latch((|| {
                let target = {
                    let mut state = self
                        .state
                        .lock()
                        .map_err(|_| invalid("poisoned pack index"))?;
                    self.check()?;
                    if let Some((path, entry)) = state.find(digest)? {
                        Some((state.pack_file(&path)?, entry))
                    } else if let Some(pending) = state.pending.as_mut() {
                        if let Some(&idx) = pending.lookup.get(digest) {
                            pending.writer.flush()?;
                            // Open the temporary pack while holding the state lock. A cloned
                            // handle may flush immediately after we release it, renaming this
                            // path; the open file descriptor remains valid across that rename.
                            Some((
                                Arc::new(File::open(&pending.path)?),
                                pending.entries[idx].1.clone(),
                            ))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                };
                if let Some((file, entry)) = target {
                    return self.read_entry(&file, &entry).map(Some);
                }
                if !self.legacy_shard_present(digest) {
                    return Ok(None);
                }
                match fs::read(self.object_path(digest)) {
                    Ok(bytes) => {
                        self.io
                            .bytes_read
                            .fetch_add(bytes.len() as u64, Ordering::Relaxed);
                        Ok(Some(bytes))
                    }
                    Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
                    Err(err) => Err(err),
                }
            })())
            .ok()
            .flatten();

        if let Some(ref bytes) = result {
            if let Ok(mut state) = self.state.lock() {
                state.cache_insert(*digest, bytes.clone(), &self.io);
            }
        }

        result
    }

    fn put_node(&mut self, digest: Digest, bytes: Vec<u8>) {
        self.put_node_impl(digest, bytes, true);
    }

    fn put_new_node(&mut self, digest: Digest, bytes: Vec<u8>) {
        self.put_node_impl(digest, bytes, false);
    }
}

impl FileNodeStore {
    /// Append a node. With `check_existing` the node is skipped when already stored; callers
    /// that have just established absence (via `contains`) pass `false` to avoid a second probe.
    fn put_node_impl(&mut self, digest: Digest, bytes: Vec<u8>, check_existing: bool) {
        let _ = self.latch((|| {
            let mut state = self
                .state
                .lock()
                .map_err(|_| invalid("poisoned pack index"))?;
            self.check()?;
            if check_existing
                && (state.find(&digest)?.is_some()
                    || state
                        .pending
                        .as_ref()
                        .is_some_and(|p| p.lookup.contains_key(&digest))
                    || self.legacy_contains(&digest)?)
            {
                return Ok(());
            }
            let length = bytes.len() as u64;
            if length == 0 || length > MAX_BODY {
                return Err(invalid("pack node exceeds body bounds"));
            }
            if state.pending.is_none() {
                state.pending = Some(self.pending()?);
            }
            let pending = state.pending.as_mut().expect("pending pack");
            let count = pending.entries.len() as u64 + 1;
            let end = pending
                .end
                .checked_add(length)
                .ok_or_else(|| invalid("pack length overflow"))?;
            if count > MAX_COUNT
                || end
                    .checked_add(count * RECORD + FOOTER)
                    .is_none_or(|size| size > MAX_PACK)
            {
                return Err(invalid("pack exceeds format bounds"));
            }
            pending.writer.write_all(&bytes)?;
            let entry = Entry {
                offset: pending.end,
                length,
                checksum: Digest::of(Domain::Value, &bytes),
            };
            pending.lookup.insert(digest, pending.entries.len());
            pending.entries.push((digest, entry));
            pending.end = end;
            self.io.writes.fetch_add(1, Ordering::Relaxed);
            // Write-through: freshly written nodes are the likeliest next reads.
            state.cache_insert(digest, bytes, &self.io);
            Ok(())
        })());
    }
}
