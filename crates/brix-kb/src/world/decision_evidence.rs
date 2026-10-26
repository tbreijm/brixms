//! Portable settled decision evidence: `brix.world.decision-evidence@1`.
//!
//! Provides checkable, portable evidence that a specific decision was settled
//! with a specific winner candidate, priority, and typed value at an exact
//! immutable revision of a persistent world.
//!
//! The artifact binds:
//! - World manifest digest
//! - Program closure digest
//! - Pinned revision sequence and digest
//! - Qualified decide name and entity key
//! - Settled candidate variant and typed value
//! - Program-bound claim context (`CertifiedAnswer { path, caller, release_id, grants }`)
//! - Merkle proof path resolving the settlement from the revision's decision trie root
//! - Transport bundle allowing independent replay and settlement recomputation
//!
//! Verification requires only the artifact and an independently selected target;
//! neither a live session nor a cached answer is trusted.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use brix_canon::{CanonReader, CanonWriter, Digest, Domain};
use brix_lower::evidence::{Authority, Disposition, Reason, Receipt, VerifiedDecisionClaim};
use serde_json::{json, Value as Json};
use soc_core::calendar::Key;
use soc_core::store::{MemoryNodeStore, NodeStore, TrieMap};

use super::audit::{
    build_genesis_bundle_up_to_revision, decode_world_audit_bundle_v1, ProgramClosureV1,
    WorldAuditDecodeLimits,
};
use super::decision_codec::{
    decision_key, decision_tuple, decode_settled_decision, SettledDecision, Value,
};
use super::policy_evidence::{require_materialized_node, PolicyError, RecordingStore};
use super::session::{atomic_write, WorldLockGuard};
use super::verify::{verify_world_audit_bundle, VerifyOptions};
use super::{WorldError, WorldKey, WorldRevision, WorldSession, WorldTuple};

pub const DECISION_EVIDENCE_SCHEMA: &str = "brix.world.decision-evidence@1";
pub const DECISION_CLAIM_BUNDLE_PREFIX: &[u8; 8] = b"BRIXDCB1";
pub const MAX_DECISION_PROOF_NODES: usize = 512;
pub const MAX_DECISION_ARTIFACT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_DECISION_GRANTS: usize = 64;
pub const MAX_DECISION_STRING_BYTES: usize = 4096;
pub const MAX_DECISION_BUNDLE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_DECISION_NODE_BYTES: usize = 64 * 1024;
pub const MAX_DECISION_REPLAY_WORK: u64 = 500_000;

/// Explicit decode and replay bounds for hostile portable decision-evidence artifacts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecisionEvidenceDecodeLimits {
    pub max_artifact_bytes: usize,
    pub max_grants: usize,
    pub max_string_bytes: usize,
    pub max_bundle_bytes: usize,
    pub max_proof_nodes: usize,
    pub max_node_bytes: usize,
    pub max_replay_work: u64,
}

impl DecisionEvidenceDecodeLimits {
    pub const fn strict() -> Self {
        Self {
            max_artifact_bytes: MAX_DECISION_ARTIFACT_BYTES,
            max_grants: MAX_DECISION_GRANTS,
            max_string_bytes: MAX_DECISION_STRING_BYTES,
            max_bundle_bytes: MAX_DECISION_BUNDLE_BYTES,
            max_proof_nodes: MAX_DECISION_PROOF_NODES,
            max_node_bytes: MAX_DECISION_NODE_BYTES,
            max_replay_work: MAX_DECISION_REPLAY_WORK,
        }
    }
}

impl Default for DecisionEvidenceDecodeLimits {
    fn default() -> Self {
        Self::strict()
    }
}

/// Read a decision evidence file from disk bounded by `limits.max_artifact_bytes` before full allocation.
pub fn read_decision_evidence_file(
    path: &Path,
    limits: &DecisionEvidenceDecodeLimits,
) -> Result<Vec<u8>, PolicyError> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take((limits.max_artifact_bytes + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limits.max_artifact_bytes {
        return Err(malformed("decision evidence file exceeds size limit"));
    }
    Ok(bytes)
}

/// Consumer's independently selected decision claim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecisionTarget {
    pub world: Digest,
    pub program: Digest,
    pub revision: u64,
    pub revision_digest: Digest,
    pub decide: String,
    pub entity: String,
    pub candidate: String,
    pub value_digest: Digest,
    pub caller: Option<String>,
    pub grants: Vec<String>,
    pub release: String,
}

impl DecisionTarget {
    pub fn to_json(&self) -> Json {
        json!({
            "world": self.world.to_hex(),
            "program": self.program.to_hex(),
            "revision": self.revision,
            "revision_digest": self.revision_digest.to_hex(),
            "decide": self.decide,
            "entity": self.entity,
            "candidate": self.candidate,
            "value_digest": self.value_digest.to_hex(),
            "caller": self.caller,
            "grants": self.grants,
            "release": self.release,
        })
    }

    pub fn id(&self) -> Digest {
        let mut writer = CanonWriter::new();
        writer.write_tag(DECISION_EVIDENCE_SCHEMA);
        writer.write_bytes(self.world.as_bytes());
        writer.write_bytes(self.program.as_bytes());
        writer.write_uint(self.revision);
        writer.write_bytes(self.revision_digest.as_bytes());
        writer.write_str(&self.decide);
        writer.write_str(&self.entity);
        writer.write_str(&self.candidate);
        writer.write_bytes(self.value_digest.as_bytes());
        match &self.caller {
            Some(c) => {
                writer.write_uint(1);
                writer.write_str(c);
            }
            None => {
                writer.write_uint(0);
            }
        }
        writer.write_uint(self.grants.len() as u64);
        for g in &self.grants {
            writer.write_str(g);
        }
        writer.write_str(&self.release);
        writer.digest(Domain::Claim)
    }
}

