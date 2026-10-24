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
//! - Caller/grants context
//! - Merkle proof path resolving the settlement from the revision's decision trie root
//! - Transport bundle allowing independent replay and settlement recomputation
//!
//! Verification requires only the artifact and an independently selected target;
//! neither a live session nor a cached answer is trusted.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::PathBuf;

use brix_canon::{CanonReader, CanonWriter, Digest, Domain};
use brix_lower::evidence::{Authority, Disposition, Reason, Receipt};
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
        if bytes.len() > MAX_DECISION_ARTIFACT_BYTES {
            return Err(malformed("decision evidence exceeds size limit"));
        }
        let val: Json = serde_json::from_slice(bytes).map_err(|e| malformed(e.to_string()))?;
        if val["schema"].as_str() != Some(DECISION_EVIDENCE_SCHEMA) {
            return Err(malformed("unsupported decision evidence schema"));
        }

        let target_json = &val["target"];
        let caller: Option<String> = target_json
            .get("caller")
            .and_then(|v| v.as_str())
            .map(String::from);
        let grants: Vec<String> = target_json["grants"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let release: String = target_json
            .get("release")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| super::audit::ExecProfileV1::default().crate_version);

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
            decide: target_json["decide"]
                .as_str()
                .ok_or_else(|| malformed("target.decide"))?
                .to_string(),
            entity: target_json["entity"]
                .as_str()
                .ok_or_else(|| malformed("target.entity"))?
                .to_string(),
            candidate: target_json["candidate"]
                .as_str()
                .ok_or_else(|| malformed("target.candidate"))?
                .to_string(),
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
        let entity_id = sett_json["entity_id"]
            .as_str()
            .ok_or_else(|| malformed("settlement.entity_id"))?
            .to_string();
        let candidate_name = sett_json["candidate_name"]
            .as_str()
            .ok_or_else(|| malformed("settlement.candidate_name"))?
            .to_string();
        let priority = sett_json["priority"]
            .as_u64()
            .ok_or_else(|| malformed("settlement.priority"))?;
        let phase = sett_json["phase"]
            .as_u64()
            .ok_or_else(|| malformed("settlement.phase"))?;

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
        if entries.len() > MAX_DECISION_PROOF_NODES {
            return Err(malformed("too many proof nodes"));
        }
        let mut nodes = BTreeMap::new();
        for entry in entries {
            let digest = digest_from_hex(
                entry["digest"]
                    .as_str()
                    .ok_or_else(|| malformed("node digest"))?,
            )?;
            let bytes: Vec<u8> = serde_json::from_value(entry["bytes"].clone())
                .map_err(|e| malformed(format!("node bytes: {e}")))?;
            if nodes.insert(digest, bytes).is_some() {
                return Err(malformed("duplicate proof node"));
            }
        }

        let bundle: Option<Vec<u8>> = if val.get("bundle").is_some() && !val["bundle"].is_null() {
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

fn is_grants_relation_name(name: &str) -> bool {
    let unqualified = name.rsplit("::").next().unwrap_or(name);
    matches!(
        unqualified,
        "grants" | "claims" | "caller_grants" | "permissions" | "authority"
    )
}

/// Modeled `(inferred_caller, modeled_grants)` extracted from revision relation tuples.
type ModeledClaimContext = (Option<String>, Vec<String>);

/// Extract modeled grants and caller from relation tuples at a revision.
/// Returns `Some((inferred_caller, modeled_grants))` if the world declares a
/// grants/claims relation, or `None` if the world schema does not model grants
/// in a dedicated relation.
fn extract_modeled_claim_context(
    relations: &BTreeMap<String, BTreeMap<WorldKey, WorldTuple>>,
    caller_filter: Option<&str>,
    entity: &str,
) -> Result<Option<ModeledClaimContext>, PolicyError> {
    let mut has_grants_rel = false;
    let mut modeled_grants = Vec::new();
    let mut callers = std::collections::BTreeSet::new();

    for (rel_name, rows) in relations {
        if !is_grants_relation_name(rel_name) {
            continue;
        }
        has_grants_rel = true;
        for tuple in rows.values() {
            let rec = super::TupleRecord::from_tuple(tuple)?;
            if let Some(row_ent) = rec
                .get_str("entity")
                .or_else(|| rec.get_str("subject"))
                .or_else(|| rec.get_str("target"))
                .or_else(|| rec.get_str("parent"))
            {
                if row_ent != entity && row_ent != "*" {
                    continue;
                }
            }
            let row_caller = rec.get_str("caller").or_else(|| rec.get_str("principal"));
            if let Some(expected_caller) = caller_filter {
                if let Some(rc) = row_caller {
                    if rc != expected_caller && rc != "*" {
                        continue;
                    }
                }
            }
            if let Some(rc) = row_caller {
                callers.insert(rc.to_string());
            }
            for field_name in ["grant", "permission", "claim"] {
                if let Some(g) = rec.get_str(field_name) {
                    if !g.is_empty() && !modeled_grants.contains(&g.to_string()) {
                        modeled_grants.push(g.to_string());
                    }
                }
            }
            if let Some(gs) = rec.get_str("grants") {
                for g in gs.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                    if !modeled_grants.contains(&g.to_string()) {
                        modeled_grants.push(g.to_string());
                    }
                }
            }
        }
    }

    if !has_grants_rel {
        return Ok(None);
    }

    let inferred_caller = caller_filter.map(String::from).or_else(|| {
        if callers.len() == 1 {
            callers.into_iter().next()
        } else {
            None
        }
    });
    Ok(Some((inferred_caller, modeled_grants)))
}

