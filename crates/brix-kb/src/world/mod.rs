//! Persistent world runtime implementation (ADR-0046, Stage P3).
//!
//! Provides the library session API ([`WorldSession`]), transactional batch mutation
//! envelope ([`WorldBatch`]), compact revision records ([`WorldRevision`]), chunked
//! bulk ingestion, and the bounded full-recompute correctness oracle ([`WorldOracle`]).

pub mod audit;
pub mod batch;
pub mod chunk_policy;
pub mod codec;
pub mod compact_deriv;
pub mod decision_codec;
pub mod decision_evidence;
pub mod error;
pub mod gc;
pub mod idempotency;
pub mod import_kb;
pub mod manifest;
pub mod network;
pub mod network_state;
pub mod oracle;
pub mod paths;
pub mod persistent;
pub mod policy_evidence;
pub mod policy_trace;
pub mod reference;
pub mod revision;
pub mod scoped_query;
pub mod session;
pub mod settlement_checkpoint;
pub mod stable_child_key;
pub mod staging;
pub mod state_cache;
pub mod state_codec;
pub mod state_store;
pub mod types;
pub mod verify;

pub use batch::{WorldBatch, WorldBatchOp, BATCH_SCHEMA};
pub use chunk_policy::{
    ChunkDecision, ChunkInput, ChunkPolicy, FixedChunkPolicy, HeuristicChunkPolicy,
    DEFAULT_APPLY_HYDRATED_BYTES, MIN_CHUNK_OPS,
};
pub use codec::{encode_secondary_key, extract_indexed_field, TupleRecord, TUPLE_MAGIC_V1};
pub use error::{CrashPoint, WorldError};
pub use manifest::{RelationDecl, WorldManifest, WORLD_PROFILE, WORLD_SCHEMA};
pub use network::{
    compute_candidate_calendar_key, compute_decision_root, decision_key, decision_tuple,
    decode_settled_decision, encode_decision_delta, eval_expr, BaseLookup, BatchCtx,
    CandidateEntry, CandidateExplanation, DecideBlock, DecisionExplanation, DerivationId,
    IntermediateTuple, NetworkDeltaReport, OperatorState, SettledDecision, TupleDelta, Value,
    WorldNetwork, WorldNetworkState,
};
pub use network_state::{
    NetworkStateHeader, OperatorRoots, ValidityKey, NETWORK_LOWERING_VERSION, NETWORK_STATE_SCHEMA,
};
pub use oracle::{DiffEvent, WorldOracle};
pub use paths::WorldPaths;
pub use revision::{
    SettlementStatus, WorldRevision, REVISION_SCHEMA, REVISION_SCHEMA_V2, REVISION_SCHEMA_V3,
};
pub use scoped_query::{
    named_query_identity_v1, world_revision_artifact_digest_v1,
    world_revision_artifact_preimage_v1, AuthorizedHostScope, HostContextExpectationV2,
    HostQueryLimitsV1, HostScopeProfileV1, NamedQueryArgsV1, QueryValueV1, ScopeError,
    ScopedCursorBindingsV1, ScopedQueryCursorV1, ScopedQueryPageRequestV1, ScopedQueryPageV1,
    HOST_SCOPE_PROFILE_PARTITION_V1, MAX_SCOPED_QUERY_CURSOR_TOKEN_BYTES,
    MAX_SCOPED_QUERY_EVAL_MATCHES, MAX_SCOPED_QUERY_EVAL_WORK, MAX_SCOPED_QUERY_INPUT_BYTES,
    MAX_SCOPED_QUERY_INPUT_ROWS, MAX_SCOPED_QUERY_OUTPUT_ROWS, MAX_SCOPED_QUERY_PAGE_ROWS,
    QUERY_PROGRAM_ARTIFACT_V1, QUERY_WORLD_ARTIFACT_V1, SCOPED_QUERY_ORDERING_V1,
};
pub use session::{
    CommitProfile, DiffPage, ExecModuleLoaderLimits, ExecProfileV1, OpenMode, OpenPolicy,
    QueryPage, RevisionReceipt, SecondaryIndexCursor, SecondaryIndexPage, WorldSession,
    WorldSnapshot, EXEC_PROFILE_SCHEMA,
};
pub use stable_child_key::CollectOrdering;
pub use types::{WorldCursor, WorldKey, WorldTuple};
