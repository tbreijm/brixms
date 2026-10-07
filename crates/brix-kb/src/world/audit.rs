//! The world audit transport bundle `brix.world.audit-bundle@1` (P6 step 5).
//!
//! Mirrors ADR-0026 ⟨D-BUNDLE⟩/⟨D-SNAPSHOT⟩: transport what is needed to
//! **re-derive** independent evidence for `brix.world@1`, never the derived
//! artifacts themselves (trie nodes, `objects/`, operator states, supports,
//! candidate frontiers, or any `WorldNetwork` internals). See
//! `docs/planning/p6-audit-contract.md` §1, §3, §4 for the normative format
//! this module implements, and `spec/adr/ADR-0026_Audit_Input_Transport_Bundle.md`
//! for the transport discipline it follows (⟨D-DECODELIMITS⟩: every bound
//! fires before the work it governs).
//!
//! # Why free text goes through `write_bytes`/`read_bytes`, not `write_ident`
//!
//! [`brix_canon::CanonWriter::write_ident`], `write_str`, `write_int`, `write_bool`
//! and `write_list` have no public decode counterpart in `brix_canon` (only
//! `read_uint`, `read_uint128`, and `read_bytes` are public on
//! [`brix_canon::CanonReader`], plus the free function [`brix_canon::read_decimal`]).
//! This module therefore encodes every text field as a raw length-prefixed byte
//! string via `write_bytes`/`read_bytes` with explicit UTF-8 validation on
//! decode, every signed integer as a fixed 8-byte big-endian two's-complement
//! blob, and every `FiniteF64` as its 8-byte big-endian bit pattern via the
//! public [`brix_canon::FiniteF64::bits`] accessor — never by reconstructing
//! `brix-canon`'s internal order-preserving integer codec. `Decimal` values
//! reuse the public [`brix_canon::read_decimal`] function instead.
//!
//! `WorldManifest` and `WorldRevision` are each embedded as their existing
//! `to_json()`/`from_json()` round trip (length-prefixed as one `write_bytes`
//! frame): `WorldManifest::from_json` already validates the manifest, and
//! `WorldRevision::from_json` already recomputes and checks `revision_digest`
//! against its stored fields — so decoding a `RevisionEntryV1` recomputes the
//! revision digest for free, through the existing authority, rather than this
//! module re-deriving `WorldRevision`'s hash scheme a second time.

use std::collections::BTreeMap;

use brix_canon::{read_decimal, CanonError, CanonReader, CanonWriter, Digest, Domain, FiniteF64};

use super::error::WorldError;
use super::manifest::WorldManifest;
use super::network::Value;
use super::revision::WorldRevision;
use super::session::WorldSession;
use super::types::{WorldKey, WorldTuple};

/// The fixed marker opening a [`WorldAuditBundleV1`] preimage
/// (`docs/planning/p6-audit-contract.md` §1). Frozen v1 ABI.
pub const BUNDLE_MARKER_V1: &[u8] = b"brix.world.audit-bundle";

/// The bundle format version.
pub const BUNDLE_VERSION_V1: u64 = 1;

/// The one v1 bundle profile.
pub const BUNDLE_PROFILE_V1: &str = "brix.world.audit-bundle@1";

// ---------------------------------------------------------------------------
// Decode limits (ADR-0026 ⟨D-DECODELIMITS⟩ applied to `brix.world.audit-bundle@1`)
// ---------------------------------------------------------------------------

/// Bounds on the work the bundle decoder may perform.
///
/// Noncanonical: this type contributes to no identity and is never written by
/// a `CanonWriter`. Every bound is enforced **before** the work it governs,
/// not after it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WorldAuditDecodeLimits {
    /// Maximum total bundle length in bytes. Checked before reading or decoding.
    pub max_total_bundle_bytes: usize,
    /// Maximum number of revision entries in the bundle.
    pub max_revisions: usize,
    /// Maximum entries in one revision's `source_delta` or `decision_delta`.
    pub max_delta_entries_per_revision: usize,
    /// Maximum bytes for one transported tuple value.
    pub max_tuple_bytes: usize,
    /// Maximum rows (relation entries or decision entries) in a checkpoint's
    /// full state.
    pub max_checkpoint_rows: usize,
    /// Maximum cumulative bytes across every program closure source file.
    pub max_sources_bytes: usize,
    /// Maximum cumulative decision entries across every revision's
    /// `decision_delta` plus the checkpoint decision map, if present.
    pub max_total_decisions: usize,
}

impl WorldAuditDecodeLimits {
    /// Default limits: 64 MiB total bundle, 1,000,000 revisions, 1,000,000
    /// delta entries per revision, 1 MiB per tuple, 10,000,000 checkpoint
    /// rows, 16 MiB cumulative source bytes, 10,000,000 cumulative decisions.
    pub const fn strict() -> Self {
        Self {
            max_total_bundle_bytes: 64 * 1024 * 1024,
            max_revisions: 1_000_000,
            max_delta_entries_per_revision: 1_000_000,
            max_tuple_bytes: 1024 * 1024,
            max_checkpoint_rows: 10_000_000,
            max_sources_bytes: 16 * 1024 * 1024,
            max_total_decisions: 10_000_000,
        }
    }
}

