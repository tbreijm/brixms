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

use brix_canon::{CanonWriter, Digest, Domain};
use brix_lower::evidence::{Authority, Disposition, Reason, Receipt};
use serde_json::{json, Value as Json};
use soc_core::calendar::Key;
use soc_core::store::{MemoryNodeStore, NodeStore, TrieMap};

use super::audit::{
    build_genesis_bundle_from_session, decode_world_audit_bundle_v1, ProgramClosureV1,
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
    pub grants: Vec<String>,
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
            "grants": self.grants,
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
        writer.write_uint(self.grants.len() as u64);
        for g in &self.grants {
            writer.write_str(g);
        }
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
        let grants: Vec<String> = target_json["grants"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

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
            grants,
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

/// Verify portable decision evidence against the consumer's expected target.
///
/// Recomputes authority independently:
/// 1. Verifies exact target matching (world, program, revision, revision_digest, decide, entity, candidate, value_digest, grants).
/// 2. Verifies the revision record's self-consistent digest.
/// 3. Resolves the Merkle lookup of `(decide, entity)` from `revision.decision_root` using the supplied proof nodes.
/// 4. If an audit bundle is included, replays and re-derives the settlement independently via the reference verifier.
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

    // 2. If bundle is included, recompute typed settlement independently
    if let Some(bundle_bytes) = &evidence.bundle {
        let limits = WorldAuditDecodeLimits::strict();
        let decoded = decode_world_audit_bundle_v1(bundle_bytes, &limits)
            .map_err(|e| malformed(format!("decode audit bundle: {e}")))?;

        if decoded.world_manifest.digest() != expected.world {
            return Err(malformed("bundle manifest digest mismatch"));
        }
        if decoded.program.program_manifest_digest != expected.program {
            return Err(malformed("bundle program digest mismatch"));
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

        let matched = report
            .verified_settlements
            .iter()
            .any(|(seq, decide, ent, cand)| {
                *seq == expected.revision
                    && decide == &expected.decide
                    && ent == &expected.entity
                    && cand == &expected.candidate
            });
        if !matched {
            return Err(malformed(
                "recomputed settlement not found in verified replay report",
            ));
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
    /// Inspect world state to construct the exact expected `DecisionTarget`.
    pub fn decision_target(
        &self,
        revision: u64,
        decide: &str,
        entity: &str,
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
            grants,
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
        let _lock = WorldLockGuard::acquire(&self.root)?;
        self.sync_gc_epoch()?;
        let target = self.decision_target(revision, decide, entity, grants)?;
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
                let bundle = build_genesis_bundle_from_session(self, closure, ep)?;
                let limits = WorldAuditDecodeLimits::strict();
                Some(
                    bundle
                        .encode(&limits)
                        .map_err(|e| WorldError::NetworkError(e.to_string()))?,
                )
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