/// Portable, untrusted decision evidence artifact.
#[derive(Clone, Debug)]
pub struct DecisionEvidence {
    pub target: DecisionTarget,
    pub revision: WorldRevision,
    pub settlement: SettledDecision,
    pub nodes: BTreeMap<Digest, Vec<u8>>,
    pub bundle: Option<Vec<u8>>,
}

fn malformed(message: impl Into<String>) -> PolicyError {
    PolicyError::Refused(Disposition::Unknown {
        reason: Reason::Malformed {
            what: message.into(),
        },
    })
}

fn digest_from_hex(value: &str) -> Result<Digest, PolicyError> {
    let bytes = WorldKey::from_hex(value)?.0;
    let bytes: [u8; 32] = bytes.try_into().map_err(|_| malformed("digest length"))?;
    Ok(Digest::from_bytes(bytes))
}

fn validate_bounded_str(
    s: &str,
    field: &str,
    limits: &DecisionEvidenceDecodeLimits,
) -> Result<String, PolicyError> {
    if s.len() > limits.max_string_bytes {
        return Err(malformed(format!("{field} exceeds maximum string length")));
    }
    Ok(s.to_string())
}

fn canonicalize_grants(
    raw_grants: &[String],
    limits: &DecisionEvidenceDecodeLimits,
) -> Result<Vec<String>, PolicyError> {
    if raw_grants.len() > limits.max_grants {
        return Err(malformed("grant count exceeds limit"));
    }
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    out.try_reserve_exact(raw_grants.len())
        .map_err(|_| malformed("grant allocation failed"))?;
    for g in raw_grants {
        if g.is_empty() || g.len() > limits.max_string_bytes {
            return Err(malformed("grant string length out of bounds"));
        }
        if !seen.insert(g.clone()) {
            return Err(malformed(format!("duplicate grant '{g}'")));
        }
        out.push(g.clone());
    }
    out.sort();
    Ok(out)
}