impl Default for WorldAuditDecodeLimits {
    fn default() -> Self {
        Self::strict()
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors decoding or validating a [`WorldAuditBundleV1`].
#[derive(Debug)]
pub enum WorldAuditBundleError {
    TotalBundleBytesExceeded { limit: usize, found: usize },
    RevisionsExceeded { limit: usize, found: usize },
    DeltaEntriesExceeded { limit: usize, found: usize },
    TupleBytesExceeded { limit: usize, found: usize },
    CheckpointRowsExceeded { limit: usize, found: usize },
    SourcesBytesExceeded { limit: usize, found: usize },
    TotalDecisionsExceeded { limit: usize, found: usize },
    CountOverflow,
    CumulativeOverflow,
    BadMarker,
    UnknownVersion(u64),
    UnknownProfile,
    BadDigestLength(usize),
    InvalidUtf8,
    TrailingBytes,
    TrailingBytesInFrame(&'static str),
    UnknownScopeTag(u64),
    UnknownValueTag(u64),
    SourcesNotSorted,
    NonContiguousRevisionSeq { expected: u64, found: u64 },
    OutOfOrderSourceDelta,
    OutOfOrderDecisionDelta,
    RevisionCountMismatch { expected: u64, found: u64 },
    HeadRevisionMismatch,
    EmptyScopeHeadMismatch,
    Manifest(WorldError),
    Revision(WorldError),
    Canon(CanonError),
}

impl std::fmt::Display for WorldAuditBundleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for WorldAuditBundleError {}

impl From<CanonError> for WorldAuditBundleError {
    fn from(e: CanonError) -> Self {
        WorldAuditBundleError::Canon(e)
    }
}

// ---------------------------------------------------------------------------
// Small encode/decode helpers (public-API only; see module docs)
// ---------------------------------------------------------------------------

fn write_text(w: &mut CanonWriter, s: &str) {
    w.write_bytes(s.as_bytes());
}

fn read_text<'a>(r: &mut CanonReader<'a>) -> Result<String, WorldAuditBundleError> {
    let b = r.read_bytes()?;
    std::str::from_utf8(b)
        .map(|s| s.to_string())
        .map_err(|_| WorldAuditBundleError::InvalidUtf8)
}

fn write_digest(w: &mut CanonWriter, d: &Digest) {
    w.write_bytes(d.as_bytes());
}

fn read_digest(r: &mut CanonReader<'_>) -> Result<Digest, WorldAuditBundleError> {
    let b = r.read_bytes()?;
    let arr: [u8; 32] = b
        .try_into()
        .map_err(|_| WorldAuditBundleError::BadDigestLength(b.len()))?;
    Ok(Digest::from_bytes(arr))
}

fn read_count(
    r: &mut CanonReader<'_>,
    limit: usize,
    on_exceed: impl FnOnce(usize) -> WorldAuditBundleError,
) -> Result<usize, WorldAuditBundleError> {
    let n64 = r.read_uint()?;
    let n = usize::try_from(n64).map_err(|_| WorldAuditBundleError::CountOverflow)?;
    if n > limit {
        return Err(on_exceed(n));
    }
    Ok(n)
}

fn write_i64(w: &mut CanonWriter, v: i64) {
    w.write_bytes(&v.to_be_bytes());
}

fn read_i64(r: &mut CanonReader<'_>) -> Result<i64, WorldAuditBundleError> {
    let b = r.read_bytes()?;
    let arr: [u8; 8] = b
        .try_into()
        .map_err(|_| WorldAuditBundleError::BadDigestLength(b.len()))?;
    Ok(i64::from_be_bytes(arr))
}

fn write_value(w: &mut CanonWriter, v: &Value) {
    match v {
        Value::Null => w.write_uint(0),
        Value::Int(i) => {
            w.write_uint(1);
            write_i64(w, *i);
        }
        Value::Str(s) => {
            w.write_uint(2);
            write_text(w, s);
        }
        Value::Bool(b) => {
            w.write_uint(3);
            w.write_uint(if *b { 1 } else { 0 });
        }
        Value::F64(f) => {
            w.write_uint(4);
            w.write_bytes(&f.bits().to_be_bytes());
        }
        Value::Decimal(d) => {
            w.write_uint(5);
            w.write_bytes(&d.canon_bytes());
        }
    }
}

fn read_value(r: &mut CanonReader<'_>) -> Result<Value, WorldAuditBundleError> {
    let tag = r.read_uint()?;
    match tag {
        0 => Ok(Value::Null),
        1 => Ok(Value::Int(read_i64(r)?)),
        2 => Ok(Value::Str(read_text(r)?)),
        3 => {
            let b = r.read_uint()?;
            match b {
                0 => Ok(Value::Bool(false)),
                1 => Ok(Value::Bool(true)),
                _ => Err(WorldAuditBundleError::UnknownValueTag(tag)),
            }
        }
        4 => {
            let bytes = r.read_bytes()?;
            let arr: [u8; 8] = bytes
                .try_into()
                .map_err(|_| WorldAuditBundleError::BadDigestLength(bytes.len()))?;
            let bits = u64::from_be_bytes(arr);
            FiniteF64::from_bits(bits)
                .map(Value::F64)
                .map_err(|_| WorldAuditBundleError::UnknownValueTag(tag))
        }
        5 => {
            let bytes = r.read_bytes()?;
            let mut sub = CanonReader::new(bytes);
            let d = read_decimal(&mut sub)?;
            if !sub.is_empty() {
                return Err(WorldAuditBundleError::TrailingBytesInFrame("decimal"));
            }
            Ok(Value::Decimal(d))
        }
        other => Err(WorldAuditBundleError::UnknownValueTag(other)),
    }
}

use brix_canon::Canonical as _;

// ---------------------------------------------------------------------------
// ExecProfileV1 (closes G4, §1.1)
// ---------------------------------------------------------------------------

/// Module loader limits recorded into an [`ExecProfileV1`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ModuleLoaderLimitsV1 {
    pub depth: u64,
    pub modules: u64,
    pub module_bytes: u64,
    pub total_bytes: u64,
}

impl ModuleLoaderLimitsV1 {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_uint(self.depth);
        w.write_uint(self.modules);
        w.write_uint(self.module_bytes);
        w.write_uint(self.total_bytes);
    }

    fn decode(r: &mut CanonReader<'_>) -> Result<Self, WorldAuditBundleError> {
        Ok(Self {
            depth: r.read_uint()?,
            modules: r.read_uint()?,
            module_bytes: r.read_uint()?,
            total_bytes: r.read_uint()?,
        })
    }
}

/// Recorded execution profile (`docs/planning/p6-audit-contract.md` §1.1).
/// Transported, never trusted: a verifier compares it to its own profile and
/// refuses on mismatch; it never adopts limits from the bundle.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ExecProfileV1 {
    pub profile: String,
    pub evaluator: String,
    pub crate_version: String,
    pub module_loader_limits: ModuleLoaderLimitsV1,
    pub numeric_semantics: String,
    pub settlement: String,
}

impl Default for ExecProfileV1 {
    fn default() -> Self {
        Self {
            profile: crate::world::EXEC_PROFILE_SCHEMA.to_string(),
            evaluator: "brix-world-net@1".to_string(),
            crate_version: env!("CARGO_PKG_VERSION").to_string(),
            module_loader_limits: ModuleLoaderLimitsV1 {
                depth: 16,
                modules: 256,
                module_bytes: 1024 * 1024,
                total_bytes: 8 * 1024 * 1024,
            },
            numeric_semantics: "ADR-0045".to_string(),
            settlement: "least-key(phase,priority,tiebreak)".to_string(),
        }
    }
}

impl ExecProfileV1 {
    pub fn current_for_network() -> Self {
        Self::default()
    }

    pub fn digest(&self) -> Digest {
        let mut w = CanonWriter::new();
        w.write_tag(crate::world::EXEC_PROFILE_SCHEMA);
        w.write_str(&self.evaluator);
        w.write_str(&self.crate_version);
        for n in [
            self.module_loader_limits.depth,
            self.module_loader_limits.modules,
            self.module_loader_limits.module_bytes,
            self.module_loader_limits.total_bytes,
        ] {
            w.write_uint(n);
        }
        w.write_str(&self.numeric_semantics);
        w.write_str(&self.settlement);
        w.write_str("same-proposal-entity-supports-must-agree@1");
        w.digest(Domain::Value)
    }

    pub fn from_json(v: &serde_json::Value) -> Result<Self, WorldError> {
        let profile = v
            .get("profile")
            .and_then(|x| x.as_str())
            .unwrap_or("brix.world@1")
            .to_string();
        let evaluator = v
            .get("evaluator")
            .and_then(|x| x.as_str())
            .ok_or_else(|| WorldError::Json("missing evaluator in exec_profile".into()))?
            .to_string();
        let crate_version = v
            .get("crate_version")
            .and_then(|x| x.as_str())
            .ok_or_else(|| WorldError::Json("missing crate_version in exec_profile".into()))?
            .to_string();
        let numeric_semantics = v
            .get("numeric_semantics")
            .and_then(|x| x.as_str())
            .unwrap_or("ADR-0045")
            .to_string();
        let settlement = v
            .get("settlement")
            .and_then(|x| x.as_str())
            .unwrap_or("least-key(phase,priority,tiebreak)")
            .to_string();
        let limits = if let Some(l) = v.get("module_loader_limits") {
            ModuleLoaderLimitsV1 {
                depth: l.get("depth").and_then(|x| x.as_u64()).unwrap_or(16),
                modules: l.get("modules").and_then(|x| x.as_u64()).unwrap_or(256),
                module_bytes: l
                    .get("module_bytes")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(1024 * 1024),
                total_bytes: l
                    .get("total_bytes")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(8 * 1024 * 1024),
            }
        } else {
            ModuleLoaderLimitsV1 {
                depth: 16,
                modules: 256,
                module_bytes: 1024 * 1024,
                total_bytes: 8 * 1024 * 1024,
            }
        };
        Ok(Self {
            profile,
            evaluator,
            crate_version,
            module_loader_limits: limits,
            numeric_semantics,
            settlement,
        })
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "profile": self.profile,
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

    fn canon_write(&self, w: &mut CanonWriter) {
        write_text(w, &self.profile);
        write_text(w, &self.evaluator);
        write_text(w, &self.crate_version);
        self.module_loader_limits.canon_write(w);
        write_text(w, &self.numeric_semantics);
        write_text(w, &self.settlement);
    }

    fn decode(r: &mut CanonReader<'_>) -> Result<Self, WorldAuditBundleError> {
        Ok(Self {
            profile: read_text(r)?,
            evaluator: read_text(r)?,
            crate_version: read_text(r)?,
            module_loader_limits: ModuleLoaderLimitsV1::decode(r)?,
            numeric_semantics: read_text(r)?,
            settlement: read_text(r)?,
        })
    }
}

// ---------------------------------------------------------------------------
// ProgramClosureV1
// ---------------------------------------------------------------------------

/// The program source closure, transported so a verifier can independently
/// re-link it (`docs/planning/p6-audit-contract.md` §1).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ProgramClosureV1 {
    pub root_module: String,
    /// Sorted, strictly increasing by module ident (hostile-input gate on decode).
    pub sources: Vec<(String, String)>,
    pub program_manifest_digest: Digest,
}

