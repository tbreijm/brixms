//! Library session API for the persistent world runtime (ADR-0046 §3.2, P3).

use brix_canon::{CanonReader, CanonWriter, Digest, Domain};
use brix_lower::module_graph::{LinkedProgram, ModuleGraph, ModuleLoaderLimits};
use brix_lower::relation_dag::{lower_relations, OperatorNode};
use soc_core::store::{FileNodeStore, TrieMap};

use super::state_store::{StateStore, DEFAULT_NODE_CACHE_BYTES};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use super::batch::{WorldBatch, WorldBatchOp};
use super::chunk_policy::{
    ChunkInput, ChunkPolicy, HeuristicChunkPolicy, DEFAULT_APPLY_HYDRATED_BYTES,
    PENDING_INDEX_BYTES_PER_NODE,
};
use super::codec::{encode_secondary_key, extract_indexed_field, TupleRecord};
use super::decision_codec::compute_decision_root;
use super::error::{CrashPoint, WorldError};
use super::idempotency::{IdempotencyIndex, IdempotencySource};
use super::manifest::{RelationDecl, WorldManifest, WORLD_PROFILE};
use super::network::{DecisionExplanation, SettledDecision, UpstreamSettlementLink, WorldNetwork};
use super::network_state::{Loaded, NetworkStateHeader, ValidityKey};
use super::oracle::DiffEvent;
use super::paths::WorldPaths;
use super::revision::{SettlementStatus, WorldRevision, REVISION_SCHEMA_V3};
use super::settlement_checkpoint::{apply_delta, SettlementCheckpoint};
use super::staging::StagingManager;
use super::state_cache::StateCache;
use super::types::{
    QueryCursor, QueryCursorKind, QueryCursorPosition, WorldCursor, WorldKey, WorldTuple,
};

fn parse_digest(hex: &str, field: &str) -> Result<Digest, WorldError> {
    if hex.len() != 64 {
        return Err(WorldError::Json(format!("{field} must be 64 hex chars")));
    }
    let mut bytes = [0u8; 32];
    let chars: Vec<char> = hex.chars().collect();
    for i in (0..64).step_by(2) {
        let hi = chars[i]
            .to_digit(16)
            .ok_or_else(|| WorldError::Json(format!("invalid hex in {field}")))?;
        let lo = chars[i + 1]
            .to_digit(16)
            .ok_or_else(|| WorldError::Json(format!("invalid hex in {field}")))?;
        bytes[i / 2] = ((hi << 4) | lo) as u8;
    }
    Ok(Digest::from_bytes(bytes))
}

fn link_closure(
    root_module: &str,
    sources: &BTreeMap<String, String>,
) -> Result<(LinkedProgram, Digest, BTreeMap<String, String>), WorldError> {
    let loader = |name: &str| sources.get(name).cloned();
    let graph = ModuleGraph::load(root_module, &loader, ModuleLoaderLimits::default())
        .map_err(|e| WorldError::NetworkError(format!("module load error: {e}")))?;
    let linked = graph
        .link()
        .map_err(|e| WorldError::NetworkError(format!("module link error: {e}")))?;
    let graph_manifest = graph.manifest(WORLD_PROFILE);
    let digest = graph_manifest.transitive_manifest_digest;
    let mut closure = BTreeMap::new();
    for name in graph_manifest.modules.keys() {
        let source = sources
            .get(name)
            .ok_or_else(|| WorldError::NetworkError(format!("missing loaded source '{name}'")))?;
        closure.insert(name.clone(), source.clone());
    }
    Ok((linked, digest, closure))
}

/// Share of the world's `node_cache_bytes` budget given to each cache tier: the raw
/// encoded-node cache (cheap to hold, decoded on a hit) gets half, the operator-state decoded
/// cache a quarter and the base-relation decoded cache an eighth, so the sum approximates the
/// resident memory the knob is meant to bound.
fn raw_cache_share(budget: usize) -> usize {
    budget / 2
}
fn state_cache_share(budget: usize) -> usize {
    budget / 4
}
fn base_cache_share(budget: usize) -> usize {
    budget / 8
}

fn manifest_node_cache_budget(manifest: &WorldManifest) -> usize {
    manifest
        .node_cache_bytes
        .map_or(DEFAULT_NODE_CACHE_BYTES, |n| {
            usize::try_from(n).unwrap_or(usize::MAX)
        })
}

fn configure_node_store_caches(store: &FileNodeStore, budget: usize) {
    store.set_cache_budget_bytes(raw_cache_share(budget));
    store.set_decoded_cache_capacity(base_cache_share(budget));
}

/// A persisted base trie as `(root, len)` only. Callers guarantee the trie was persisted.
fn dehydrate_base(
    tries: BTreeMap<String, TrieMap<WorldKey, WorldTuple>>,
) -> BTreeMap<String, TrieMap<WorldKey, WorldTuple>> {
    tries
        .into_iter()
        .map(|(name, t)| {
            let d = TrieMap::from_root_digest(t.root_digest(), t.len());
            (name, d)
        })
        .collect()
}

/// What chunking did for one batch (operational; never reaches a digest).
struct ChunkRun {
    chunks: usize,
    persist_ms: f64,
    /// Objects written by the between-chunk persists.
    objects: usize,
    fallback: bool,
}

impl ChunkRun {
    fn single() -> Self {
        Self {
            chunks: 1,
            persist_ms: 0.0,
            objects: 0,
            fallback: false,
        }
    }
}

/// Errors that are about the environment (disk, store, lock), not about the batch's logic.
/// A chunked run that hits one fails the batch; any other error may be an artifact of the
/// chunk boundaries, so the batch is re-run unchunked and that result is authoritative.
fn is_environmental(e: &WorldError) -> bool {
    matches!(
        e,
        WorldError::Io(_)
            | WorldError::InjectedCrash(_)
            | WorldError::State(_)
            | WorldError::CorruptState(_)
            | WorldError::MissingState(_)
            | WorldError::MissingObject(_)
            | WorldError::CorruptedObject(_)
            | WorldError::WorldLocked(_)
            | WorldError::StateMismatch(_)
            | WorldError::ValidityMismatch(_)
    )
}

/// Apply `ops` to `sn` (a private clone of `net`) in the chunks `policy` plans, persisting and
/// dehydrating the staged network between chunks so the hydrated working set stays near the
/// budget. Output (roots, decision delta, revision) is identical for any chunking; with no
/// limit set, a batch the policy keeps whole runs exactly the unchunked path.
#[allow(clippy::too_many_arguments)]
fn apply_ops_chunked(
    mut sn: WorldNetwork,
    net: &WorldNetwork,
    ops: &[WorldBatchOp],
    limits: Option<&super::network::NetworkLimits>,
    lookup: &super::network::BaseLookup<'_>,
    policy: &dyn ChunkPolicy,
    budget: u64,
    crash: Option<CrashPoint>,
) -> Result<(WorldNetwork, super::network::NetworkDeltaReport, ChunkRun), WorldError> {
    let default_limits = super::network::NetworkLimits::default();
    let lim = limits.unwrap_or(&default_limits);
    let mut input = ChunkInput {
        ops_total: ops.len(),
        ops_done: 0,
        prev_chunk_ops: 0,
        prev_nodes_written: 0,
        pending_index_bytes: 0,
        budget_bytes: budget,
    };
    // Any execution bound is accounted per batch, so a bounded batch is never split (H3).
    let first = if *lim == default_limits {
        policy.chunk_plan(&input)
    } else {
        super::chunk_policy::ChunkDecision {
            ops: ops.len(),
            flush_pack: false,
        }
    };
    if first.ops >= ops.len() {
        let report = sn.apply_ops_staged_view(ops, lim, Some(lookup))?;
        return Ok((sn, report, ChunkRun::single()));
    }

    let mut run = ChunkRun::single();
    let chunked: Result<super::network::NetworkDeltaReport, WorldError> = (|| {
        let mut ctx = sn.begin_batch();
        let mut decision = first;
        loop {
            let remaining = ops.len() - input.ops_done;
            let n = decision.ops.clamp(1, remaining);
            sn.apply_chunk(
                &mut ctx,
                &ops[input.ops_done..input.ops_done + n],
                lim,
                Some(lookup),
            )?;
            input.ops_done += n;
            input.prev_chunk_ops = n;
            if input.ops_done >= ops.len() {
                break;
            }
            let started = Instant::now();
            let written = sn.persist_and_dehydrate_in_place()?;
            run.objects += written;
            run.persist_ms += ms_since(started);
            input.prev_nodes_written = written;
            if crash == Some(CrashPoint::AfterChunkPersistedBeforeNextChunk) {
                return Err(WorldError::InjectedCrash(
                    CrashPoint::AfterChunkPersistedBeforeNextChunk,
                ));
            }
            input.pending_index_bytes =
                sn.state_store().pending_stats().0 as u64 * PENDING_INDEX_BYTES_PER_NODE;
            decision = policy.chunk_plan(&input);
            if decision.flush_pack {
                let started = Instant::now();
                sn.state_store().flush()?;
                run.persist_ms += ms_since(started);
                if crash == Some(CrashPoint::AfterIntermediatePackFlush) {
                    return Err(WorldError::InjectedCrash(
                        CrashPoint::AfterIntermediatePackFlush,
                    ));
                }
            }
        }
        run.chunks = ctx.chunks();
        sn.finish_batch_cascade(ctx, lim, Some(lookup))
    })();
    match chunked {
        Ok(report) => Ok((sn, report, run)),
        Err(e) if is_environmental(&e) => Err(e),
        Err(_) => {
            // The error may be an artifact of where the chunks fell (a transient state the
            // whole batch never reaches). The unchunked run is authoritative: it accepts
            // exactly what it accepted before chunking existed.
            let mut sn = net.clone();
            let report = sn.apply_ops_staged_view(ops, lim, Some(lookup))?;
            Ok((
                sn,
                report,
                ChunkRun {
                    fallback: true,
                    ..run
                },
            ))
        }
    }
}

fn validate_program_relations(
    manifest: &WorldManifest,
    network: &WorldNetwork,
) -> Result<(), WorldError> {
    let declared: std::collections::BTreeSet<_> = manifest.relations.keys().collect();
    let program: std::collections::BTreeSet<_> = network.base_relations.keys().collect();
    if declared != program {
        return Err(WorldError::InvalidSchema(format!(
            "world relations do not match executable program base relations (world: {:?}, program: {:?})",
            declared, program,
        )));
    }
    Ok(())
}

pub(super) fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), WorldError> {
    let temp = path.with_extension(format!("tmp.{}", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&temp)?;
    use std::io::Write;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&temp, path)?;
    if let Some(parent) = path.parent() {
        fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn ms_since(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// Where the time of one commit went: wall milliseconds per phase of `apply_batch`, in order,
/// plus the state store's physical IO over the whole commit. Observation only (never part of
/// a receipt, a revision or a digest); recorded for every commit in
/// [`WorldSession::last_commit_profile`].
#[derive(Clone, PartialEq, Debug, Default)]
pub struct CommitProfile {
    /// `(phase, milliseconds)`: `clone_network`, `apply_ops` (operator network deltas and
    /// settlement), `decision_encode`, `stage_base` (base tries and secondary indexes),
    /// `persist_base`, `persist_state`, `state_header`, `flush`, `publish` (revision, receipt
    /// and HEAD, each fsynced), `dehydrate`.
    pub phases: Vec<(&'static str, f64)>,
    pub total_ms: f64,
    /// Node reads that reached the store (decoded-node-cache misses) during the commit.
    pub state_node_reads: u64,
    /// Body bytes read from packs during the commit.
    pub state_bytes_read: u64,
    /// Bytes appended to the pack (framing and index included) during the commit.
    pub state_bytes_written: u64,
    /// Chunks the batch's operator network was applied in (1: not chunked, CPD off or the
    /// batch under the budget).
    pub chunks: usize,
    /// Milliseconds spent persisting and dehydrating between chunks (inside `apply_ops`).
    pub chunk_persist_ms: f64,
    /// Chunking hit a non-environmental error and the batch was re-run unchunked.
    pub chunk_fallback: bool,
}

/// Outcome receipt returned upon applying a batch.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RevisionReceipt {
    pub revision_seq: u64,
    pub revision_digest: Digest,
    pub idempotency_key: String,
    pub is_idempotent_replay: bool,
    pub changed_keys_count: usize,
    pub objects_written: usize,
}

/// A paged query slice of records from a relation.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct QueryPage {
    pub relation: String,
    pub entries: Vec<(WorldKey, WorldTuple)>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

/// Opaque continuation for one secondary-index query at one immutable revision.
///
/// Tokens are transport encodings, not authorization credentials. A token cannot be
/// reused for a different revision, relation, index, or encoded indexed value.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SecondaryIndexCursor {
    revision: Digest,
    relation: String,
    index_field: String,
    indexed_value: WorldKey,
    position: WorldCursor,
}

impl SecondaryIndexCursor {
    pub fn to_token(&self) -> String {
        let mut writer = CanonWriter::new();
        writer.write_str("brix.secondary-cursor@1");
        writer.write_bytes(self.revision.as_bytes());
        writer.write_str(&self.relation);
        writer.write_str(&self.index_field);
        writer.write_bytes(self.indexed_value.as_bytes());
        writer.write_bytes(&self.position.hash);
        writer.write_bytes(self.position.key.as_bytes());
        WorldKey::new(writer.finish()).to_hex()
    }

    pub fn from_token(token: &str) -> Result<Self, WorldError> {
        let bytes = WorldKey::from_hex(token)?;
        let mut reader = CanonReader::new(bytes.as_bytes());
        let invalid = || WorldError::InvalidCursor("malformed secondary-index cursor".into());
        if reader.read_bytes().map_err(|_| invalid())? != b"brix.secondary-cursor@1" {
            return Err(invalid());
        }
        let revision: [u8; 32] = reader
            .read_bytes()
            .map_err(|_| invalid())?
            .try_into()
            .map_err(|_| invalid())?;
        let relation = std::str::from_utf8(reader.read_bytes().map_err(|_| invalid())?)
            .map_err(|_| invalid())?
            .to_owned();
        let index_field = std::str::from_utf8(reader.read_bytes().map_err(|_| invalid())?)
            .map_err(|_| invalid())?
            .to_owned();
        let indexed_value = WorldKey::new(reader.read_bytes().map_err(|_| invalid())?.to_vec());
        let hash: [u8; 32] = reader
            .read_bytes()
            .map_err(|_| invalid())?
            .try_into()
            .map_err(|_| invalid())?;
        let key = WorldKey::new(reader.read_bytes().map_err(|_| invalid())?.to_vec());
        if !reader.is_empty() {
            return Err(invalid());
        }
        Ok(Self {
            revision: Digest::from_bytes(revision),
            relation,
            index_field,
            indexed_value,
            position: WorldCursor::new(hash, key),
        })
    }
}

/// Bounded primary-key memberships returned by an equality secondary index.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SecondaryIndexPage {
    pub keys: Vec<WorldKey>,
    pub next_cursor: Option<SecondaryIndexCursor>,
}

fn secondary_index_page(
    indexes: &BTreeMap<String, TrieMap<WorldKey, WorldTuple>>,
    store: &FileNodeStore,
    revision: Digest,
    query: (&str, &str, &WorldKey),
    cursor: Option<&SecondaryIndexCursor>,
    limit: usize,
) -> Result<SecondaryIndexPage, WorldError> {
    let (relation, index_field, indexed_value) = query;
    if limit == 0 {
        return Err(WorldError::InvalidSchema(
            "secondary-index page limit must be positive".into(),
        ));
    }
    if let Some(cursor) = cursor {
        if cursor.revision != revision
            || cursor.relation != relation
            || cursor.index_field != index_field
            || cursor.indexed_value != *indexed_value
        {
            return Err(WorldError::InvalidCursor(
                "secondary-index cursor query scope mismatch".into(),
            ));
        }
    }
    let sec_name = format!("{relation}:{index_field}");
    let index = indexes
        .get(&sec_name)
        .ok_or_else(|| WorldError::UnknownSecondaryIndex(sec_name.clone()))?;
    let Some(membership) = index.get_with_store(indexed_value, store)? else {
        return Ok(SecondaryIndexPage {
            keys: Vec::new(),
            next_cursor: None,
        });
    };
    let digest: [u8; 32] = membership
        .as_bytes()
        .try_into()
        .map_err(|_| WorldError::CorruptedObject(index.root_digest()))?;
    let members = TrieMap::<WorldKey, WorldTuple>::from_root_digest(Digest::from_bytes(digest), 0);
    let (entries, next) = members.iter_page_with_store(
        cursor.map(|c| (&c.position.hash, &c.position.key)),
        limit,
        store,
    )?;
    Ok(SecondaryIndexPage {
        keys: entries.into_iter().map(|(key, _)| key).collect(),
        next_cursor: next.map(|(hash, key)| SecondaryIndexCursor {
            revision,
            relation: relation.to_owned(),
            index_field: index_field.to_owned(),
            indexed_value: indexed_value.clone(),
            position: WorldCursor::new(hash, key),
        }),
    })
}

/// A paged diff slice of modifications between two revisions.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DiffPage {
    pub relation: String,
    pub from_revision: u64,
    pub to_revision: u64,
    pub events: Vec<DiffEvent>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

/// Cache entry for committed batch idempotency.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommittedBatchInfo {
    pub seq: u64,
    pub revision_digest: Digest,
    pub batch_digest: Option<Digest>,
}