impl DecisionEvidence {
    pub fn encode(&self) -> Result<Vec<u8>, PolicyError> {
        let nodes: Vec<Json> = self
            .nodes
            .iter()
            .map(|(digest, bytes)| json!({"digest": digest.to_hex(), "bytes": bytes}))
            .collect();

        let val_bytes = self.settlement.value.to_canon_bytes();
        let settlement_json = json!({
            "entity_id": self.settlement.entity_id,
            "candidate_name": self.settlement.candidate_name,
            "priority": self.settlement.priority,
            "phase": self.settlement.phase,
            "value": self.settlement.value.to_query_json(),
            "value_canon_bytes": val_bytes,
            "calendar_key": {
                "phase": self.settlement.calendar_key.phase,
                "priority": self.settlement.calendar_key.priority,
                "tiebreak": self.settlement.calendar_key.tiebreak.to_hex(),
            }
        });

        let bytes = serde_json::to_vec(&json!({
            "schema": DECISION_EVIDENCE_SCHEMA,
            "target": self.target.to_json(),
            "revision": self.revision.to_json(),
            "settlement": settlement_json,
            "nodes": nodes,
            "bundle": self.bundle,
        }))
        .map_err(|e| malformed(e.to_string()))?;

        if bytes.len() > MAX_DECISION_ARTIFACT_BYTES || self.nodes.len() > MAX_DECISION_PROOF_NODES
        {
            return Err(malformed("decision evidence exceeds size limit"));
        }
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, PolicyError> {
        Self::decode_with_limits(bytes, &DecisionEvidenceDecodeLimits::strict())
    }

    pub fn decode_with_limits(
        bytes: &[u8],
        limits: &DecisionEvidenceDecodeLimits,
    ) -> Result<Self, PolicyError> {
        if bytes.len() > limits.max_artifact_bytes {
            return Err(malformed("decision evidence exceeds size limit"));
        }
        let val: Json = serde_json::from_slice(bytes).map_err(|e| malformed(e.to_string()))?;
        if val["schema"].as_str() != Some(DECISION_EVIDENCE_SCHEMA) {
            return Err(malformed("unsupported decision evidence schema"));
        }

        let target_json = &val["target"];
        let caller: Option<String> = match target_json.get("caller") {
            None | Some(Json::Null) => None,
            Some(Json::String(s)) => Some(validate_bounded_str(s, "target.caller", limits)?),
            Some(_) => return Err(malformed("target.caller must be string or null")),
        };

        let grants_arr = target_json
            .get("grants")
            .and_then(|v| v.as_array())
            .ok_or_else(|| malformed("target.grants must be an array"))?;
        if grants_arr.len() > limits.max_grants {
            return Err(malformed("target.grants exceeds max_grants"));
        }
        let mut raw_grants = Vec::new();
        raw_grants
            .try_reserve_exact(grants_arr.len())
            .map_err(|_| malformed("target.grants allocation failed"))?;
        for g_val in grants_arr {
            let s = g_val
                .as_str()
                .ok_or_else(|| malformed("target.grants element must be string"))?;
            raw_grants.push(validate_bounded_str(s, "target.grants", limits)?);
        }
        let grants = canonicalize_grants(&raw_grants, limits)?;

        let release: String = match target_json.get("release") {
            Some(Json::String(s)) => validate_bounded_str(s, "target.release", limits)?,
            None | Some(Json::Null) => String::new(),
            Some(_) => return Err(malformed("target.release must be string")),
        };

        let decide = validate_bounded_str(
            target_json["decide"]
                .as_str()
                .ok_or_else(|| malformed("target.decide"))?,
            "target.decide",
            limits,
        )?;
        let entity = validate_bounded_str(
            target_json["entity"]
                .as_str()
                .ok_or_else(|| malformed("target.entity"))?,
            "target.entity",
            limits,
        )?;
        let candidate = validate_bounded_str(
            target_json["candidate"]
                .as_str()
                .ok_or_else(|| malformed("target.candidate"))?,
            "target.candidate",
            limits,
        )?;

        let target = DecisionTarget {
            world: digest_from_hex(
                target_json["world"]
                    .as_str()
                    .ok_or_else(|| malformed("target.world"))?,
            )?,
            program: digest_from_hex(
                target_json["program"]
                    .as_str()
                    .ok_or_else(|| malformed("target.program"))?,
            )?,
            revision: target_json["revision"]
                .as_u64()
                .ok_or_else(|| malformed("target.revision"))?,
            revision_digest: digest_from_hex(
                target_json["revision_digest"]
                    .as_str()
                    .ok_or_else(|| malformed("target.revision_digest"))?,
            )?,
            decide,
            entity,
            candidate,
            value_digest: digest_from_hex(
                target_json["value_digest"]
                    .as_str()
                    .ok_or_else(|| malformed("target.value_digest"))?,
            )?,
            caller,
            grants,
            release,
        };

        let sett_json = &val["settlement"];
        let entity_id = validate_bounded_str(
            sett_json["entity_id"]
                .as_str()
                .ok_or_else(|| malformed("settlement.entity_id"))?,
            "settlement.entity_id",
            limits,
        )?;
        let candidate_name = validate_bounded_str(
            sett_json["candidate_name"]
                .as_str()
                .ok_or_else(|| malformed("settlement.candidate_name"))?,
            "settlement.candidate_name",
            limits,
        )?;
        let priority = sett_json["priority"]
            .as_u64()
            .ok_or_else(|| malformed("settlement.priority"))?;
        let phase = sett_json["phase"]
            .as_u64()
            .ok_or_else(|| malformed("settlement.phase"))?;

        let val_arr = sett_json["value_canon_bytes"]
            .as_array()
            .ok_or_else(|| malformed("settlement.value_canon_bytes"))?;
        if val_arr.len() > limits.max_node_bytes {
            return Err(malformed("settlement.value_canon_bytes exceeds limit"));
        }
        let val_bytes: Vec<u8> = serde_json::from_value(sett_json["value_canon_bytes"].clone())
            .map_err(|e| malformed(format!("settlement.value_canon_bytes: {e}")))?;
        let value = Value::from_canon_bytes(&val_bytes)
            .map_err(|e| malformed(format!("decode value: {e}")))?;

        let cal_json = &sett_json["calendar_key"];
        let cal_phase = cal_json["phase"]
            .as_u64()
            .ok_or_else(|| malformed("calendar_key.phase"))?;
        let cal_prio = cal_json["priority"]
            .as_u64()
            .ok_or_else(|| malformed("calendar_key.priority"))?;
        let tiebreak = digest_from_hex(
            cal_json["tiebreak"]
                .as_str()
                .ok_or_else(|| malformed("calendar_key.tiebreak"))?,
        )?;

        let settlement = SettledDecision {
            entity_id,
            candidate_name,
            priority,
            phase,
            value,
            calendar_key: Key {
                phase: cal_phase,
                priority: cal_prio,
                tiebreak,
            },
        };

        let entries = val["nodes"]
            .as_array()
            .ok_or_else(|| malformed("nodes array"))?;
        if entries.len() > limits.max_proof_nodes {
            return Err(malformed("too many proof nodes"));
        }
        let mut nodes = BTreeMap::new();
        let mut total_node_bytes = 0usize;
        for entry in entries {
            let digest = digest_from_hex(
                entry["digest"]
                    .as_str()
                    .ok_or_else(|| malformed("node digest"))?,
            )?;
            let byte_arr = entry["bytes"]
                .as_array()
                .ok_or_else(|| malformed("node bytes array"))?;
            if byte_arr.len() > limits.max_node_bytes {
                return Err(malformed("proof node exceeds size limit"));
            }
            let node_bytes: Vec<u8> = serde_json::from_value(entry["bytes"].clone())
                .map_err(|e| malformed(format!("node bytes: {e}")))?;
            total_node_bytes = total_node_bytes.saturating_add(node_bytes.len());
            if total_node_bytes > limits.max_artifact_bytes {
                return Err(malformed("total proof node bytes exceed limit"));
            }
            if nodes.insert(digest, node_bytes).is_some() {
                return Err(malformed("duplicate proof node"));
            }
        }

        let bundle: Option<Vec<u8>> = if val.get("bundle").is_some() && !val["bundle"].is_null() {
            let bundle_arr = val["bundle"]
                .as_array()
                .ok_or_else(|| malformed("bundle must be byte array"))?;
            if bundle_arr.len() > limits.max_bundle_bytes {
                return Err(malformed("embedded bundle exceeds size limit"));
            }
            Some(
                serde_json::from_value(val["bundle"].clone())
                    .map_err(|e| malformed(format!("bundle: {e}")))?,
            )
        } else {
            None
        };

        Ok(Self {
            target,
            revision: WorldRevision::from_json(&val["revision"])?,
            settlement,
            nodes,
            bundle,
        })
    }
}

/// Program-bound claim context declared inside a settled decision value (`CertifiedAnswer`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeclaredClaimContext {
    pub path: Value,
    pub caller: String,
    pub release_id: String,
    pub grants: Vec<String>,
}

