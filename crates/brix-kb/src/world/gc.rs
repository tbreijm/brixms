//! Garbage collection and compaction of a world's node store (`brix world gc`).
//!
//! The node store is append-only: every commit adds packs and nothing removes them, but most
//! of what a commit writes is superseded almost at once (the per-revision network-state tries
//! of earlier revisions, path-copied interior nodes of replaced tries). GC computes the set
//! of nodes still reachable from the *roots* below, re-writes exactly those into fresh packs
//! and then removes the old packs.
//!
//! # Roots and retention (decision, 2026-10-10)
//!
//! * Every revision that is retained contributes its `relation_roots`, `secondary_index_roots`
//!   and `decision_root`: those are what `pin_revision`, `explain --rev` / `historical_settlement`,
//!   `decision_delta` and the audit builders read from the store.
//! * The HEAD revision additionally contributes its `network_state_root` (the header and
//!   everything it reaches: operator indexes, frontier, decision sub-tries and interned
//!   derivations). Older revisions' network-state tries are **not** roots: open restores from
//!   HEAD only and a replay rebuilds any other revision's state, so they are the bulk of the
//!   garbage.
//! * The state-cache sidecar's `network_state_root` (when it parses) is a root too.
//! * Default policy keeps **every** revision whose record is on disk readable
//!   ([`GcOptions::keep_revisions`] `= None`), so `world verify`, audit bundles and historical
//!   reads behave exactly as before. With `Some(k)` only the newest `k` revisions plus the
//!   genesis, HEAD and checkpoint-pinned revisions are roots; the older revisions' records
//!   remain on disk but reading their tries fails with a missing-node error. Journal files
//!   (`revisions/`, `receipts/`, decision deltas) are never touched by GC.
//!
//! # Reachability
//!
//! The store holds trie nodes of several key/value types plus the network-state header and
//! interned derivation objects. Rather than a typed walk per kind, every object's bytes are
//! scanned for the canonical embedding of a digest (`write_bytes` of 32 bytes: the two bytes
//! `01 20` followed by the digest); a candidate is a reference iff that digest is present in
//! the store. This is **conservative**: a coincidental match only keeps an extra node, and
//! because every codec in the crate writes digests with `write_bytes`, a real reference is
//! never missed. [`GcOptions::verify_after`] re-checks the result.
//!
//! # Crash safety
//!
//! Live nodes are written to new packs, published atomically (idx fsynced, pack renamed,
//! directory fsynced). Only then are old packs unlinked, one by one. At every instant each
//! live node is in some published pack, so a crash leaves a store that holds the live set
//! (old packs, new packs, or both: a superset of the same content) and that opens and reads
//! identically. The next GC removes the leftovers (and any `.tmp` files). GC holds the world's
//! single-writer lock.
//!
//! # Sessions open across a GC (generation marker)
//!
//! A [`super::session::WorldSession`] open before a GC keeps caches that can vouch for nodes the
//! GC deletes (the decoded-node cache is taken as proof a node is stored, so an identical
//! node re-created by a later commit would never be written again). GC therefore rewrites the
//! marker `objects/GC_EPOCH` (atomically, a fresh nonce plus the history floor) **before** it
//! retires anything and again after; every session write path (`apply_batch`,
//! `refresh_state_cache`, `persist_idempotency_index`) re-reads it under the writer lock and,
//! when it changed, reloads the pack set and clears every cache first. A stale session thus
//! keeps working transparently instead of committing a HEAD whose nodes are gone. Read-only
//! use of a stale session can still see pre-GC cached nodes (harmless: they were valid) and
//! should reopen for a fresh view.
//!
//! # Compacted history
//!
//! `keep_revisions` leaves a `floor` in the marker: revisions below it other than genesis and
//! checkpoint pins no longer have their tries, and `pin_revision` reports them as
//! [`WorldError::RevisionCompacted`] (status `revision-compacted`), distinct from damage
//! (`corrupt-state`) and from absent state (`missing-state`). In-memory reader pins
//! (`pin_reader`) live in one process and are invisible to a GC run from another: GC cannot
//! honour them (only the persisted checkpoint pins), so `--keep-revisions` must not run while
//! readers hold older revisions.

// Membership tests and an order-insensitive final check only; GC output never depends on
// iteration order (nodes are content-addressed).
#![allow(clippy::disallowed_types)]

use std::collections::HashSet;
use std::io;
use std::path::Path;
use std::time::Instant;

use brix_canon::Digest;
use soc_core::store::{FileNodeStore, NodeStore, TrieMap};

use super::error::WorldError;
use super::idempotency::IdempotencySidecar;
use super::paths::WorldPaths;
use super::revision::WorldRevision;
use super::session::{OpenPolicy, WorldLockGuard, WorldSession};
use super::state_cache::StateCache;
use super::types::{WorldKey, WorldTuple};

