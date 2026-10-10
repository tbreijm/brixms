//! The persisted idempotency index: a content-addressed trie in the node store mapping an
//! idempotency key to the batch that committed under it, so opening a world and checking a
//! key cost O(log n) node reads instead of a scan over every receipt and revision file.
//!
//! Like the state-cache sidecar it is a *cache*, never authoritative: the receipts (and the
//! revision records of worlds that predate receipts) remain the source of truth, no revision
//! or network-state digest covers it, and its root is named by a sibling file
//! `idempotency.json` (`brix.world.idempotency-index@1`). Any sidecar that is missing,
//! unparseable, for another world or revision, unreadable in the store, or too far behind
//! HEAD is a cache miss and the open falls back to the receipt scan (which it then upgrades
//! from, best effort). A sidecar that lags HEAD by a few revisions (a crash between the HEAD
//! rename and the sidecar write) is caught up from exactly those revisions' receipts.
//!
//! Lookup semantics are exactly the scan's: the same key set, the same `(seq,
//! revision_digest, batch_digest)` per key; `tests/r0_idempotency_index.rs` holds the two to
//! each other.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;

use brix_canon::{CanonWriter, Digest};
use soc_core::store::{FileNodeStore, TrieMap};

use super::error::WorldError;
use super::paths::WorldPaths;
use super::session::CommittedBatchInfo;
use super::types::{WorldKey, WorldTuple};

pub const IDEMPOTENCY_SCHEMA: &str = "brix.world.idempotency-index@1";

/// A sidecar further behind HEAD than this is rebuilt from the scan instead of caught up.
const MAX_TAIL: u64 = 4096;

fn parse_hex(s: &str) -> Option<Digest> {
    if s.len() != 64 || !s.is_ascii() {
        return None;
    }
    let mut b = [0u8; 32];
    for (i, byte) in b.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(Digest::from_bytes(b))
}

/// Trie key of an idempotency key.
pub fn idempotency_trie_key(key: &str) -> WorldKey {
    let mut w = CanonWriter::new();
    w.write_tag("brix.world.idempotency-key@1");
    w.write_str(key);
    WorldKey::new(w.finish())
}

fn encode_info(info: &CommittedBatchInfo) -> WorldTuple {
    let mut b = Vec::with_capacity(8 + 32 + 1 + 32);
    b.extend_from_slice(&info.seq.to_be_bytes());
    b.extend_from_slice(info.revision_digest.as_bytes());
    match info.batch_digest {
        Some(d) => {
            b.push(1);
            b.extend_from_slice(d.as_bytes());
        }
        None => b.push(0),
    }
    WorldTuple::new(b)
}

fn decode_info(t: &WorldTuple) -> Result<CommittedBatchInfo, WorldError> {
    let b = t.as_bytes();
    let bad = || WorldError::NetworkError("corrupt idempotency index entry".into());
    if b.len() < 41 {
        return Err(bad());
    }
    let digest = |s: &[u8]| {
        let mut d = [0u8; 32];
        d.copy_from_slice(s);
        Digest::from_bytes(d)
    };
    let seq = u64::from_be_bytes(b[0..8].try_into().map_err(|_| bad())?);
    let revision_digest = digest(&b[8..40]);
    let batch_digest = match (b[40], b.len()) {
        (0, 41) => None,
        (1, 73) => Some(digest(&b[41..73])),
        _ => return Err(bad()),
    };
    Ok(CommittedBatchInfo {
        seq,
        revision_digest,
        batch_digest,
    })
}

/// Parse a receipt (or, for a world without receipts, a revision record) into the key it
/// committed and its info. Anything past `head_seq` (an uncommitted attempt) is ignored.
fn parse_record(path: &Path, head_seq: u64) -> Option<(String, CommittedBatchInfo)> {
    let content = fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&content).ok()?;
    let seq = v.get("seq")?.as_u64()?;
    let ikey = v.get("idempotency_key")?.as_str()?;
    let d_hex = v.get("revision_digest")?.as_str()?;
    if seq == 0 || seq > head_seq {
        return None;
    }
    let revision_digest = parse_hex(d_hex)?;
    let batch_digest = v
        .get("batch_digest")
        .and_then(|x| x.as_str())
        .and_then(parse_hex);
    Some((
        ikey.to_string(),
        CommittedBatchInfo {
            seq,
            revision_digest,
            batch_digest,
        },
    ))
}

