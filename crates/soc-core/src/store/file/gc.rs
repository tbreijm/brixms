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
            state.cached_bytes = 0;
            // A decoded-cache hit is taken as proof a node is stored (trie persistence), which
            // no longer holds for nodes that lived only in the retired packs.
            self.decoded.clear();
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

    /// Re-read the set of published packs from disk and drop every table and cache derived
    /// from the old view (Bloom filters, sparse tables, idx handles, the `known` set, the raw
    /// and decoded node caches). For a handle that outlived a compaction done by another
    /// handle or process: afterwards no cache can vouch for a node the compaction removed.
    /// A pending (unflushed) pack is kept. A latched failure that was *damage* (checksum or
    /// format, not an OS error) is cleared: damage is re-detected by the next read of the
    /// damaged node, so nothing is hidden, but one unreadable cache node need not poison the
    /// handle for good. An I/O latch (possibly a torn write) is kept.
    pub fn reload_from_disk(&self) -> io::Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| invalid("poisoned pack index"))?;
        let mut packs = Vec::new();
        for item in fs::read_dir(&self.objects_dir)? {
            let path = item?.path();
            if path.extension().and_then(|s| s.to_str()) == Some("pack") {
                packs.push(path);
            }
        }
        packs.sort();
        let mut fresh = Vec::with_capacity(packs.len());
        for path in packs {
            fresh.push(self.open_pack(&path)?);
        }
        for (i, flag) in self.legacy_shards.iter().enumerate() {
            let present = self.objects_dir.join(format!("{i:02x}")).is_dir();
            flag.store(present, Ordering::Relaxed);
        }
        state.packs = fresh;
        state.idx_files.clear();
        state.pack_files.clear();
        state.known.clear();
        state.node_cache.clear();
        state.cache_lru.clear();
        state.cached_bytes = 0;
        self.decoded.clear();
        self.clear_corruption_latch();
        Ok(())
    }

    /// Forget a latched failure that was *damage* (checksum or format), keeping an I/O latch.
    /// For callers that treat the unreadable node as a cache miss: damage is re-detected by
    /// the next read of the damaged node, so nothing is hidden, but one unreadable cache node
    /// no longer poisons every other read of the handle.
    pub fn clear_corruption_latch(&self) {
        if self.corrupt.load(Ordering::SeqCst) {
            self.corrupt.store(false, Ordering::SeqCst);
            self.failed.store(false, Ordering::SeqCst);
        }
    }

    /// Number of distinct digests across the published packs (a k-way merge of the sorted idx
    /// files, constant memory) plus legacy loose objects. [`Self::node_count`] counts entries,
    /// so a node re-written by several commits counts once per pack there.
    pub fn unique_node_count(&self) -> io::Result<u64> {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;
        let idx_paths: Vec<(PathBuf, u64)> = {
            let state = self
                .state
                .lock()
                .map_err(|_| invalid("poisoned pack index"))?;
            state
                .packs
                .iter()
                .map(|p| (p.idx_path.clone(), p.count))
                .collect()
        };
        struct Cursor {
            reader: BufReader<File>,
            left: u64,
        }
        fn next(c: &mut Cursor) -> io::Result<Option<Digest>> {
            if c.left == 0 {
                return Ok(None);
            }
            let mut rec = [0u8; RECORD as usize];
            c.reader.read_exact(&mut rec)?;
            c.left -= 1;
            Ok(Some(digest_at(&rec, 0)))
        }
        let mut cursors = Vec::with_capacity(idx_paths.len());
        let mut heap = BinaryHeap::new();
        for (i, (path, count)) in idx_paths.iter().enumerate() {
            let mut file = File::open(path)?;
            file.seek(SeekFrom::Start(IDX_HEADER))?;
            let mut c = Cursor {
                reader: BufReader::with_capacity(64 * 1024, file),
                left: *count,
            };
            if let Some(d) = next(&mut c)? {
                heap.push(Reverse((d, i)));
            }
            cursors.push(c);
        }
        let mut unique = 0u64;
        let mut last: Option<Digest> = None;
        while let Some(Reverse((d, i))) = heap.pop() {
            if last != Some(d) {
                unique += 1;
                last = Some(d);
            }
            if let Some(n) = next(&mut cursors[i])? {
                heap.push(Reverse((n, i)));
            }
        }
        // Loose legacy objects: counted as stored (rare, and they may repeat packed nodes).
        let mut loose = 0u64;
        for item in fs::read_dir(&self.objects_dir)? {
            let path = item?.path();
            if path.is_dir() {
                loose += fs::read_dir(&path)?
                    .flatten()
                    .filter(|f| f.path().extension().and_then(|s| s.to_str()) == Some("bin"))
                    .count() as u64;
            }
        }
        Ok(unique + loose)
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