/// Bytes of node data after which GC publishes a pack and starts the next one.
const PACK_TARGET_BYTES: u64 = 256 * 1024 * 1024;

/// Fault injection for the GC protocol (tests only; `None` in production).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GcCrashPoint {
    /// After the first new pack was published, while more live nodes are still being copied.
    AfterFirstNewPack,
    /// All live nodes copied, the last pack not yet published (a `.tmp` pack is left).
    BeforeFinalFlush,
    /// New packs published, no old pack removed yet.
    AfterCopyBeforeRetire,
    /// The first old pack removed, the rest still present.
    MidRetire,
    /// All old packs removed, legacy loose objects not yet.
    AfterRetire,
}

#[derive(Clone, Debug)]
pub struct GcOptions {
    /// Compute and report only; the store is not modified (stale `.tmp` files are kept).
    pub dry_run: bool,
    /// `None`: keep every revision readable. `Some(k)`: see the module docs.
    pub keep_revisions: Option<u64>,
    /// After a real run, reopen the store, audit every pack and check every live node resolves.
    pub verify_after: bool,
    /// Override the pack size target (tests); `None` uses 256 MiB.
    pub pack_target_bytes: Option<u64>,
    pub crash: Option<GcCrashPoint>,
}

impl Default for GcOptions {
    fn default() -> Self {
        Self {
            dry_run: false,
            keep_revisions: None,
            verify_after: true,
            pack_target_bytes: None,
            crash: None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GcReport {
    pub dry_run: bool,
    pub head_revision: u64,
    pub revisions_total: u64,
    pub revisions_retained: u64,
    pub roots: u64,
    pub live_nodes: u64,
    pub live_bytes: u64,
    /// Nodes in published packs and legacy loose objects before the run.
    pub nodes_before: u64,
    pub dead_nodes: u64,
    pub bytes_before: u64,
    /// Equals `bytes_before` for a dry run.
    pub bytes_after: u64,
    pub packs_before: u64,
    pub packs_after: u64,
    pub files_swept: u64,
    pub elapsed_ms: u64,
}

fn inject(opts: &GcOptions, at: GcCrashPoint) -> Result<(), WorldError> {
    if opts.crash == Some(at) {
        return Err(WorldError::Io(io::Error::other(format!(
            "injected gc crash: {at:?}"
        ))));
    }
    Ok(())
}

/// Damage found while collecting is `CorruptState` (as everywhere else); only OS errors stay `Io`.
fn storage(e: soc_core::store::StorageError) -> WorldError {
    use soc_core::store::StorageError as S;
    match e {
        S::Io(m) => WorldError::Io(io::Error::other(format!("gc: {m}"))),
        S::MissingNode(_) => WorldError::MissingState(super::state_store::StateError::Storage(e)),
        e => WorldError::CorruptState(super::state_store::StateError::Storage(e)),
    }
}

/// The GC generation marker: the whole content of `objects/GC_EPOCH` (empty when no GC ever
/// ran). Compared for equality by sessions; see the module docs.
pub(crate) fn read_gc_epoch(paths: &WorldPaths) -> String {
    std::fs::read_to_string(paths.gc_epoch_file()).unwrap_or_default()
}

/// The history floor recorded by the last `keep_revisions` GC (0 = nothing was compacted).
pub(crate) fn read_gc_floor(paths: &WorldPaths) -> u64 {
    read_gc_epoch(paths)
        .lines()
        .find_map(|l| l.strip_prefix("floor "))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0)
}

/// Atomically replace the marker with a fresh nonce and `floor`, durable before returning.
fn write_gc_epoch(paths: &WorldPaths, floor: u64) -> Result<(), WorldError> {
    use std::io::Write;
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let body = format!("epoch {}-{nanos}-{n}\nfloor {floor}\n", std::process::id());
    let tmp = paths.gc_epoch_tmp();
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(body.as_bytes())?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, paths.gc_epoch_file())?;
    std::fs::File::open(paths.objects_dir())?.sync_all()?;
    Ok(())
}

/// Every digest-shaped reference in `bytes` that names a node present in `store`.
fn references(
    bytes: &[u8],
    store: &FileNodeStore,
    seen: &HashSet<Digest>,
    out: &mut Vec<Digest>,
) -> Result<(), WorldError> {
    let mut i = 0;
    while i + 34 <= bytes.len() {
        if bytes[i] == 0x01 && bytes[i + 1] == 0x20 {
            let mut raw = [0u8; 32];
            raw.copy_from_slice(&bytes[i + 2..i + 34]);
            let d = Digest::from_bytes(raw);
            if !seen.contains(&d) && store.try_contains(&d)? {
                out.push(d);
            }
        }
        i += 1;
    }
    Ok(())
}

struct Roots {
    digests: Vec<Digest>,
    /// Relation, secondary-index and decision roots of the retained revisions (trie roots
    /// that must decode), for `verify_after`.
    trie_roots: Vec<Digest>,
    total: u64,
    retained: u64,
    head: u64,
}

fn collect_roots(root: &Path, paths: &WorldPaths, keep: Option<u64>) -> Result<Roots, WorldError> {
    let head_str = std::fs::read_to_string(paths.head())?;
    let head: u64 = head_str
        .split_whitespace()
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| WorldError::CorruptedHead("invalid HEAD".into()))?;
    let pinned: std::collections::BTreeSet<u64> = std::fs::read(paths.pins_file())
        .ok()
        .and_then(|b| serde_json::from_slice::<Vec<u64>>(&b).ok())
        .map(|v| v.into_iter().collect())
        .unwrap_or_default();
    let mut digests = Vec::new();
    let mut trie_roots = Vec::new();
    let (mut total, mut retained) = (0, 0);
    for seq in 0..=head {
        let file = paths.revision_file(seq);
        if !file.exists() {
            if seq == head {
                return Err(WorldError::RevisionNotFound(seq));
            }
            continue; // compacted history
        }
        total += 1;
        let keep_it = match keep {
            None => true,
            Some(k) => seq == 0 || seq == head || pinned.contains(&seq) || head - seq < k,
        };
        if !keep_it {
            continue;
        }
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(file)?)
            .map_err(|e| WorldError::Json(e.to_string()))?;
        let rev = WorldRevision::from_json(&v)?;
        if rev.seq != seq {
            return Err(WorldError::InvalidSchema(format!(
                "revision file {seq} contains revision {}",
                rev.seq
            )));
        }
        retained += 1;
        digests.extend(rev.relation_roots.values().copied());
        digests.extend(rev.secondary_index_roots.values().copied());
        digests.extend(rev.decision_root);
        trie_roots.extend(rev.relation_roots.values().copied());
        trie_roots.extend(rev.secondary_index_roots.values().copied());
        trie_roots.extend(rev.decision_root);
        if seq == head {
            digests.extend(rev.network_state_root);
        }
    }
    let _ = root;
    if let Some(c) = StateCache::read(&paths.state_cache_file()) {
        digests.push(c.network_state_root);
    }
    // The idempotency index is a best-effort cache named by a sibling sidecar; its trie
    // nodes must survive GC or every open falls back to the receipt scan.
    if let Some(sc) = IdempotencySidecar::read(&paths.idempotency_file()) {
        digests.push(sc.root);
    }
    Ok(Roots {
        digests,
        trie_roots,
        total,
        retained,
        head,
    })
}