/// Extract the program-bound claim context from a settled decision value when its
/// typed result is (or wraps) a `CertifiedAnswer { path, caller, release_id, grants }` record.
pub fn extract_declared_claim_context(
    settled_value: &Value,
    limits: &DecisionEvidenceDecodeLimits,
) -> Result<Option<DeclaredClaimContext>, PolicyError> {
    let record_fields = match settled_value {
        Value::Record { nominal, fields } => {
            let unqual = nominal.rsplit("::").next().unwrap_or(nominal);
            if unqual == "CertifiedAnswer" {
                Some(fields)
            } else {
                None
            }
        }
        Value::Variant { args, .. } if args.len() == 1 => match &args[0] {
            Value::Record { nominal, fields } => {
                let unqual = nominal.rsplit("::").next().unwrap_or(nominal);
                if unqual == "CertifiedAnswer" {
                    Some(fields)
                } else {
                    None
                }
            }
            _ => None,
        },
        _ => None,
    };

    let Some(fields) = record_fields else {
        return Ok(None);
    };

    let path = fields
        .get("path")
        .cloned()
        .ok_or_else(|| malformed("CertifiedAnswer missing 'path' field"))?;

    let caller = match fields.get("caller") {
        Some(Value::Str(s)) if !s.is_empty() => {
            validate_bounded_str(s, "CertifiedAnswer.caller", limits)?
        }
        _ => return Err(malformed("CertifiedAnswer.caller must be non-empty Str")),
    };

    let release_id = match fields.get("release_id") {
        Some(Value::Str(s)) if !s.is_empty() => {
            validate_bounded_str(s, "CertifiedAnswer.release_id", limits)?
        }
        _ => {
            return Err(malformed(
                "CertifiedAnswer.release_id must be non-empty Str",
            ))
        }
    };

    let raw_grants = match fields.get("grants") {
        Some(Value::List(items)) => {
            if items.len() > limits.max_grants {
                return Err(malformed("CertifiedAnswer.grants exceeds max_grants"));
            }
            let mut out = Vec::new();
            out.try_reserve_exact(items.len())
                .map_err(|_| malformed("CertifiedAnswer.grants allocation failed"))?;
            for item in items {
                match item {
                    Value::Str(s) => out.push(s.clone()),
                    _ => return Err(malformed("CertifiedAnswer.grants elements must be Str")),
                }
            }
            out
        }
        _ => return Err(malformed("CertifiedAnswer.grants must be List<Str>")),
    };

    let grants = canonicalize_grants(&raw_grants, limits)?;

    Ok(Some(DeclaredClaimContext {
        path,
        caller,
        release_id,
        grants,
    }))
}

type DecodedClaimBundleHeader<'a> = (Option<String>, String, Vec<String>, &'a [u8]);

fn decode_claim_bundle_header<'a>(
    bundle_bytes: &'a [u8],
    limits: &DecisionEvidenceDecodeLimits,
) -> Result<DecodedClaimBundleHeader<'a>, PolicyError> {
    if bundle_bytes.len() > limits.max_bundle_bytes {
        return Err(malformed("embedded bundle exceeds size limit"));
    }
    let Some(payload) = bundle_bytes.strip_prefix(DECISION_CLAIM_BUNDLE_PREFIX) else {
        return Err(malformed(
            "replay bundle missing revision-bound claim context header",
        ));
    };

    let mut reader = CanonReader::new(payload);
    let has_caller = reader
        .read_uint()
        .map_err(|e| malformed(format!("has_caller: {e:?}")))?;
    let caller = match has_caller {
        0 => None,
        1 => {
            let bytes = reader
                .read_bytes()
                .map_err(|e| malformed(format!("caller bytes: {e:?}")))?;
            if bytes.len() > limits.max_string_bytes {
                return Err(malformed("caller exceeds maximum string length"));
            }
            let s = std::str::from_utf8(bytes).map_err(|e| malformed(e.to_string()))?;
            if s.is_empty() {
                return Err(malformed("caller string must not be empty"));
            }
            Some(s.to_string())
        }
        _ => return Err(malformed("invalid has_caller flag in claim bundle header")),
    };

    let rel_bytes = reader
        .read_bytes()
        .map_err(|e| malformed(format!("release bytes: {e:?}")))?;
    if rel_bytes.len() > limits.max_string_bytes {
        return Err(malformed("release exceeds maximum string length"));
    }
    let release = std::str::from_utf8(rel_bytes)
        .map_err(|e| malformed(e.to_string()))?
        .to_string();

    let count = usize::try_from(
        reader
            .read_uint()
            .map_err(|e| malformed(format!("grant count: {e:?}")))?,
    )
    .map_err(|_| malformed("grant count out of range"))?;

    if count > limits.max_grants {
        return Err(malformed("grant count exceeds limit"));
    }

    let mut grants = Vec::new();
    grants
        .try_reserve_exact(count)
        .map_err(|_| malformed("grant allocation failed"))?;
    let mut seen = BTreeSet::new();
    for _ in 0..count {
        let bytes = reader
            .read_bytes()
            .map_err(|e| malformed(format!("grant bytes: {e:?}")))?;
        if bytes.is_empty() || bytes.len() > limits.max_string_bytes {
            return Err(malformed("grant string length out of bounds"));
        }
        let s = std::str::from_utf8(bytes).map_err(|e| malformed(e.to_string()))?;
        if !seen.insert(s.to_string()) {
            return Err(malformed(format!("duplicate grant '{s}' in bundle header")));
        }
        grants.push(s.to_string());
    }
    grants.sort();

    let raw_bundle = reader
        .read_bytes()
        .map_err(|e| malformed(format!("raw audit bundle: {e:?}")))?;
    if raw_bundle.len() > limits.max_bundle_bytes {
        return Err(malformed("raw audit bundle exceeds size limit"));
    }
    if !reader.is_empty() {
        return Err(malformed("trailing bytes in decision claim bundle header"));
    }

    Ok((caller, release, grants, raw_bundle))
}