/// The record of revision `seq`: its receipt, else its revision file.
pub(crate) fn record_of(
    paths: &WorldPaths,
    seq: u64,
    head_seq: u64,
) -> Option<(String, CommittedBatchInfo)> {
    parse_record(&paths.receipt_file(seq), head_seq)
        .or_else(|| parse_record(&paths.revision_file(seq), head_seq))
}

/// The sidecar naming the index root for a revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdempotencySidecar {
    pub revision_seq: u64,
    pub revision_digest: Digest,
    pub root: Digest,
    pub len: u64,
}

impl IdempotencySidecar {
    pub fn read(path: &Path) -> Option<Self> {
        let v: serde_json::Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
        if v.get("schema")?.as_str()? != IDEMPOTENCY_SCHEMA {
            return None;
        }
        Some(Self {
            revision_seq: v.get("revision_seq")?.as_u64()?,
            revision_digest: parse_hex(v.get("revision_digest")?.as_str()?)?,
            root: parse_hex(v.get("root")?.as_str()?)?,
            len: v.get("len")?.as_u64()?,
        })
    }

    /// Write via a temp file and rename. Not fsynced: a torn or lost sidecar is a cache miss.
    pub fn write(&self, paths: &WorldPaths) -> std::io::Result<()> {
        let json = serde_json::json!({
            "schema": IDEMPOTENCY_SCHEMA,
            "revision_seq": self.revision_seq,
            "revision_digest": self.revision_digest.to_hex(),
            "root": self.root.to_hex(),
            "len": self.len,
        });
        let tmp = paths.idempotency_tmp();
        let mut f = fs::File::create(&tmp)?;
        f.write_all(&serde_json::to_vec(&json)?)?;
        drop(f);
        fs::rename(tmp, paths.idempotency_file())
    }
}

type Trie = TrieMap<WorldKey, WorldTuple>;

/// How the index was obtained at open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdempotencySource {
    /// A fresh world (revision 0): empty, nothing to read.
    Fresh,
    /// The sidecar's trie, exactly at HEAD.
    Sidecar,
    /// The sidecar's trie plus the receipts of this many revisions after it.
    SidecarTail(u64),
    /// No usable sidecar: rebuilt from every receipt and revision file.
    Scan,
}

impl IdempotencySource {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Sidecar => "sidecar",
            Self::SidecarTail(_) => "sidecar+tail",
            Self::Scan => "scan",
        }
    }
}

#[derive(Clone)]
pub struct IdempotencyIndex {
    trie: Trie,
    /// Keys committed but not in `trie` (an insert failed after a durable commit).
    overlay: BTreeMap<String, CommittedBatchInfo>,
    /// Whether `trie` holds every key, so that persisting its root is sound.
    complete: bool,
    pub source: IdempotencySource,
}

impl IdempotencyIndex {
    pub fn empty() -> Self {
        Self {
            trie: Trie::new(),
            overlay: BTreeMap::new(),
            complete: true,
            source: IdempotencySource::Fresh,
        }
    }

