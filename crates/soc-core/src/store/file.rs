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
//! Cold open reads O(all pack index bytes), retains O(unique node metadata), and
//! does not scan bodies. Warm lookup is O(log unique nodes); flush writes only
//! the new batch index. Body buffering is fixed at 64 KiB, with one requested
//! body allocated per read. Clones share the index and pending writer.

use super::NodeStore;
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
#[derive(Debug)]
struct Location {
    path: Arc<PathBuf>,
    entry: Entry,
}
#[derive(Debug)]
struct Pending {
    path: PathBuf,
    writer: BufWriter<CountedFile>,
    entries: Vec<(Digest, Entry)>,
    lookup: BTreeMap<Digest, usize>,
    end: u64,
}
#[derive(Debug, Default)]
struct State {
    index: BTreeMap<Digest, Location>,
    pending: Option<Pending>,
    root_synced: bool,
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
        };
        let mut state = store
            .state
            .lock()
            .map_err(|_| invalid("poisoned pack index"))?;
        for item in fs::read_dir(&store.objects_dir)? {
            let path = item?.path();
            if path.extension().and_then(|s| s.to_str()) == Some("pack") {
                store.load_index(&path, &mut state)?;
            }
        }
        drop(state);
        Ok(store)
    }

    fn load_index(&self, path: &Path, state: &mut State) -> io::Result<()> {
        let mut file = BufReader::new(File::open(path)?);
        let size = file.get_ref().metadata()?.len();
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
        file.seek(SeekFrom::Start(start))?;
        let path = Arc::new(path.to_path_buf());
        let mut chain = chain_start();
        let mut expected_offset = 8;
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
            let digest = digest_at(&record, 0);
            if state.index.contains_key(&digest) {
                return Err(invalid("duplicate digest across pack index"));
            }
            state.index.insert(
                digest,
                Location {
                    path: path.clone(),
                    entry,
                },
            );
        }
        if expected_offset != start || chain != digest_at(&footer, 16) {
            return Err(invalid("pack index checksum mismatch"));
        }
        self.io
            .bytes_read
            .fetch_add(8 + FOOTER + count * RECORD, Ordering::Relaxed);
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
        }
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

    fn legacy_contains(&self, digest: &Digest) -> io::Result<bool> {
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
        let mut count = state.index.len() + state.pending.as_ref().map_or(0, |p| p.entries.len());
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
            fs::rename(&pending.path, &published)?;
            File::open(&self.objects_dir)?.sync_all()?;
            self.io.directories_synced.fetch_add(1, Ordering::Relaxed);
            if !state.root_synced {
                File::open(self.objects_dir.parent().expect("objects parent"))?.sync_all()?;
                self.io.directories_synced.fetch_add(1, Ordering::Relaxed);
                state.root_synced = true;
            }
            let pending = state.pending.take().expect("pending pack");
            let path = Arc::new(published);
            for (digest, entry) in pending.entries {
                state.index.insert(
                    digest,
                    Location {
                        path: path.clone(),
                        entry,
                    },
                );
            }
            Ok(())
        })())
    }

    fn read_entry(&self, mut file: File, entry: &Entry) -> io::Result<Vec<u8>> {
        file.seek(SeekFrom::Start(entry.offset))?;
        let mut bytes = vec![0; entry.length as usize];
        file.read_exact(&mut bytes)?;
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
    fn contains(&self, digest: &Digest) -> bool {
        self.latch((|| {
            let state = self
                .state
                .lock()
                .map_err(|_| invalid("poisoned pack index"))?;
            self.check()?;
            if state.index.contains_key(digest)
                || state
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
        self.latch((|| {
            let target = {
                let mut state = self
                    .state
                    .lock()
                    .map_err(|_| invalid("poisoned pack index"))?;
                self.check()?;
                if let Some(location) = state.index.get(digest) {
                    Some((File::open(location.path.as_ref())?, location.entry.clone()))
                } else if let Some(pending) = state.pending.as_mut() {
                    if let Some(&idx) = pending.lookup.get(digest) {
                        pending.writer.flush()?;
                        // Open the temporary pack while holding the state lock. A cloned
                        // handle may flush immediately after we release it, renaming this
                        // path; the open file descriptor remains valid across that rename.
                        Some((File::open(&pending.path)?, pending.entries[idx].1.clone()))
                    } else {
                        None
                    }
                } else {
                    None
                }
            };
            if let Some((file, entry)) = target {
                return self.read_entry(file, &entry).map(Some);
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
        .flatten()
    }

    fn put_node(&mut self, digest: Digest, bytes: Vec<u8>) {
        let _ = self.latch((|| {
            let mut state = self
                .state
                .lock()
                .map_err(|_| invalid("poisoned pack index"))?;
            self.check()?;
            if state.index.contains_key(&digest)
                || state
                    .pending
                    .as_ref()
                    .is_some_and(|p| p.lookup.contains_key(&digest))
                || self.legacy_contains(&digest)?
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
            Ok(())
        })());
    }
}