impl ProgramClosureV1 {
    fn canon_write(&self, w: &mut CanonWriter) {
        write_text(w, &self.root_module);
        w.write_uint(self.sources.len() as u64);
        for (ident, src) in &self.sources {
            write_text(w, ident);
            write_text(w, src);
        }
        write_digest(w, &self.program_manifest_digest);
    }

    fn decode(
        r: &mut CanonReader<'_>,
        limits: &WorldAuditDecodeLimits,
    ) -> Result<Self, WorldAuditBundleError> {
        let root_module = read_text(r)?;
        // Sources count is bounded by the cumulative-bytes charge below; a
        // dedicated count limit is unnecessary since every source contributes
        // at least its own ident+text bytes, already charged per entry.
        let count64 = r.read_uint()?;
        let count = usize::try_from(count64).map_err(|_| WorldAuditBundleError::CountOverflow)?;
        let mut sources = Vec::with_capacity(count.min(limits.max_sources_bytes));
        let mut cumulative = 0usize;
        let mut previous: Option<String> = None;
        for _ in 0..count {
            let ident = read_text(r)?;
            let src = read_text(r)?;
            cumulative = cumulative
                .checked_add(ident.len())
                .and_then(|c| c.checked_add(src.len()))
                .ok_or(WorldAuditBundleError::CumulativeOverflow)?;
            if cumulative > limits.max_sources_bytes {
                return Err(WorldAuditBundleError::SourcesBytesExceeded {
                    limit: limits.max_sources_bytes,
                    found: cumulative,
                });
            }
            if let Some(prev) = &previous {
                if *prev >= ident {
                    return Err(WorldAuditBundleError::SourcesNotSorted);
                }
            }
            previous = Some(ident.clone());
            sources.push((ident, src));
        }
        let program_manifest_digest = read_digest(r)?;
        Ok(Self {
            root_module,
            sources,
            program_manifest_digest,
        })
    }
}

// ---------------------------------------------------------------------------
// DecisionTupleV1
// ---------------------------------------------------------------------------

/// Fields of a settled decision, field-compatible with (but independently
/// encoded from) `network::SettledDecision` / the private `decision_tuple()`
/// (`docs/planning/p6-audit-contract.md` §1).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DecisionTupleV1 {
    pub candidate_name: String,
    pub priority: u64,
    pub phase: u64,
    pub value: Value,
    pub tiebreak: Digest,
}

impl DecisionTupleV1 {
    fn canon_write(&self, w: &mut CanonWriter) {
        write_text(w, &self.candidate_name);
        w.write_uint(self.priority);
        w.write_uint(self.phase);
        write_value(w, &self.value);
        write_digest(w, &self.tiebreak);
    }

    fn decode(r: &mut CanonReader<'_>) -> Result<Self, WorldAuditBundleError> {
        Ok(Self {
            candidate_name: read_text(r)?,
            priority: r.read_uint()?,
            phase: r.read_uint()?,
            value: read_value(r)?,
            tiebreak: read_digest(r)?,
        })
    }
}

// ---------------------------------------------------------------------------
// CheckpointStateV1
// ---------------------------------------------------------------------------

/// Full state at a checkpoint revision: every relation's complete key/tuple
/// set, and the complete settlement map, both sorted.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CheckpointStateV1 {
    pub record: WorldRevision,
    pub relations: Vec<(String, Vec<(WorldKey, WorldTuple)>)>,
    pub decisions: Vec<(String, String, DecisionTupleV1)>,
}

impl CheckpointStateV1 {
    pub fn digest(&self) -> Digest {
        let mut w = CanonWriter::new();
        self.canon_write(&mut w);
        w.digest(Domain::Value)
    }

    fn canon_write(&self, w: &mut CanonWriter) {
        let record_bytes =
            serde_json::to_vec(&self.record.to_json()).expect("WorldRevision::to_json serializes");
        w.write_bytes(&record_bytes);
        w.write_uint(self.relations.len() as u64);
        for (rel, rows) in &self.relations {
            write_text(w, rel);
            w.write_uint(rows.len() as u64);
            for (k, t) in rows {
                w.write_bytes(k.as_bytes());
                w.write_bytes(t.as_bytes());
            }
        }
        w.write_uint(self.decisions.len() as u64);
        for (decide, entity, tuple) in &self.decisions {
            write_text(w, decide);
            write_text(w, entity);
            tuple.canon_write(w);
        }
    }

    fn decode(
        r: &mut CanonReader<'_>,
        limits: &WorldAuditDecodeLimits,
    ) -> Result<Self, WorldAuditBundleError> {
        let record_bytes = r.read_bytes()?;
        if record_bytes.len() > limits.max_tuple_bytes {
            return Err(WorldAuditBundleError::TupleBytesExceeded {
                limit: limits.max_tuple_bytes,
                found: record_bytes.len(),
            });
        }
        let record_val: serde_json::Value = serde_json::from_slice(record_bytes)
            .map_err(|e| WorldAuditBundleError::Revision(WorldError::Json(e.to_string())))?;
        let record =
            WorldRevision::from_json(&record_val).map_err(WorldAuditBundleError::Revision)?;

        let rel_count = read_count(r, limits.max_checkpoint_rows, |found| {
            WorldAuditBundleError::CheckpointRowsExceeded {
                limit: limits.max_checkpoint_rows,
                found,
            }
        })?;
        let mut relations = Vec::with_capacity(rel_count);
        let mut total_rows = 0usize;
        for _ in 0..rel_count {
            let rel = read_text(r)?;
            let row_count64 = r.read_uint()?;
            let row_count =
                usize::try_from(row_count64).map_err(|_| WorldAuditBundleError::CountOverflow)?;
            total_rows = total_rows
                .checked_add(row_count)
                .ok_or(WorldAuditBundleError::CumulativeOverflow)?;
            if total_rows > limits.max_checkpoint_rows {
                return Err(WorldAuditBundleError::CheckpointRowsExceeded {
                    limit: limits.max_checkpoint_rows,
                    found: total_rows,
                });
            }
            let mut rows = Vec::with_capacity(row_count);
            let mut previous: Option<Vec<u8>> = None;
            for _ in 0..row_count {
                let key_bytes = r.read_bytes()?;
                if let Some(prev) = &previous {
                    if prev.as_slice() >= key_bytes {
                        return Err(WorldAuditBundleError::OutOfOrderSourceDelta);
                    }
                }
                previous = Some(key_bytes.to_vec());
                let key = WorldKey::new(key_bytes.to_vec());
                let tuple_bytes = r.read_bytes()?;
                if tuple_bytes.len() > limits.max_tuple_bytes {
                    return Err(WorldAuditBundleError::TupleBytesExceeded {
                        limit: limits.max_tuple_bytes,
                        found: tuple_bytes.len(),
                    });
                }
                let tuple = WorldTuple::new(tuple_bytes.to_vec());
                rows.push((key, tuple));
            }
            relations.push((rel, rows));
        }

        let dec_count = read_count(r, limits.max_total_decisions, |found| {
            WorldAuditBundleError::TotalDecisionsExceeded {
                limit: limits.max_total_decisions,
                found,
            }
        })?;
        let mut decisions = Vec::with_capacity(dec_count);
        let mut previous: Option<(String, String)> = None;
        for _ in 0..dec_count {
            let decide = read_text(r)?;
            let entity = read_text(r)?;
            if let Some((pd, pe)) = &previous {
                if (pd.as_str(), pe.as_str()) >= (decide.as_str(), entity.as_str()) {
                    return Err(WorldAuditBundleError::OutOfOrderDecisionDelta);
                }
            }
            previous = Some((decide.clone(), entity.clone()));
            let tuple = DecisionTupleV1::decode(r)?;
            decisions.push((decide, entity, tuple));
        }

        Ok(Self {
            record,
            relations,
            decisions,
        })
    }
}