/// Opaque result of complete independent world replay. There is no public
/// constructor, deserializer, witness token, or caller-supplied proof shortcut.
///
/// Even a crate with the same name cannot access the private fields:
/// ```compile_fail,E0451
/// use brix_kb::world::decision_evidence::VerifiedDecisionEvidence;
/// use brix_lower::evidence::{Receipt, VerifiedDecisionClaim};
/// fn forge(claim: VerifiedDecisionClaim, receipt: Receipt) -> VerifiedDecisionEvidence {
///     VerifiedDecisionEvidence { claim, receipt }
/// }
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedDecisionEvidence {
    claim: VerifiedDecisionClaim,
    receipt: Receipt,
}

impl VerifiedDecisionEvidence {
    pub fn claim(&self) -> &VerifiedDecisionClaim {
        &self.claim
    }
    pub fn receipt(&self) -> &Receipt {
        &self.receipt
    }
    pub fn into_receipt(self) -> Receipt {
        self.receipt
    }
    pub fn authentic(&self) -> bool {
        true
    }

    /// Install this replayed claim into a finite runtime's trusted environment.
    pub fn admit_into(&self, registry: &mut brix_lower::l3_v2::DocumentaryRegistry) {
        registry.admit_trusted_runtime_provider(self);
    }
}

impl brix_lower::l3_v2::TrustedRuntimeEvidenceProvider for VerifiedDecisionEvidence {
    fn runtime_claim(&self) -> &VerifiedDecisionClaim {
        &self.claim
    }
}

impl std::ops::Deref for VerifiedDecisionEvidence {
    type Target = Receipt;
    fn deref(&self) -> &Receipt {
        &self.receipt
    }
}

/// Verify portable decision evidence against the consumer's expected target using default strict limits.
pub fn verify_decision_evidence(
    evidence: &DecisionEvidence,
    expected: &DecisionTarget,
) -> Result<VerifiedDecisionEvidence, PolicyError> {
    verify_decision_evidence_with_limits(
        evidence,
        expected,
        &DecisionEvidenceDecodeLimits::strict(),
    )
}

