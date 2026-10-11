//! Error types for the persistent world runtime (ADR-0046, P3).

use brix_canon::{CanonError, Digest};
use std::fmt;

use super::types::WorldKey;

/// Injected crash points for persistence boundary fault-tolerance verification.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrashPoint {
    /// Crash after staging objects but before fsyncing them.
    BeforeObjectsFsync,
    /// Crash after objects fsync, but before revision journal record fsync.
    AfterObjectsFsyncBeforeRevisionFsync,
    /// Crash after durable decision delta, before revision publication.
    AfterDecisionDeltaFsyncBeforeRevisionFsync,
    /// Crash after revision journal record fsync, but before atomic HEAD rename.
    AfterRevisionFsyncBeforeHeadRename,
    /// Crash after atomic HEAD rename, but before parent directory fsync.
    AfterHeadRenameBeforeDirectoryFsync,
    /// Crash after the base relation and index objects are written, before any network
    /// state object (operator indexes, frontier, decisions, header) is.
    AfterBaseObjectsBeforeNetworkObjects,
    /// Crash after every object, network state header included, is written but before
    /// any is flushed.
    AfterNetworkObjectsBeforeFlush,
    /// State-cache refresh: header written, not yet flushed.
    SidecarBeforeFlush,
    /// State-cache refresh: objects flushed, sidecar written to its temp file only.
    SidecarAfterFlushBeforeRename,
    /// State-cache refresh: sidecar renamed into place.
    SidecarAfterRename,
    /// Settlement checkpoint refresh: written to its temp file only, not renamed into place.
    CheckpointAfterTmpBeforeRename,
}

impl fmt::Display for CrashPoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BeforeObjectsFsync => write!(f, "crash before objects fsync"),
            Self::AfterObjectsFsyncBeforeRevisionFsync => {
                write!(f, "crash after objects fsync before revision fsync")
            }
            Self::AfterDecisionDeltaFsyncBeforeRevisionFsync => {
                write!(f, "crash after decision delta fsync before revision fsync")
            }
            Self::AfterRevisionFsyncBeforeHeadRename => {
                write!(f, "crash after revision fsync before HEAD rename")
            }
            Self::AfterHeadRenameBeforeDirectoryFsync => {
                write!(f, "crash after HEAD rename before directory fsync")
            }
            Self::AfterBaseObjectsBeforeNetworkObjects => {
                write!(f, "crash after base objects before network objects")
            }
            Self::AfterNetworkObjectsBeforeFlush => {
                write!(f, "crash after network objects before flush")
            }
            Self::SidecarBeforeFlush => write!(f, "crash before state-cache flush"),
            Self::SidecarAfterFlushBeforeRename => {
                write!(f, "crash after state-cache flush before rename")
            }
            Self::SidecarAfterRename => write!(f, "crash after state-cache rename"),
            Self::CheckpointAfterTmpBeforeRename => {
                write!(
                    f,
                    "crash after settlement checkpoint temp write before rename"
                )
            }
        }
    }
}

/// Errors originating from the persistent world runtime.
#[derive(Debug)]
pub enum WorldError {
    /// Filesystem I/O error.
    Io(std::io::Error),
    /// World manifest `world.json` was not found.
    ManifestNotFound,
    /// World directory already exists when attempting `create`.
    WorldAlreadyExists,
    /// The expected base revision does not match the world's current HEAD.
    StaleBaseRevision { expected: u64, current: u64 },
    /// Contradictory operations within the same proposed batch.
    BatchConflict {
        relation: String,
        key: WorldKey,
        reason: String,
    },
    /// Target relation is not declared in the world manifest.
    UnknownRelation(String),
    /// Schema or contract validation failure.
    InvalidSchema(String),
    /// Revision record not found in the journal.
    RevisionNotFound(u64),
    /// Object corruption or digest mismatch.
    CorruptedObject(Digest),
    /// The HEAD file is missing or unparseable.
    CorruptedHead(String),
    /// Canonical encoding or decoding failure.
    Canon(CanonError),
    /// Deliberate simulated crash at a persistence boundary.
    InjectedCrash(CrashPoint),
    /// Pagination cursor string is malformed.
    InvalidCursor(String),
    /// Bulk staging upload failure.
    StagingError(String),
    /// JSON parsing or schema error.
    Json(String),
    /// World directory is locked by another writer.
    WorldLocked(String),
    /// A garbage collection ran underneath a cache write that was in progress; it was
    /// abandoned (a cache write only) and nothing was published.
    StaleSession(String),
    /// The revision's state was dropped on purpose by `gc --keep-revisions`: history, not
    /// damage (the journal records remain).
    RevisionCompacted(u64),
    /// Object file missing from disk.
    MissingObject(Digest),
    /// Revision record digest does not match its contents.
    CorruptedRevision {
        seq: u64,
        expected: Digest,
        actual: Digest,
    },
    /// Invalid upload id for staging.
    InvalidUploadId(String),
    /// Idempotency key reused with mismatched batch payload.
    IdempotencyConflict { key: String, reason: String },
    /// Target secondary index is not declared.
    UnknownSecondaryIndex(String),
    /// Missing or unextractable indexed field in tuple.
    MissingIndexField {
        relation: String,
        field: String,
        reason: String,
    },
    /// Invalid secondary index declaration in relation.
    InvalidIndexDeclaration(String),
    /// Relational operator network or deliberation error.
    NetworkError(String),
    /// Computational budget or work limit exhausted.
    BudgetExhausted,
    /// Persisted network-state store failure that is neither corruption nor absence
    /// (I/O while flushing or opening the state store).
    State(super::state_store::StateError),
    /// Persisted network state failed verification: a node did not hash to its address,
    /// failed canonical decoding, or a stored derivation does not match its operator.
    /// Never recoverable by retrying; the operator replays explicitly (`open_replay`).
    CorruptState(super::state_store::StateError),
    /// Persisted network state refers to a node or root the store does not hold.
    MissingState(super::state_store::StateError),
    /// Persisted network state was produced under a different validity key (program,
    /// execution profile, compiler or codec version) than the one opening it.
    ValidityMismatch(String),
    /// Persisted network state is intact but differs from a replay of the base facts
    /// (`WorldSession::verify_state`).
    StateMismatch(String),
}

