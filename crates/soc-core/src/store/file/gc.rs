//! Pack-level primitives for garbage collection and integrity audits.
//!
//! GC itself (what is live) is policy of the caller; this module only offers the physical
//! operations it needs: which packs are published, durable removal of a set of packs
//! (dropping every in-memory table derived from them), cleanup of unpublished leftovers, and
//! a full scan that re-checks every pack body against its recorded checksum.
//!
//! Crash model for a compaction built from these pieces: live nodes are first re-written to
//! fresh packs (published atomically by `flush`), and only then are the old packs unlinked one
//! by one. At every instant each live node is present in some published pack, so a store
//! opened after a crash holds a superset of the live set (old packs, new packs, or both).

use super::*;

/// What [`FileNodeStore::verify_store`] checked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StoreVerifyReport {
    pub packs: usize,
    pub nodes: u64,
    pub body_bytes: u64,
    pub legacy_nodes: u64,
}

impl FileNodeStore {
    /// Paths of the currently published packs (flushed, indexed), oldest name first.
    pub fn published_packs(&self) -> io::Result<Vec<PathBuf>> {
        let state = self
            .state
            .lock()
            .map_err(|_| invalid("poisoned pack index"))?;
        Ok(state.packs.iter().map(|p| p.path.to_path_buf()).collect())
    }

    /// Bytes buffered or written to the pending (unpublished) pack.
    pub fn pending_bytes(&self) -> u64 {
        self.state
            .lock()
            .ok()
            .and_then(|s| s.pending.as_ref().map(|p| p.end))
            .unwrap_or(0)
    }

    /// Delete unpublished leftovers (`*.tmp` packs and idx files) and `*.idx` files whose pack
    /// is gone. The caller must hold the world's writer lock: a concurrent writer's pending
    /// pack is itself a `.tmp` file. Returns the number of files removed.
    pub fn sweep_unpublished(&self) -> io::Result<usize> {
        let own_pending = {
            let state = self
                .state
                .lock()
                .map_err(|_| invalid("poisoned pack index"))?;
            state.pending.as_ref().map(|p| p.path.clone())
        };
        let mut removed = 0;
        for item in fs::read_dir(&self.objects_dir)? {
            let path = item?.path();
            if !path.is_file() {
                continue;
            }
            let ext = path.extension().and_then(|s| s.to_str());
            let stale = match ext {
                Some("tmp") => own_pending.as_ref() != Some(&path),
                Some("idx") => !path.with_extension("pack").exists(),
                _ => false,
            };
            if stale {
                fs::remove_file(&path)?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Durably remove `packs` (which must be published and flushed) and everything derived from
    /// them: their Bloom filters, sparse tables, open handles, the `known` set and the node
    /// cache. `after_each(i)` runs after the `i`-th pack (and its idx) is unlinked; an error
    /// from it aborts, leaving the remaining packs in place (used for crash injection).
    /// Returns the bytes freed. The caller must have re-written every live node elsewhere.
    pub fn retire_packs(
        &self,
        packs: &[PathBuf],
        after_each: &mut dyn FnMut(usize) -> io::Result<()>,
    ) -> io::Result<u64> {
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| invalid("poisoned pack index"))?;
            self.check()?;
            state.packs.retain(|p| !packs.contains(&*p.path));
            state.pack_files.retain(|(p, _)| !packs.contains(&**p));
            // Idx handles are keyed by position in `packs`, which just shifted.
            state.idx_files.clear();
            state.known.clear();
            state.node_cache.clear();
            state.cache_lru.clear();
        }
        let mut freed = 0;
        for (i, pack) in packs.iter().enumerate() {
            let idx = pack.with_extension("idx");
            for path in [pack, &idx] {
                match fs::metadata(path) {
                    Ok(meta) => {
                        freed += meta.len();
                        fs::remove_file(path)?;
                    }
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
            }
            after_each(i)?;
        }
        File::open(&self.objects_dir)?.sync_all()?;
        Ok(freed)
    }

    /// Delete every legacy loose object (`objects/xx/*.bin`). Only for GC, after the live ones
    /// were re-written into packs. Returns the bytes freed.
    pub fn retire_legacy_objects(&self) -> io::Result<u64> {
        let mut freed = 0;
        for item in fs::read_dir(&self.objects_dir)? {
            let path = item?.path();
            let name_ok = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.len() == 2 && u8::from_str_radix(n, 16).is_ok());
            if path.is_dir() && name_ok {
                for f in fs::read_dir(&path)? {
                    freed += f?.metadata()?.len();
                }
                fs::remove_dir_all(&path)?;
            }
        }
        File::open(&self.objects_dir)?.sync_all()?;
        Ok(freed)
    }

    /// Whether any legacy loose object may exist.
    pub fn has_legacy_objects(&self) -> bool {
        self.legacy_shards.iter().any(|f| f.load(Ordering::Relaxed))
    }

    /// Full audit of the physical store: every published pack's framing and index chain, the
    /// sorted idx against its pack, and every body against its recorded checksum
    /// (O(total bytes)). Damage is `InvalidData`. Does not decode nodes, so it cannot tell a
    /// wrongly addressed node from a right one; the Merkle digest is checked on trie reads.
    pub fn verify_store(&self) -> io::Result<StoreVerifyReport> {
        self.verify_indexes()?;
        let paths = self.published_packs()?;
        let mut report = StoreVerifyReport {
            packs: paths.len(),
            ..Default::default()
        };
        for path in paths {
            let (entries, _) = self.scan_pack(&path)?;
            let file = File::open(&path)?;
            for (_, entry) in &entries {
                self.read_entry(&file, entry)?;
                report.nodes += 1;
                report.body_bytes += entry.length;
            }
        }
        for item in fs::read_dir(&self.objects_dir)? {
            let path = item?.path();
            if path.is_dir() {
                report.legacy_nodes += fs::read_dir(&path)?
                    .flatten()
                    .filter(|f| f.path().extension().and_then(|s| s.to_str()) == Some("bin"))
                    .count() as u64;
            }
        }
        Ok(report)
    }
}
