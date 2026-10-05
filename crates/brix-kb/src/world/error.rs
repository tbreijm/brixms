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
}

impl fmt::Display for CrashPoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BeforeObjectsFsync => write!(f, "crash before objects fsync"),
            Self::AfterObjectsFsyncBeforeRevisionFsync => {
                write!(f, "crash after objects fsync before revision fsync")
            }
            Self::AfterDecisionDeltaFsyncBeforeRevisionFsync => write!(f, "crash after decision delta fsync before revision fsync"),
            Self::AfterRevisionFsyncBeforeHeadRename => {
                write!(f, "crash after revision fsync before HEAD rename")
            }
            Self::AfterHeadRenameBeforeDirectoryFsync => {
                write!(f, "crash after HEAD rename before directory fsync")
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
        }
    }
}

impl std::error::Error for WorldError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for WorldError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
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