impl fmt::Display for WorldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "world I/O error: {e}"),
            Self::ManifestNotFound => write!(f, "world manifest 'world.json' not found"),
            Self::WorldAlreadyExists => write!(f, "world directory already exists"),
            Self::StaleBaseRevision { expected, current } => {
                write!(
                    f,
                    "stale base revision: expected {expected}, current HEAD is {current}"
                )
            }
            Self::BatchConflict {
                relation,
                key,
                reason,
            } => {
                write!(
                    f,
                    "batch conflict in relation '{relation}' at key {:?}: {reason}",
                    key
                )
            }
            Self::UnknownRelation(r) => write!(f, "unknown relation '{r}'"),
            Self::InvalidSchema(s) => write!(f, "invalid world schema: {s}"),
            Self::RevisionNotFound(seq) => write!(f, "revision {seq} not found"),
            Self::CorruptedObject(d) => write!(f, "corrupted object digest {}", d.to_hex()),
            Self::CorruptedHead(s) => write!(f, "corrupted HEAD pointer: {s}"),
            Self::Canon(e) => write!(f, "canonical encoding error: {e:?}"),
            Self::InjectedCrash(cp) => write!(f, "injected crash point reached: {cp}"),
            Self::InvalidCursor(c) => write!(f, "invalid pagination cursor '{c}'"),
            Self::StagingError(s) => write!(f, "staging error: {s}"),
            Self::Json(s) => write!(f, "JSON error: {s}"),
            Self::WorldLocked(s) => write!(f, "world locked: {s}"),
            Self::StaleSession(s) => write!(f, "stale session: {s}"),
            Self::RevisionCompacted(seq) => write!(
                f,
                "revision {seq} was compacted away by garbage collection (--keep-revisions)"
            ),
            Self::MissingObject(d) => write!(f, "missing object digest {}", d.to_hex()),
            Self::CorruptedRevision {
                seq,
                expected,
                actual,
            } => {
                write!(
                    f,
                    "corrupted revision {seq}: expected digest {}, got {}",
                    expected.to_hex(),
                    actual.to_hex()
                )
            }
            Self::InvalidUploadId(s) => write!(f, "invalid upload id: {s}"),
            Self::IdempotencyConflict { key, reason } => {
                write!(f, "idempotency conflict for key '{key}': {reason}")
            }
            Self::UnknownSecondaryIndex(idx) => write!(f, "unknown secondary index '{idx}'"),
            Self::MissingIndexField {
                relation,
                field,
                reason,
            } => {
                write!(
                    f,
                    "missing indexed field '{field}' in relation '{relation}': {reason}"
                )
            }
            Self::InvalidIndexDeclaration(msg) => {
                write!(f, "invalid secondary index declaration: {msg}")
            }
            Self::NetworkError(msg) => write!(f, "network error: {msg}"),
            Self::BudgetExhausted => write!(f, "computational budget exhausted"),
            Self::State(e) => write!(f, "world state error: {e}"),
            Self::CorruptState(e) => write!(f, "corrupt persisted world state: {e}"),
            Self::MissingState(e) => write!(f, "missing persisted world state: {e}"),
            Self::ValidityMismatch(m) => write!(f, "persisted world state validity mismatch: {m}"),
            Self::StateMismatch(m) => write!(f, "persisted world state differs from replay: {m}"),
        }
    }
}

impl std::error::Error for WorldError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::State(e) | Self::CorruptState(e) | Self::MissingState(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for WorldError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<super::state_store::StateError> for WorldError {
    /// Classify: absence is `MissingState`, any verification/decoding failure is
    /// `CorruptState`, remaining I/O failures stay `State`.
    fn from(e: super::state_store::StateError) -> Self {
        use super::state_store::StateError as S;
        use soc_core::store::StorageError as St;
        match &e {
            S::Storage(St::MissingNode(_)) | S::RootNotPersisted(_) => Self::MissingState(e),
            S::Storage(St::CorruptedNode(_) | St::DecodeError(_)) | S::Codec(_) => {
                Self::CorruptState(e)
            }
            S::Storage(St::Io(_)) | S::Io(_) => Self::State(e),
        }
    }
}

impl From<CanonError> for WorldError {
    fn from(e: CanonError) -> Self {
        Self::Canon(e)
    }
}

impl From<brix_lower::relation_dag::RelationalLowerError> for WorldError {
    fn from(e: brix_lower::relation_dag::RelationalLowerError) -> Self {
        Self::NetworkError(e.to_string())
    }
}

impl From<soc_core::store::StorageError> for WorldError {
    fn from(e: soc_core::store::StorageError) -> Self {
        match e {
            soc_core::store::StorageError::MissingNode(d) => Self::MissingObject(d),
            soc_core::store::StorageError::CorruptedNode(d) => Self::CorruptedObject(d),
            soc_core::store::StorageError::DecodeError(ce) => Self::Canon(ce),
            soc_core::store::StorageError::Io(s) => Self::Io(std::io::Error::other(s)),
        }
    }
}