// ---------------------------------------------------------------------------
// ScopeV1
// ---------------------------------------------------------------------------

/// The bundle's starting point: from genesis, or from a trusted checkpoint.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ScopeV1 {
    Genesis,
    Checkpoint {
        seq: u64,
        revision_digest: Digest,
        state: Box<CheckpointStateV1>,
    },
}

impl ScopeV1 {
    fn start_seq(&self) -> u64 {
        match self {
            ScopeV1::Genesis => 0,
            ScopeV1::Checkpoint { seq, .. } => *seq,
        }
    }

    fn canon_write(&self, w: &mut CanonWriter) {
        match self {
            ScopeV1::Genesis => w.write_uint(0),
            ScopeV1::Checkpoint {
                seq,
                revision_digest,
                state,
            } => {
                w.write_uint(1);
                w.write_uint(*seq);
                write_digest(w, revision_digest);
                state.canon_write(w);
            }
        }
    }

    fn decode(
        r: &mut CanonReader<'_>,
        limits: &WorldAuditDecodeLimits,
    ) -> Result<Self, WorldAuditBundleError> {
        let tag = r.read_uint()?;
        match tag {
            0 => Ok(ScopeV1::Genesis),
            1 => {
                let seq = r.read_uint()?;
                let revision_digest = read_digest(r)?;
                let state = CheckpointStateV1::decode(r, limits)?;
                Ok(ScopeV1::Checkpoint {
                    seq,
                    revision_digest,
                    state: Box::new(state),
                })
            }
            other => Err(WorldAuditBundleError::UnknownScopeTag(other)),
        }
    }
}

// ---------------------------------------------------------------------------
// RevisionEntryV1
// ---------------------------------------------------------------------------

/// One revision's record plus the deltas needed to re-derive it from the
/// previous revision's state (`docs/planning/p6-audit-contract.md` §1).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RevisionEntryV1 {
    pub record: WorldRevision,
    /// Strictly increasing by `(relation, key bytes)`.
    pub source_delta: Vec<(String, WorldKey, Option<WorldTuple>)>,
    /// Strictly increasing by `(decide, entity)`.
    pub decision_delta: Vec<(String, String, Option<DecisionTupleV1>)>,
}

impl RevisionEntryV1 {
    fn canon_write(&self, w: &mut CanonWriter) {
        let record_json = self.record.to_json();
        let record_bytes =
            serde_json::to_vec(&record_json).expect("WorldRevision JSON encoding cannot fail");
        w.write_bytes(&record_bytes);

        w.write_uint(self.source_delta.len() as u64);
        for (rel, key, tuple) in &self.source_delta {
            write_text(w, rel);
            w.write_bytes(key.as_bytes());
            match tuple {
                None => w.write_uint(0),
                Some(t) => {
                    w.write_uint(1);
                    w.write_bytes(t.as_bytes());
                }
            }
        }

        w.write_uint(self.decision_delta.len() as u64);
        for (decide, entity, tuple) in &self.decision_delta {
            write_text(w, decide);
            write_text(w, entity);
            match tuple {
                None => w.write_uint(0),
                Some(t) => {
                    w.write_uint(1);
                    t.canon_write(w);
                }
            }
        }
    }

    fn decode(
        r: &mut CanonReader<'_>,
        limits: &WorldAuditDecodeLimits,
    ) -> Result<Self, WorldAuditBundleError> {
        let record_bytes = r.read_bytes()?;
        let record_json: serde_json::Value = serde_json::from_slice(record_bytes)
            .map_err(|e| WorldAuditBundleError::Revision(WorldError::Json(e.to_string())))?;
        let record =
            WorldRevision::from_json(&record_json).map_err(WorldAuditBundleError::Revision)?;

        let source_count = read_count(r, limits.max_delta_entries_per_revision, |found| {
            WorldAuditBundleError::DeltaEntriesExceeded {
                limit: limits.max_delta_entries_per_revision,
                found,
            }
        })?;
        let mut source_delta = Vec::with_capacity(source_count);
        let mut previous: Option<(String, Vec<u8>)> = None;
        for _ in 0..source_count {
            let rel = read_text(r)?;
            let key_bytes = r.read_bytes()?.to_vec();
            if let Some((pr, pk)) = &previous {
                if (pr.as_str(), pk.as_slice()) >= (rel.as_str(), key_bytes.as_slice()) {
                    return Err(WorldAuditBundleError::OutOfOrderSourceDelta);
                }
            }
            previous = Some((rel.clone(), key_bytes.clone()));
            let key = WorldKey::new(key_bytes);
            let has_tuple = r.read_uint()?;
            let tuple = match has_tuple {
                0 => None,
                1 => {
                    let tb = r.read_bytes()?;
                    if tb.len() > limits.max_tuple_bytes {
                        return Err(WorldAuditBundleError::TupleBytesExceeded {
                            limit: limits.max_tuple_bytes,
                            found: tb.len(),
                        });
                    }
                    Some(WorldTuple::new(tb.to_vec()))
                }
                other => return Err(WorldAuditBundleError::UnknownValueTag(other)),
            };
            source_delta.push((rel, key, tuple));
        }

        let decision_count = read_count(r, limits.max_delta_entries_per_revision, |found| {
            WorldAuditBundleError::DeltaEntriesExceeded {
                limit: limits.max_delta_entries_per_revision,
                found,
            }
        })?;
        let mut decision_delta = Vec::with_capacity(decision_count);
        let mut previous: Option<(String, String)> = None;
        for _ in 0..decision_count {
            let decide = read_text(r)?;
            let entity = read_text(r)?;
            if let Some((pd, pe)) = &previous {
                if (pd.as_str(), pe.as_str()) >= (decide.as_str(), entity.as_str()) {
                    return Err(WorldAuditBundleError::OutOfOrderDecisionDelta);
                }
            }
            previous = Some((decide.clone(), entity.clone()));
            let has_tuple = r.read_uint()?;
            let tuple = match has_tuple {
                0 => None,
                1 => Some(DecisionTupleV1::decode(r)?),
                other => return Err(WorldAuditBundleError::UnknownValueTag(other)),
            };
            decision_delta.push((decide, entity, tuple));
        }

        Ok(Self {
            record,
            source_delta,
            decision_delta,
        })
    }
}

