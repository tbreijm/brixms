//! The revision log: canon-encoded, append-only (Ring0 §1.7; Part XXVIII
//! §28.3: "the revision log (ground facts only, canonical bytes) is truth;
//! checkpoints are optimization; recovery = checkpoint + tail replay +
//! settle").
//!
//! Only **ground** operations are logged — asserts, retractions,
//! supersessions, and Driver-committed protocol outcomes (which are ground
//! claims from the engine's point of view, made under a capability
//! boundary). Derived structure is never logged: it is recomputed by
//! resettling logged ground state through the current `ProgramRevision`,
//! which is exactly what makes recovery = "checkpoint + tail replay +
//! settle" correct and is why `brix-canon`'s doc comment says "the log needs
//! no second serializer, ever" — every [`LogOp`] already implements
//! `Canonical`.
//!
//! This module ships the format plus an in-memory implementation
//! ([`InMemoryRevisionLog`]). An mmap-backed implementation is explicitly a
//! follow-up (Ring0 §1.7 lists "mmap" alongside "canon-encoded, append-only"
//! as the target shape) — [`RevisionLog`] is the seam it will implement
//! against; nothing above this trait should need to change when it lands.

use std::collections::BTreeMap;

use brix_canon::{CanonWriter, Canonical, EdgeId};

use crate::ids::{DataRevision, RelationRef, TransactionId};
use crate::value::EdgeRoleTuple;

/// One ground-level effect of a committed transaction (Part VII §2:
/// `ensure`/`fresh`/`assert`/`set`/`retract`/`supersede`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum LogOp {
    /// A new ground claim (`assert`, or the assert half of `set`).
    Assert {
        /// The relation the claim belongs to.
        relation: RelationRef,
        /// The claimed edge's role bindings.
        roles: EdgeRoleTuple,
        /// The resulting edge identity (`GraphCore::edge_id` over
        /// `relation`/`roles` — logged rather than recomputed on read so a
        /// log reader never needs a `GraphCore` just to know the identity
        /// of what it is replaying).
        edge: EdgeId,
        /// The retry-stable claim identity `assert` returns.
        claim: brix_canon::ClaimId,
    },
    /// Withdraw one source's claim (`retract`, consuming a `ClaimRef`).
    Retract {
        /// The claim being withdrawn.
        claim: brix_canon::ClaimId,
    },
    /// Explicit lineage (`supersede newEdge over oldEdge`, and the
    /// supersede half of `set`).
    Supersede {
        /// The claim asserted in this transaction.
        newer: brix_canon::ClaimId,
        /// The claim it supersedes.
        older: brix_canon::ClaimId,
    },
}

impl Canonical for LogOp {
    fn canon_write(&self, w: &mut CanonWriter) {
        match self {
            LogOp::Assert {
                relation,
                roles,
                edge,
                claim,
            } => {
                w.write_uint(0);
                relation.canon_write(w);
                roles.canon_write(w);
                w.write_bytes(edge.digest().as_bytes());
                w.write_bytes(claim.digest().as_bytes());
            }
            LogOp::Retract { claim } => {
                w.write_uint(1);
                w.write_bytes(claim.digest().as_bytes());
            }
            LogOp::Supersede { newer, older } => {
                w.write_uint(2);
                w.write_bytes(newer.digest().as_bytes());
                w.write_bytes(older.digest().as_bytes());
            }
        }
    }
}

/// One committed transaction's ground effects, at the revision they
/// published as (Part III §4: "A revision is published fully settled or not
/// at all"). `ops` is in operation-ordinal order — the same order `ClaimId`
/// derives from (Part III §3: `ClaimId = Hash(transaction intent, operation
/// ordinal, source scope)`), so replaying a `LogEntry` in `ops` order
/// reproduces the same claim identities a fresh commit would have.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LogEntry {
    /// The revision this transaction published as.
    pub revision: DataRevision,
    /// The committing transaction's intent identity.
    pub transaction: TransactionId,
    /// Ground effects, in operation-ordinal order.
    pub ops: Vec<LogOp>,
}

impl Canonical for LogEntry {
    fn canon_write(&self, w: &mut CanonWriter) {
        self.revision.canon_write(w);
        self.transaction.canon_write(w);
        w.write_uint(self.ops.len() as u64);
        for op in &self.ops {
            op.canon_write(w);
        }
    }
}

/// Why an [`LogEntry`] was rejected by [`RevisionLog::append`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RevisionLogError {
    /// The log already has an entry at this (or a later) revision — the log
    /// is append-only and revisions are monotone.
    NotMonotone {
        attempted: DataRevision,
        head: DataRevision,
    },
}