/// Verify portable decision evidence against the consumer's expected target.
///
/// Recomputes authority independently:
/// 1. Verifies exact target matching (world, program, revision, revision_digest, decide, entity, candidate, value_digest, caller, grants, release).
/// 2. Verifies the revision record's self-consistent digest.
/// 3. Resolves the Merkle lookup of `(decide, entity)` from `revision.decision_root` using the supplied proof nodes.
/// 4. Requires an audit bundle and replays the world from genesis up to `expected.revision` via the reference verifier,
///    confirming the recomputed settlement value, release identity, and modeled claim context.
pub fn verify_decision_evidence(
    evidence: &DecisionEvidence,
    expected: &DecisionTarget,
) -> Result<Receipt, PolicyError> {
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

    if evidence.nodes.len() > MAX_DECISION_PROOF_NODES
        || evidence.nodes.values().map(Vec::len).sum::<usize>() > MAX_DECISION_ARTIFACT_BYTES
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

    // 2. Independent replay is mandatory before issuing RuntimeCheck receipt
    let bundle_bytes = evidence
        .bundle
        .as_ref()
        .ok_or_else(|| malformed("independent replay bundle is mandatory for runtime authority"))?;

    let (replayed_caller, replayed_release, replayed_grants, raw_bundle_bytes) =
        if bundle_bytes.starts_with(DECISION_CLAIM_BUNDLE_PREFIX) {
            let mut r = CanonReader::new(&bundle_bytes[DECISION_CLAIM_BUNDLE_PREFIX.len()..]);
            let has_caller = r.read_uint().map_err(|e| malformed(format!("{e:?}")))?;
            let caller = if has_caller == 1 {
                let bytes = r.read_bytes().map_err(|e| malformed(format!("{e:?}")))?;
                let s = std::str::from_utf8(bytes).map_err(|e| malformed(e.to_string()))?;
                Some(s.to_string())
            } else {
                None
            };
            let rel_bytes = r.read_bytes().map_err(|e| malformed(format!("{e:?}")))?;
            let release = std::str::from_utf8(rel_bytes)
                .map_err(|e| malformed(e.to_string()))?
                .to_string();
            let count = r.read_uint().map_err(|e| malformed(format!("{e:?}")))? as usize;
            let mut grants = Vec::with_capacity(count);
            for _ in 0..count {
                let bytes = r.read_bytes().map_err(|e| malformed(format!("{e:?}")))?;
                let s = std::str::from_utf8(bytes).map_err(|e| malformed(e.to_string()))?;
                grants.push(s.to_string());
            }
            let raw_bundle = r.read_bytes().map_err(|e| malformed(format!("{e:?}")))?;
            (caller, release, grants, raw_bundle)
        } else {
            return Err(malformed(
                "replay bundle missing revision-bound claim context header",
            ));
        };

    if replayed_caller != expected.caller {
        return Err(PolicyError::Refused(Disposition::Unknown {
            reason: Reason::ContextMismatch {
                expected: format!("caller={:?}", expected.caller),
                found: format!("caller={replayed_caller:?}"),
            },
        }));
    }

    if replayed_release != expected.release {
        return Err(PolicyError::Refused(Disposition::Unknown {
            reason: Reason::ContextMismatch {
                expected: format!("release={}", expected.release),
                found: format!("release={replayed_release}"),
            },
        }));
    }

    if replayed_grants != expected.grants {
        return Err(PolicyError::Refused(Disposition::Unknown {
            reason: Reason::ContextMismatch {
                expected: format!("grants={:?}", expected.grants),
                found: format!("grants={replayed_grants:?}"),
            },
        }));
    }

    let limits = WorldAuditDecodeLimits::strict();
    let decoded = decode_world_audit_bundle_v1(raw_bundle_bytes, &limits)
        .map_err(|e| malformed(format!("decode audit bundle: {e}")))?;

    if decoded.world_manifest.digest() != expected.world {
        return Err(malformed("bundle manifest digest mismatch"));
    }
    if decoded.program.program_manifest_digest != expected.program {
        return Err(malformed("bundle program digest mismatch"));
    }
    if decoded.exec_profile.crate_version != expected.release {
        return Err(PolicyError::Refused(Disposition::Unknown {
            reason: Reason::ContextMismatch {
                expected: format!("crate_version={}", expected.release),
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
            limits,
            max_work: None,
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

    // Verify grants against the replayed revision's modeled claim context when a grants relation is present
    if let Some((modeled_caller, modeled_grants)) = extract_modeled_claim_context(
        &report.head_relations,
        expected.caller.as_deref(),
        &expected.entity,
    )? {
        if let (Some(exp_caller), Some(mod_caller)) = (&expected.caller, &modeled_caller) {
            if exp_caller != mod_caller {
                return Err(PolicyError::Refused(Disposition::Unknown {
                    reason: Reason::ContextMismatch {
                        expected: format!("caller={exp_caller}"),
                        found: format!("modeled_caller={mod_caller}"),
                    },
                }));
            }
        }
        for g in &expected.grants {
            if !modeled_grants.contains(g) {
                return Err(PolicyError::Refused(Disposition::Unknown {
                    reason: Reason::ContextMismatch {
                        expected: format!("grant '{g}' in modeled revision context"),
                        found: format!("modeled_grants={modeled_grants:?}"),
                    },
                }));
            }
        }
    }

    Ok(Receipt::issue(
        format!(
            "decision:{}:{}:{}",
            expected.decide, expected.entity, expected.candidate
        ),
        expected.entity.clone(),
        expected.revision_digest.to_hex(),
        Authority::RuntimeCheck,
        expected.grants.clone(),
        None,
    ))
}

impl WorldSession {
    fn revision_relations_at(
        &self,
        revision: u64,
    ) -> Result<BTreeMap<String, BTreeMap<WorldKey, WorldTuple>>, PolicyError> {
        let snapshot = self.pin_revision(revision)?;
        let mut relations = BTreeMap::new();
        for rel_name in self.manifest.relations.keys() {
            let mut rows = BTreeMap::new();
            if let Some(root) = snapshot.revision.relation_roots.get(rel_name) {
                let card = snapshot
                    .revision
                    .relation_cardinalities
                    .get(rel_name)
                    .copied()
                    .unwrap_or(0);
                let trie = TrieMap::<WorldKey, WorldTuple>::from_root_digest(*root, card);
                let (entries, _) = trie
                    .iter_page_with_store(None, usize::MAX, &snapshot.node_store)
                    .map_err(WorldError::from)?;
                for (k, v) in entries {
                    rows.insert(k, v);
                }
            }
            relations.insert(rel_name.clone(), rows);
        }
        Ok(relations)
    }

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
        let snapshot = self.pin_revision(revision)?;
        let settlement = self
            .historical_settlement(revision, Some(decide), entity)?
            .ok_or_else(|| {
                malformed(format!(
                    "no settlement for decide '{decide}' entity '{entity}' at revision {revision}"
                ))
            })?
            .1;

        let relations = self.revision_relations_at(revision)?;
        let (resolved_caller, resolved_grants) =
            match extract_modeled_claim_context(&relations, caller.as_deref(), entity)? {
                Some((modeled_caller, modeled_grants)) => {
                    let final_grants = if grants.is_empty() {
                        modeled_grants
                    } else {
                        for g in &grants {
                            if !modeled_grants.contains(g) {
                                return Err(PolicyError::Refused(Disposition::Unknown {
                                    reason: Reason::ContextMismatch {
                                        expected: format!(
                                            "grant '{g}' in modeled revision context"
                                        ),
                                        found: format!("modeled_grants={modeled_grants:?}"),
                                    },
                                }));
                            }
                        }
                        grants
                    };
                    (caller.or(modeled_caller), final_grants)
                }
                None => (caller, grants),
            };

        let value_digest = settlement.value.compute_digest();
        let release = super::audit::ExecProfileV1::default().crate_version;

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
            release,
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
        let path = self.decision_evidence_path(expected);
        let mut bytes = Vec::new();
        fs::File::open(&path)?
            .take((MAX_DECISION_ARTIFACT_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        let evidence = DecisionEvidence::decode(&bytes)?;
        verify_decision_evidence(&evidence, expected)?;
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
    ) -> Result<Receipt, PolicyError> {
        let expected = self.decision_target(revision, decide, entity, grants)?;
        verify_decision_evidence(evidence, &expected)
    }
}