/// Collect garbage in the node store of the world at `dir` (see the module docs).
pub fn gc_world(dir: impl AsRef<Path>, opts: &GcOptions) -> Result<GcReport, WorldError> {
    let started = Instant::now();
    let root = dir.as_ref();
    let paths = WorldPaths::new(root);
    if !paths.world_json().exists() {
        return Err(WorldError::ManifestNotFound);
    }
    let _lock = WorldLockGuard::acquire(root)?;
    let roots = collect_roots(root, &paths, opts.keep_revisions)?;

    let mut store = FileNodeStore::new(root)?;
    let mut report = GcReport {
        dry_run: opts.dry_run,
        head_revision: roots.head,
        revisions_total: roots.total,
        revisions_retained: roots.retained,
        ..Default::default()
    };
    if !opts.dry_run {
        report.files_swept = store.sweep_unpublished()? as u64 + sweep_checkpoint_tmp(root);
    }
    let old_packs = store.published_packs()?;
    report.packs_before = old_packs.len() as u64;
    report.bytes_before = store.total_bytes();
    // Distinct digests: a node re-written by several commits sits in several packs.
    report.nodes_before = store.unique_node_count()?;

    // Mark, copying each live node into the new pack as soon as it is read.
    let target = opts.pack_target_bytes.unwrap_or(PACK_TARGET_BYTES);
    let mut live: HashSet<Digest> = HashSet::new();
    let mut stack: Vec<Digest> = Vec::new();
    for d in &roots.digests {
        if store.try_contains(d)? && live.insert(*d) {
            stack.push(*d);
            report.roots += 1;
        }
    }
    let mut new_packs = 0u32;
    let mut found = Vec::new();
    while let Some(d) = stack.pop() {
        let Some(bytes) = store.get_node(&d).map_err(storage)? else {
            continue;
        };
        report.live_bytes += bytes.len() as u64;
        found.clear();
        references(&bytes, &store, &live, &mut found)?;
        for r in found.drain(..) {
            if live.insert(r) {
                stack.push(r);
            }
        }
        if !opts.dry_run {
            store.put_new_node(d, bytes);
            if store.pending_bytes() >= target {
                store.flush()?;
                new_packs += 1;
                if new_packs == 1 {
                    inject(opts, GcCrashPoint::AfterFirstNewPack)?;
                }
            }
        }
    }
    report.live_nodes = live.len() as u64;
    report.dead_nodes = report.nodes_before.saturating_sub(report.live_nodes);
    if opts.dry_run {
        report.bytes_after = report.bytes_before;
        report.packs_after = report.packs_before;
        report.elapsed_ms = started.elapsed().as_millis() as u64;
        return Ok(report);
    }

    inject(opts, GcCrashPoint::BeforeFinalFlush)?;
    store.flush()?;
    inject(opts, GcCrashPoint::AfterCopyBeforeRetire)?;
    // Generation marker, durable BEFORE anything is retired: a session open across this GC
    // must notice even if we crash half way through the retirement.
    let floor = match opts.keep_revisions {
        None => read_gc_floor(&paths),
        Some(k) => read_gc_floor(&paths).max(if k == 0 {
            roots.head
        } else {
            roots.head.saturating_sub(k - 1)
        }),
    };
    write_gc_epoch(&paths, floor)?;
    let mut crash_mid = |i: usize| -> io::Result<()> {
        if i == 0 && opts.crash == Some(GcCrashPoint::MidRetire) {
            return Err(io::Error::other("injected gc crash: MidRetire"));
        }
        Ok(())
    };
    store.retire_packs(&old_packs, &mut crash_mid)?;
    inject(opts, GcCrashPoint::AfterRetire)?;
    if store.has_legacy_objects() {
        store.retire_legacy_objects()?;
    }
    write_gc_epoch(&paths, floor)?;
    report.packs_after = store.published_packs()?.len() as u64;
    report.bytes_after = store.total_bytes();
    drop(store);

    if opts.verify_after {
        verify_reachability(root, &roots, &live)?;
    }
    report.elapsed_ms = started.elapsed().as_millis() as u64;
    Ok(report)
}