/// How long a commit waits for the writer lock before reporting `WorldLocked` (readers hold
/// it only for sub-millisecond publish steps; see [`WorldLockGuard::acquire_wait`]).
const COMMIT_LOCK_WAIT: std::time::Duration = std::time::Duration::from_millis(500);

/// RAII guard representing exclusive writer ownership over a world directory.
pub struct WorldLockGuard {
    _file: fs::File,
}

impl WorldLockGuard {
    pub fn acquire(root: &Path) -> Result<Self, WorldError> {
        let lock_path = root.join(".lock");
        match fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
        {
            Ok(file) => match file.try_lock() {
                Ok(()) => Ok(Self { _file: file }),
                Err(fs::TryLockError::WouldBlock) => Err(WorldError::WorldLocked(format!(
                    "world '{}' is locked by another writer",
                    root.display()
                ))),
                Err(fs::TryLockError::Error(e)) => Err(WorldError::Io(e)),
            },
            Err(e) => Err(WorldError::Io(e)),
        }
    }
}

impl WorldLockGuard {
    /// [`Self::acquire`], retrying for up to `wait` while another handle holds the lock. For
    /// commits: a reader's opportunistic cache write holds the lock only for a few
    /// milliseconds (it does its heavy work outside it), so a short wait turns a spurious
    /// `WorldLocked` into success, while a genuine second writer still fails after `wait`.
    pub fn acquire_wait(root: &Path, wait: std::time::Duration) -> Result<Self, WorldError> {
        let deadline = Instant::now() + wait;
        loop {
            match Self::acquire(root) {
                Err(WorldError::WorldLocked(m)) => {
                    if Instant::now() >= deadline {
                        return Err(WorldError::WorldLocked(m));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                other => return other,
            }
        }
    }
}

// The OS releases the lock when the handle closes, including process termination.
// Do not unlink the lock file: a replacement inode would allow a second writer.

/// Tracks reader and checkpoint pins to protect revisions from compaction.
#[derive(Clone, Debug, Default)]
pub struct PinRegistry {
    next_pin_id: u64,
    reader_pins: BTreeMap<u64, (u64, std::time::Instant)>,
    checkpoint_pins: std::collections::BTreeSet<u64>,
}

impl PinRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pin_reader(&mut self, seq: u64) -> u64 {
        self.next_pin_id += 1;
        let id = self.next_pin_id;
        self.reader_pins
            .insert(id, (seq, std::time::Instant::now()));
        id
    }

    pub fn unpin_reader(&mut self, pin_id: u64) -> bool {
        self.reader_pins.remove(&pin_id).is_some()
    }

    pub fn pin_checkpoint(&mut self, seq: u64) {
        self.checkpoint_pins.insert(seq);
    }

    pub fn unpin_checkpoint(&mut self, seq: u64) -> bool {
        self.checkpoint_pins.remove(&seq)
    }

    pub fn checkpoint_pins(&self) -> &std::collections::BTreeSet<u64> {
        &self.checkpoint_pins
    }

    pub fn is_pinned(&self, seq: u64) -> bool {
        if self.checkpoint_pins.contains(&seq) {
            return true;
        }
        self.reader_pins.values().any(|&(s, _)| s == seq)
    }

    pub fn pinned_revisions(&self) -> std::collections::BTreeSet<u64> {
        let mut set = self.checkpoint_pins.clone();
        for &(seq, _) in self.reader_pins.values() {
            set.insert(seq);
        }
        set
    }

    pub fn cleanup_expired(&mut self, max_age: std::time::Duration) {
        let now = std::time::Instant::now();
        self.reader_pins
            .retain(|_, &mut (_, created)| now.duration_since(created) <= max_age);
    }
}

/// Outcome of a history compaction run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CompactionReport {
    pub revisions_reclaimed: usize,
    pub revisions_retained: usize,
    pub bytes_reclaimed: u64,
    pub pinned_revisions: std::collections::BTreeSet<u64>,
}

/// An immutable read snapshot pinned to a specific revision.
#[derive(Clone, Debug)]
pub struct WorldSnapshot {
    pub world_id: String,
    pub revision: WorldRevision,
    pub relations: BTreeMap<String, TrieMap<WorldKey, WorldTuple>>,
    pub secondary_indexes: BTreeMap<String, TrieMap<WorldKey, WorldTuple>>,
    pub node_store: FileNodeStore,
}

impl WorldSnapshot {
    pub fn revision_seq(&self) -> u64 {
        self.revision.seq
    }

    pub fn revision_digest(&self) -> Digest {
        self.revision.revision_digest
    }

    pub fn get(&self, relation: &str, key: &WorldKey) -> Result<Option<WorldTuple>, WorldError> {
        let trie = self
            .relations
            .get(relation)
            .ok_or_else(|| WorldError::UnknownRelation(relation.to_string()))?;
        let val = trie.get_with_store(key, &self.node_store)?;
        Ok(val)
    }

    /// Collect every matching primary key. For bounded memory, use the paged API.
    /// `indexed_value` must be produced by [`encode_secondary_key`].
    pub fn query_secondary_index(
        &self,
        relation: &str,
        index_field: &str,
        indexed_value: &WorldKey,
    ) -> Result<Vec<WorldKey>, WorldError> {
        let mut keys = Vec::new();
        let mut cursor = None;
        loop {
            let page = self.query_secondary_index_page(
                relation,
                index_field,
                indexed_value,
                cursor.as_ref(),
                1024,
            )?;
            keys.extend(page.keys);
            cursor = page.next_cursor;
            if cursor.is_none() {
                return Ok(keys);
            }
        }
    }

    /// Read a bounded page using an explicitly encoded secondary key.
    /// Continuations stay valid on this pinned revision after the writer advances.
    pub fn query_secondary_index_page(
        &self,
        relation: &str,
        index_field: &str,
        indexed_value: &WorldKey,
        cursor: Option<&SecondaryIndexCursor>,
        limit: usize,
    ) -> Result<SecondaryIndexPage, WorldError> {
        secondary_index_page(
            &self.secondary_indexes,
            &self.node_store,
            self.revision.revision_digest,
            (relation, index_field, indexed_value),
            cursor,
            limit,
        )
    }

    pub fn query_page(
        &self,
        relation: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<QueryPage, WorldError> {
        let trie = self
            .relations
            .get(relation)
            .ok_or_else(|| WorldError::UnknownRelation(relation.to_string()))?;

        let parsed_cursor = match cursor {
            Some(token) => {
                let qc = QueryCursor::from_token(token)?;
                if qc.kind == QueryCursorKind::Legacy {
                    return Err(WorldError::InvalidCursor(
                        "legacy cursor not accepted for revision-pinned query".into(),
                    ));
                }
                if qc.world_id != self.world_id
                    || qc.revision != self.revision.revision_digest
                    || qc.relation != relation
                    || qc.ordering != "canonical"
                {
                    return Err(WorldError::InvalidCursor(
                        "query cursor scope mismatch".into(),
                    ));
                }
                match qc.position {
                    QueryCursorPosition::Hamt { hash, key } => Some((hash, key)),
                    QueryCursorPosition::Offset(_) => {
                        return Err(WorldError::InvalidCursor(
                            "query cursor ordering mismatch (offset cursor on hamt relation)"
                                .into(),
                        ));
                    }
                }
            }
            None => None,
        };

        let (entries, next_pos) = trie.iter_page_with_store(
            parsed_cursor.as_ref().map(|(h, k)| (h, k)),
            limit,
            &self.node_store,
        )?;

        let next_cursor = next_pos.map(|(hash, key)| {
            QueryCursor::new_hamt(
                &self.world_id,
                self.revision.revision_digest,
                relation,
                hash,
                key,
            )
            .to_token()
        });
        let has_more = next_cursor.is_some();

        Ok(QueryPage {
            relation: relation.to_string(),
            entries,
            next_cursor,
            has_more,
        })
    }
}

/// Schema tag for [`ExecProfileV1`] (ADR-0046 P6 §1.1).
pub const EXEC_PROFILE_SCHEMA: &str = "brix.world.exec@1";

/// Module-loader resource limits as recorded in [`ExecProfileV1`] — the same
/// four bounds as `brix_lower::module_graph::ModuleLoaderLimits`, named per
/// the P6 audit contract (`depth`/`modules`/`module_bytes`/`total_bytes`)
/// rather than that type's internal field names.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ExecModuleLoaderLimits {
    pub depth: usize,
    pub modules: usize,
    pub module_bytes: usize,
    pub total_bytes: usize,
}

/// Recorded, **non-authoritative** execution profile (`brix.world.exec@1`,
/// ADR-0046 P6 §1.1, decided 2026-10-04): written into `program.json` at
/// `world init` as an additive field (older files simply lack it — see
/// [`WorldSession::open`]). A verifier *compares* this to its own profile and
/// refuses on mismatch; it never adopts limits from an untrusted artifact
/// (ADR-0046 §3.8, "host operational limits must never be relaxed by an
/// untrusted artifact").
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ExecProfileV1 {
    /// Which evaluator produced the world this profile is recorded in —
    /// `"brix-world-net@1"` for the maintained incremental `WorldNetwork`, or
    /// `"brix-world-ref@1"` for the independent reference evaluator.
    pub evaluator: String,
    pub crate_version: String,
    pub module_loader_limits: ExecModuleLoaderLimits,
    pub numeric_semantics: String,
    pub settlement: String,
}

impl ExecProfileV1 {
    /// This crate's current profile for worlds driven by [`WorldNetwork`].
    pub fn current_for_network() -> Self {
        let limits = ModuleLoaderLimits::default();
        Self {
            evaluator: "brix-world-net@1".to_string(),
            crate_version: env!("CARGO_PKG_VERSION").to_string(),
            module_loader_limits: ExecModuleLoaderLimits {
                depth: limits.max_import_depth,
                modules: limits.max_import_modules,
                module_bytes: limits.max_module_source_bytes,
                total_bytes: limits.max_total_source_bytes,
            },
            numeric_semantics: "ADR-0045".to_string(),
            settlement: "least-key(phase,priority,tiebreak)".to_string(),
        }
    }

    /// Whether this execution profile is supported by the current runtime evaluator.
    pub fn is_supported(&self) -> bool {
        let current = Self::current_for_network();
        self.evaluator == current.evaluator
            && self.numeric_semantics == current.numeric_semantics
            && self.settlement == current.settlement
            && self.module_loader_limits == current.module_loader_limits
            && Self::is_supported_version(&self.crate_version)
    }

    pub fn is_supported_version(version: &str) -> bool {
        matches!(
            version,
            "0.1.0-alpha.5" | "0.1.0-alpha.6" | "0.1.0-alpha.7" | "0.1.0-alpha.8"
        ) || version == env!("CARGO_PKG_VERSION")
    }

    /// Canonical profile identity, including the candidate-value admissibility rule.
    pub fn digest(&self) -> Digest {
        let mut w = CanonWriter::new();
        w.write_tag(EXEC_PROFILE_SCHEMA);
        w.write_str(&self.evaluator);
        w.write_str(&self.crate_version);
        for n in [
            self.module_loader_limits.depth,
            self.module_loader_limits.modules,
            self.module_loader_limits.module_bytes,
            self.module_loader_limits.total_bytes,
        ] {
            w.write_uint(n as u64);
        }
        w.write_str(&self.numeric_semantics);
        w.write_str(&self.settlement);
        w.write_str("same-proposal-entity-supports-must-agree@1");
        w.digest(Domain::Value)
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "profile": EXEC_PROFILE_SCHEMA,
            "evaluator": self.evaluator,
            "crate_version": self.crate_version,
            "module_loader_limits": {
                "depth": self.module_loader_limits.depth,
                "modules": self.module_loader_limits.modules,
                "module_bytes": self.module_loader_limits.module_bytes,
                "total_bytes": self.module_loader_limits.total_bytes,
            },
            "numeric_semantics": self.numeric_semantics,
            "settlement": self.settlement,
        })
    }

    pub fn from_json(v: &serde_json::Value) -> Result<Self, WorldError> {
        let profile = v
            .get("profile")
            .and_then(|x| x.as_str())
            .ok_or_else(|| WorldError::Json("exec_profile missing 'profile'".into()))?;
        if profile != EXEC_PROFILE_SCHEMA {
            return Err(WorldError::InvalidSchema(format!(
                "expected exec profile {EXEC_PROFILE_SCHEMA}, got {profile}"
            )));
        }
        let evaluator = v
            .get("evaluator")
            .and_then(|x| x.as_str())
            .ok_or_else(|| WorldError::Json("exec_profile missing 'evaluator'".into()))?
            .to_string();
        let crate_version = v
            .get("crate_version")
            .and_then(|x| x.as_str())
            .ok_or_else(|| WorldError::Json("exec_profile missing 'crate_version'".into()))?
            .to_string();
        let limits_val = v.get("module_loader_limits").ok_or_else(|| {
            WorldError::Json("exec_profile missing 'module_loader_limits'".into())
        })?;
        let get_usize = |field: &str| -> Result<usize, WorldError> {
            limits_val
                .get(field)
                .and_then(|x| x.as_u64())
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| {
                    WorldError::Json(format!(
                        "exec_profile.module_loader_limits missing '{field}'"
                    ))
                })
        };
        let module_loader_limits = ExecModuleLoaderLimits {
            depth: get_usize("depth")?,
            modules: get_usize("modules")?,
            module_bytes: get_usize("module_bytes")?,
            total_bytes: get_usize("total_bytes")?,
        };
        let numeric_semantics = v
            .get("numeric_semantics")
            .and_then(|x| x.as_str())
            .ok_or_else(|| WorldError::Json("exec_profile missing 'numeric_semantics'".into()))?
            .to_string();
        let settlement = v
            .get("settlement")
            .and_then(|x| x.as_str())
            .ok_or_else(|| WorldError::Json("exec_profile missing 'settlement'".into()))?
            .to_string();
        Ok(Self {
            evaluator,
            crate_version,
            module_loader_limits,
            numeric_semantics,
            settlement,
        })
    }
}

/// How a session obtained its operator network when it was opened (R0 M4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenMode {
    /// The persisted network state the HEAD revision names was installed lazily; no base
    /// fact was replayed.
    Restored,
    /// The network was rebuilt by replaying every resident base fact (a legacy revision,
    /// a validity-key change, an explicit `open_replay`, or a freshly created world).
    Replay,
    /// The world has no executable program, so there is no network.
    NoNetwork,
}

impl OpenMode {
    /// The name surfaced in CLI and serve results.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Restored => "restored",
            Self::Replay => "replay",
            Self::NoNetwork => "none",
        }
    }
}

/// A checkpoint further behind HEAD than this many revisions is not used (the frontier is).
const CHECKPOINT_MAX_TAIL: u64 = 256;
/// A checkpoint read that applied more tail revisions than this refreshes the checkpoint.
const CHECKPOINT_REFRESH_TAIL: u64 = 32;
/// Entries of a checkpoint read spot-checked against the persisted decision trie.
const CHECKPOINT_PROBES: usize = 64;