    pub fn len(&self) -> usize {
        self.trie.len() + self.overlay.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(
        &self,
        key: &str,
        store: &FileNodeStore,
    ) -> Result<Option<CommittedBatchInfo>, WorldError> {
        if let Some(i) = self.overlay.get(key) {
            return Ok(Some(i.clone()));
        }
        match self
            .trie
            .get_with_store(&idempotency_trie_key(key), store)?
        {
            Some(t) => Ok(Some(decode_info(&t)?)),
            None => Ok(None),
        }
    }

    /// Record a committed key. Never fails: on a store error the key lives in the overlay.
    pub fn record(&mut self, key: &str, info: CommittedBatchInfo, store: &FileNodeStore) {
        match self
            .trie
            .insert_with_store(idempotency_trie_key(key), encode_info(&info), store)
        {
            Ok((t, _)) => self.trie = t,
            Err(_) => {
                self.complete = false;
                self.overlay.insert(key.to_string(), info);
            }
        }
    }

    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// Write the trie's not-yet-persisted nodes into `store` (durable at its next flush);
    /// returns how many. Nothing is written for an incomplete index.
    pub fn stage(&self, store: &mut FileNodeStore) -> usize {
        if !self.complete {
            return 0;
        }
        self.trie.persist_to_store(store)
    }

    /// Point the sidecar at this trie for revision `seq`, whose nodes are already durable
    /// (staged and flushed), then drop the in-memory nodes. `Ok(false)` if the index is not
    /// complete: a sidecar must never name a root that lacks a committed key.
    pub fn publish(
        &mut self,
        paths: &WorldPaths,
        revision_seq: u64,
        revision_digest: Digest,
    ) -> Result<bool, WorldError> {
        if !self.complete {
            return Ok(false);
        }
        let (root, len) = (self.trie.root_digest(), self.trie.len());
        IdempotencySidecar {
            revision_seq,
            revision_digest,
            root,
            len: len as u64,
        }
        .write(paths)?;
        self.trie = Trie::from_root_digest(root, len);
        Ok(true)
    }

    /// [`Self::stage`], flush, [`Self::publish`]: refresh the sidecar outside a commit.
    pub fn persist(
        &mut self,
        store: &mut FileNodeStore,
        paths: &WorldPaths,
        revision_seq: u64,
        revision_digest: Digest,
    ) -> Result<bool, WorldError> {
        if !self.complete {
            return Ok(false);
        }
        self.stage(store);
        store.flush()?;
        self.publish(paths, revision_seq, revision_digest)
    }

    /// Open the index for a world at `head_seq`/`head_digest`: the sidecar (caught up from
    /// the receipts after it) when sound, else a full scan.
    pub fn load(
        paths: &WorldPaths,
        store: &FileNodeStore,
        head_seq: u64,
        head_digest: Digest,
    ) -> Self {
        if let Some(idx) = Self::from_sidecar(paths, store, head_seq, head_digest) {
            return idx;
        }
        Self::from_scan(paths, store, head_seq)
    }

    fn from_sidecar(
        paths: &WorldPaths,
        store: &FileNodeStore,
        head_seq: u64,
        head_digest: Digest,
    ) -> Option<Self> {
        let sc = IdempotencySidecar::read(&paths.idempotency_file())?;
        if sc.revision_seq > head_seq || head_seq - sc.revision_seq > MAX_TAIL {
            return None;
        }
        if sc.revision_seq == head_seq {
            if sc.revision_digest != head_digest {
                return None;
            }
        } else if sc.revision_seq > 0 {
            // The sidecar's own revision must be this world's: its receipt agrees.
            let (_, info) = record_of(paths, sc.revision_seq, head_seq)?;
            if info.revision_digest != sc.revision_digest {
                return None;
            }
        }
        let len = usize::try_from(sc.len).ok()?;
        let mut trie = Trie::from_root_digest(sc.root, len);
        // Force the root node: an absent or damaged store is a miss, not a later hard error.
        trie.get_with_store(&idempotency_trie_key(""), store).ok()?;
        for seq in sc.revision_seq + 1..=head_seq {
            let (key, info) = record_of(paths, seq, head_seq)?;
            let (t, _) = trie
                .insert_with_store(idempotency_trie_key(&key), encode_info(&info), store)
                .ok()?;
            trie = t;
        }
        Some(Self {
            trie,
            overlay: BTreeMap::new(),
            complete: true,
            source: if sc.revision_seq == head_seq {
                IdempotencySource::Sidecar
            } else {
                IdempotencySource::SidecarTail(head_seq - sc.revision_seq)
            },
        })
    }

    /// Every receipt, then every revision record without a receipt (a revision record carries
    /// its whole changed-key list, so it is parsed only when the receipt is missing).
    fn from_scan(paths: &WorldPaths, store: &FileNodeStore, head_seq: u64) -> Self {
        let mut found: BTreeMap<String, CommittedBatchInfo> = BTreeMap::new();
        let mut receipted = std::collections::BTreeSet::new();
        if let Ok(entries) = fs::read_dir(paths.receipts_dir()) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(seq) = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .and_then(|s| s.parse::<u64>().ok())
                {
                    receipted.insert(seq);
                }
                if let Some((k, i)) = parse_record(&path, head_seq) {
                    found.insert(k, i);
                }
            }
        }
        if let Ok(entries) = fs::read_dir(paths.revisions_dir()) {
            for entry in entries.flatten() {
                let path = entry.path();
                let seq = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .and_then(|s| s.parse::<u64>().ok());
                if path.extension().and_then(|e| e.to_str()) == Some("json")
                    && seq.is_some_and(|s| receipted.contains(&s))
                {
                    continue;
                }
                if let Some((k, i)) = parse_record(&path, head_seq) {
                    found.insert(k, i);
                }
            }
        }
        let mut idx = Self::empty();
        idx.source = IdempotencySource::Scan;
        for (k, i) in found {
            idx.record(&k, i, store);
        }
        idx
    }
}