/// Verify portable decision evidence against the consumer's expected target and explicit decode/replay limits.
/// Verification sequence:
/// 1. Independently expected target checked against envelope fields and bounded decode limits.
/// 2. Merkle proof verified against the pinned revision's `decision_root`.
/// 3. Pinned program replayed from genesis through the pinned revision under a bounded work budget.
/// 4. Exact settled value obtained from the independent replay report.
/// 5. Declared claim context (`CertifiedAnswer`) extracted from the replayed settled value.
/// 6. Every claimed component (`caller`, `release_id`, `grants`, `candidate`, `value_digest`) compared.
/// 7. Verifier-restricted [`VerifiedDecisionEvidence`] handle issued.
pub fn verify_decision_evidence_with_limits(
    evidence: &DecisionEvidence,
    expected: &DecisionTarget,
    limits: &DecisionEvidenceDecodeLimits,
) -> Result<VerifiedDecisionEvidence, PolicyError> {
    if expected.decide.is_empty() || expected.entity.is_empty() || expected.candidate.is_empty() {
        return Err(malformed(
            "expected decision target must specify non-empty decide, entity, and candidate",
        ));
    }
    let expected_grants = canonicalize_grants(&expected.grants, limits)?;
    if expected_grants != expected.grants {
        return Err(malformed(
            "expected decision target grants must be canonically sorted and deduplicated",
        ));
    }

    if evidence.target.caller != expected.caller {
        return Err(PolicyError::Refused(Disposition::Unknown {
            reason: Reason::ContextMismatch {
                expected: format!("caller={:?}", expected.caller),
                found: format!("caller={:?}", evidence.target.caller),
            },
        }));
    }
    if evidence.target.release != expected.release {
        return Err(PolicyError::Refused(Disposition::Unknown {
            reason: Reason::ContextMismatch {
                expected: format!("release={}", expected.release),
                found: format!("release={}", evidence.target.release),
            },
        }));
    }
    if evidence.target.grants != expected.grants {
        return Err(PolicyError::Refused(Disposition::Unknown {
            reason: Reason::ContextMismatch {
                expected: format!("grants={:?}", expected.grants),
                found: format!("grants={:?}", evidence.target.grants),
            },
        }));
    }
    if &evidence.target != expected {
        return Err(PolicyError::Refused(Disposition::Unknown {
            reason: Reason::ContextMismatch {
                expected: format!("{expected:?}"),
                found: format!("{:?}", evidence.target),
            },
        }));
    }

    if evidence.revision.seq != expected.revision
        || evidence.revision.revision_digest != expected.revision_digest
        || evidence.revision.compute_digest_for_record() != expected.revision_digest
    {
        return Err(malformed("revision identity mismatch"));
    }

    if evidence.settlement.candidate_name != expected.candidate {
        return Err(malformed(format!(
            "candidate mismatch: expected '{}', found '{}'",
            expected.candidate, evidence.settlement.candidate_name
        )));
    }
    if evidence.settlement.entity_id != expected.entity {
        return Err(malformed(format!(
            "entity mismatch: expected '{}', found '{}'",
            expected.entity, evidence.settlement.entity_id
        )));
    }

    let actual_value_digest = evidence.settlement.value.compute_digest();
    if actual_value_digest != expected.value_digest {
        return Err(malformed(format!(
            "value digest mismatch: expected '{}', found '{}'",
            expected.value_digest.to_hex(),
            actual_value_digest.to_hex()
        )));
    }

    if evidence.nodes.len() > limits.max_proof_nodes
        || evidence
            .nodes
            .values()
            .any(|b| b.len() > limits.max_node_bytes)
        || evidence.nodes.values().map(Vec::len).sum::<usize>() > limits.max_artifact_bytes
    {
        return Err(malformed("proof size limit"));
    }

    // 1. Verify Merkle path in decision trie
    let mut store = MemoryNodeStore::new();
    for (d, bytes) in &evidence.nodes {
        require_materialized_node(d, bytes).map_err(WorldError::from)?;
        store.put_node(*d, bytes.clone());
    }

    let decision_root = evidence
        .revision
        .decision_root
        .ok_or_else(|| malformed("missing decision root in revision"))?;

    let trie = TrieMap::<WorldKey, WorldTuple>::from_root_digest(decision_root, 1);
    let key = decision_key(&expected.decide, &expected.entity);
    let tuple = trie
        .get_with_store(&key, &store)
        .map_err(WorldError::from)?
        .ok_or_else(|| malformed("decision key not found in Merkle proof"))?;

    let expected_tuple = decision_tuple(&evidence.settlement);
    if tuple != expected_tuple {
        return Err(malformed("Merkle proof tuple does not match settlement"));
    }

    // 2. Independent replay is mandatory before issuing VerifiedDecisionEvidence
    let bundle_bytes = evidence
        .bundle
        .as_ref()
        .ok_or_else(|| malformed("independent replay bundle is mandatory for runtime authority"))?;

    let (header_caller, header_release, header_grants, raw_bundle_bytes) =
        decode_claim_bundle_header(bundle_bytes, limits)?;

    // Untrusted transport metadata in BRIXDCB1 must agree with the expected target
    if header_caller != expected.caller {
        return Err(PolicyError::Refused(Disposition::Unknown {
            reason: Reason::ContextMismatch {
                expected: format!("caller={:?}", expected.caller),
                found: format!("caller={header_caller:?}"),
            },
        }));
    }
    if header_release != expected.release {
        return Err(PolicyError::Refused(Disposition::Unknown {
            reason: Reason::ContextMismatch {
                expected: format!("release={}", expected.release),
                found: format!("release={header_release}"),
            },
        }));
    }
    if header_grants != expected.grants {
        return Err(PolicyError::Refused(Disposition::Unknown {
            reason: Reason::ContextMismatch {
                expected: format!("grants={:?}", expected.grants),
                found: format!("grants={header_grants:?}"),
            },
        }));
    }

    let mut audit_limits = WorldAuditDecodeLimits::strict();
    audit_limits.max_total_bundle_bytes = audit_limits
        .max_total_bundle_bytes
        .min(limits.max_bundle_bytes);
    let decoded = decode_world_audit_bundle_v1(raw_bundle_bytes, &audit_limits)
        .map_err(|e| malformed(format!("decode audit bundle: {e}")))?;

    if decoded.world_manifest.digest() != expected.world {
        return Err(malformed("bundle manifest digest mismatch"));
    }
    if decoded.program.program_manifest_digest != expected.program {
        return Err(malformed("bundle program digest mismatch"));
    }

    // Separate runtime version from Atlas release identity: ExecProfileV1.crate_version
    // remains execution-profile metadata checked against the running verifier crate version.
    let expected_crate_version = super::audit::ExecProfileV1::default().crate_version;
    if decoded.exec_profile.crate_version != expected_crate_version {
        return Err(PolicyError::Refused(Disposition::Unknown {
            reason: Reason::ContextMismatch {
                expected: format!("crate_version={expected_crate_version}"),
                found: format!("crate_version={}", decoded.exec_profile.crate_version),
            },
        }));
    }

    let report = verify_world_audit_bundle(
        &decoded,
        &VerifyOptions {
            expect_head: expected.revision_digest,
            expect_program: expected.program,
            trust_checkpoint: None,
            limits: audit_limits,
            max_work: Some(limits.max_replay_work),
        },
    )
    .map_err(|e| malformed(format!("audit bundle verification failed: {e:?}")))?;

    let replayed_settlement = report
        .head_settlements
        .get(&expected.decide)
        .and_then(|ents| ents.get(&expected.entity))
        .ok_or_else(|| malformed("recomputed settlement not found in verified replay report"))?;

    if replayed_settlement.candidate_name != expected.candidate
        || replayed_settlement.value.compute_digest() != expected.value_digest
        || replayed_settlement.value != evidence.settlement.value
        || replayed_settlement.priority != evidence.settlement.priority
        || replayed_settlement.phase != evidence.settlement.phase
    {
        return Err(malformed(
            "recomputed settlement from replay disagrees with claimed decision evidence",
        ));
    }

    // 3. Extract program-bound claim context from the replayed settled value
    match extract_declared_claim_context(&replayed_settlement.value, limits)? {
        Some(declared) => {
            if expected.caller.as_deref() != Some(declared.caller.as_str()) {
                return Err(PolicyError::Refused(Disposition::Unknown {
                    reason: Reason::ContextMismatch {
                        expected: format!("caller={:?}", expected.caller),
                        found: format!("declared_caller=Some({:?})", declared.caller),
                    },
                }));
            }
            if expected.release != declared.release_id {
                return Err(PolicyError::Refused(Disposition::Unknown {
                    reason: Reason::ContextMismatch {
                        expected: format!("release={}", expected.release),
                        found: format!("declared_release_id={}", declared.release_id),
                    },
                }));
            }
            if expected.grants != declared.grants {
                return Err(PolicyError::Refused(Disposition::Unknown {
                    reason: Reason::ContextMismatch {
                        expected: format!("grants={:?}", expected.grants),
                        found: format!("declared_grants={:?}", declared.grants),
                    },
                }));
            }
        }
        None => {
            // Settlement-only claim: if the decision has no declared CertifiedAnswer context,
            // it must refuse any requested caller, release catalogue ID, or grant authority.
            if expected.caller.is_some() {
                return Err(PolicyError::Refused(Disposition::Unknown {
                    reason: Reason::ContextMismatch {
                        expected: format!("caller={:?}", expected.caller),
                        found: "decision has no declared CertifiedAnswer claim context (caller authority refused)".to_string(),
                    },
                }));
            }
            if !expected.grants.is_empty() {
                return Err(PolicyError::Refused(Disposition::Unknown {
                    reason: Reason::ContextMismatch {
                        expected: format!("grants={:?}", expected.grants),
                        found: "decision has no declared CertifiedAnswer claim context (grant authority refused)".to_string(),
                    },
                }));
            }
            if !expected.release.is_empty() {
                return Err(PolicyError::Refused(Disposition::Unknown {
                    reason: Reason::ContextMismatch {
                        expected: format!("release={}", expected.release),
                        found: "decision has no declared CertifiedAnswer claim context (release authority refused)".to_string(),
                    },
                }));
            }
        }
    }

    let receipt = Receipt::issue(
        format!(
            "decision:{}:{}:{}",
            expected.decide, expected.entity, expected.candidate
        ),
        expected.entity.clone(),
        expected.revision_digest.to_hex(),
        Authority::RuntimeCheck,
        expected.grants.clone(),
        None,
    );
    let claim = VerifiedDecisionClaim {
        world: expected.world,
        program: expected.program,
        revision: expected.revision,
        revision_digest: expected.revision_digest,
        decide: expected.decide.clone(),
        entity: expected.entity.clone(),
        candidate: expected.candidate.clone(),
        value_digest: expected.value_digest,
        caller: expected.caller.clone(),
        grants: expected.grants.clone(),
        release: expected.release.clone(),
    };

    Ok(VerifiedDecisionEvidence { claim, receipt })
}