/// The append-only, canon-encoded revision log (Ring0 §1.7). Implementors
/// own persistence; this crate ships [`InMemoryRevisionLog`] now and an
/// mmap-backed implementation is deferred (see module docs).
pub trait RevisionLog {
    /// Append `entry`. Fails if `entry.revision` does not immediately
    /// follow the current head (append-only, monotone).
    fn append(&mut self, entry: LogEntry) -> Result<(), RevisionLogError>;

    /// The most recently published revision, or `None` if the log is empty.
    fn head(&self) -> Option<DataRevision>;

    /// Look up the entry published at `revision`.
    fn get(&self, revision: DataRevision) -> Option<&LogEntry>;

    /// Iterate all entries from `from` (inclusive) to the head, in revision
    /// order — the shape `recovery = checkpoint + tail replay + settle`
    /// needs.
    fn iter_from(&self, from: DataRevision) -> Box<dyn Iterator<Item = &LogEntry> + '_>;
}

/// An in-memory [`RevisionLog`]. Entries are canon-encodable via
/// [`Canonical`] on [`LogEntry`]; this implementation keeps them as typed
/// Rust values rather than raw bytes so tests and callers don't pay an
/// encode/decode round trip until something actually needs the bytes (e.g.
/// hashing a checkpoint, or a future mmap-backed implementation adopting the
/// same encoding).
#[derive(Default)]
pub struct InMemoryRevisionLog {
    // Keyed by revision (not just a `Vec`) so `get`/`iter_from` are simple
    // and the monotonicity check has one obvious source of truth; the
    // append-only discipline is enforced in `append`, not by the container.
    entries: BTreeMap<DataRevision, LogEntry>,
}

impl InMemoryRevisionLog {
    /// An empty log.
    pub fn new() -> Self {
        Self::default()
    }
}

impl RevisionLog for InMemoryRevisionLog {
    fn append(&mut self, entry: LogEntry) -> Result<(), RevisionLogError> {
        // An empty log accepts any single revision as its genesis (a fresh
        // log may attach after a checkpoint at any revision); a non-empty
        // log only ever accepts exactly the next one.
        if let Some(head) = self.head() {
            if entry.revision != head.next() {
                return Err(RevisionLogError::NotMonotone {
                    attempted: entry.revision,
                    head,
                });
            }
        }
        self.entries.insert(entry.revision, entry);
        Ok(())
    }

    fn head(&self) -> Option<DataRevision> {
        self.entries.keys().next_back().copied()
    }

    fn get(&self, revision: DataRevision) -> Option<&LogEntry> {
        self.entries.get(&revision)
    }

    fn iter_from(&self, from: DataRevision) -> Box<dyn Iterator<Item = &LogEntry> + '_> {
        Box::new(self.entries.range(from..).map(|(_, e)| e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brix_canon::{ClaimId, Digest, Domain};

    fn claim(tag: &[u8]) -> ClaimId {
        ClaimId::from_canon(tag)
    }

    fn txn(tag: &[u8]) -> TransactionId {
        TransactionId(Digest::of(Domain::Value, tag))
    }

    fn entry(rev: u64) -> LogEntry {
        LogEntry {
            revision: DataRevision(rev),
            transaction: txn(format!("t{rev}").as_bytes()),
            ops: vec![LogOp::Retract {
                claim: claim(format!("c{rev}").as_bytes()),
            }],
        }
    }

    #[test]
    fn append_requires_monotone_revisions() {
        let mut log = InMemoryRevisionLog::new();
        assert!(log.append(entry(0)).is_ok());
        assert!(log.append(entry(1)).is_ok());
        // Skipping a revision is rejected.
        let err = log.append(entry(3)).unwrap_err();
        assert_eq!(
            err,
            RevisionLogError::NotMonotone {
                attempted: DataRevision(3),
                head: DataRevision(1)
            }
        );
        // Re-appending an already-published revision is rejected too
        // (append-only).
        let err = log.append(entry(1)).unwrap_err();
        assert_eq!(
            err,
            RevisionLogError::NotMonotone {
                attempted: DataRevision(1),
                head: DataRevision(1)
            }
        );
    }

    #[test]
    fn get_and_iter_from_see_published_entries() {
        let mut log = InMemoryRevisionLog::new();
        for r in 0..5 {
            log.append(entry(r)).unwrap();
        }
        assert_eq!(log.head(), Some(DataRevision(4)));
        assert!(log.get(DataRevision(2)).is_some());
        assert!(log.get(DataRevision(9)).is_none());

        let tail: Vec<DataRevision> = log.iter_from(DataRevision(2)).map(|e| e.revision).collect();
        assert_eq!(
            tail,
            vec![DataRevision(2), DataRevision(3), DataRevision(4)]
        );
    }

    #[test]
    fn log_entry_canon_round_trips_bytes_deterministically() {
        let e = entry(7);
        let a = e.canon_bytes();
        let b = e.canon_bytes();
        assert_eq!(a, b);
    }
}