/// Operational choices for [`WorldSession::open_with_policy`]. They never change what a
/// session computes, only how much work open does and leaves behind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenPolicy {
    /// Rebuild the network by replay and never read persisted operator state.
    pub force_replay: bool,
    /// After a non-forced replay (legacy revision, changed validity key), try to persist the
    /// rebuilt state and a state-cache sidecar so the next open restores. Best effort:
    /// failures (read-only filesystem, held lock) never fail the open.
    pub refresh_after_replay: bool,
}

impl Default for OpenPolicy {
    fn default() -> Self {
        Self {
            force_replay: false,
            refresh_after_replay: true,
        }
    }
}

/// The active persistent world runtime session.
pub struct WorldSession {
    pub root: PathBuf,
    pub paths: WorldPaths,
    pub manifest: WorldManifest,
    pub current_revision: u64,
    pub current_revision_digest: Option<Digest>,
    pub node_store: FileNodeStore,
    pub relations: BTreeMap<String, TrieMap<WorldKey, WorldTuple>>,
    pub secondary_indexes: BTreeMap<String, TrieMap<WorldKey, WorldTuple>>,
    pub idempotency: IdempotencyIndex,
    /// The GC generation marker (`objects/GC_EPOCH`) this session's store handle and caches
    /// were built under; compared under the writer lock before anything is written (see
    /// [`Self::sync_gc_epoch`]).
    gc_epoch: String,
    /// Whether `all_settlements` may leave a settlement checkpoint behind (best effort);
    /// off for `open_replay`.
    pub checkpoint_autowrite: bool,
    pub crash_injector: Option<CrashPoint>,
    pub network: Option<WorldNetwork>,
    /// Diagnostic: post-commit dehydrations that failed (network kept hydrated).
    pub dehydrate_failures: u64,
    /// Phase timings and IO of the most recent committed batch (`None` before the first
    /// commit of this session; an idempotent replay commits nothing and leaves it unchanged).
    pub last_commit_profile: Option<CommitProfile>,
    /// Decides where a large batch is split into chunks (CPD). Operational only: no root,
    /// digest or decision delta depends on it.
    pub chunk_policy: Arc<dyn ChunkPolicy>,
    /// Recorded (non-authoritative) execution profile (ADR-0046 P6 §1.1,
    /// decided 2026-10-04); `None` for a storage-only world or a
    /// `program.json` written before this field existed.
    pub exec_profile: Option<ExecProfileV1>,
    /// Retention pins protecting revisions from compaction.
    pub pins: PinRegistry,
    /// How the network was obtained at open (see [`OpenMode`]).
    pub open_mode: OpenMode,
    /// Why a replay happened when one did (`forced`, `legacy-revision`, `validity-key`).
    pub replay_reason: Option<&'static str>,
    /// Wall-clock milliseconds `open` took.
    pub open_ms: u64,
    /// Committed batches applied on top of the persisted network state at open. Every
    /// commit persists its own state, so this is `0` today; a replay rebuilds the network
    /// from the resident base facts, not from batches, and also reports `0` (see
    /// `open_mode` and `replay_reason`). Reserved for checkpointed state.
    pub tail_batches: u64,
    /// The `network_state_root` of the HEAD revision, if it has one.
    pub network_state_root: Option<Digest>,
    /// Where restored state came from: `"revision"` (named by the HEAD record) or
    /// `"sidecar"` (the state-cache file); `None` unless `open_mode` is `Restored`.
    pub state_source: Option<&'static str>,
    /// Whether this open persisted a state-cache sidecar after replaying.
    pub state_refreshed: bool,
    /// Why a post-replay refresh did not happen, when it was attempted and failed.
    pub state_refresh_error: Option<String>,
}

impl WorldSession {
    /// Create a new world directory from a manifest.
    pub fn create(root: impl AsRef<Path>, manifest: WorldManifest) -> Result<Self, WorldError> {
        manifest.validate()?;
        let root = root.as_ref().to_path_buf();
        if root.exists() && (root.join("world.json").exists() || root.join("HEAD").exists()) {
            return Err(WorldError::WorldAlreadyExists);
        }

        let paths = WorldPaths::new(&root);
        paths.ensure_dirs()?;

        // Write world.json manifest
        let manifest_bytes = serde_json::to_vec_pretty(&manifest.to_json())
            .map_err(|e| WorldError::Json(e.to_string()))?;
        fs::write(paths.world_json(), manifest_bytes)?;

        // Write initial HEAD pointing to genesis (revision 0)
        fs::write(paths.head(), "0\n")?;

        let gc_epoch = super::gc::read_gc_epoch(&paths);
        let node_store = FileNodeStore::new(&root)?;
        configure_node_store_caches(&node_store, manifest_node_cache_budget(&manifest));
        let mut relations = BTreeMap::new();
        let mut secondary_indexes = BTreeMap::new();

        let mut rev0_sec_roots = BTreeMap::new();
        for (name, decl) in &manifest.relations {
            relations.insert(name.clone(), TrieMap::new());
            for idx in &decl.secondary_indexes {
                let sec_name = format!("{}:{}", name, idx);
                let trie = TrieMap::new();
                rev0_sec_roots.insert(sec_name.clone(), trie.root_digest());
                secondary_indexes.insert(sec_name, trie);
            }
        }

        // Write revision 0 record
        let rev0 = WorldRevision::new(
            0,
            &manifest.created_at,
            0,
            "genesis",
            None,
            None,
            None,
            BTreeMap::new(),
            BTreeMap::new(),
            rev0_sec_roots,
            None,
            BTreeMap::new(),
            SettlementStatus::Committed,
            None,
        );
        let rev0_bytes = serde_json::to_vec_pretty(&rev0.to_json())
            .map_err(|e| WorldError::Json(e.to_string()))?;
        fs::write(paths.revision_file(0), rev0_bytes)?;

        Ok(Self {
            root,
            paths,
            manifest,
            current_revision: 0,
            current_revision_digest: Some(rev0.revision_digest),
            node_store,
            relations,
            secondary_indexes,
            idempotency: IdempotencyIndex::empty(),
            gc_epoch,
            checkpoint_autowrite: true,
            crash_injector: None,
            network: None,
            dehydrate_failures: 0,
            last_commit_profile: None,
            chunk_policy: Arc::new(HeuristicChunkPolicy),
            exec_profile: None,
            pins: PinRegistry::default(),
            open_mode: OpenMode::NoNetwork,
            replay_reason: None,
            open_ms: 0,
            tail_batches: 0,
            network_state_root: None,
            state_source: None,
            state_refreshed: false,
            state_refresh_error: None,
        })
    }

