//! Snapshot/MVCC bookkeeping types (Ring0 §1.7). Part III §4: "A transaction
//! reads one settled snapshot and commits atomically as a later revision, or
//! fails." Part XXVIII §28.3: "MVCC snapshot reads, optimistic commit
//! validation, history-invisible batching."
//!
//! This module carries the *bookkeeping* — what a snapshot identifies, and
//! which revisions currently have live readers pinning them for retention —
//! not the settle scheduler or the commit/conflict-validation pipeline
//! (both named in the full `OWNER.md` contract, out of this Day-1 slice; see
//! the crate root docs for what is deferred).

use brix_canon::{CanonWriter, Canonical, SnapshotId};

use crate::ids::{DataRevision, ProgramRevision};

/// One settled view a reader can be bound to (Part III §4:
/// `SnapshotId = (namespace, DataRevision, ProgramRevision)`; "it appears on
/// every query result, watch delta, explanation, and export").
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Snapshot {
    /// The namespace this snapshot belongs to.
    pub namespace: String,
    /// The committed data revision.
    pub data_revision: DataRevision,
    /// The program revision the data was settled under.
    pub program_revision: ProgramRevision,
}

impl Snapshot {
    /// Derive this snapshot's canon [`SnapshotId`].
    pub fn id(&self) -> SnapshotId {
        let mut w = CanonWriter::new();
        w.write_ident(&self.namespace);
        self.data_revision.canon_write(&mut w);
        self.program_revision.canon_write(&mut w);
        SnapshotId::from_canon(&w.finish())
    }
}

impl Canonical for Snapshot {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_ident(&self.namespace);
        self.data_revision.canon_write(w);
        self.program_revision.canon_write(w);
    }
}

/// Per-namespace MVCC bookkeeping: which revision is currently committed,
/// and which older revisions still have a snapshot reader pinning them (so
/// retention/compaction knows what it must not fold away yet — Part XXVIII
/// §28.3: "compaction folds below retention into sealed `CompactionRecord`s
/// with `audit` pins").
#[derive(Default)]
pub struct Mvcc {
    committed: Option<DataRevision>,
    // A multiset: several readers may pin the same revision. `BTreeMap` of
    // revision -> open reader count keeps this a semantic-path-legal
    // structure (no `HashMap`) and makes "is anything pinning revision r"
    // and "the oldest pinned revision" both cheap.
    pins: std::collections::BTreeMap<DataRevision, u32>,
}

impl Mvcc {
    /// Fresh bookkeeping for an empty namespace.
    pub fn new() -> Self {
        Self::default()
    }

    /// The most recently published revision, if any transaction has
    /// committed yet.
    pub fn committed(&self) -> Option<DataRevision> {
        self.committed
    }

    /// Record that a transaction published `revision`. Revisions must be
    /// published in order — the same discipline the revision log enforces
    /// (`RevisionLog::append`); `Mvcc` re-checks it locally so a caller
    /// wiring these two together independently still gets the invariant.
    pub fn publish(&mut self, revision: DataRevision) {
        debug_assert!(
            self.committed.is_none_or(|c| revision == c.next()),
            "revisions must publish in monotone order"
        );
        self.committed = Some(revision);
    }

    /// Pin `revision` for a new snapshot reader (a reader binds to it and
    /// must keep seeing it even if later revisions publish and compaction
    /// wants to reclaim older history).
    pub fn pin(&mut self, revision: DataRevision) {
        *self.pins.entry(revision).or_insert(0) += 1;
    }

    /// Release one reader's pin on `revision`.
    pub fn unpin(&mut self, revision: DataRevision) {
        if let Some(count) = self.pins.get_mut(&revision) {
            *count -= 1;
            if *count == 0 {
                self.pins.remove(&revision);
            }
        }
    }

    /// The oldest revision any live reader still needs — the retention
    /// floor compaction must respect. `None` means nothing is pinned and
    /// compaction is free to fold everything below `committed()`.
    pub fn retention_floor(&self) -> Option<DataRevision> {
        self.pins.keys().next().copied()
    }

    /// Whether `revision` currently has at least one open reader.
    pub fn is_pinned(&self, revision: DataRevision) -> bool {
        self.pins.contains_key(&revision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brix_canon::Domain;

    fn pr(tag: &[u8]) -> ProgramRevision {
        ProgramRevision(brix_canon::Digest::of(Domain::Value, tag))
    }

    #[test]
    fn snapshot_id_is_deterministic() {
        let s = Snapshot {
            namespace: "default".into(),
            data_revision: DataRevision(3),
            program_revision: pr(b"p1"),
        };
        assert_eq!(s.id(), s.id());
    }

    #[test]
    fn distinct_namespaces_get_distinct_snapshot_ids() {
        let a = Snapshot {
            namespace: "a".into(),
            data_revision: DataRevision(1),
            program_revision: pr(b"p"),
        };
        let b = Snapshot {
            namespace: "b".into(),
            data_revision: DataRevision(1),
            program_revision: pr(b"p"),
        };
        assert_ne!(a.id(), b.id());
    }

    #[test]
    fn pin_unpin_tracks_retention_floor() {
        let mut mvcc = Mvcc::new();
        mvcc.publish(DataRevision(0));
        mvcc.publish(DataRevision(1));
        mvcc.publish(DataRevision(2));

        assert_eq!(mvcc.retention_floor(), None);
        mvcc.pin(DataRevision(1));
        mvcc.pin(DataRevision(2));
        assert_eq!(mvcc.retention_floor(), Some(DataRevision(1)));

        mvcc.unpin(DataRevision(1));
        assert_eq!(mvcc.retention_floor(), Some(DataRevision(2)));
        assert!(!mvcc.is_pinned(DataRevision(1)));
        assert!(mvcc.is_pinned(DataRevision(2)));
    }

    #[test]
    fn shared_pins_require_matching_unpins() {
        let mut mvcc = Mvcc::new();
        mvcc.pin(DataRevision(5));
        mvcc.pin(DataRevision(5));
        assert!(mvcc.is_pinned(DataRevision(5)));
        mvcc.unpin(DataRevision(5));
        assert!(mvcc.is_pinned(DataRevision(5)), "still one reader left");
        mvcc.unpin(DataRevision(5));
        assert!(!mvcc.is_pinned(DataRevision(5)));
    }
}