// ---------------------------------------------------------------------------
// HeadRefV1
// ---------------------------------------------------------------------------

/// The bundle's claimed head revision.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct HeadRefV1 {
    pub seq: u64,
    pub revision_digest: Digest,
}

// ---------------------------------------------------------------------------
// WorldAuditBundleV1
// ---------------------------------------------------------------------------

/// The world audit transport bundle (`docs/planning/p6-audit-contract.md` §1).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct WorldAuditBundleV1 {
    pub world_manifest: WorldManifest,
    pub program: ProgramClosureV1,
    pub exec_profile: ExecProfileV1,
    pub scope: ScopeV1,
    pub revisions: Vec<RevisionEntryV1>,
    pub head: HeadRefV1,
}

impl WorldAuditBundleV1 {
    /// Content-addressed identity of this bundle (§1, line `Identity:`).
    pub fn id(&self) -> WorldAuditBundleIdV1 {
        WorldAuditBundleIdV1::of(self)
    }

    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(BUNDLE_MARKER_V1);
        w.write_uint(BUNDLE_VERSION_V1);
        write_text(w, BUNDLE_PROFILE_V1);

        let manifest_bytes = serde_json::to_vec(&self.world_manifest.to_json())
            .expect("WorldManifest JSON encoding cannot fail");
        w.write_bytes(&manifest_bytes);

        self.program.canon_write(w);
        self.exec_profile.canon_write(w);
        self.scope.canon_write(w);

        w.write_uint(self.revisions.len() as u64);
        for entry in &self.revisions {
            entry.canon_write(w);
        }

        w.write_uint(self.head.seq);
        write_digest(w, &self.head.revision_digest);
    }

    fn canon_bytes(&self) -> Vec<u8> {
        let mut w = CanonWriter::new();
        self.canon_write(&mut w);
        w.finish()
    }

    /// Encode this bundle into canonical bytes under `limits`.
    pub fn encode(
        &self,
        limits: &WorldAuditDecodeLimits,
    ) -> Result<Vec<u8>, WorldAuditBundleError> {
        let bytes = self.canon_bytes();
        if bytes.len() > limits.max_total_bundle_bytes {
            return Err(WorldAuditBundleError::TotalBundleBytesExceeded {
                limit: limits.max_total_bundle_bytes,
                found: bytes.len(),
            });
        }
        if self.revisions.len() > limits.max_revisions {
            return Err(WorldAuditBundleError::RevisionsExceeded {
                limit: limits.max_revisions,
                found: self.revisions.len(),
            });
        }
        Ok(bytes)
    }
}

/// Content-addressed identity of a [`WorldAuditBundleV1`]
/// (`docs/planning/p6-audit-contract.md` §1, `Identity:` line).
///
/// `Digest(Domain::Value, "brix.world.audit-bundle" ∥ 1 ∥ "brix.world.audit-bundle@1" ∥ body)`,
/// i.e. the digest of the bundle's own canonical encoding (whose first three
/// fields are exactly that marker, version, and profile).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct WorldAuditBundleIdV1(pub Digest);

impl WorldAuditBundleIdV1 {
    pub fn of(bundle: &WorldAuditBundleV1) -> Self {
        WorldAuditBundleIdV1(Digest::of(Domain::Value, &bundle.canon_bytes()))
    }

    pub fn digest(&self) -> Digest {
        self.0
    }

    pub fn to_hex(&self) -> String {
        self.0.to_hex()
    }
}

/// Canonically decode untrusted bytes into a [`WorldAuditBundleV1`] under the
/// given `limits`.
///
/// Enforces every limit **before** the work it governs: total bundle bytes
/// before constructing a reader; marker/version/profile before any revision or
/// checkpoint work; every count with a checked `u64 -> usize` conversion
/// before allocating or looping; every tuple frame length before copying;
/// cumulative source bytes and cumulative decision counts with `checked_add`
/// before the governed work; strictly increasing ordinals/keys/entity pairs
/// before any semantic work; every nested frame fully consumed and outer
/// trailing bytes rejected.
pub fn decode_world_audit_bundle_v1(
    bytes: &[u8],
    limits: &WorldAuditDecodeLimits,
) -> Result<WorldAuditBundleV1, WorldAuditBundleError> {
    if bytes.len() > limits.max_total_bundle_bytes {
        return Err(WorldAuditBundleError::TotalBundleBytesExceeded {
            limit: limits.max_total_bundle_bytes,
            found: bytes.len(),
        });
    }

    let mut r = CanonReader::new(bytes);

    let marker = r.read_bytes()?;
    if marker != BUNDLE_MARKER_V1 {
        return Err(WorldAuditBundleError::BadMarker);
    }
    let version = r.read_uint()?;
    if version != BUNDLE_VERSION_V1 {
        return Err(WorldAuditBundleError::UnknownVersion(version));
    }
    let profile = read_text(&mut r)?;
    if profile != BUNDLE_PROFILE_V1 {
        return Err(WorldAuditBundleError::UnknownProfile);
    }

    let manifest_bytes = r.read_bytes()?;
    let manifest_json: serde_json::Value = serde_json::from_slice(manifest_bytes)
        .map_err(|e| WorldAuditBundleError::Manifest(WorldError::Json(e.to_string())))?;
    let world_manifest =
        WorldManifest::from_json(&manifest_json).map_err(WorldAuditBundleError::Manifest)?;

    let program = ProgramClosureV1::decode(&mut r, limits)?;
    let exec_profile = ExecProfileV1::decode(&mut r)?;
    let scope = ScopeV1::decode(&mut r, limits)?;

    let revisions_count = read_count(&mut r, limits.max_revisions, |found| {
        WorldAuditBundleError::RevisionsExceeded {
            limit: limits.max_revisions,
            found,
        }
    })?;
    let mut revisions = Vec::with_capacity(revisions_count);
    let scope_start = scope.start_seq();
    for i in 0..revisions_count {
        let entry = RevisionEntryV1::decode(&mut r, limits)?;
        let expected_seq = scope_start + 1 + i as u64;
        if entry.record.seq != expected_seq {
            return Err(WorldAuditBundleError::NonContiguousRevisionSeq {
                expected: expected_seq,
                found: entry.record.seq,
            });
        }
        revisions.push(entry);
    }

    let head_seq = r.read_uint()?;
    let head_revision_digest = read_digest(&mut r)?;
    let head = HeadRefV1 {
        seq: head_seq,
        revision_digest: head_revision_digest,
    };

    if !r.is_empty() {
        return Err(WorldAuditBundleError::TrailingBytes);
    }

    // Structural cross-checks (§1: "revisions: contiguous seq = scope.start+1 .. head.seq").
    let expected_count = head.seq.saturating_sub(scope_start);
    if revisions.len() as u64 != expected_count {
        return Err(WorldAuditBundleError::RevisionCountMismatch {
            expected: expected_count,
            found: revisions.len() as u64,
        });
    }
    match revisions.last() {
        Some(last) => {
            if last.record.seq != head.seq || last.record.revision_digest != head.revision_digest {
                return Err(WorldAuditBundleError::HeadRevisionMismatch);
            }
        }
        None => {
            // No revisions transported: the head must be exactly the scope's
            // own starting point (genesis, or the checkpoint itself).
            let scope_digest_matches = match &scope {
                ScopeV1::Genesis => head.seq == 0,
                ScopeV1::Checkpoint {
                    seq,
                    revision_digest,
                    ..
                } => head.seq == *seq && head.revision_digest == *revision_digest,
            };
            if !scope_digest_matches {
                return Err(WorldAuditBundleError::EmptyScopeHeadMismatch);
            }
        }
    }

    Ok(WorldAuditBundleV1 {
        world_manifest,
        program,
        exec_profile,
        scope,
        revisions,
        head,
    })
}