/// Leftover `settlements.tmp.<pid>` files of readers that died while writing a checkpoint.
/// Only files untouched for a minute are removed (a live reader may be mid-write).
fn sweep_checkpoint_tmp(root: &Path) -> u64 {
    let mut removed = 0;
    let Ok(items) = std::fs::read_dir(root) else {
        return 0;
    };
    for item in items.flatten() {
        let name = item.file_name();
        if !name.to_string_lossy().starts_with("settlements.tmp.") {
            continue;
        }
        let old = item
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age.as_secs() >= 60);
        if old && std::fs::remove_file(item.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// After a real run: reopen the store (every pack, every live node present), then reopen the
/// world and prove what a reader needs is reachable: the restored network equals a replay
/// (`verify_state`), the idempotency trie reads end to end, and every retained revision's
/// relation, secondary-index and decision roots resolve through the store.
fn verify_reachability(
    root: &Path,
    roots: &Roots,
    live: &HashSet<Digest>,
) -> Result<(), WorldError> {
    let reopened = FileNodeStore::new(root)?;
    reopened.verify_store().map_err(map_pack_error)?;
    for d in live {
        if !reopened.try_contains(d)? {
            return Err(WorldError::State(super::state_store::StateError::Storage(
                soc_core::store::StorageError::MissingNode(*d),
            )));
        }
    }
    for d in &roots.trie_roots {
        TrieMap::<WorldKey, WorldTuple>::from_root_digest(*d, 0)
            .get_with_store(&WorldKey::new(Vec::new()), &reopened)
            .map_err(storage)?;
    }
    drop(reopened);
    // Never writes: the lock is already ours and caches are not refreshed by a check.
    let session = WorldSession::open_with_policy(
        root,
        OpenPolicy {
            force_replay: false,
            refresh_after_replay: false,
        },
    )?;
    session.verify_state()?;
    session.idempotency.walk(&session.node_store)?;
    Ok(())
}

/// Result of [`verify_store`].
pub use soc_core::store::StoreVerifyReport;

/// Audit every pack of the world at `dir` (framing, index chains, every body checksum).
/// Read-only; O(total bytes). Pack damage is `CorruptState`.
pub fn verify_store(dir: impl AsRef<Path>) -> Result<StoreVerifyReport, WorldError> {
    let store = FileNodeStore::new(dir.as_ref()).map_err(map_pack_error)?;
    store.verify_store().map_err(map_pack_error)
}

fn map_pack_error(e: io::Error) -> WorldError {
    if e.kind() == io::ErrorKind::InvalidData {
        WorldError::CorruptState(super::state_store::StateError::Io(e.to_string()))
    } else {
        WorldError::Io(e)
    }
}