impl WorldSession {
    /// Inspect world state to construct the exact expected `DecisionTarget`.
    pub fn decision_target(
        &self,
        revision: u64,
        decide: &str,
        entity: &str,
        grants: Vec<String>,
    ) -> Result<DecisionTarget, PolicyError> {
        self.decision_target_with_caller(revision, decide, entity, None, grants)
    }

    /// Inspect world state to construct the exact expected `DecisionTarget` with an optional caller binding.
    pub fn decision_target_with_caller(
        &self,
        revision: u64,
        decide: &str,
        entity: &str,
        caller: Option<String>,
        grants: Vec<String>,
    ) -> Result<DecisionTarget, PolicyError> {
        let limits = DecisionEvidenceDecodeLimits::strict();
        let snapshot = self.pin_revision(revision)?;
        let settlement = self
            .historical_settlement(revision, Some(decide), entity)?
            .ok_or_else(|| {
                malformed(format!(
                    "no settlement for decide '{decide}' entity '{entity}' at revision {revision}"
                ))
            })?
            .1;

        let requested_grants = canonicalize_grants(&grants, &limits)?;
        let (resolved_caller, resolved_release, resolved_grants) =
            match extract_declared_claim_context(&settlement.value, &limits)? {
                Some(declared) => {
                    if let Some(req_caller) = &caller {
                        if req_caller != &declared.caller {
                            return Err(PolicyError::Refused(Disposition::Unknown {
                                reason: Reason::ContextMismatch {
                                    expected: format!("caller={req_caller}"),
                                    found: format!("declared_caller={}", declared.caller),
                                },
                            }));
                        }
                    }
                    if !requested_grants.is_empty() && requested_grants != declared.grants {
                        return Err(PolicyError::Refused(Disposition::Unknown {
                            reason: Reason::ContextMismatch {
                                expected: format!("grants={requested_grants:?}"),
                                found: format!("declared_grants={:?}", declared.grants),
                            },
                        }));
                    }
                    (Some(declared.caller), declared.release_id, declared.grants)
                }
                None => {
                    if caller.is_some() || !requested_grants.is_empty() {
                        return Err(PolicyError::Refused(Disposition::Unknown {
                            reason: Reason::ContextMismatch {
                                expected: format!("caller={caller:?}, grants={requested_grants:?}"),
                                found: "decision has no declared CertifiedAnswer claim context"
                                    .to_string(),
                            },
                        }));
                    }
                    (None, String::new(), Vec::new())
                }
            };

        let value_digest = settlement.value.compute_digest();

        Ok(DecisionTarget {
            world: self.manifest.digest(),
            program: self.manifest.program_digest,
            revision,
            revision_digest: snapshot.revision.revision_digest,
            decide: decide.to_string(),
            entity: entity.to_string(),
            candidate: settlement.candidate_name,
            value_digest,
            caller: resolved_caller,
            grants: resolved_grants,
            release: resolved_release,
        })
    }

    pub fn decision_evidence_path(&self, target: &DecisionTarget) -> PathBuf {
        self.paths.revisions_dir().join(format!(
            "{}.decision-{}.evidence",
            target.revision,
            target.id().to_hex()
        ))
    }