// ---------------------------------------------------------------------------
// Producer (§1: "Producer function... ONLY if it uses public WorldSession
// APIs as they exist at f1ec1b8; otherwise leave a documented stub signature.")
// ---------------------------------------------------------------------------

/// Build a `Genesis`-scoped [`WorldAuditBundleV1`] from an opened
/// [`WorldSession`], covering every committed revision `1..=current_revision`.
///
/// # Scope of this producer (honest limitation, not a shortcut)
///
/// This reads `source_delta` for each revision from public APIs only:
/// [`WorldSession::pin_revision`] for the revision record and `changed_keys`,
/// and [`super::session::WorldSnapshot::get`] at the previous and current
/// revision to resolve each changed key's tuple value (mirroring the walk
/// `WorldSession::diff_page` already does, per the contract's §1 note).
///
/// It does **not** populate `decision_delta`: at `f1ec1b8` the operator
/// network's settlement history is not persisted (G2/G3 in
/// `docs/planning/p6-audit-contract.md` §0 — `decision_tree` is in-memory
/// only, and `NetworkDeltaReport.settlements` removals are discarded after
/// each batch). Reconstructing a historical settlement diff would require
/// either replaying the reference evaluator at every revision (contract §1,
/// accepted as the bootstrap cost but not yet wired to this producer) or a
/// persisted settlement trace (G3, P6 step 2, a different lane's PR). This
/// producer therefore refuses with [`WorldAuditBundleError::UnknownValueTag`]
/// when any requested revision's `decision_root` is `Some` — i.e. it only
/// produces honest bundles for worlds with no `decide` blocks (storage-only
/// worlds, or executable worlds with purely relational programs). A verifier
/// or later producer wiring a persisted settlement trace, or the reference
/// evaluator, is the integration point the coordinator must close before this
/// function can cover executable worlds with decisions.
pub fn build_genesis_bundle_from_session(
    session: &WorldSession,
    program: ProgramClosureV1,
    exec_profile: ExecProfileV1,
) -> Result<WorldAuditBundleV1, WorldError> {
    let head_seq = session.current_revision();
    let mut revisions = Vec::new();

    for seq in 1..=head_seq {
        let prev_snapshot = session.pin_revision(seq - 1)?;
        let snapshot = session.pin_revision(seq)?;
        let record = snapshot.revision.clone();

        let decision_delta = if record.decision_root.is_some() {
            session
                .decision_delta(seq)?
                .into_iter()
                .map(|((d, e), opt)| {
                    (
                        d,
                        e,
                        opt.map(|s| DecisionTupleV1 {
                            candidate_name: s.candidate_name,
                            priority: s.priority,
                            phase: s.phase,
                            value: s.value,
                            tiebreak: s.calendar_key.tiebreak,
                        }),
                    )
                })
                .collect()
        } else {
            Vec::new()
        };

        let mut source_delta = Vec::new();
        for (relation, keys) in &record.changed_keys {
            for key in keys {
                let before = prev_snapshot.get(relation, key)?;
                let after = snapshot.get(relation, key)?;
                if before != after {
                    source_delta.push((relation.clone(), key.clone(), after));
                }
            }
        }
        source_delta
            .sort_by(|a, b| (a.0.as_str(), a.1.as_bytes()).cmp(&(b.0.as_str(), b.1.as_bytes())));

        revisions.push(RevisionEntryV1 {
            record,
            source_delta,
            decision_delta,
        });
    }

    let head_revision_digest = session.pin_revision(head_seq)?.revision.revision_digest;

    Ok(WorldAuditBundleV1 {
        world_manifest: session.manifest().clone(),
        program,
        exec_profile,
        scope: ScopeV1::Genesis,
        revisions,
        head: HeadRefV1 {
            seq: head_seq,
            revision_digest: head_revision_digest,
        },
    })
}

