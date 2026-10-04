//! Persistent world runtime implementation (ADR-0046, Stage P3).
//!
//! Provides the library session API ([`WorldSession`]), transactional batch mutation
//! envelope ([`WorldBatch`]), compact revision records ([`WorldRevision`]), chunked
//! bulk ingestion, and the bounded full-recompute correctness oracle ([`WorldOracle`]).

pub mod batch;
pub mod codec;
pub mod error;
pub mod manifest;
pub mod network;
pub mod oracle;
pub mod paths;
pub mod persistent;
pub mod revision;
pub mod session;
pub mod staging;
pub mod types;

pub use batch::{WorldBatch, WorldBatchOp, BATCH_SCHEMA};
pub use codec::{encode_secondary_key, extract_indexed_field, TupleRecord, TUPLE_MAGIC_V1};
pub use error::{CrashPoint, WorldError};
pub use manifest::{RelationDecl, WorldManifest, WORLD_PROFILE, WORLD_SCHEMA};
pub use network::{
    compute_candidate_calendar_key, compute_decision_root, eval_expr, CandidateEntry,
    CandidateExplanation, DecideBlock, DecisionExplanation, DerivationId, IntermediateTuple,
    NetworkDeltaReport, OperatorState, SettledDecision, TupleDelta, Value, WorldNetwork,
    WorldNetworkState,
};
pub use oracle::{DiffEvent, WorldOracle};
pub use paths::WorldPaths;
pub use revision::{SettlementStatus, WorldRevision, REVISION_SCHEMA};
pub use session::{
    DiffPage, QueryPage, RevisionReceipt, SecondaryIndexCursor, SecondaryIndexPage, WorldSession,
    WorldSnapshot,
};
pub use types::{WorldCursor, WorldKey, WorldTuple};