    /// Load program closure from `program.json`.
    pub fn program_closure(&self) -> Result<ProgramClosureV1, WorldError> {
        let program_file = self.paths.root.join("program.json");
        if !program_file.exists() {
            return Err(WorldError::InvalidSchema(
                "executable world is missing program.json source closure".into(),
            ));
        }
        let bytes = fs::read(&program_file)?;
        let val: Json =
            serde_json::from_slice(&bytes).map_err(|e| WorldError::Json(e.to_string()))?;
        let root_mod = val
            .get("root_module")
            .and_then(|x| x.as_str())
            .ok_or_else(|| WorldError::Json("program closure missing root_module".into()))?
            .to_string();
        let src_map = val
            .get("sources")
            .and_then(|x| x.as_object())
            .ok_or_else(|| WorldError::Json("program closure missing sources object".into()))?;
        let mut sources = Vec::new();
        for (name, source) in src_map {
            let text = source.as_str().ok_or_else(|| {
                WorldError::Json(format!("program source '{name}' must be a string"))
            })?;
            sources.push((name.clone(), text.to_owned()));
        }
        sources.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(ProgramClosureV1 {
            root_module: root_mod,
            sources,
            program_manifest_digest: self.manifest.program_digest,
        })
    }

    /// Request, verify, and durably cache portable decision evidence.
    pub fn request_decision_evidence(
        &mut self,
        revision: u64,
        decide: &str,
        entity: &str,
        grants: Vec<String>,
        include_bundle: bool,
    ) -> Result<DecisionEvidence, PolicyError> {
        self.request_decision_evidence_with_caller(
            revision,
            decide,
            entity,
            None,
            grants,
            include_bundle,
        )
    }

    /// Request, verify, and durably cache portable decision evidence with an optional caller binding.
    pub fn request_decision_evidence_with_caller(
        &mut self,
        revision: u64,
        decide: &str,
        entity: &str,
        caller: Option<String>,
        grants: Vec<String>,
        include_bundle: bool,
    ) -> Result<DecisionEvidence, PolicyError> {
        let _lock = WorldLockGuard::acquire(&self.root)?;
        self.sync_gc_epoch()?;
        let target = self.decision_target_with_caller(revision, decide, entity, caller, grants)?;
        let path = self.decision_evidence_path(&target);
        if path.exists() {
            return self.load_decision_evidence(&target);
        }

        let snapshot = self.pin_revision(revision)?;
        let store = RecordingStore {
            inner: &snapshot.node_store,
            nodes: RefCell::default(),
        };

        let decision_root = snapshot
            .revision
            .decision_root
            .ok_or_else(|| malformed("missing decision root in revision"))?;

        let trie = TrieMap::<WorldKey, WorldTuple>::from_root_digest(decision_root, 1);
        let key = decision_key(decide, entity);
        let tuple = trie
            .get_with_store(&key, &store)
            .map_err(WorldError::from)?
            .ok_or_else(|| malformed(format!("decision key not found: {decide}:{entity}")))?;

        let settlement = decode_settled_decision(entity, tuple.as_bytes())?;

        let bundle_bytes = if include_bundle {
            if let Ok(closure) = self.program_closure() {
                let ep = super::audit::ExecProfileV1::default();
                let bundle =
                    build_genesis_bundle_up_to_revision(self, closure, ep, target.revision)?;
                let limits = WorldAuditDecodeLimits::strict();
                let raw_bundle = bundle
                    .encode(&limits)
                    .map_err(|e| WorldError::NetworkError(e.to_string()))?;
                let mut w = CanonWriter::new();
                match &target.caller {
                    Some(c) => {
                        w.write_uint(1);
                        w.write_str(c);
                    }
                    None => {
                        w.write_uint(0);
                    }
                }
                w.write_str(&target.release);
                w.write_uint(target.grants.len() as u64);
                for g in &target.grants {
                    w.write_str(g);
                }
                w.write_bytes(&raw_bundle);
                let mut full_payload = DECISION_CLAIM_BUNDLE_PREFIX.to_vec();
                full_payload.extend_from_slice(&w.finish());
                Some(full_payload)
            } else {
                None
            }
        } else {
            None
        };

        let evidence = DecisionEvidence {
            target: target.clone(),
            revision: snapshot.revision,
            settlement,
            nodes: store.nodes.into_inner(),
            bundle: bundle_bytes,
        };

        verify_decision_evidence(&evidence, &target)?;
        atomic_write(&path, &evidence.encode()?)?;
        Ok(evidence)
    }

    /// Load and recheck retained decision evidence from disk.
    pub fn load_decision_evidence(
        &self,
        expected: &DecisionTarget,
    ) -> Result<DecisionEvidence, PolicyError> {
        if expected.world != self.manifest.digest() {
            return Err(malformed("world identity mismatch"));
        }
        let limits = DecisionEvidenceDecodeLimits::strict();
        let path = self.decision_evidence_path(expected);
        let bytes = read_decision_evidence_file(&path, &limits)?;
        let evidence = DecisionEvidence::decode_with_limits(&bytes, &limits)?;
        verify_decision_evidence_with_limits(&evidence, expected, &limits)?;
        Ok(evidence)
    }

    /// Check evidence against an expected target in this world session.
    pub fn use_decision_evidence(
        &self,
        evidence: &DecisionEvidence,
        revision: u64,
        decide: &str,
        entity: &str,
        grants: Vec<String>,
    ) -> Result<VerifiedDecisionEvidence, PolicyError> {
        let expected = self.decision_target(revision, decide, entity, grants)?;
        verify_decision_evidence(evidence, &expected)
    }
}