/// Build a checkpoint-scoped audit bundle covering a suffix from `checkpoint_seq` to HEAD.
pub fn build_checkpoint_bundle_from_session(
    session: &WorldSession,
    checkpoint_seq: u64,
    program: ProgramClosureV1,
    exec_profile: ExecProfileV1,
) -> Result<WorldAuditBundleV1, WorldError> {
    if checkpoint_seq == 0 {
        return build_genesis_bundle_from_session(session, program, exec_profile);
    }
    let head_seq = session.current_revision();
    if checkpoint_seq > head_seq {
        return Err(WorldError::RevisionNotFound(checkpoint_seq));
    }

    let checkpoint_snapshot = session.pin_revision(checkpoint_seq)?;
    let mut relations = Vec::new();
    for rel_name in session.manifest().relations.keys() {
        let mut rows = Vec::new();
        let mut cursor = None;
        loop {
            let page = checkpoint_snapshot.query_page(rel_name, cursor.as_deref(), 1000)?;
            for (key, tuple) in page.entries {
                rows.push((key, tuple));
            }
            if !page.has_more {
                break;
            }
            cursor = page.next_cursor;
        }
        rows.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        relations.push((rel_name.clone(), rows));
    }
    relations.sort_by(|a, b| a.0.cmp(&b.0));

    let mut decisions_map: BTreeMap<(String, String), DecisionTupleV1> = BTreeMap::new();
    for s in 1..=checkpoint_seq {
        for ((d, e), opt) in session.decision_delta(s)? {
            if let Some(dec) = opt {
                decisions_map.insert(
                    (d, e),
                    DecisionTupleV1 {
                        candidate_name: dec.candidate_name,
                        priority: dec.priority,
                        phase: dec.phase,
                        value: dec.value,
                        tiebreak: dec.calendar_key.tiebreak,
                    },
                );
            } else {
                decisions_map.remove(&(d, e));
            }
        }
    }
    let mut decisions = Vec::new();
    for ((d, e), tuple) in decisions_map {
        decisions.push((d, e, tuple));
    }
    decisions.sort_by(|a, b| (a.0.as_str(), a.1.as_str()).cmp(&(b.0.as_str(), b.1.as_str())));

    let state = CheckpointStateV1 {
        record: checkpoint_snapshot.revision.clone(),
        relations,
        decisions,
    };

    let mut revisions = Vec::new();
    for seq in (checkpoint_seq + 1)..=head_seq {
        let prev_snapshot = session.pin_revision(seq - 1)?;
        let snapshot = session.pin_revision(seq)?;
        let record = snapshot.revision.clone();

        let decision_delta = if record.decision_root.is_some() {
            session
                .decision_delta(seq)?
                .into_iter()
                .map(|((d, e), opt)| {
                    (
                        d,
                        e,
                        opt.map(|s| DecisionTupleV1 {
                            candidate_name: s.candidate_name,
                            priority: s.priority,
                            phase: s.phase,
                            value: s.value,
                            tiebreak: s.calendar_key.tiebreak,
                        }),
                    )
                })
                .collect()
        } else {
            Vec::new()
        };

        let mut source_delta = Vec::new();
        for (relation, keys) in &record.changed_keys {
            for key in keys {
                let before = prev_snapshot.get(relation, key)?;
                let after = snapshot.get(relation, key)?;
                if before != after {
                    source_delta.push((relation.clone(), key.clone(), after));
                }
            }
        }
        source_delta
            .sort_by(|a, b| (a.0.as_str(), a.1.as_bytes()).cmp(&(b.0.as_str(), b.1.as_bytes())));

        revisions.push(RevisionEntryV1 {
            record,
            source_delta,
            decision_delta,
        });
    }

    let head_revision_digest = session.pin_revision(head_seq)?.revision.revision_digest;

    Ok(WorldAuditBundleV1 {
        world_manifest: session.manifest().clone(),
        program,
        exec_profile,
        scope: ScopeV1::Checkpoint {
            seq: checkpoint_seq,
            revision_digest: checkpoint_snapshot.revision.revision_digest,
            state: Box::new(state),
        },
        revisions,
        head: HeadRefV1 {
            seq: head_seq,
            revision_digest: head_revision_digest,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::manifest::RelationDecl;
    use crate::world::revision::SettlementStatus;
    use std::collections::BTreeMap;

    fn sample_exec_profile() -> ExecProfileV1 {
        ExecProfileV1 {
            profile: "brix.world.exec@1".to_string(),
            evaluator: "brix-world-net@1".to_string(),
            crate_version: env!("CARGO_PKG_VERSION").to_string(),
            module_loader_limits: ModuleLoaderLimitsV1 {
                depth: 16,
                modules: 256,
                module_bytes: 1_000_000,
                total_bytes: 16_000_000,
            },
            numeric_semantics: "ADR-0045".to_string(),
            settlement: "least-key(phase,priority,tiebreak)".to_string(),
        }
    }

    fn sample_program() -> ProgramClosureV1 {
        ProgramClosureV1 {
            root_module: "root".to_string(),
            sources: vec![(
                "root".to_string(),
                "rel input orders: {} key id".to_string(),
            )],
            program_manifest_digest: Digest::of(Domain::Value, b"program"),
        }
    }

    fn sample_manifest() -> WorldManifest {
        WorldManifest::new(
            "world-1",
            "2026-10-04T00:00:00Z",
            Digest::of(Domain::Value, b"program"),
            vec![RelationDecl::new(
                "orders",
                vec!["id".to_string()],
                vec!["item".to_string()],
                vec![],
            )],
        )
    }

    fn sample_revision(seq: u64, prev: Option<Digest>) -> WorldRevision {
        let mut roots = BTreeMap::new();
        roots.insert("orders".to_string(), Digest::of(Domain::Value, b"root"));
        let mut cards = BTreeMap::new();
        cards.insert("orders".to_string(), 1usize);
        WorldRevision::new(
            seq,
            "2026-10-04T00:00:00Z",
            seq.saturating_sub(1),
            format!("idem-{seq}"),
            Some(Digest::of(Domain::Value, b"batch")),
            None,
            prev,
            roots,
            cards,
            BTreeMap::new(),
            None,
            BTreeMap::new(),
            SettlementStatus::Committed,
            None,
        )
    }

    fn sample_bundle() -> WorldAuditBundleV1 {
        let rev1 = sample_revision(1, None);
        let head_digest = rev1.revision_digest;
        WorldAuditBundleV1 {
            world_manifest: sample_manifest(),
            program: sample_program(),
            exec_profile: sample_exec_profile(),
            scope: ScopeV1::Genesis,
            revisions: vec![RevisionEntryV1 {
                record: rev1,
                source_delta: vec![(
                    "orders".to_string(),
                    WorldKey::from_str("k1"),
                    Some(WorldTuple::from_str("v1")),
                )],
                decision_delta: Vec::new(),
            }],
            head: HeadRefV1 {
                seq: 1,
                revision_digest: head_digest,
            },
        }
    }

    #[test]
    fn roundtrip_encode_decode() {
        let bundle = sample_bundle();
        let limits = WorldAuditDecodeLimits::strict();
        let bytes = bundle.encode(&limits).expect("encode");
        let decoded = decode_world_audit_bundle_v1(&bytes, &limits).expect("decode");
        assert_eq!(decoded, bundle);
    }

    #[test]
    fn empty_genesis_bundle_roundtrips() {
        let bundle = WorldAuditBundleV1 {
            world_manifest: sample_manifest(),
            program: sample_program(),
            exec_profile: sample_exec_profile(),
            scope: ScopeV1::Genesis,
            revisions: Vec::new(),
            head: HeadRefV1 {
                seq: 0,
                revision_digest: Digest::of(Domain::Value, b"genesis"),
            },
        };
        let limits = WorldAuditDecodeLimits::strict();
        let bytes = bundle.encode(&limits).expect("encode");
        let decoded = decode_world_audit_bundle_v1(&bytes, &limits).expect("decode");
        assert_eq!(decoded, bundle);
    }

    #[test]
    fn two_different_bundles_have_different_ids() {
        let a = sample_bundle();
        let mut b = sample_bundle();
        b.revisions[0].source_delta[0].1 = WorldKey::from_str("k2");
        assert_ne!(a.id(), b.id());
    }

    // -----------------------------------------------------------------
    // Hostile-input gates (the point of this module, per task instructions)
    // -----------------------------------------------------------------

    #[test]
    fn rejects_truncated_bundle() {
        let bundle = sample_bundle();
        let limits = WorldAuditDecodeLimits::strict();
        let bytes = bundle.encode(&limits).expect("encode");
        for cut in [0usize, 1, bytes.len() / 2, bytes.len() - 1] {
            let truncated = &bytes[..cut];
            assert!(
                decode_world_audit_bundle_v1(truncated, &limits).is_err(),
                "truncation at {cut} must be rejected"
            );
        }
    }

    #[test]
    fn rejects_trailing_bytes() {
        let bundle = sample_bundle();
        let limits = WorldAuditDecodeLimits::strict();
        let mut bytes = bundle.encode(&limits).expect("encode");
        bytes.push(0xFF);
        let err = decode_world_audit_bundle_v1(&bytes, &limits).unwrap_err();
        assert!(matches!(err, WorldAuditBundleError::TrailingBytes));
    }

    #[test]
    fn rejects_bad_marker() {
        let bundle = sample_bundle();
        let limits = WorldAuditDecodeLimits::strict();
        let mut w = CanonWriter::new();
        w.write_bytes(b"not-the-marker");
        w.write_uint(BUNDLE_VERSION_V1);
        write_text(&mut w, BUNDLE_PROFILE_V1);
        let mut bad = w.finish();
        // Pad so it is not merely a shorter-but-valid prefix situation.
        let rest = bundle.canon_bytes();
        bad.extend_from_slice(&rest[rest.len().saturating_sub(0)..]);
        let err = decode_world_audit_bundle_v1(&bad, &limits).unwrap_err();
        assert!(matches!(err, WorldAuditBundleError::BadMarker));
    }

    #[test]
    fn rejects_unknown_version() {
        let mut w = CanonWriter::new();
        w.write_bytes(BUNDLE_MARKER_V1);
        w.write_uint(99);
        write_text(&mut w, BUNDLE_PROFILE_V1);
        let bytes = w.finish();
        let limits = WorldAuditDecodeLimits::strict();
        let err = decode_world_audit_bundle_v1(&bytes, &limits).unwrap_err();
        assert!(matches!(err, WorldAuditBundleError::UnknownVersion(99)));
    }

    #[test]
    fn rejects_unknown_profile() {
        let mut w = CanonWriter::new();
        w.write_bytes(BUNDLE_MARKER_V1);
        w.write_uint(BUNDLE_VERSION_V1);
        write_text(&mut w, "brix.world.audit-bundle@2");
        let bytes = w.finish();
        let limits = WorldAuditDecodeLimits::strict();
        let err = decode_world_audit_bundle_v1(&bytes, &limits).unwrap_err();
        assert!(matches!(err, WorldAuditBundleError::UnknownProfile));
    }

    #[test]
    fn rejects_non_contiguous_revision_seq() {
        let mut bundle = sample_bundle();
        // Add a second revision entry whose seq skips to 3 instead of 2.
        let mut rev3 = sample_revision(3, Some(bundle.revisions[0].record.revision_digest));
        rev3.seq = 3; // already 3 from sample_revision(3, ..), kept explicit for clarity
        let rev3_digest = rev3.revision_digest;
        bundle.revisions.push(RevisionEntryV1 {
            record: rev3,
            source_delta: Vec::new(),
            decision_delta: Vec::new(),
        });
        bundle.head = HeadRefV1 {
            seq: 3,
            revision_digest: rev3_digest,
        };
        let limits = WorldAuditDecodeLimits::strict();
        // Bypass bundle.encode()'s own bookkeeping and go straight to raw bytes,
        // since we are deliberately producing a structurally invalid bundle.
        let bytes = bundle.canon_bytes();
        let err = decode_world_audit_bundle_v1(&bytes, &limits).unwrap_err();
        assert!(matches!(
            err,
            WorldAuditBundleError::NonContiguousRevisionSeq { .. }
        ));
    }

    #[test]
    fn rejects_duplicate_revision_seq() {
        let mut bundle = sample_bundle();
        let dup = bundle.revisions[0].clone();
        bundle.revisions.push(dup);
        bundle.head = HeadRefV1 {
            seq: 1,
            revision_digest: bundle.revisions[0].record.revision_digest,
        };
        let limits = WorldAuditDecodeLimits::strict();
        let bytes = bundle.canon_bytes();
        let err = decode_world_audit_bundle_v1(&bytes, &limits).unwrap_err();
        assert!(matches!(
            err,
            WorldAuditBundleError::NonContiguousRevisionSeq { .. }
        ));
    }

    #[test]
    fn rejects_out_of_order_source_delta() {
        let mut bundle = sample_bundle();
        bundle.revisions[0].source_delta = vec![
            (
                "orders".to_string(),
                WorldKey::from_str("k2"),
                Some(WorldTuple::from_str("v2")),
            ),
            (
                "orders".to_string(),
                WorldKey::from_str("k1"),
                Some(WorldTuple::from_str("v1")),
            ),
        ];
        let limits = WorldAuditDecodeLimits::strict();
        let bytes = bundle.canon_bytes();
        let err = decode_world_audit_bundle_v1(&bytes, &limits).unwrap_err();
        assert!(matches!(err, WorldAuditBundleError::OutOfOrderSourceDelta));
    }

    #[test]
    fn rejects_duplicate_source_delta_key() {
        let mut bundle = sample_bundle();
        let entry = bundle.revisions[0].source_delta[0].clone();
        bundle.revisions[0].source_delta.push(entry);
        let limits = WorldAuditDecodeLimits::strict();
        let bytes = bundle.canon_bytes();
        let err = decode_world_audit_bundle_v1(&bytes, &limits).unwrap_err();
        assert!(matches!(err, WorldAuditBundleError::OutOfOrderSourceDelta));
    }

    #[test]
    fn rejects_revision_count_mismatch() {
        let mut bundle = sample_bundle();
        bundle.head.seq = 2; // claims 2 revisions but only 1 is transported
        let limits = WorldAuditDecodeLimits::strict();
        let bytes = bundle.canon_bytes();
        let err = decode_world_audit_bundle_v1(&bytes, &limits).unwrap_err();
        assert!(matches!(
            err,
            WorldAuditBundleError::NonContiguousRevisionSeq { .. }
                | WorldAuditBundleError::RevisionCountMismatch { .. }
        ));
    }

    #[test]
    fn rejects_head_digest_mismatch() {
        let mut bundle = sample_bundle();
        bundle.head.revision_digest = Digest::of(Domain::Value, b"forged");
        let limits = WorldAuditDecodeLimits::strict();
        let bytes = bundle.canon_bytes();
        let err = decode_world_audit_bundle_v1(&bytes, &limits).unwrap_err();
        assert!(matches!(err, WorldAuditBundleError::HeadRevisionMismatch));
    }

    #[test]
    fn rejects_tampered_revision_record() {
        // Edit a byte inside the revision's JSON frame directly: this must
        // break WorldRevision::from_json's own digest recomputation check.
        let bundle = sample_bundle();
        let limits = WorldAuditDecodeLimits::strict();
        let mut bytes = bundle.encode(&limits).expect("encode");
        // Find the idempotency key substring and flip one byte in it.
        let needle = b"idem-1";
        let pos = bytes
            .windows(needle.len())
            .position(|w| w == needle)
            .expect("idempotency key bytes must be present");
        bytes[pos] ^= 0xFF;
        let err = decode_world_audit_bundle_v1(&bytes, &limits).unwrap_err();
        assert!(matches!(err, WorldAuditBundleError::Revision(_)));
    }

    #[test]
    fn rejects_revisions_exceeding_limit() {
        let bundle = sample_bundle();
        let tight_limits = WorldAuditDecodeLimits {
            max_revisions: 0,
            ..WorldAuditDecodeLimits::strict()
        };
        let err = bundle.encode(&tight_limits).unwrap_err();
        assert!(matches!(
            err,
            WorldAuditBundleError::RevisionsExceeded { .. }
        ));
    }

    #[test]
    fn rejects_tuple_exceeding_limit() {
        let mut bundle = sample_bundle();
        bundle.revisions[0].source_delta[0].2 = Some(WorldTuple::new(vec![0u8; 64]));
        let limits = WorldAuditDecodeLimits {
            max_tuple_bytes: 8,
            ..WorldAuditDecodeLimits::strict()
        };
        let bytes = bundle.canon_bytes();
        let err = decode_world_audit_bundle_v1(&bytes, &limits).unwrap_err();
        assert!(matches!(
            err,
            WorldAuditBundleError::TupleBytesExceeded { .. }
        ));
    }

    #[test]
    fn non_collision_against_sibling_identity_domains() {
        use crate::revision::RevisionRecord;
        use crate::world::batch::WorldBatch;

        let bundle_id = sample_bundle().id().digest();

        // brix.world@1
        let world_digest = sample_manifest().digest();
        assert_ne!(bundle_id, world_digest);

        // brix.world.revision@1 (the record's own digest)
        let rev_digest = sample_revision(1, None).revision_digest;
        assert_ne!(bundle_id, rev_digest);

        // brix.world.batch@1
        let batch_digest = WorldBatch::new(0, "idem", Vec::new()).digest();
        assert_ne!(bundle_id, batch_digest);

        // brix.kb.revision@1 (a different crate module's revision identity)
        let kb_rev = RevisionRecord {
            seq: 0,
            parent: None,
            program_id: brix_lower::finite_decision::FiniteDecisionProgramId(Digest::of(
                Domain::Value,
                b"kb-program",
            )),
            program_path: "programs/a.brix".to_string(),
            snapshot_id: brix_lower::input::InputSnapshotId(Digest::of(Domain::Value, b"kb-snap")),
            snapshot_path: "snapshots/a.json".to_string(),
            change: crate::revision::Change::Init,
            result: crate::revision::RevisionResult {
                status: crate::revision::Status::Quiescent,
                candidate: None,
                decision_digest: None,
                context_id: None,
                facts_digest: None,
                outcomes_digest: None,
                diagnostics: Vec::new(),
            },
        };
        assert_ne!(bundle_id, kb_rev.digest());

        // brix.soc.audit-input-bundle (ADR-0026's own bundle identity domain)
        let soc_bundle = soc_core::audit_bundle::SettlementAuditInputBundleV1 {
            context: brix_semantic::ContextId(Digest::of(Domain::Value, b"ctx")),
            entries: Vec::new(),
            final_chain_digest: Digest::of(Domain::Value, b"final"),
        };
        assert_ne!(bundle_id, soc_bundle.id().digest());
    }
}