    /// Open an existing world directory (lazy cold open). When the HEAD revision names
    /// persisted network state valid for this binary, it is restored without replaying any
    /// base fact; otherwise (a legacy revision or a changed validity key) the network is
    /// rebuilt by replay and the next commit records restorable state. Persisted state that
    /// is corrupt, missing, or inconsistent with its revision is a hard error, never a
    /// silent fallback: call [`Self::open_replay`] to rebuild deliberately.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, WorldError> {
        Self::open_with_policy(root.as_ref(), OpenPolicy::default())
    }

    /// Open, always rebuilding the network by replaying the resident base facts and never
    /// reading persisted operator state (tests, verification, repair).
    pub fn open_replay(root: impl AsRef<Path>) -> Result<Self, WorldError> {
        Self::open_with_policy(
            root.as_ref(),
            OpenPolicy {
                force_replay: true,
                refresh_after_replay: false,
            },
        )
    }

    /// Open under an explicit [`OpenPolicy`].
    pub fn open_with_policy(root: &Path, policy: OpenPolicy) -> Result<Self, WorldError> {
        let started = Instant::now();
        let root = root.to_path_buf();
        let paths = WorldPaths::new(&root);

        if !paths.world_json().exists() {
            return Err(WorldError::ManifestNotFound);
        }

        let manifest_bytes = fs::read(paths.world_json())?;
        let manifest_val: serde_json::Value =
            serde_json::from_slice(&manifest_bytes).map_err(|e| WorldError::Json(e.to_string()))?;
        let manifest = WorldManifest::from_json(&manifest_val)?;

        if !paths.head().exists() {
            return Err(WorldError::CorruptedHead("HEAD file missing".to_string()));
        }

        let head_str = fs::read_to_string(paths.head())?;
        let head_line = head_str.lines().next().unwrap_or("").trim();
        let current_revision: u64 = head_line
            .split_whitespace()
            .next()
            .ok_or_else(|| WorldError::CorruptedHead("empty HEAD".to_string()))?
            .parse()
            .map_err(|_| WorldError::CorruptedHead("invalid integer in HEAD".to_string()))?;

        // Read the GC marker before the store is built: a collection in between is then seen
        // (the marker differs) rather than silently baked into this session's caches.
        let gc_epoch = super::gc::read_gc_epoch(&paths);
        let node_store = FileNodeStore::new(&root)?;
        configure_node_store_caches(&node_store, manifest_node_cache_budget(&manifest));

        // Read current revision record
        let rev_file = paths.revision_file(current_revision);
        if !rev_file.exists() {
            return Err(WorldError::RevisionNotFound(current_revision));
        }
        let rev_bytes = fs::read(rev_file)?;
        let rev_val: serde_json::Value =
            serde_json::from_slice(&rev_bytes).map_err(|e| WorldError::Json(e.to_string()))?;
        let current_rev = WorldRevision::from_json(&rev_val)?;
        let head_fields: Vec<_> = head_line.split_whitespace().collect();
        if current_rev.seq != current_revision
            || head_fields.len() > 2
            || (head_fields.len() == 1 && current_revision != 0)
            || (head_fields.len() == 2 && head_fields[1] != current_rev.revision_digest.to_hex())
        {
            return Err(WorldError::CorruptedHead(
                "HEAD revision digest mismatch".into(),
            ));
        }

        if manifest.program_required && current_rev.program_digest != Some(manifest.program_digest)
        {
            return Err(WorldError::InvalidSchema(
                "executable world revision is not bound to its manifest program digest".into(),
            ));
        }

        // Lazy hydration: construct TrieMaps from Merkle root digests without loading full tries
        let mut relations = BTreeMap::new();
        for name in manifest.relations.keys() {
            let root_digest = current_rev.relation_roots.get(name).copied();
            let card = current_rev
                .relation_cardinalities
                .get(name)
                .copied()
                .unwrap_or(0);
            match root_digest {
                Some(digest) => {
                    relations.insert(name.clone(), TrieMap::from_root_digest(digest, card));
                }
                None => {
                    relations.insert(name.clone(), TrieMap::new());
                }
            }
        }

        let mut secondary_indexes = BTreeMap::new();
        for (idx_name, &digest) in &current_rev.secondary_index_roots {
            secondary_indexes.insert(idx_name.clone(), TrieMap::from_root_digest(digest, 0));
        }
        for (name, decl) in &manifest.relations {
            for idx in &decl.secondary_indexes {
                let sec_name = format!("{}:{}", name, idx);
                secondary_indexes
                    .entry(sec_name)
                    .or_insert_with(TrieMap::new);
            }
        }

        // The idempotency index: the persisted trie (plus the receipts after it), else a scan.
        let idempotency = IdempotencyIndex::load(
            &paths,
            &node_store,
            current_revision,
            current_rev.revision_digest,
        );

        // Restore persisted checkpoint pins
        let mut pins = PinRegistry::default();
        if paths.pins_file().exists() {
            if let Ok(bytes) = fs::read(paths.pins_file()) {
                if let Ok(list) = serde_json::from_slice::<Vec<u64>>(&bytes) {
                    for seq in list {
                        pins.pin_checkpoint(seq);
                    }
                }
            }
        }

        let mut session = Self {
            root,
            paths,
            manifest,
            current_revision,
            current_revision_digest: Some(current_rev.revision_digest),
            node_store,
            relations,
            secondary_indexes,
            idempotency,
            gc_epoch,
            checkpoint_autowrite: policy.refresh_after_replay,
            crash_injector: None,
            network: None,
            dehydrate_failures: 0,
            last_commit_profile: None,
            chunk_policy: Arc::new(HeuristicChunkPolicy),
            exec_profile: None,
            pins,
            open_mode: OpenMode::NoNetwork,
            replay_reason: None,
            open_ms: 0,
            tail_batches: 0,
            network_state_root: current_rev.network_state_root,
            state_source: None,
            state_refreshed: false,
            state_refresh_error: None,
        };
        let mut pending_network: Option<WorldNetwork> = None;

        let program_file = session.paths.root.join("program.json");
        if session.manifest.program_required && !program_file.exists() {
            return Err(WorldError::InvalidSchema(
                "executable world is missing program.json source closure".into(),
            ));
        }
        if program_file.exists() {
            let bytes = fs::read(&program_file)?;
            let val: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|e| WorldError::Json(e.to_string()))?;
            let root_mod = val
                .get("root_module")
                .and_then(|x| x.as_str())
                .ok_or_else(|| WorldError::Json("program closure missing root_module".into()))?;
            let src_map = val
                .get("sources")
                .and_then(|x| x.as_object())
                .ok_or_else(|| WorldError::Json("program closure missing sources object".into()))?;
            let mut sources = BTreeMap::new();
            for (name, source) in src_map {
                let text = source.as_str().ok_or_else(|| {
                    WorldError::Json(format!("program source '{name}' must be a string"))
                })?;
                sources.insert(name.clone(), text.to_owned());
            }
            if val.get("schema").and_then(|x| x.as_str()) != Some("brix.world.program@1") {
                return Err(WorldError::InvalidSchema(
                    "unsupported program closure schema".into(),
                ));
            }
            let (linked, closure_digest, closure_sources) = link_closure(root_mod, &sources)?;
            if closure_sources.len() != sources.len() {
                return Err(WorldError::InvalidSchema(
                    "program source map contains modules outside the transitive closure".into(),
                ));
            }
            let pinned = val
                .get("program_manifest_digest")
                .and_then(|x| x.as_str())
                .ok_or_else(|| {
                    WorldError::Json("program closure missing program_manifest_digest".into())
                })?;
            let pinned = parse_digest(pinned, "program_manifest_digest")?;
            if pinned != closure_digest || pinned != session.manifest.program_digest {
                return Err(WorldError::InvalidSchema(
                    "program closure digest does not match world manifest".into(),
                ));
            }
            let network = WorldNetwork::from_program(&linked)?;
            validate_program_relations(&session.manifest, &network)?;
            pending_network = Some(network);

            // Additive field (ADR-0046 P6 §1.1, decided 2026-10-04): a
            // `program.json` written before `exec_profile` existed simply
            // lacks the key, and decodes as `None` here, not a default-valued
            // profile — the file still opens either way.
            if let Some(profile_val) = val.get("exec_profile") {
                session.exec_profile = Some(ExecProfileV1::from_json(profile_val)?);
            }
        }

        if let Some(bound) = current_rev.exec_profile_digest {
            if session.exec_profile.as_ref().map(ExecProfileV1::digest) != Some(bound) {
                return Err(WorldError::InvalidSchema(
                    "execution profile digest mismatch".into(),
                ));
            }
        }
        if let Some(profile) = &session.exec_profile {
            if !profile.is_supported() {
                return Err(WorldError::InvalidSchema(
                    "unsupported execution profile".into(),
                ));
            }
        }
        if let Some(network) = pending_network {
            session.install_network(network, &current_rev, policy)?;
        }
        if current_rev.decision_delta_digest.is_some() {
            session.decision_delta(current_revision)?;
        }
        if let (Some(expected), Some(net)) = (current_rev.decision_root, &session.network) {
            if net.decision_root() != expected {
                return Err(WorldError::NetworkError(
                    "restored decision root mismatch".into(),
                ));
            }
        }
        if policy.refresh_after_replay
            && session.idempotency.source == IdempotencySource::Scan
            && !session.idempotency.is_empty()
        {
            // A world without the index (written before it existed, or whose sidecar was
            // lost): persist the scan's result so the next open does not scan. Best effort.
            let _ = session.persist_idempotency_index();
        }
        session.open_ms = started.elapsed().as_millis() as u64;
        Ok(session)
    }

    /// Re-read the GC generation marker and, when a garbage collection ran since this session
    /// built its store handle, rebuild the handle's view of the packs and drop every cache that
    /// could vouch for a node GC removed: the file store's known set, Bloom filters, idx
    /// handles, raw and decoded caches, and the network's state-store cache. Called under the
    /// writer lock before any write that persists trie nodes (a decoded-cache hit is taken as
    /// proof a node is stored), so a session that was open across a GC re-writes what GC
    /// deleted instead of committing a HEAD whose nodes are gone. Transparent to the caller.
    pub(super) fn sync_gc_epoch(&mut self) -> Result<(), WorldError> {
        let now = super::gc::read_gc_epoch(&self.paths);
        if now != self.gc_epoch {
            self.node_store.reload_from_disk()?;
            if let Some(n) = &self.network {
                n.state_store().clear_cache();
            }
            self.gc_epoch = now;
        }
        Ok(())
    }

    /// Persist the idempotency index and its sidecar for the current HEAD. A cache refresh,
    /// never authoritative, and built not to get in a writer's way: it fails fast with
    /// `WorldLocked` when a writer holds the lock, writes the trie's nodes (content-addressed
    /// and additive, so safe beside a writer) *without* the lock, and takes the lock only to
    /// publish the sidecar, re-checking under it that HEAD and the GC generation are what the
    /// nodes were written for.
    pub fn persist_idempotency_index(&mut self) -> Result<bool, WorldError> {
        drop(WorldLockGuard::acquire(&self.root)?);
        let Some(digest) = self.current_revision_digest else {
            return Ok(false);
        };
        self.sync_gc_epoch()?;
        let epoch = self.gc_epoch.clone();
        if !self.idempotency.persist_nodes(&mut self.node_store)? {
            return Ok(false);
        }
        let _lock = WorldLockGuard::acquire(&self.root)?;
        if super::gc::read_gc_epoch(&self.paths) != epoch {
            return Err(WorldError::StaleSession(
                "garbage collection ran while the idempotency index was written".into(),
            ));
        }
        if self.read_disk_head()? != self.current_revision {
            return Err(WorldError::StaleBaseRevision {
                expected: self.current_revision,
                current: self.read_disk_head()?,
            });
        }
        self.idempotency
            .publish(&self.paths, self.current_revision, digest)
    }

    /// The batch committed under `idempotency_key`, if any.
    pub fn idempotency_lookup(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<CommittedBatchInfo>, WorldError> {
        match self.idempotency.get(idempotency_key, &self.node_store) {
            Ok(found) => Ok(found),
            // A cache: damage is answered from the receipts, never an error.
            Err(_) => {
                IdempotencyIndex::from_scan(&self.paths, &self.node_store, self.current_revision)
                    .get(idempotency_key, &self.node_store)
            }
        }
    }

    /// [`Self::idempotency_lookup`] for a commit: when the persisted index cannot answer (a
    /// missing or damaged trie node, an undecodable entry), rebuild it from the receipts and
    /// ask again. The index is a cache and never authority, so damage to it must not block a
    /// commit or an open. The rebuilt index is private to this session (its nodes may share
    /// digests with the damaged ones, which are never re-written, so it is not published) and
    /// the sidecar is removed so the next open rescans the receipts.
    fn committed_info(&mut self, key: &str) -> Result<Option<CommittedBatchInfo>, WorldError> {
        match self.idempotency.get(key, &self.node_store) {
            Ok(found) => Ok(found),
            Err(_) => {
                // A damaged cache node latches the file store's corruption flag; it is
                // cleared (damage is re-detected on any read of the damaged node).
                self.node_store.clear_corruption_latch();
                let mut rebuilt = IdempotencyIndex::from_scan(
                    &self.paths,
                    &self.node_store,
                    self.current_revision,
                );
                rebuilt.set_ephemeral();
                let _ = fs::remove_file(self.paths.idempotency_file());
                self.idempotency = rebuilt;
                self.idempotency.get(key, &self.node_store)
            }
        }
    }

    fn node_cache_budget(&self) -> usize {
        manifest_node_cache_budget(&self.manifest)
    }

    /// The next up-to-`n` base facts (as upserts) of the relations `network` declares, in
    /// relation then key order, continuing from `cursor` (`(relation index, key cursor)`).
    fn next_base_facts(
        &self,
        network: &WorldNetwork,
        cursor: &mut (usize, Option<([u8; 32], WorldKey)>),
        n: usize,
    ) -> Result<Vec<WorldBatchOp>, WorldError> {
        let rels: Vec<(&String, &TrieMap<WorldKey, WorldTuple>)> = self
            .relations
            .iter()
            .filter(|(name, _)| network.base_relations.contains_key(*name))
            .collect();
        let mut out = Vec::with_capacity(n.min(1 << 16));
        while out.len() < n && cursor.0 < rels.len() {
            let (name, trie) = rels[cursor.0];
            let want = n - out.len();
            let (entries, next) = trie.iter_page_with_store(
                cursor.1.as_ref().map(|(h, k)| (h, k)),
                want,
                &self.node_store,
            )?;
            for (key, tuple) in entries {
                out.push(WorldBatchOp::Upsert {
                    relation: name.clone(),
                    key,
                    tuple,
                });
            }
            match next {
                Some(c) => cursor.1 = Some(c),
                None => *cursor = (cursor.0 + 1, None),
            }
        }
        Ok(out)
    }

    /// Replay every base fact the session holds into `network` as one batch. A world big
    /// enough to exceed the hydrated-bytes budget is applied in chunks, persisting and
    /// dehydrating the network between them and streaming the facts from the base tries
    /// instead of materialising all of them (same output as one `apply_ops`).
    fn apply_base_facts(&self, network: &mut WorldNetwork) -> Result<(), WorldError> {
        let total: usize = self
            .relations
            .iter()
            .filter(|(name, _)| network.base_relations.contains_key(*name))
            .map(|(_, t)| t.len())
            .sum();
        if total == 0 {
            return Ok(());
        }
        let budget = self
            .manifest
            .apply_hydrated_bytes
            .unwrap_or(DEFAULT_APPLY_HYDRATED_BYTES);
        let mut input = ChunkInput {
            ops_total: total,
            ops_done: 0,
            prev_chunk_ops: 0,
            prev_nodes_written: 0,
            pending_index_bytes: 0,
            budget_bytes: budget,
        };
        let mut decision = self.chunk_policy.chunk_plan(&input);
        let mut cursor = (0usize, None);
        if decision.ops >= total {
            let ops = self.next_base_facts(network, &mut cursor, total)?;
            network.apply_ops(&ops)?;
            return Ok(());
        }
        let pristine = network.clone();
        let chunked: Result<(), WorldError> = (|| {
            let limits = super::network::NetworkLimits::default();
            let mut ctx = network.begin_batch();
            loop {
                let remaining = total - input.ops_done;
                let n = decision.ops.clamp(1, remaining);
                let ops = self.next_base_facts(network, &mut cursor, n)?;
                network.apply_chunk(&mut ctx, &ops, &limits, None)?;
                input.ops_done += n;
                input.prev_chunk_ops = n;
                if input.ops_done >= total {
                    break;
                }
                input.prev_nodes_written = network.persist_and_dehydrate_in_place()?;
                input.pending_index_bytes =
                    network.state_store().pending_stats().0 as u64 * PENDING_INDEX_BYTES_PER_NODE;
                decision = self.chunk_policy.chunk_plan(&input);
                if decision.flush_pack {
                    network.state_store().flush()?;
                }
            }
            network.finish_batch_cascade(ctx, &limits, None)?;
            Ok(())
        })();
        match chunked {
            Ok(()) => Ok(()),
            Err(e) if is_environmental(&e) => Err(e),
            Err(_) => {
                // As in a commit: the whole-world application is authoritative.
                *network = pristine;
                let mut cursor = (0usize, None);
                let ops = self.next_base_facts(network, &mut cursor, total)?;
                network.apply_ops(&ops)?;
                Ok(())
            }
        }
    }

    /// The validity key under which this binary would persist (and accept) state of
    /// `network` in this world. The network contributes its own structural fingerprint, so
    /// state is never filed under a program other than the one that computed it.
    pub fn validity_key_for(&self, network: &WorldNetwork) -> ValidityKey {
        ValidityKey::current(
            self.manifest.program_digest,
            self.exec_profile.as_ref().map(ExecProfileV1::digest),
            network.fingerprint(),
        )
    }

    /// Install the state `root` names into `network`, the single restore path for both a
    /// revision's own `network_state_root` and the state-cache sidecar. `Ok(None)`: restored
    /// and installed. `Ok(Some(why))`: intact but written under another validity key, so
    /// the caller rebuilds. Corrupt, missing or inconsistent state is a hard error.
    fn restore_from_header(
        &mut self,
        network: &mut WorldNetwork,
        rev: &WorldRevision,
        root: Digest,
        source: &'static str,
    ) -> Result<Option<&'static str>, WorldError> {
        let store = StateStore::from_file(
            self.node_store.clone(),
            state_cache_share(self.node_cache_budget()),
        );
        let key = self.validity_key_for(network);
        match NetworkStateHeader::load(&store, &root, &key)? {
            // Produced under another schema, execution profile, program structure or
            // codec/lowering version: sound but not reusable by this binary.
            Loaded::Stale(why) => Ok(Some(why)),
            Loaded::Header(header) => {
                let decision_ok = match rev.decision_root {
                    Some(expected) => header.decision.root == expected,
                    None => header.decision.len == 0,
                };
                if !decision_ok {
                    return Err(WorldError::ValidityMismatch(
                        "persisted network state decision root does not match the revision".into(),
                    ));
                }
                network.attach_state_store(store);
                network.restore_state(&header)?;
                network.current_revision = self.current_revision;
                self.open_mode = OpenMode::Restored;
                self.replay_reason = None;
                self.state_source = Some(source);
                self.tail_batches = 0;
                Ok(None)
            }
        }
    }

    /// Give a freshly built network its state: restore it from the persisted header the
    /// revision names (or, failing that, the state-cache sidecar), or replay the base facts.
    fn install_network(
        &mut self,
        mut network: WorldNetwork,
        rev: &WorldRevision,
        policy: OpenPolicy,
    ) -> Result<(), WorldError> {
        let reason: &'static str = if policy.force_replay {
            "forced"
        } else {
            let reason = match rev.network_state_root {
                Some(root) => {
                    match self.restore_from_header(&mut network, rev, root, "revision")? {
                        None => {
                            self.network = Some(network);
                            return Ok(());
                        }
                        Some(why) => why,
                    }
                }
                None if rev.schema == REVISION_SCHEMA_V3 => {
                    // A revision that claims restorable state must name it; treating the
                    // absence as a legacy world would hide a damaged or truncated record.
                    return Err(WorldError::InvalidSchema(
                        "revision@3 of an executable world has no network_state_root".into(),
                    ));
                }
                None => "legacy-revision",
            };
            // The sidecar: a cache hit only for exactly this revision under exactly this key.
            let key_digest = self.validity_key_for(&network).digest();
            if let Some(c) = StateCache::read(&self.paths.state_cache_file()) {
                if c.revision_digest == rev.revision_digest && c.validity_key_digest == key_digest {
                    let root = c.network_state_root;
                    if self
                        .restore_from_header(&mut network, rev, root, "sidecar")?
                        .is_none()
                    {
                        self.network = Some(network);
                        return Ok(());
                    }
                }
            }
            reason
        };
        self.set_network(network)?;
        self.replay_reason = Some(reason);
        self.state_source = None;
        if !policy.force_replay && policy.refresh_after_replay {
            match self.refresh_state_cache_inner(false) {
                Ok(()) => self.state_refreshed = true,
                Err(e) => self.state_refresh_error = Some(e.to_string()),
            }
        }
        Ok(())
    }

    /// Persist the current network state and point `state-cache.json` at it, so the next
    /// open of this revision restores instead of replaying. Non-blocking: a world another
    /// writer holds is `WorldLocked`. The sidecar is renamed into place only after the
    /// state is flushed and with HEAD re-checked under the lock; it is a cache, never
    /// authoritative.
    pub fn refresh_state_cache(&mut self) -> Result<(), WorldError> {
        self.refresh_state_cache_inner(true)
    }

    /// `hold_lock`: the explicit operator refresh keeps the writer lock for the whole
    /// operation. Without it (the opportunistic refresh an `open` performs after a replay,
    /// which may persist gigabytes at scale) the lock is only probed up front (a writer
    /// holding it means skip, as `WorldLocked`) and taken again for the final HEAD re-check
    /// and the sidecar rename; the state nodes themselves are content-addressed additions that
    /// need no exclusion, and a GC generation change in between abandons the refresh.
    fn refresh_state_cache_inner(&mut self, hold_lock: bool) -> Result<(), WorldError> {
        let held = if hold_lock {
            Some(WorldLockGuard::acquire(&self.root)?)
        } else {
            drop(WorldLockGuard::acquire(&self.root)?);
            None
        };
        self.sync_gc_epoch()?;
        let epoch = self.gc_epoch.clone();
        let Some(net) = self.network.as_ref() else {
            return Err(WorldError::NetworkError("world has no network".into()));
        };
        net.persist_state()?;
        let key = self.validity_key_for(net);
        let root = net.state_header(key.clone()).write(net.state_store());
        if self.crash_injector == Some(CrashPoint::SidecarBeforeFlush) {
            return Err(WorldError::InjectedCrash(CrashPoint::SidecarBeforeFlush));
        }
        self.node_store.flush()?;
        let _relock = match held {
            Some(_) => None,
            None => Some(WorldLockGuard::acquire(&self.root)?),
        };
        if super::gc::read_gc_epoch(&self.paths) != epoch {
            return Err(WorldError::StaleSession(
                "garbage collection ran while the state cache was written".into(),
            ));
        }
        let disk_head = self.read_disk_head()?;
        let Some(revision_digest) = self.current_revision_digest else {
            return Err(WorldError::CorruptedHead(
                "no current revision digest".into(),
            ));
        };
        if disk_head != self.current_revision {
            return Err(WorldError::StaleBaseRevision {
                expected: self.current_revision,
                current: disk_head,
            });
        }
        let cache = StateCache {
            revision_seq: self.current_revision,
            revision_digest,
            validity_key_digest: key.digest(),
            network_state_root: root,
        };
        cache.write_tmp(&self.paths.state_cache_tmp())?;
        if self.crash_injector == Some(CrashPoint::SidecarAfterFlushBeforeRename) {
            return Err(WorldError::InjectedCrash(
                CrashPoint::SidecarAfterFlushBeforeRename,
            ));
        }
        fs::rename(self.paths.state_cache_tmp(), self.paths.state_cache_file())?;
        fs::File::open(&self.root)?.sync_all()?;
        if self.crash_injector == Some(CrashPoint::SidecarAfterRename) {
            return Err(WorldError::InjectedCrash(CrashPoint::SidecarAfterRename));
        }
        Ok(())
    }

    pub fn set_crash_point(&mut self, cp: Option<CrashPoint>) {
        self.crash_injector = cp;
    }

    pub fn current_revision(&self) -> u64 {
        self.current_revision
    }

    pub fn current_revision_digest(&self) -> Option<Digest> {
        self.current_revision_digest
    }

    pub fn manifest(&self) -> &WorldManifest {
        &self.manifest
    }

    fn read_disk_head(&self) -> Result<u64, WorldError> {
        if !self.paths.head().exists() {
            return Err(WorldError::CorruptedHead("HEAD file missing".to_string()));
        }
        let head_str = fs::read_to_string(self.paths.head())?;
        let head_line = head_str.lines().next().unwrap_or("").trim();
        let current_revision: u64 = head_line
            .split_whitespace()
            .next()
            .ok_or_else(|| WorldError::CorruptedHead("empty HEAD".to_string()))?
            .parse()
            .map_err(|_| WorldError::CorruptedHead("invalid integer in HEAD".to_string()))?;
        Ok(current_revision)
    }

    /// Read a record by key from a relation.
    pub fn get(&self, relation: &str, key: &WorldKey) -> Result<Option<WorldTuple>, WorldError> {
        let trie = self
            .relations
            .get(relation)
            .ok_or_else(|| WorldError::UnknownRelation(relation.to_string()))?;
        let val = trie.get_with_store(key, &self.node_store)?;
        Ok(val)
    }

    /// Collect every matching primary key. For bounded memory, use the paged API.
    /// `indexed_value` must be produced by [`encode_secondary_key`].
    pub fn query_secondary_index(
        &self,
        relation: &str,
        index_field: &str,
        indexed_value: &WorldKey,
    ) -> Result<Vec<WorldKey>, WorldError> {
        let mut keys = Vec::new();
        let mut cursor = None;
        loop {
            let page = self.query_secondary_index_page(
                relation,
                index_field,
                indexed_value,
                cursor.as_ref(),
                1024,
            )?;
            keys.extend(page.keys);
            cursor = page.next_cursor;
            if cursor.is_none() {
                return Ok(keys);
            }
        }
    }

    /// Read a bounded page using an explicitly encoded secondary key.
    /// A continuation from an earlier revision is rejected; pin a snapshot to page
    /// across concurrent commits without changing the result set.
    pub fn query_secondary_index_page(
        &self,
        relation: &str,
        index_field: &str,
        indexed_value: &WorldKey,
        cursor: Option<&SecondaryIndexCursor>,
        limit: usize,
    ) -> Result<SecondaryIndexPage, WorldError> {
        let revision = self
            .current_revision_digest
            .ok_or_else(|| WorldError::CorruptedHead("current revision digest missing".into()))?;
        secondary_index_page(
            &self.secondary_indexes,
            &self.node_store,
            revision,
            (relation, index_field, indexed_value),
            cursor,
            limit,
        )
    }

    /// Apply a transactional mutation batch.
    pub fn apply_batch(&mut self, batch: WorldBatch) -> Result<RevisionReceipt, WorldError> {
        self.apply_batch_bounded(batch, None)
    }

    /// Apply a transactional mutation batch with explicit execution resource bounds.
    pub fn apply_batch_bounded(
        &mut self,
        batch: WorldBatch,
        limits: Option<&super::network::NetworkLimits>,
    ) -> Result<RevisionReceipt, WorldError> {
        let _lock = WorldLockGuard::acquire_wait(&self.root, COMMIT_LOCK_WAIT)?;
        self.sync_gc_epoch()?;
        let commit_started = Instant::now();
        let io_before = self
            .network
            .as_ref()
            .and_then(|n| n.state_store().io_stats());
        let mut phases: Vec<(&'static str, f64)> = Vec::with_capacity(10);
        let mut phase_started = Instant::now();
        macro_rules! phase {
            ($name:expr) => {{
                phases.push(($name, ms_since(phase_started)));
                phase_started = Instant::now();
            }};
        }

        // Verify disk HEAD matches in-memory current_revision
        let disk_head = self.read_disk_head()?;
        if disk_head != self.current_revision {
            return Err(WorldError::StaleBaseRevision {
                expected: batch.expected_base_revision,
                current: disk_head,
            });
        }

        // 1. Check idempotency key
        let batch_digest = batch.digest();
        if let Some(info) = self.committed_info(&batch.idempotency_key)? {
            if info.batch_digest == Some(batch_digest) {
                return Ok(RevisionReceipt {
                    revision_seq: info.seq,
                    revision_digest: info.revision_digest,
                    idempotency_key: batch.idempotency_key,
                    is_idempotent_replay: true,
                    changed_keys_count: 0,
                    objects_written: 0,
                });
            } else {
                return Err(WorldError::IdempotencyConflict {
                    key: batch.idempotency_key,
                    reason: "idempotency key reused with different batch payload".to_string(),
                });
            }
        }

        // 2. Validate expected base revision
        if batch.expected_base_revision != self.current_revision {
            return Err(WorldError::StaleBaseRevision {
                expected: batch.expected_base_revision,
                current: self.current_revision,
            });
        }

        // 3. Normalize operations and reject conflicting duplicate operations
        let normalized_ops = batch.validate_and_normalize()?;

        let mut chunk_run = ChunkRun::single();
        // Stage operator network updates and deliberation if network is present
        let (staged_network, decision_root, decision_delta_digest, decision_delta_body) =
            if let Some(ref net) = self.network {
                let mut sn = net.clone();
                phase!("clone_network");
                // Pre-batch facts for a network that keeps no copy of its own.
                let base_relations = &self.relations;
                let base_store = &self.node_store;
                let lookup =
                    |rel: &str, key: &WorldKey| -> Result<Option<WorldTuple>, WorldError> {
                        let trie = base_relations
                            .get(rel)
                            .ok_or_else(|| WorldError::UnknownRelation(rel.to_string()))?;
                        Ok(trie.get_with_store(key, base_store)?)
                    };
                let hydrated_budget = self
                    .manifest
                    .apply_hydrated_bytes
                    .unwrap_or(DEFAULT_APPLY_HYDRATED_BYTES);
                let (sn_applied, delta_report, run) = apply_ops_chunked(
                    sn,
                    net,
                    &normalized_ops,
                    limits,
                    &lookup,
                    self.chunk_policy.as_ref(),
                    hydrated_budget,
                    self.crash_injector,
                )?;
                sn = sn_applied;
                chunk_run = run;
                phase!("apply_ops");
                if sn.decides.is_empty() {
                    (Some(sn), None, None, None)
                } else {
                    let d_root = Some(sn.decision_root());
                    let body = super::network::encode_decision_delta(
                        &delta_report.settlements,
                        &delta_report.removed_settlements,
                    );
                    let d_delta_digest = Some(Digest::of(Domain::Value, &body));
                    (Some(sn), d_root, d_delta_digest, Some(body))
                }
            } else {
                (None, None, None, None)
            };

        phase!("decision_encode");
        // 4. Stage updates across relations and maintain changed keys
        let mut staged_relations = self.relations.clone();
        let mut staged_secondary_indexes = self.secondary_indexes.clone();
        let mut changed_keys: BTreeMap<String, Vec<WorldKey>> = BTreeMap::new();

        let mut inner_objects_written = 0;
        for op in normalized_ops {
            let rel_name = op.relation().to_string();
            let decl = match self.manifest.relations.get(&rel_name) {
                Some(d) => d,
                None => return Err(WorldError::UnknownRelation(rel_name)),
            };

            let trie = staged_relations
                .get_mut(&rel_name)
                .expect("relation in manifest");

            match op {
                WorldBatchOp::Upsert { key, tuple, .. } => {
                    let current_val = trie.get_with_store(&key, &self.node_store)?;
                    if current_val.as_ref() == Some(&tuple) {
                        // Value is identical: no-op!
                        continue;
                    }

                    // Update secondary indexes if any
                    for idx in &decl.secondary_indexes {
                        let sec_name = format!("{}:{}", rel_name, idx);
                        if let Some(sec_trie) = staged_secondary_indexes.get_mut(&sec_name) {
                            let old_field_opt = match &current_val {
                                Some(old_t) => {
                                    Some(extract_indexed_field(&rel_name, decl, idx, old_t)?)
                                }
                                None => None,
                            };
                            let new_field = extract_indexed_field(&rel_name, decl, idx, &tuple)?;

                            // Skip maintenance when projected key is unchanged!
                            if old_field_opt.as_ref() == Some(&new_field) {
                                continue;
                            }

                            // 1. If old field exists, remove key from old customer's set
                            if let Some(old_val) = old_field_opt {
                                let old_k = encode_secondary_key(&old_val);
                                if let Some(old_set_tuple) =
                                    sec_trie.get_with_store(&old_k, &self.node_store)?
                                {
                                    if old_set_tuple.as_bytes().len() == 32 {
                                        let mut d = [0u8; 32];
                                        d.copy_from_slice(old_set_tuple.as_bytes());
                                        let old_digest = Digest::from_bytes(d);
                                        let old_set =
                                            TrieMap::<WorldKey, WorldTuple>::from_root_digest(
                                                old_digest, 0,
                                            );
                                        let (next_old_set, _) =
                                            old_set.remove_with_store(&key, &self.node_store)?;
                                        inner_objects_written +=
                                            next_old_set.persist_to_store(&mut self.node_store);

                                        let empty_d = Digest::of(
                                            brix_canon::Domain::Value,
                                            b"brix:trie:empty:v1",
                                        );
                                        if next_old_set.is_empty()
                                            || next_old_set.root_digest() == empty_d
                                        {
                                            let (next_sec, _) = sec_trie
                                                .remove_with_store(&old_k, &self.node_store)?;
                                            *sec_trie = next_sec;
                                        } else {
                                            let next_set_root = next_old_set.root_digest();
                                            let (next_sec, _) = sec_trie.insert_with_store(
                                                old_k,
                                                WorldTuple::new(next_set_root.as_bytes().to_vec()),
                                                &self.node_store,
                                            )?;
                                            *sec_trie = next_sec;
                                        }
                                    }
                                }
                            }

                            // 2. Insert key into new customer's set
                            let new_k = encode_secondary_key(&new_field);
                            let current_set =
                                match sec_trie.get_with_store(&new_k, &self.node_store)? {
                                    Some(set_tuple) if set_tuple.as_bytes().len() == 32 => {
                                        let mut d = [0u8; 32];
                                        d.copy_from_slice(set_tuple.as_bytes());
                                        let digest = Digest::from_bytes(d);
                                        TrieMap::<WorldKey, WorldTuple>::from_root_digest(digest, 0)
                                    }
                                    _ => TrieMap::<WorldKey, WorldTuple>::new(),
                                };
                            let (next_set, _) = current_set.insert_with_store(
                                key.clone(),
                                WorldTuple::new(vec![]),
                                &self.node_store,
                            )?;
                            inner_objects_written +=
                                next_set.persist_to_store(&mut self.node_store);
                            let next_set_root = next_set.root_digest();
                            let (next_sec, _) = sec_trie.insert_with_store(
                                new_k,
                                WorldTuple::new(next_set_root.as_bytes().to_vec()),
                                &self.node_store,
                            )?;
                            *sec_trie = next_sec;
                        }
                    }

                    let (next_trie, _) =
                        trie.insert_with_store(key.clone(), tuple, &self.node_store)?;
                    *trie = next_trie;
                    changed_keys.entry(rel_name).or_default().push(key);
                }
                WorldBatchOp::Remove { key, .. } => {
                    let current_val = trie.get_with_store(&key, &self.node_store)?;
                    if current_val.is_none() {
                        // Key does not exist: no-op!
                        continue;
                    }

                    if let Some(ref old_t) = current_val {
                        for idx in &decl.secondary_indexes {
                            let sec_name = format!("{}:{}", rel_name, idx);
                            if let Some(sec_trie) = staged_secondary_indexes.get_mut(&sec_name) {
                                let old_val = extract_indexed_field(&rel_name, decl, idx, old_t)?;
                                let old_k = encode_secondary_key(&old_val);
                                if let Some(old_set_tuple) =
                                    sec_trie.get_with_store(&old_k, &self.node_store)?
                                {
                                    if old_set_tuple.as_bytes().len() == 32 {
                                        let mut d = [0u8; 32];
                                        d.copy_from_slice(old_set_tuple.as_bytes());
                                        let old_digest = Digest::from_bytes(d);
                                        let old_set =
                                            TrieMap::<WorldKey, WorldTuple>::from_root_digest(
                                                old_digest, 0,
                                            );
                                        let (next_old_set, _) =
                                            old_set.remove_with_store(&key, &self.node_store)?;
                                        inner_objects_written +=
                                            next_old_set.persist_to_store(&mut self.node_store);

                                        let empty_d = Digest::of(
                                            brix_canon::Domain::Value,
                                            b"brix:trie:empty:v1",
                                        );
                                        if next_old_set.is_empty()
                                            || next_old_set.root_digest() == empty_d
                                        {
                                            let (next_sec, _) = sec_trie
                                                .remove_with_store(&old_k, &self.node_store)?;
                                            *sec_trie = next_sec;
                                        } else {
                                            let next_set_root = next_old_set.root_digest();
                                            let (next_sec, _) = sec_trie.insert_with_store(
                                                old_k,
                                                WorldTuple::new(next_set_root.as_bytes().to_vec()),
                                                &self.node_store,
                                            )?;
                                            *sec_trie = next_sec;
                                        }
                                    }
                                }
                            }
                        }
                    }

                    let (next_trie, _) = trie.remove_with_store(&key, &self.node_store)?;
                    *trie = next_trie;
                    changed_keys.entry(rel_name).or_default().push(key);
                }
            }
        }

        phase!("stage_base");
        // 5. Persist modified trie nodes to node store
        let mut objects_written = inner_objects_written + chunk_run.objects;
        for trie in staged_relations.values() {
            objects_written += trie.persist_to_store(&mut self.node_store);
        }
        let mut secondary_index_roots = BTreeMap::new();
        for (sec_name, trie) in &staged_secondary_indexes {
            objects_written += trie.persist_to_store(&mut self.node_store);
            secondary_index_roots.insert(sec_name.clone(), trie.root_digest());
        }

        phase!("persist_base");
        // Crash injection seam: base objects written, no network object yet.
        if self.crash_injector == Some(CrashPoint::AfterBaseObjectsBeforeNetworkObjects) {
            return Err(WorldError::InjectedCrash(
                CrashPoint::AfterBaseObjectsBeforeNetworkObjects,
            ));
        }
        // Persist the settled-decision trie's own nodes (ADR-0046 P6 G2,
        // decided 2026-10-04): previously only its root digest reached the
        // revision record, so historical settlements could not be read back
        // without replaying every base record. Same persistence seam as the
        // relation/secondary-index tries above — same crash-recovery story.
        if let Some(ref sn) = staged_network {
            // Operator state, derived relations, the candidate frontier and the settled
            // decision trie, through the one state store (same pack, same flush).
            objects_written += sn.persist_state()?;
        }
        phase!("persist_state");
        // The header that makes the persisted network state restorable, written in the
        // same pack as everything it names and flushed with it, before HEAD can move.
        let network_state_root = staged_network.as_ref().map(|sn| {
            objects_written += 1;
            sn.state_header(self.validity_key_for(sn))
                .write(sn.state_store())
        });

        let new_seq = self.current_revision + 1;
        // 6. Assemble revision record
        let mut relation_roots = BTreeMap::new();
        let mut relation_cardinalities = BTreeMap::new();
        for (rel, trie) in &staged_relations {
            relation_roots.insert(rel.clone(), trie.root_digest());
            relation_cardinalities.insert(rel.clone(), trie.len());
        }

        let new_revision = WorldRevision::new(
            new_seq,
            iso_now(),
            self.current_revision,
            &batch.idempotency_key,
            Some(batch_digest),
            self.manifest
                .program_required
                .then_some(self.manifest.program_digest),
            self.current_revision_digest,
            relation_roots,
            relation_cardinalities,
            secondary_index_roots,
            decision_root,
            changed_keys.clone(),
            SettlementStatus::Committed,
            decision_delta_digest,
        )
        .bind_exec_profile(self.exec_profile.as_ref().map(ExecProfileV1::digest));
        let new_revision = match network_state_root {
            Some(root) => new_revision.with_network_state_root(root),
            None => new_revision,
        };

        // The idempotency index with this commit's key, its nodes written into the same pack
        // and flushed with everything else. Staged on a copy: it replaces the session's only
        // once the commit is published. (The revision digest does not depend on the flush.)
        let mut next_idempotency = self.idempotency.clone();
        next_idempotency.record(
            &batch.idempotency_key,
            CommittedBatchInfo {
                seq: new_seq,
                revision_digest: new_revision.revision_digest,
                batch_digest: Some(batch_digest),
            },
            &self.node_store,
        );
        objects_written += next_idempotency.stage(&mut self.node_store);

        phase!("state_header");
        // Crash injection seam: every object written, none flushed.
        if self.crash_injector == Some(CrashPoint::AfterNetworkObjectsBeforeFlush) {
            return Err(WorldError::InjectedCrash(
                CrashPoint::AfterNetworkObjectsBeforeFlush,
            ));
        }

        // Crash injection seam 1
        if self.crash_injector == Some(CrashPoint::BeforeObjectsFsync) {
            return Err(WorldError::InjectedCrash(CrashPoint::BeforeObjectsFsync));
        }

        // Fsync newly written objects
        self.node_store.flush()?;
        phase!("flush");

        // Crash injection seam 2
        if self.crash_injector == Some(CrashPoint::AfterObjectsFsyncBeforeRevisionFsync) {
            return Err(WorldError::InjectedCrash(
                CrashPoint::AfterObjectsFsyncBeforeRevisionFsync,
            ));
        }

        // Write the decision-delta body durably BEFORE the revision record
        // (ADR-0046 P6 G2/G3, decided 2026-10-04): tmp-write, fsync, rename —
        // the same atomic-write discipline as the revision record itself, one
        // step earlier in the publication order. An uncommitted attempt may
        // leave an orphaned `{seq}.decisions` file; harmless, since nothing
        // ever reads one without the matching revision record's digest.
        if let Some(ref body) = decision_delta_body {
            let tmp_delta_file = self.paths.decision_delta_tmp_file(new_seq);
            fs::write(&tmp_delta_file, body)?;
            {
                let f = fs::File::open(&tmp_delta_file)?;
                f.sync_all()?;
            }
            fs::rename(&tmp_delta_file, self.paths.decision_delta_file(new_seq))?;
            fs::File::open(self.paths.revisions_dir())?.sync_all()?;
        }

        if self.crash_injector == Some(CrashPoint::AfterDecisionDeltaFsyncBeforeRevisionFsync) {
            return Err(WorldError::InjectedCrash(
                CrashPoint::AfterDecisionDeltaFsyncBeforeRevisionFsync,
            ));
        }
        let rev_json = serde_json::to_vec_pretty(&new_revision.to_json())
            .map_err(|e| WorldError::Json(e.to_string()))?;

        let tmp_rev_file = self.paths.revision_tmp_file(new_seq);
        fs::write(&tmp_rev_file, rev_json)?;
        {
            let f = fs::File::open(&tmp_rev_file)?;
            f.sync_all()?;
        }
        fs::rename(&tmp_rev_file, self.paths.revision_file(new_seq))?;
        fs::File::open(self.paths.revisions_dir())?.sync_all()?;

        // Durable idempotency receipt (preserved independently across compaction)
        let receipt_info = serde_json::json!({
            "seq": new_seq,
            "idempotency_key": batch.idempotency_key,
            "revision_digest": new_revision.revision_digest.to_hex(),
            "batch_digest": batch_digest.to_hex(),
        });
        let receipt_json = serde_json::to_vec_pretty(&receipt_info)
            .map_err(|e| WorldError::Json(e.to_string()))?;
        let tmp_receipt_file = self.paths.receipt_tmp_file(new_seq);
        fs::write(&tmp_receipt_file, receipt_json)?;
        {
            let f = fs::File::open(&tmp_receipt_file)?;
            f.sync_all()?;
        }
        fs::rename(&tmp_receipt_file, self.paths.receipt_file(new_seq))?;
        fs::File::open(self.paths.receipts_dir())?.sync_all()?;

        // Crash injection seam 3
        if self.crash_injector == Some(CrashPoint::AfterRevisionFsyncBeforeHeadRename) {
            return Err(WorldError::InjectedCrash(
                CrashPoint::AfterRevisionFsyncBeforeHeadRename,
            ));
        }

        // 7. Atomically publish HEAD pointer
        let head_content = format!("{} {}\n", new_seq, new_revision.revision_digest.to_hex());
        fs::write(self.paths.head_tmp(), &head_content)?;
        {
            let f = fs::File::open(self.paths.head_tmp())?;
            f.sync_all()?;
        }
        fs::rename(self.paths.head_tmp(), self.paths.head())?;

        // Crash injection seam 4
        if self.crash_injector == Some(CrashPoint::AfterHeadRenameBeforeDirectoryFsync) {
            return Err(WorldError::InjectedCrash(
                CrashPoint::AfterHeadRenameBeforeDirectoryFsync,
            ));
        }

        // Fsync parent directory
        fs::File::open(&self.root)?.sync_all()?;

        phase!("publish");
        // 8. Update in-memory session state
        let total_changed: usize = changed_keys.values().map(|v| v.len()).sum();
        self.current_revision = new_seq;
        self.current_revision_digest = Some(new_revision.revision_digest);
        self.network_state_root = new_revision.network_state_root;
        // Both were persisted above; keep only (root, len) so the next commit does not
        // re-probe unchanged hydrated siblings and the session does not pin the base copy.
        self.relations = dehydrate_base(staged_relations);
        self.secondary_indexes = dehydrate_base(staged_secondary_indexes);
        if let Some(sn) = staged_network {
            // The commit is already durable: dehydrating must never fail it. On the
            // (unexpected) error keep the hydrated network, which is merely larger.
            self.network = Some(match sn.dehydrated() {
                Ok(d) => d,
                Err(_) => {
                    self.dehydrate_failures += 1;
                    debug_assert!(false, "dehydrate after durable commit failed");
                    sn
                }
            });
        }
        self.idempotency = next_idempotency;
        // Best effort, after the commit is durable (its nodes were flushed above): a failure
        // only leaves the sidecar behind HEAD, which the next open catches up from receipts.
        let _ = self
            .idempotency
            .publish(&self.paths, new_seq, new_revision.revision_digest);

        phases.push(("dehydrate", ms_since(phase_started)));
        let io_after = self
            .network
            .as_ref()
            .and_then(|n| n.state_store().io_stats());
        let (reads, read_bytes, written_bytes) = match (io_before, io_after) {
            (Some(b), Some(a)) => (
                a.reads.saturating_sub(b.reads),
                a.bytes_read.saturating_sub(b.bytes_read),
                a.bytes_written.saturating_sub(b.bytes_written),
            ),
            _ => (0, 0, 0),
        };
        self.last_commit_profile = Some(CommitProfile {
            phases,
            total_ms: ms_since(commit_started),
            state_node_reads: reads,
            state_bytes_read: read_bytes,
            state_bytes_written: written_bytes,
            chunks: chunk_run.chunks,
            chunk_persist_ms: chunk_run.persist_ms,
            chunk_fallback: chunk_run.fallback,
        });
        Ok(RevisionReceipt {
            revision_seq: new_seq,
            revision_digest: new_revision.revision_digest,
            idempotency_key: batch.idempotency_key,
            is_idempotent_replay: false,
            changed_keys_count: total_changed,
            objects_written,
        })
    }

    /// Pin an immutable read snapshot at revision `seq`.
    pub fn pin_revision(&self, seq: u64) -> Result<WorldSnapshot, WorldError> {
        if seq > self.current_revision {
            return Err(WorldError::RevisionNotFound(seq));
        }
        let rev_file = self.paths.revision_file(seq);
        if !rev_file.exists() {
            return Err(WorldError::RevisionNotFound(seq));
        }
        // History a `--keep-revisions` GC dropped is not damage: say so.
        if seq != 0
            && seq < super::gc::read_gc_floor(&self.paths)
            && !self.pins.checkpoint_pins().contains(&seq)
        {
            return Err(WorldError::RevisionCompacted(seq));
        }
        let rev_bytes = fs::read(rev_file)?;
        let rev_val: serde_json::Value =
            serde_json::from_slice(&rev_bytes).map_err(|e| WorldError::Json(e.to_string()))?;
        let rev = WorldRevision::from_json(&rev_val)?;
        if rev.seq != seq {
            return Err(WorldError::InvalidSchema(format!(
                "revision file {seq} contains revision {}",
                rev.seq
            )));
        }
        if self.manifest.program_required
            && rev.program_digest != Some(self.manifest.program_digest)
        {
            return Err(WorldError::InvalidSchema(format!(
                "revision {seq} is not bound to the world's executable program digest"
            )));
        }

        let mut relations = BTreeMap::new();
        for name in self.manifest.relations.keys() {
            let root_digest = rev.relation_roots.get(name).copied();
            let card = rev.relation_cardinalities.get(name).copied().unwrap_or(0);
            match root_digest {
                Some(digest) => {
                    relations.insert(name.clone(), TrieMap::from_root_digest(digest, card));
                }
                None => {
                    relations.insert(name.clone(), TrieMap::new());
                }
            }
        }

        let mut secondary_indexes = BTreeMap::new();
        for (sec_name, &digest) in &rev.secondary_index_roots {
            secondary_indexes.insert(sec_name.clone(), TrieMap::from_root_digest(digest, 0));
        }

        Ok(WorldSnapshot {
            world_id: self.manifest.world_id.clone(),
            revision: rev,
            relations,
            secondary_indexes,
            node_store: self.node_store.clone(),
        })
    }

    /// Pin a revision for an active reader, returning an opaque pin handle ID.
    pub fn pin_reader(&mut self, seq: u64) -> Result<u64, WorldError> {
        if seq > self.current_revision || !self.paths.revision_file(seq).exists() {
            return Err(WorldError::RevisionNotFound(seq));
        }
        Ok(self.pins.pin_reader(seq))
    }

    /// Release an active reader pin.
    pub fn unpin_reader(&mut self, pin_id: u64) -> bool {
        self.pins.unpin_reader(pin_id)
    }

    fn persist_pins(&self) -> Result<(), WorldError> {
        let list: Vec<u64> = self.pins.checkpoint_pins().iter().copied().collect();
        let bytes =
            serde_json::to_vec_pretty(&list).map_err(|e| WorldError::Json(e.to_string()))?;
        let tmp = self.paths.pins_tmp_file();
        fs::write(&tmp, bytes)?;
        {
            let f = fs::File::open(&tmp)?;
            f.sync_all()?;
        }
        fs::rename(&tmp, self.paths.pins_file())?;
        fs::File::open(&self.root)?.sync_all()?;
        Ok(())
    }

    /// Pin a revision as an audit checkpoint or evidence anchor.
    pub fn pin_checkpoint(&mut self, seq: u64) -> Result<(), WorldError> {
        if seq > self.current_revision || !self.paths.revision_file(seq).exists() {
            return Err(WorldError::RevisionNotFound(seq));
        }
        self.pins.pin_checkpoint(seq);
        self.persist_pins()?;
        Ok(())
    }

    /// Release a checkpoint pin.
    pub fn unpin_checkpoint(&mut self, seq: u64) -> bool {
        let removed = self.pins.unpin_checkpoint(seq);
        if removed {
            let _ = self.persist_pins();
        }
        removed
    }

    /// Check whether a revision is currently protected from compaction.
    pub fn is_revision_pinned(&self, seq: u64) -> bool {
        seq == 0 || seq == self.current_revision || self.pins.is_pinned(seq)
    }

    /// Measure total retained history bytes across all revision records and decision deltas.
    pub fn measure_retained_bytes(&self) -> Result<u64, WorldError> {
        let mut total_bytes = 0u64;
        let rev_dir = self.paths.revisions_dir();
        if rev_dir.exists() {
            for entry in fs::read_dir(&rev_dir)? {
                let entry = entry?;
                if entry.file_type()?.is_file() {
                    total_bytes += entry.metadata()?.len();
                }
            }
        }
        Ok(total_bytes)
    }

    /// Compact historical revision logs up to `up_to_seq` (inclusive), skipping pinned revisions.
    pub fn compact_history(&mut self, up_to_seq: u64) -> Result<CompactionReport, WorldError> {
        let _lock = WorldLockGuard::acquire(&self.root)?;
        let mut report = CompactionReport::default();
        let limit = up_to_seq.min(self.current_revision);

        for seq in 1..=limit {
            if self.is_revision_pinned(seq) {
                report.pinned_revisions.insert(seq);
                report.revisions_retained += 1;
                continue;
            }

            let mut reclaimed_any = false;
            let rev_path = self.paths.revision_file(seq);
            if rev_path.exists() {
                if let Ok(meta) = fs::metadata(&rev_path) {
                    report.bytes_reclaimed += meta.len();
                }
                let _ = fs::remove_file(&rev_path);
                reclaimed_any = true;
            }

            let delta_path = self.paths.decision_delta_file(seq);
            if delta_path.exists() {
                if let Ok(meta) = fs::metadata(&delta_path) {
                    report.bytes_reclaimed += meta.len();
                }
                let _ = fs::remove_file(&delta_path);
                reclaimed_any = true;
            }

            if reclaimed_any {
                report.revisions_reclaimed += 1;
            }
        }

        Ok(report)
    }

    /// Paginate records from a relation.
    pub fn query_page(
        &self,
        relation: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<QueryPage, WorldError> {
        let current_digest = self
            .current_revision_digest
            .ok_or_else(|| WorldError::CorruptedHead("current revision digest missing".into()))?;
        let trie = self
            .relations
            .get(relation)
            .ok_or_else(|| WorldError::UnknownRelation(relation.to_string()))?;

        let parsed_cursor = match cursor {
            Some(token) => {
                let qc = QueryCursor::from_token(token)?;
                if qc.kind == QueryCursorKind::Versioned
                    && (qc.world_id != self.manifest.world_id
                        || qc.revision != current_digest
                        || qc.relation != relation
                        || qc.ordering != "canonical")
                {
                    return Err(WorldError::InvalidCursor(
                        "query cursor scope mismatch".into(),
                    ));
                }
                match qc.position {
                    QueryCursorPosition::Hamt { hash, key } => Some((hash, key)),
                    QueryCursorPosition::Offset(_) => {
                        return Err(WorldError::InvalidCursor(
                            "query cursor ordering mismatch (offset cursor on hamt relation)"
                                .into(),
                        ));
                    }
                }
            }
            None => None,
        };

        let (entries, next_pos) = trie.iter_page_with_store(
            parsed_cursor.as_ref().map(|(h, k)| (h, k)),
            limit,
            &self.node_store,
        )?;

        let next_cursor = next_pos.map(|(hash, key)| {
            QueryCursor::new_hamt(&self.manifest.world_id, current_digest, relation, hash, key)
                .to_token()
        });
        let has_more = next_cursor.is_some();

        Ok(QueryPage {
            relation: relation.to_string(),
            entries,
            next_cursor,
            has_more,
        })
    }

    /// Retrieve derived tuples for a relation from the active operator network.
    ///
    /// `Ok(None)` if there is no network or `relation` is not derived; `Err` if persisted
    /// network state cannot be read (missing or corrupt node).
    pub fn get_derived(&self, relation: &str) -> Result<Option<Vec<TupleRecord>>, WorldError> {
        let Some(net) = self.network.as_ref() else {
            return Ok(None);
        };
        if let Some(rows) = net.get_derived_tuples(relation)? {
            return Ok(Some(rows));
        }
        match net
            .derived_relations
            .keys()
            .find(|k| k.ends_with(&format!("::{relation}")))
        {
            Some(k) => net.get_derived_tuples(k),
            None => Ok(None),
        }
    }

    /// Query a paged slice of derived tuples from the active operator network.
    pub fn query_derived_page(
        &self,
        relation: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<QueryPage, WorldError> {
        let current_digest = self
            .current_revision_digest
            .ok_or_else(|| WorldError::CorruptedHead("current revision digest missing".into()))?;
        if limit == 0 {
            return Err(WorldError::InvalidSchema(
                "query page limit must be positive".to_string(),
            ));
        }
        let records = self
            .get_derived(relation)?
            .ok_or_else(|| WorldError::UnknownRelation(relation.to_string()))?;

        let offset = match cursor {
            Some(token) => {
                let qc = QueryCursor::from_token(token)?;
                if qc.kind == QueryCursorKind::Versioned
                    && (qc.world_id != self.manifest.world_id
                        || qc.revision != current_digest
                        || qc.relation != relation
                        || qc.ordering != "offset")
                {
                    return Err(WorldError::InvalidCursor(
                        "query cursor scope mismatch".into(),
                    ));
                }
                match qc.position {
                    QueryCursorPosition::Offset(off) => off,
                    QueryCursorPosition::Hamt { .. } => {
                        return Err(WorldError::InvalidCursor(
                            "query cursor ordering mismatch (hamt cursor on derived relation)"
                                .into(),
                        ));
                    }
                }
            }
            None => 0,
        };

        let total = records.len();
        let entries: Vec<(WorldKey, WorldTuple)> = records
            .iter()
            .skip(offset)
            .take(limit)
            .enumerate()
            .map(|(i, r)| {
                let key = WorldKey::from_str(&format!("{}", offset + i));
                (key, r.to_tuple())
            })
            .collect();

        let next_offset = offset + entries.len();
        let has_more = next_offset < total;
        let next_cursor = if has_more {
            Some(
                QueryCursor::new_offset(
                    &self.manifest.world_id,
                    current_digest,
                    relation,
                    next_offset,
                )
                .to_token(),
            )
        } else {
            None
        };

        Ok(QueryPage {
            relation: relation.to_string(),
            entries,
            next_cursor,
            has_more,
        })
    }

    /// Retrieve derived tuples for a relation at a specific historical revision `seq`.
    pub fn get_derived_at(
        &self,
        seq: u64,
        relation: &str,
    ) -> Result<Option<Vec<TupleRecord>>, WorldError> {
        if seq == self.current_revision {
            return self.get_derived(relation);
        }
        let snapshot = self.pin_revision(seq)?;
        let Some(network) = self.network.as_ref() else {
            return Ok(None);
        };
        let mut historical = network.clear_data();
        let mut ops = Vec::new();
        for (rel, trie) in &snapshot.relations {
            if !network.base_relations.contains_key(rel) {
                continue;
            }
            let (entries, _) = trie.iter_page_with_store(None, usize::MAX, &snapshot.node_store)?;
            ops.extend(
                entries
                    .into_iter()
                    .map(|(key, tuple)| WorldBatchOp::Upsert {
                        relation: rel.clone(),
                        key,
                        tuple,
                    }),
            );
        }
        historical.apply_ops(&ops)?;
        if let Some(rows) = historical.get_derived_tuples(relation)? {
            return Ok(Some(rows));
        }
        let qualified = historical
            .derived_relations
            .keys()
            .find(|k| k.ends_with(&format!("::{relation}")))
            .cloned();
        if let Some(k) = qualified {
            historical.get_derived_tuples(&k)
        } else {
            Ok(None)
        }
    }

    /// Query a paged slice of derived tuples at a specific historical revision `seq`.
    pub fn query_derived_page_at(
        &self,
        seq: u64,
        relation: &str,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<QueryPage, WorldError> {
        let snapshot = self.pin_revision(seq)?;
        let rev_digest = snapshot.revision_digest();
        if limit == 0 {
            return Err(WorldError::InvalidSchema(
                "query page limit must be positive".to_string(),
            ));
        }
        let records = self
            .get_derived_at(seq, relation)?
            .ok_or_else(|| WorldError::UnknownRelation(relation.to_string()))?;

        let offset = match cursor {
            Some(token) => {
                let qc = QueryCursor::from_token(token)?;
                if qc.kind == QueryCursorKind::Legacy {
                    return Err(WorldError::InvalidCursor(
                        "legacy cursor not accepted for revision-pinned query".into(),
                    ));
                }
                if qc.world_id != self.manifest.world_id
                    || qc.revision != rev_digest
                    || qc.relation != relation
                    || qc.ordering != "offset"
                {
                    return Err(WorldError::InvalidCursor(
                        "query cursor scope mismatch".into(),
                    ));
                }
                match qc.position {
                    QueryCursorPosition::Offset(off) => off,
                    QueryCursorPosition::Hamt { .. } => {
                        return Err(WorldError::InvalidCursor(
                            "query cursor ordering mismatch (hamt cursor on derived relation)"
                                .into(),
                        ));
                    }
                }
            }
            None => 0,
        };

        let total = records.len();
        let entries: Vec<(WorldKey, WorldTuple)> = records
            .iter()
            .skip(offset)
            .take(limit)
            .enumerate()
            .map(|(i, r)| {
                let key = WorldKey::from_str(&format!("{}", offset + i));
                (key, r.to_tuple())
            })
            .collect();

        let next_offset = offset + entries.len();
        let has_more = next_offset < total;
        let next_cursor = if has_more {
            Some(
                QueryCursor::new_offset(&self.manifest.world_id, rev_digest, relation, next_offset)
                    .to_token(),
            )
        } else {
            None
        };

        Ok(QueryPage {
            relation: relation.to_string(),
            entries,
            next_cursor,
            has_more,
        })
    }

    /// Retrieve all settled decisions across all decide blocks.
    ///
    /// Served from the settlement checkpoint (one sequential read, plus the decision deltas of
    /// any revisions after it) when a valid one exists, else from the candidate frontier; the
    /// result is identical either way (the checkpoint is a cache, see
    /// [`super::settlement_checkpoint`]). A frontier read, or a read that had to apply a long
    /// tail, leaves a fresh checkpoint behind (best effort) unless `checkpoint_autowrite` is
    /// off, so the next read is fast; commits never write one.
    pub fn all_settlements(
        &self,
    ) -> Result<BTreeMap<String, BTreeMap<String, SettledDecision>>, WorldError> {
        Ok(self.all_settlements_with_source()?.0)
    }

    /// [`Self::all_settlements`] and where it came from: `"none"` (no network or no decide
    /// blocks), `"checkpoint"`, `"checkpoint+tail"` or `"frontier"`.
    #[allow(clippy::type_complexity)]
    pub fn all_settlements_with_source(
        &self,
    ) -> Result<
        (
            BTreeMap<String, BTreeMap<String, SettledDecision>>,
            &'static str,
        ),
        WorldError,
    > {
        let Some(net) = self.network.as_ref() else {
            return Ok((BTreeMap::new(), "none"));
        };
        if net.decides.is_empty() {
            return Ok((net.all_settlements()?, "none"));
        }
        if let Some((map, tail)) = self.settlements_from_checkpoint(net) {
            if tail > CHECKPOINT_REFRESH_TAIL && self.checkpoint_autowrite {
                let _ = self.write_settlement_checkpoint(net, &map, false);
            }
            return Ok((
                map,
                if tail == 0 {
                    "checkpoint"
                } else {
                    "checkpoint+tail"
                },
            ));
        }
        let map = net.all_settlements()?;
        if self.checkpoint_autowrite {
            let _ = self.write_settlement_checkpoint(net, &map, false);
        }
        Ok((map, "frontier"))
    }

    /// One page of settlements straight from the candidate frontier (never the checkpoint),
    /// at most `limit` buckets after `cursor`, in trie (not name) order. For consumers that
    /// do not need every decision.
    #[allow(clippy::type_complexity)]
    pub fn settlements_page(
        &self,
        cursor: Option<&super::state_store::PageCursor<super::state_codec::FrontierKey>>,
        limit: usize,
    ) -> Result<
        (
            Vec<(String, String, SettledDecision)>,
            Option<super::state_store::PageCursor<super::state_codec::FrontierKey>>,
        ),
        WorldError,
    > {
        match self.network.as_ref() {
            Some(n) => n.settlements_page(cursor, limit),
            None => Ok((Vec::new(), None)),
        }
    }

    /// The checkpoint's settlements brought to HEAD, and how many revisions that took; `None`
    /// on any miss (absent, corrupt, foreign, stale, or a delta that cannot be authenticated).
    fn settlements_from_checkpoint(
        &self,
        net: &WorldNetwork,
    ) -> Option<(super::settlement_checkpoint::Settlements, u64)> {
        let ck = SettlementCheckpoint::read_authentic(&self.paths)?;
        if ck.validity_key_digest != self.validity_key_for(net).digest()
            || ck.revision_seq > self.current_revision
        {
            return None;
        }
        let tail = self.current_revision - ck.revision_seq;
        if tail > CHECKPOINT_MAX_TAIL {
            return None;
        }
        if tail == 0 {
            if Some(ck.revision_digest) != self.current_revision_digest
                || ck.decision_root != Some(net.decision_root())
            {
                return None;
            }
            self.probe_settlements(net, &ck.settlements, &[])?;
            return Some((ck.settlements, 0));
        }
        if ck.revision_seq > 0 {
            let (_, info) =
                super::idempotency::record_of(&self.paths, ck.revision_seq, ck.revision_seq)?;
            if info.revision_digest != ck.revision_digest {
                return None;
            }
        }
        let mut map = ck.settlements;
        let mut touched: Vec<(String, String)> = Vec::new();
        for seq in ck.revision_seq + 1..=self.current_revision {
            let delta = self.decision_delta(seq).ok()?;
            touched.extend(delta.keys().cloned());
            apply_delta(&mut map, delta);
        }
        touched.sort();
        touched.dedup();
        // Every key the tail touched, not a sample.
        self.probe_settlements(net, &map, &touched)?;
        Some((map, tail))
    }

    /// Spot-check `map` against HEAD's persisted decision trie: the same number of
    /// settlements, a spread sample of `CHECKPOINT_PROBES` entries present with exactly the
    /// trie's value, and every `extra` key (those a tail changed) agreeing. A checkpoint with
    /// a flipped, dropped or forged entry is overwhelmingly likely to fail one of these; a
    /// full recompute of the decision root is `verify_settlement_checkpoint`.
    fn probe_settlements(
        &self,
        net: &WorldNetwork,
        map: &super::settlement_checkpoint::Settlements,
        extra: &[(String, String)],
    ) -> Option<()> {
        let count: usize = map.values().map(|m| m.len()).sum();
        if count != net.decision_count() {
            return None;
        }
        let stride = (count / CHECKPOINT_PROBES).max(1);
        let mut i = 0usize;
        for (decide, entities) in map {
            for (entity, d) in entities {
                if i.is_multiple_of(stride) && !net.decision_matches(decide, entity, d).ok()? {
                    return None;
                }
                i += 1;
            }
        }
        for (decide, entity) in extra {
            let ok = match map.get(decide).and_then(|m| m.get(entity)) {
                Some(d) => net.decision_matches(decide, entity, d),
                None => net.decision_absent(decide, entity),
            };
            if !ok.ok()? {
                return None;
            }
        }
        Some(())
    }

    /// Write `map` as the HEAD checkpoint, authenticated under the world's secret. Unless the
    /// caller already proved it (`verified`), the settlements must first hash to the decision
    /// root the checkpoint is stamped with: the frontier walk and a tail-applied map are both
    /// derived data, and a checkpoint is only ever as good as what it was written from.
    fn write_settlement_checkpoint(
        &self,
        net: &WorldNetwork,
        map: &super::settlement_checkpoint::Settlements,
        verified: bool,
    ) -> Result<(), WorldError> {
        let Some(digest) = self.current_revision_digest else {
            return Ok(());
        };
        if !verified && !map.is_empty() && compute_decision_root(map) != net.decision_root() {
            return Err(WorldError::StateMismatch(
                "settlements do not hash to the decision root; checkpoint not written".into(),
            ));
        }
        let Some(key) = super::settlement_checkpoint::load_or_create_key(&self.paths) else {
            return Ok(());
        };
        let bytes = SettlementCheckpoint::encode_parts(
            self.current_revision,
            digest,
            self.validity_key_for(net).digest(),
            Some(net.decision_root()),
            map,
            Some(&key),
        );
        let crash = self.crash_injector == Some(CrashPoint::CheckpointAfterTmpBeforeRename);
        SettlementCheckpoint::write_bytes(&self.paths, &bytes, crash).map_err(|e| {
            if crash {
                WorldError::InjectedCrash(CrashPoint::CheckpointAfterTmpBeforeRename)
            } else {
                e.into()
            }
        })
    }

    /// Rebuild the settlement checkpoint at HEAD from the candidate frontier, after checking
    /// that the settlements hash to the revision's decision root (a mismatch is corrupt
    /// state and nothing is written). Returns the number of settlements.
    pub fn refresh_settlement_checkpoint(&mut self) -> Result<usize, WorldError> {
        let Some(net) = self.network.as_ref() else {
            return Err(WorldError::NetworkError("world has no network".into()));
        };
        let map = net.all_settlements()?;
        if !map.is_empty() && compute_decision_root(&map) != net.decision_root() {
            return Err(WorldError::StateMismatch(
                "settlements do not hash to the decision root".into(),
            ));
        }
        self.write_settlement_checkpoint(net, &map, true)?;
        Ok(map.values().map(|m| m.len()).sum())
    }

    /// Check the settlement checkpoint on disk: `Ok(None)` if there is none, else its entry
    /// count after strict decoding, a recompute of its decision root from its settlements,
    /// and (when it is at HEAD) equality with the frontier's settlements.
    pub fn verify_settlement_checkpoint(&self) -> Result<Option<usize>, WorldError> {
        let path = self.paths.settlement_checkpoint_file();
        if !path.exists() {
            return Ok(None);
        }
        let ck = SettlementCheckpoint::decode(&fs::read(path)?)?;
        ck.verify_root()?;
        if ck.revision_seq == self.current_revision {
            if let Some(net) = self.network.as_ref() {
                if ck.settlements != net.all_settlements()? {
                    return Err(WorldError::StateMismatch(
                        "settlement checkpoint differs from the candidate frontier".into(),
                    ));
                }
            }
        }
        Ok(Some(ck.count()))
    }

    /// Explain the decision deliberation for an entity key.
    pub fn explain_decision(
        &self,
        entity_key: &str,
    ) -> Result<Option<DecisionExplanation>, WorldError> {
        match self.network.as_ref() {
            Some(n) => n.explain_decision(entity_key),
            None => Ok(None),
        }
    }

    /// Explain the decision deliberation for an entity key within a specific decide block.
    pub fn explain_decision_for(
        &self,
        decide_name: &str,
        entity_key: &str,
    ) -> Result<Option<DecisionExplanation>, WorldError> {
        match self.network.as_ref() {
            Some(n) => n.explain_decision_for(decide_name, entity_key),
            None => Ok(None),
        }
    }

    pub fn historical_explain_decision_for(
        &self,
        seq: u64,
        decide_name: &str,
        entity_key: &str,
    ) -> Result<Option<DecisionExplanation>, WorldError> {
        let snapshot = self.pin_revision(seq)?;
        let Some(network) = self.network.as_ref() else {
            return Ok(None);
        };
        let mut historical = network.clear_data();
        let mut ops = Vec::new();
        for (relation, trie) in &snapshot.relations {
            if !network.base_relations.contains_key(relation) {
                continue;
            }
            let (entries, _) = trie.iter_page_with_store(None, usize::MAX, &snapshot.node_store)?;
            ops.extend(
                entries
                    .into_iter()
                    .map(|(key, tuple)| WorldBatchOp::Upsert {
                        relation: relation.clone(),
                        key,
                        tuple,
                    }),
            );
        }
        historical.apply_ops(&ops)?;
        let mut explanation = historical.explain_decision_for(decide_name, entity_key)?;
        if let Some(explanation) = &mut explanation {
            let revisions =
                self.historical_settlement_revisions(seq, &explanation.upstream_settlements)?;
            for link in &mut explanation.upstream_settlements {
                let key = (link.decide.clone(), link.entity_id.clone());
                link.revision = *revisions.get(&key).ok_or_else(|| {
                    WorldError::NetworkError(format!(
                        "historical provenance has no settlement journal entry for '{}' entity '{}' at revision {seq}",
                        link.decide, link.entity_id
                    ))
                })?;
            }
        }
        Ok(explanation)
    }

    fn historical_settlement_revisions(
        &self,
        seq: u64,
        links: &[UpstreamSettlementLink],
    ) -> Result<BTreeMap<(String, String), u64>, WorldError> {
        let mut unresolved: BTreeSet<(String, String)> = links
            .iter()
            .map(|link| (link.decide.clone(), link.entity_id.clone()))
            .collect();
        let mut found = BTreeMap::new();
        for revision in (1..=seq).rev() {
            if unresolved.is_empty() {
                break;
            }
            for (key, settlement) in self.decision_delta(revision)? {
                if unresolved.remove(&key) && settlement.is_some() {
                    found.insert(key, revision);
                }
            }
        }
        if !unresolved.is_empty() {
            let (decide, entity) = unresolved.iter().next().unwrap();
            return Err(WorldError::NetworkError(format!(
                "historical provenance is missing a settlement journal entry for '{decide}' entity '{entity}' at revision {seq}"
            )));
        }
        Ok(found)
    }

    /// Read and authenticate a committed revision's canonical decision changes.
    /// Legacy records have no trustworthy delta body and explicitly refuse.
    pub fn decision_delta(
        &self,
        seq: u64,
    ) -> Result<super::decision_codec::DecisionDelta, WorldError> {
        let snapshot = self.pin_revision(seq)?;
        let rev = snapshot.revision;
        if !rev.has_decision_evidence() {
            return Err(WorldError::NetworkError(
                "legacy-history-unavailable".into(),
            ));
        }
        let Some(expected) = rev.decision_delta_digest else {
            if rev.decision_root.is_some() {
                return Err(WorldError::NetworkError(
                    "missing decision delta digest".into(),
                ));
            }
            return Ok(BTreeMap::new());
        };
        let path = self.paths.decision_delta_file(seq);
        const MAX_BYTES: u64 = 64 * 1024 * 1024;
        if fs::metadata(&path)?.len() > MAX_BYTES {
            return Err(WorldError::NetworkError("decision delta byte limit".into()));
        }
        let bytes = fs::read(path)?;
        if Digest::of(Domain::Value, &bytes) != expected {
            return Err(WorldError::NetworkError(
                "decision delta digest mismatch".into(),
            ));
        }
        super::decision_codec::decode_decision_delta(&bytes, 1_000_000)
    }

    /// Read a settled decision directly from the persisted decision trie at a
    /// past revision (ADR-0046 P6 G2, decided 2026-10-04) — `brix world
    /// explain --rev n` reads real historical evidence from the node store,
    /// not a replay. Decide-block declarations never change after genesis
    /// (`save_program_closure` refuses re-binding once revisions exist), so
    /// the *current* session's known decide names are valid candidates to
    /// search at any past revision of the same program. Returns `Ok(None)`
    /// when revision `seq` records no settlement for `entity_id` in any
    /// (optionally filtered) decide block — distinct from a decode failure,
    /// which is a typed error.
    pub fn historical_settlement(
        &self,
        seq: u64,
        decide_filter: Option<&str>,
        entity_id: &str,
    ) -> Result<Option<(String, SettledDecision)>, WorldError> {
        let rev = self.pin_revision(seq)?.revision;
        if !rev.has_decision_evidence() {
            return Err(WorldError::NetworkError(
                "legacy-history-unavailable: revision@1 did not persist decision evidence".into(),
            ));
        }
        let Some(decision_root) = rev.decision_root else {
            return Ok(None);
        };

        let known_decides: Vec<String> = self
            .network
            .as_ref()
            .map(|n| n.decides.keys().cloned().collect())
            .unwrap_or_default();
        let candidates: Vec<String> = match decide_filter {
            Some(d) => known_decides
                .into_iter()
                .filter(|k| k == d || k.ends_with(&format!("::{d}")))
                .collect(),
            None => known_decides,
        };

        let tree = TrieMap::<WorldKey, WorldTuple>::from_root_digest(decision_root, 0);
        for decide_name in candidates {
            let key = super::network::decision_key(&decide_name, entity_id);
            if let Some(tuple) = tree.get_with_store(&key, &self.node_store)? {
                let settled = super::network::decode_settled_decision(entity_id, tuple.as_bytes())?;
                return Ok(Some((decide_name, settled)));
            }
        }
        Ok(None)
    }

    /// Attach an operator network to this session and hydrate it with existing committed base records.
    pub fn set_network(&mut self, mut network: WorldNetwork) -> Result<(), WorldError> {
        // A world's decisions and state are filed under the network that computed them;
        // swapping in a different program would let later commits attest foreign decisions.
        if let Some(existing) = &self.network {
            if existing.fingerprint() != network.fingerprint() {
                return Err(WorldError::InvalidSchema(
                    "program differs from the program this world is bound to".into(),
                ));
            }
        }
        let cache = self.node_cache_budget();
        network.attach_state_store(StateStore::from_file(
            self.node_store.clone(),
            state_cache_share(cache),
        ));
        self.apply_base_facts(&mut network)?;
        network.current_revision = self.current_revision;
        self.open_mode = OpenMode::Replay;
        self.replay_reason = Some("set-network");
        self.tail_batches = 0;
        // Replay built tries that were never persisted; write them once and drop the
        // in-memory copies so the first commit does not rewrite O(world).
        network.persist_state()?;
        self.node_store.flush()?;
        self.network = Some(network.dehydrated()?);
        Ok(())
    }

    /// Save program closure (source map of modules) to `program.json` in the world directory.
    pub fn save_program_closure(
        &mut self,
        root_module: &str,
        sources: &BTreeMap<String, String>,
    ) -> Result<(), WorldError> {
        let (linked, closure_digest, closure_sources) = link_closure(root_module, sources)?;
        let network = WorldNetwork::from_program(&linked)?;
        validate_program_relations(&self.manifest, &network)?;
        if self.current_revision > 0 && self.manifest.program_digest != closure_digest {
            return Err(WorldError::InvalidSchema(
                "cannot change executable program after world revisions exist".into(),
            ));
        }
        let exec_profile = ExecProfileV1::current_for_network();
        let val = serde_json::json!({
            "schema": "brix.world.program@1",
            "root_module": root_module,
            "sources": closure_sources,
            "program_manifest_digest": closure_digest.to_hex(),
            "exec_profile": exec_profile.to_json(),
        });
        let bytes = serde_json::to_vec_pretty(&val).map_err(|e| WorldError::Json(e.to_string()))?;
        atomic_write(&self.paths.root.join("program.json"), &bytes)?;
        self.exec_profile = Some(exec_profile);
        if self.current_revision == 0
            || self.manifest.program_digest != closure_digest
            || !self.manifest.program_required
        {
            let mut bound_manifest = self.manifest.clone();
            bound_manifest.program_digest = closure_digest;
            bound_manifest.program_required = true;
            let genesis_path = self.paths.revision_file(0);
            let genesis_json: serde_json::Value = serde_json::from_slice(&fs::read(&genesis_path)?)
                .map_err(|e| WorldError::Json(e.to_string()))?;
            let genesis = WorldRevision::from_json(&genesis_json)?;
            if genesis.seq != 0 || self.current_revision != 0 {
                return Err(WorldError::InvalidSchema(
                    "program closure can only bind at genesis".into(),
                ));
            }
            let rebound = WorldRevision::new(
                genesis.seq,
                genesis.timestamp,
                genesis.expected_base_revision,
                genesis.idempotency_key,
                genesis.batch_digest,
                Some(closure_digest),
                genesis.previous_revision_digest,
                genesis.relation_roots,
                genesis.relation_cardinalities,
                genesis.secondary_index_roots,
                genesis.decision_root,
                genesis.changed_keys,
                genesis.status,
                genesis.decision_delta_digest,
            )
            .bind_exec_profile(self.exec_profile.as_ref().map(ExecProfileV1::digest));
            let manifest_bytes = serde_json::to_vec_pretty(&bound_manifest.to_json())
                .map_err(|e| WorldError::Json(e.to_string()))?;
            let revision_bytes = serde_json::to_vec_pretty(&rebound.to_json())
                .map_err(|e| WorldError::Json(e.to_string()))?;
            // Write the revision before publishing the manifest requirement. A crash
            // between these writes leaves a reopenable storage-only world.
            atomic_write(&genesis_path, &revision_bytes)?;
            atomic_write(&self.paths.world_json(), &manifest_bytes)?;
            self.manifest = bound_manifest;
            self.current_revision_digest = Some(rebound.revision_digest);
        }
        self.set_network(network)?;
        Ok(())
    }

    /// Initialize or open a world session bound to a linked program.
    pub fn from_program(
        root: impl AsRef<Path>,
        program: &LinkedProgram,
    ) -> Result<Self, WorldError> {
        let root = root.as_ref().to_path_buf();
        if root.exists() && (root.join("world.json").exists() || root.join("HEAD").exists()) {
            let mut session = Self::open(&root)?;
            let network = WorldNetwork::from_program(program)?;
            validate_program_relations(&session.manifest, &network)?;
            match &session.network {
                // `open` already installed the world's own program (restored or replayed).
                // The given program must be that program; installing a different one would
                // file its state under the manifest's program digest.
                Some(existing) => {
                    if existing.fingerprint() != network.fingerprint() {
                        return Err(WorldError::InvalidSchema(
                            "program differs from the program this world is bound to".into(),
                        ));
                    }
                }
                None => session.set_network(network)?,
            }
            return Ok(session);
        }

        let dag = lower_relations(program)?;
        let mut relations = Vec::new();
        for node in &dag.nodes {
            if let OperatorNode::Scan {
                relation,
                schema,
                key_fields,
            } = node
            {
                let all_fields: Vec<String> = match schema {
                    brix_syntax::ast::Ty::Record(fields) => {
                        fields.iter().map(|f| f.name.clone()).collect()
                    }
                    _ => vec![],
                };
                let value_fields: Vec<String> = all_fields
                    .iter()
                    .filter(|f| !key_fields.contains(f))
                    .cloned()
                    .collect();
                relations.push(RelationDecl::new(
                    relation.clone(),
                    key_fields.clone(),
                    value_fields,
                    vec![],
                ));
            }
        }

        let manifest = WorldManifest::new(
            "world-1",
            iso_now(),
            Digest::of(Domain::Value, program.root_module.as_bytes()),
            relations,
        );
        let mut manifest = manifest;
        manifest.program_required = true;
        let mut session = Self::create(&root, manifest)?;
        let network = WorldNetwork::from_program(program)?;
        session.set_network(network)?;
        Ok(session)
    }

    /// Initialize a world directly from its source closure, binding the full graph digest.
    pub fn from_program_with_sources(
        root: impl AsRef<Path>,
        root_module: &str,
        sources: &BTreeMap<String, String>,
    ) -> Result<Self, WorldError> {
        let (linked, _, _) = link_closure(root_module, sources)?;
        let mut session = Self::from_program(root, &linked)?;
        session.save_program_closure(root_module, sources)?;
        Ok(session)
    }

    /// Verify differential correctness between incremental network state and oracle full recompute.
    ///
    /// A restored network keeps no base facts of its own, so it is checked against a fresh
    /// replay of the session's relations instead ([`Self::verify_state`]).
    pub fn verify_oracle(&self) -> Result<(), WorldError> {
        if let Some(ref net) = self.network {
            if net.base_resident() {
                net.verify_differential_correctness()?;
            } else {
                self.verify_state()?;
            }
        }
        Ok(())
    }

    /// A fresh in-memory network rebuilt by replaying every base fact the session holds.
    /// Reads no persisted operator state.
    pub fn replay_network(&self) -> Result<WorldNetwork, WorldError> {
        let net = self
            .network
            .as_ref()
            .ok_or_else(|| WorldError::NetworkError("world has no network".into()))?;
        let mut fresh = net.clear_data();
        fresh.attach_state_store(StateStore::memory(self.node_cache_budget()));
        self.apply_base_facts(&mut fresh)?;
        fresh.current_revision = self.current_revision;
        Ok(fresh)
    }

    /// Recompute the network state from the base facts and require every persisted root
    /// (operator indexes, derived, frontier, decisions) to equal what this session holds.
    pub fn verify_state(&self) -> Result<(), WorldError> {
        let Some(net) = self.network.as_ref() else {
            return Ok(());
        };
        let fresh = self.replay_network()?;
        let key = self.validity_key_for(net);
        let (have, want) = (net.state_header(key.clone()), fresh.state_header(key));
        if have != want {
            return Err(WorldError::StateMismatch(format!(
                "session={have:?} replay={want:?}"
            )));
        }
        Ok(())
    }

    /// Paginate diff events between two revisions without scanning or serializing the entire world.
    pub fn diff_page(
        &self,
        relation: &str,
        from_rev: u64,
        to_rev: u64,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<DiffPage, WorldError> {
        if from_rev == to_rev {
            return Ok(DiffPage {
                relation: relation.to_string(),
                from_revision: from_rev,
                to_revision: to_rev,
                events: Vec::new(),
                next_cursor: None,
                has_more: false,
            });
        }

        let snap_from = self.pin_revision(from_rev)?;
        let snap_to = self.pin_revision(to_rev)?;

        let from_root = snap_from
            .revision
            .relation_roots
            .get(relation)
            .copied()
            .unwrap_or_else(|| Digest::of(brix_canon::Domain::Value, b"brix:trie:empty:v1"));
        let to_root = snap_to
            .revision
            .relation_roots
            .get(relation)
            .copied()
            .unwrap_or_else(|| Digest::of(brix_canon::Domain::Value, b"brix:trie:empty:v1"));

        // O(1) fast path: equal Merkle roots imply identical relation state!
        if from_root == to_root {
            return Ok(DiffPage {
                relation: relation.to_string(),
                from_revision: from_rev,
                to_revision: to_rev,
                events: Vec::new(),
                next_cursor: None,
                has_more: false,
            });
        }

        // Collect changed keys from revision journal
        let (min_rev, max_rev) = if from_rev < to_rev {
            (from_rev + 1, to_rev)
        } else {
            (to_rev + 1, from_rev)
        };

        let mut affected_keys = BTreeMap::new();
        for r in min_rev..=max_rev {
            if let Ok(snap) = self.pin_revision(r) {
                if let Some(keys) = snap.revision.changed_keys.get(relation) {
                    for k in keys {
                        affected_keys.insert(k.clone(), ());
                    }
                }
            }
        }

        let page_limit = if limit == 0 { 100 } else { limit };
        let start_key = match cursor {
            Some(token) => {
                if token.contains(':') {
                    Some(WorldCursor::from_token(token)?.key)
                } else {
                    Some(WorldKey::from_hex(token)?)
                }
            }
            None => None,
        };

        let mut events = Vec::new();
        let mut next_cursor = None;
        let mut has_more = false;

        let iter: Box<dyn Iterator<Item = (&WorldKey, &())>> = match &start_key {
            Some(sk) => Box::new(
                affected_keys.range((std::ops::Bound::Excluded(sk), std::ops::Bound::Unbounded)),
            ),
            None => Box::new(affected_keys.iter()),
        };

        for (k, _) in iter {
            let val_from = snap_from.get(relation, k)?;
            let val_to = snap_to.get(relation, k)?;

            let event = match (val_from, val_to) {
                (None, Some(t)) => Some(DiffEvent::Upserted {
                    key: k.clone(),
                    tuple: t,
                }),
                (Some(_), Some(t)) => Some(DiffEvent::Upserted {
                    key: k.clone(),
                    tuple: t,
                }),
                (Some(_), None) => Some(DiffEvent::Removed { key: k.clone() }),
                (None, None) => None,
            };

            if let Some(ev) = event {
                if events.len() < page_limit {
                    events.push(ev);
                } else {
                    has_more = true;
                    break;
                }
            }
        }

        if has_more {
            if let Some(last_ev) = events.last() {
                next_cursor = Some(last_ev.key().to_hex());
            }
        }

        Ok(DiffPage {
            relation: relation.to_string(),
            from_revision: from_rev,
            to_revision: to_rev,
            events,
            next_cursor,
            has_more,
        })
    }

    /// Stage a chunk for bulk ingestion.
    pub fn stage_chunk(
        &mut self,
        upload_id: &str,
        chunk_seq: u64,
        batch: &WorldBatch,
    ) -> Result<(), WorldError> {
        StagingManager::stage_chunk(
            self.paths.staging_dir().as_path(),
            upload_id,
            chunk_seq,
            batch,
        )
    }

    /// Consolidate staged chunks and atomically publish as a world revision.
    pub fn commit_staged(
        &mut self,
        upload_id: &str,
        expected_chunks: u64,
        idempotency_key: &str,
    ) -> Result<RevisionReceipt, WorldError> {
        let consolidated_batch = StagingManager::consolidate(
            self.paths.staging_dir().as_path(),
            upload_id,
            expected_chunks,
            self.current_revision,
            idempotency_key,
        )?;
        let receipt = self.apply_batch(consolidated_batch)?;
        StagingManager::cleanup_upload(self.paths.staging_dir().as_path(), upload_id)?;
        Ok(receipt)
    }

    /// Close the session.
    pub fn close(self) -> Result<(), WorldError> {
        self.node_store.flush()?;
        Ok(())
    }

    /// Reopen the session from disk.
    pub fn reopen(root: impl AsRef<Path>) -> Result<Self, WorldError> {
        Self::open(root)
    }
}

fn iso_now() -> String {
    "2026-10-04T00:00:00Z".to_string()
}
