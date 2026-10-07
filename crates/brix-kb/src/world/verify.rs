//! Independent verifier for `brix.world.audit@1` bundles (ADR-0046 P6 §2).
//!
//! Two external pins are mandatory (ADR-0026 ⟨D-PIN⟩):
//! - `--expect-head <hex>` (head revision digest)
//! - `--expect-program <hex>` (`program_manifest_digest`)
//!
//! Verification recomputes all authority independently without trusting
//! transported indexes, tries, operator caches, or claimed decisions.

use std::collections::{BTreeMap, BTreeSet};

use brix_canon::{Digest, Domain};
use brix_lower::module_graph::{ModuleGraph, ModuleLoaderLimits};
use soc_core::calendar::Key;
use soc_core::store::TrieMap;

use super::audit::{
    DecisionTupleV1, ExecProfileV1, ScopeV1, WorldAuditBundleV1, WorldAuditDecodeLimits,
};
use super::codec::{encode_secondary_key, extract_indexed_field};
use super::decision_codec::{compute_decision_root, encode_decision_delta, SettledDecision, Value};
use super::error::WorldError;
use super::reference::{self, ReferenceSettlement};
use super::revision::{SettlementStatus, REVISION_SCHEMA_V2};
use super::types::{WorldKey, WorldTuple};

/// Configuration and external trust pins for verification.
#[derive(Clone, Debug)]
pub struct VerifyOptions {
    /// Expected HEAD revision digest (mandatory pin).
    pub expect_head: Digest,
    /// Expected program manifest digest (mandatory pin).
    pub expect_program: Digest,
    /// Optional trusted checkpoint digest for suffix verification.
    pub trust_checkpoint: Option<Digest>,
    /// Decode limits governing verification work.
    pub limits: WorldAuditDecodeLimits,
    /// Optional maximum work budget.
    pub max_work: Option<u64>,
}

/// Work performed during audit verification.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VerifyWorkReport {
    pub tuples_decoded: u64,
    pub trie_nodes_built: u64,
    pub index_rows_rebuilt: u64,
    pub rows_evaluated: u64,
    pub candidates_evaluated: u64,
    pub settlements_computed: u64,
    pub revisions_replayed: u64,
    pub expressions_evaluated: u64,
    pub join_pairs_evaluated: u64,
}

impl VerifyWorkReport {
    pub fn total_work(&self) -> u64 {
        self.tuples_decoded
            .saturating_add(self.trie_nodes_built)
            .saturating_add(self.index_rows_rebuilt)
            .saturating_add(self.rows_evaluated)
            .saturating_add(self.candidates_evaluated)
            .saturating_add(self.settlements_computed)
            .saturating_add(self.revisions_replayed)
            .saturating_add(self.expressions_evaluated)
            .saturating_add(self.join_pairs_evaluated)
    }
}

/// Report returned upon successful verification.
#[derive(Clone, Debug)]
pub struct VerifyReport {
    pub scope: String,
    pub head_revision: u64,
    pub head_digest: Digest,
    pub program_digest: Digest,
    pub work: VerifyWorkReport,
    pub verified_settlements: Vec<(u64, String, String, String)>,
}

/// Errors refusing verification. Every refusal maps to an explicit `Unknown(<reason>)`.
#[derive(Debug)]
pub enum VerifyError {
    Usage(String),
    BadMarker,
    UnknownProfile,
    UnknownVersion(u64),
    ProgramPinMismatch {
        expected: Digest,
        found: Digest,
    },
    HeadPinMismatch {
        expected: Digest,
        found: Digest,
    },
    ManifestDigestMismatch,
    NonContiguousSeq {
        expected: u64,
        found: u64,
    },
    PreviousDigestMismatch {
        seq: u64,
    },
    RevisionDigestMismatch {
        seq: u64,
    },
    UncommittedRevision {
        seq: u64,
    },
    RelationRootMismatch {
        seq: u64,
        relation: String,
        expected: Digest,
        found: Digest,
    },
    RelationCardinalityMismatch {
        seq: u64,
        relation: String,
        expected: usize,
        found: usize,
    },
    ChangedKeysMismatch {
        seq: u64,
        relation: String,
    },
    SecondaryIndexRootMismatch {
        seq: u64,
        index: String,
        expected: Digest,
        found: Digest,
    },
    DecisionRootMismatch {
        seq: u64,
        expected: Option<Digest>,
        found: Option<Digest>,
    },
    DecisionDeltaMismatch {
        seq: u64,
    },
    ExecProfileMismatch {
        expected: Digest,
        found: Digest,
    },
    UnsupportedExecProfile(String),
    CheckpointUntrusted,
    BudgetExhausted,
    World(WorldError),
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Usage(msg) => write!(f, "Unknown(usage-refusal:{msg})"),
            Self::BadMarker => write!(f, "Unknown(bad-marker)"),
            Self::UnknownProfile => write!(f, "Unknown(unknown-profile)"),
            Self::UnknownVersion(v) => write!(f, "Unknown(unknown-version:{v})"),
            Self::ProgramPinMismatch { expected, found } => {
                write!(
                    f,
                    "Unknown(program-pin-mismatch:expected={},found={})",
                    expected.to_hex(),
                    found.to_hex()
                )
            }
            Self::HeadPinMismatch { expected, found } => {
                write!(
                    f,
                    "Unknown(head-pin-mismatch:expected={},found={})",
                    expected.to_hex(),
                    found.to_hex()
                )
            }
            Self::ManifestDigestMismatch => write!(f, "Unknown(manifest-digest-mismatch)"),
            Self::NonContiguousSeq { expected, found } => {
                write!(
                    f,
                    "Unknown(non-contiguous-seq:expected={expected},found={found})"
                )
            }
            Self::PreviousDigestMismatch { seq } => {
                write!(f, "Unknown(previous-digest-mismatch:seq={seq})")
            }
            Self::RevisionDigestMismatch { seq } => {
                write!(f, "Unknown(revision-digest-mismatch:seq={seq})")
            }
            Self::UncommittedRevision { seq } => {
                write!(f, "Unknown(uncommitted-revision:seq={seq})")
            }
            Self::RelationRootMismatch {
                seq,
                relation,
                expected,
                found,
            } => {
                write!(
                    f,
                    "Unknown(relation-root-mismatch:seq={seq},rel={relation},expected={},found={})",
                    expected.to_hex(),
                    found.to_hex()
                )
            }
            Self::RelationCardinalityMismatch {
                seq,
                relation,
                expected,
                found,
            } => {
                write!(f, "Unknown(cardinality-mismatch:seq={seq},rel={relation},expected={expected},found={found})")
            }
            Self::ChangedKeysMismatch { seq, relation } => {
                write!(f, "Unknown(changed-keys-mismatch:seq={seq},rel={relation})")
            }
            Self::SecondaryIndexRootMismatch {
                seq,
                index,
                expected,
                found,
            } => {
                write!(f, "Unknown(secondary-index-root-mismatch:seq={seq},idx={index},expected={},found={})", expected.to_hex(), found.to_hex())
            }
            Self::DecisionRootMismatch {
                seq,
                expected,
                found,
            } => {
                write!(f, "Unknown(decision-root-mismatch:seq={seq},expected={expected:?},found={found:?})")
            }
            Self::DecisionDeltaMismatch { seq } => {
                write!(f, "Unknown(decision-delta-mismatch:seq={seq})")
            }
            Self::ExecProfileMismatch { expected, found } => {
                write!(
                    f,
                    "Unknown(exec-profile-mismatch:expected={},found={})",
                    expected.to_hex(),
                    found.to_hex()
                )
            }
            Self::UnsupportedExecProfile(reason) => {
                write!(f, "Unknown(unsupported-exec-profile:{reason})")
            }
            Self::CheckpointUntrusted => write!(f, "Unknown(checkpoint-untrusted)"),
            Self::BudgetExhausted => write!(f, "Unknown(verification-budget-exhausted)"),
            Self::World(err) => write!(f, "Unknown(world-error:{err})"),
        }
    }
}

impl std::error::Error for VerifyError {}

impl From<WorldError> for VerifyError {
    fn from(err: WorldError) -> Self {
        Self::World(err)
    }
}

fn convert_reference_settlement(s: &ReferenceSettlement) -> SettledDecision {
    SettledDecision {
        entity_id: s.entity_id.clone(),
        candidate_name: s.candidate_name.clone(),
        priority: s.priority,
        phase: s.phase,
        value: match &s.value {
            reference::Value::Null => Value::Null,
            reference::Value::Bool(b) => Value::Bool(*b),
            reference::Value::Int(i) => Value::Int(*i),
            reference::Value::Str(st) => Value::Str(st.clone()),
            reference::Value::F64(f) => Value::F64(*f),
            reference::Value::Decimal(d) => Value::Decimal(*d),
        },
        calendar_key: s.calendar_key,
    }
}

fn check_budget(work: &VerifyWorkReport, max_work: Option<u64>) -> Result<(), VerifyError> {
    if let Some(max) = max_work {
        if work.total_work() > max {
            return Err(VerifyError::BudgetExhausted);
        }
    }
    Ok(())
}

/// Verifies an audit bundle independently from genesis or trusted checkpoint.
pub fn verify_world_audit_bundle(
    bundle: &WorldAuditBundleV1,
    options: &VerifyOptions,
) -> Result<VerifyReport, VerifyError> {
    let mut work = VerifyWorkReport::default();

    // 1. Program manifest verification
    let source_map: BTreeMap<String, String> = bundle.program.sources.iter().cloned().collect();
    let loader = |name: &str| source_map.get(name).cloned();
    let loader_limits = ModuleLoaderLimits {
        max_import_depth: bundle.exec_profile.module_loader_limits.depth as usize,
        max_import_modules: bundle.exec_profile.module_loader_limits.modules as usize,
        max_module_source_bytes: bundle.exec_profile.module_loader_limits.module_bytes as usize,
        max_total_source_bytes: bundle.exec_profile.module_loader_limits.total_bytes as usize,
    };

    if bundle.world_manifest.program_required {
        if bundle.program.sources.is_empty() {
            return Err(VerifyError::Usage(
                "executable world requires program sources".into(),
            ));
        }
        if bundle.world_manifest.program_digest != options.expect_program {
            return Err(VerifyError::ProgramPinMismatch {
                expected: options.expect_program,
                found: bundle.world_manifest.program_digest,
            });
        }
        if bundle.exec_profile.profile != crate::world::EXEC_PROFILE_SCHEMA {
            return Err(VerifyError::UnknownProfile);
        }
        if bundle.exec_profile != ExecProfileV1::default() {
            return Err(VerifyError::UnsupportedExecProfile(format!(
                "{:?}",
                bundle.exec_profile
            )));
        }
    }

    let linked_prog = if !bundle.program.sources.is_empty() {
        let graph = ModuleGraph::load(&bundle.program.root_module, &loader, loader_limits)
            .map_err(|e| VerifyError::Usage(format!("failed to load module graph: {e}")))?;
        let manifest = graph.manifest(crate::world::manifest::WORLD_PROFILE);
        if manifest.transitive_manifest_digest != options.expect_program {
            return Err(VerifyError::ProgramPinMismatch {
                expected: options.expect_program,
                found: manifest.transitive_manifest_digest,
            });
        }
        if bundle.program.program_manifest_digest != options.expect_program {
            return Err(VerifyError::ProgramPinMismatch {
                expected: options.expect_program,
                found: bundle.program.program_manifest_digest,
            });
        }
        Some(
            graph
                .link()
                .map_err(|e| VerifyError::Usage(format!("failed to link program: {e}")))?,
        )
    } else {
        if bundle.world_manifest.program_required {
            return Err(VerifyError::Usage(
                "executable world requires program sources".into(),
            ));
        }
        None
    };

    // Recompute WorldManifest digest
    let computed_manifest_digest = bundle.world_manifest.digest();
    let _ = computed_manifest_digest;

    let ref_prog = if let Some(ref linked) = linked_prog {
        Some(reference::from_program(linked)?)
    } else {
        if bundle.world_manifest.program_required {
            return Err(VerifyError::Usage(
                "executable world requires reference program".into(),
            ));
        }
        None
    };

    // 2. Scope & Initial state
    let mut relations: BTreeMap<String, BTreeMap<WorldKey, WorldTuple>> = BTreeMap::new();
    let mut tries: BTreeMap<String, TrieMap<WorldKey, WorldTuple>> = BTreeMap::new();
    for rel_name in bundle.world_manifest.relations.keys() {
        relations.insert(rel_name.clone(), BTreeMap::new());
        tries.insert(rel_name.clone(), TrieMap::new());
    }

    let mut prev_settlements: BTreeMap<String, BTreeMap<String, SettledDecision>> = BTreeMap::new();

    let (scope_str, mut prev_seq, mut prev_digest) = match &bundle.scope {
        ScopeV1::Genesis => {
            // Compute expected revision 0 (genesis)
            let mut rev0_sec_roots = BTreeMap::new();
            for (name, decl) in &bundle.world_manifest.relations {
                for idx in &decl.secondary_indexes {
                    let sec_name = format!("{}:{}", name, idx);
                    let trie = TrieMap::<WorldKey, WorldTuple>::new();
                    rev0_sec_roots.insert(sec_name, trie.root_digest());
                }
            }
            let rev0 = super::revision::WorldRevision::new(
                0,
                &bundle.world_manifest.created_at,
                0,
                "genesis",
                None,
                if bundle.world_manifest.program_required {
                    Some(bundle.world_manifest.program_digest)
                } else {
                    None
                },
                None,
                BTreeMap::new(),
                BTreeMap::new(),
                rev0_sec_roots,
                None,
                BTreeMap::new(),
                SettlementStatus::Committed,
                None,
            )
            .bind_exec_profile(if bundle.world_manifest.program_required {
                Some(bundle.exec_profile.digest())
            } else {
                None
            });
            let genesis_digest = rev0.revision_digest;
            let genesis_digest_unbound = rev0.bind_exec_profile(None).revision_digest;

            // If revisions is empty, head must be revision 0
            if bundle.revisions.is_empty() {
                if bundle.head.seq != 0 {
                    return Err(VerifyError::NonContiguousSeq {
                        expected: bundle.head.seq,
                        found: 0,
                    });
                }
                if bundle.head.revision_digest != genesis_digest
                    && bundle.head.revision_digest != genesis_digest_unbound
                {
                    return Err(VerifyError::HeadPinMismatch {
                        expected: options.expect_head,
                        found: bundle.head.revision_digest,
                    });
                }
            } else {
                let first_rev = &bundle.revisions[0].record;
                if first_rev.seq != 1 {
                    return Err(VerifyError::NonContiguousSeq {
                        expected: 1,
                        found: first_rev.seq,
                    });
                }
                if first_rev.previous_revision_digest != Some(genesis_digest)
                    && first_rev.previous_revision_digest != Some(genesis_digest_unbound)
                {
                    return Err(VerifyError::PreviousDigestMismatch { seq: 1 });
                }
            }

            (
                "complete-from-genesis (0..HEAD)".to_string(),
                0u64,
                bundle
                    .revisions
                    .first()
                    .and_then(|r| r.record.previous_revision_digest)
                    .or(Some(genesis_digest)),
            )
        }
        ScopeV1::Checkpoint {
            seq,
            revision_digest,
            state,
        } => {
            let Some(trusted) = options.trust_checkpoint else {
                return Err(VerifyError::CheckpointUntrusted);
            };
            if *revision_digest != trusted && state.digest() != trusted {
                return Err(VerifyError::CheckpointUntrusted);
            }

            // Verify checkpoint record fields
            if state.record.seq != *seq {
                return Err(VerifyError::NonContiguousSeq {
                    expected: *seq,
                    found: state.record.seq,
                });
            }
            if state.record.revision_digest != *revision_digest {
                return Err(VerifyError::RevisionDigestMismatch { seq: *seq });
            }
            if state.record.compute_digest_for_record() != state.record.revision_digest {
                return Err(VerifyError::RevisionDigestMismatch { seq: *seq });
            }
            if bundle.world_manifest.program_required
                && state.record.program_digest != Some(options.expect_program)
            {
                return Err(VerifyError::ProgramPinMismatch {
                    expected: options.expect_program,
                    found: state.record.program_digest.unwrap_or(options.expect_head),
                });
            }
            if let Some(bound_profile) = state.record.exec_profile_digest {
                if bundle.exec_profile.digest() != bound_profile {
                    return Err(VerifyError::ExecProfileMismatch {
                        expected: bound_profile,
                        found: bundle.exec_profile.digest(),
                    });
                }
            }

            // Populate and verify checkpoint relation roots & cardinalities
            let state_rel_map: BTreeMap<&str, &Vec<(WorldKey, WorldTuple)>> = state
                .relations
                .iter()
                .map(|(k, v)| (k.as_str(), v))
                .collect();
            for (rel, _) in &state.relations {
                if !bundle.world_manifest.relations.contains_key(rel) {
                    return Err(VerifyError::Usage(format!(
                        "unknown relation in checkpoint state: {rel}"
                    )));
                }
            }
            for rel in state.record.relation_roots.keys() {
                if !bundle.world_manifest.relations.contains_key(rel) {
                    return Err(VerifyError::Usage(format!(
                        "checkpoint relation root for undeclared relation: {rel}"
                    )));
                }
            }

            let empty_rows = Vec::new();
            for rel in bundle.world_manifest.relations.keys() {
                let rows = state_rel_map
                    .get(rel.as_str())
                    .copied()
                    .unwrap_or(&empty_rows);
                let mut trie = TrieMap::new();
                let rel_map = relations.entry(rel.clone()).or_default();
                for (k, v) in rows {
                    work.tuples_decoded += 1;
                    check_budget(&work, options.max_work)?;
                    rel_map.insert(k.clone(), v.clone());
                    trie = trie.insert(k.clone(), v.clone());
                    work.trie_nodes_built += 1;
                    check_budget(&work, options.max_work)?;
                }
                let expected_root = state.record.relation_roots.get(rel);
                let expected_card = state
                    .record
                    .relation_cardinalities
                    .get(rel)
                    .copied()
                    .unwrap_or(0);
                if let Some(exp_root) = expected_root {
                    if trie.root_digest() != *exp_root {
                        return Err(VerifyError::RelationRootMismatch {
                            seq: *seq,
                            relation: rel.clone(),
                            expected: *exp_root,
                            found: trie.root_digest(),
                        });
                    }
                } else if !trie.is_empty() {
                    return Err(VerifyError::RelationCardinalityMismatch {
                        seq: *seq,
                        relation: rel.clone(),
                        expected: 0,
                        found: trie.len(),
                    });
                }
                if trie.len() != expected_card {
                    return Err(VerifyError::RelationCardinalityMismatch {
                        seq: *seq,
                        relation: rel.clone(),
                        expected: expected_card,
                        found: trie.len(),
                    });
                }
                tries.insert(rel.clone(), trie);
            }
            check_budget(&work, options.max_work)?;

            // Rebuild and check secondary index roots
            for (rel, decl) in &bundle.world_manifest.relations {
                let empty_map = BTreeMap::new();
                let rel_map = relations.get(rel).unwrap_or(&empty_map);
                for sec_field in &decl.secondary_indexes {
                    let sec_key_name = format!("{rel}:{sec_field}");
                    let mut groups: BTreeMap<WorldKey, BTreeSet<WorldKey>> = BTreeMap::new();
                    for (key, tuple) in rel_map {
                        if let Ok(sec_val) = extract_indexed_field(rel, decl, sec_field, tuple) {
                            let sec_key = encode_secondary_key(&sec_val);
                            groups.entry(sec_key).or_default().insert(key.clone());
                        }
                    }
                    let mut sec_trie = TrieMap::new();
                    for (sec_key, primary_keys) in groups {
                        let mut inner_set = TrieMap::new();
                        for pk in primary_keys {
                            inner_set = inner_set.insert(pk, WorldTuple::new(vec![]));
                            work.index_rows_rebuilt += 1;
                            check_budget(&work, options.max_work)?;
                        }
                        sec_trie = sec_trie.insert(
                            sec_key,
                            WorldTuple::new(inner_set.root_digest().as_bytes().to_vec()),
                        );
                    }
                    if let Some(expected_sec_root) =
                        state.record.secondary_index_roots.get(&sec_key_name)
                    {
                        if sec_trie.root_digest() != *expected_sec_root {
                            return Err(VerifyError::SecondaryIndexRootMismatch {
                                seq: *seq,
                                index: sec_key_name,
                                expected: *expected_sec_root,
                                found: sec_trie.root_digest(),
                            });
                        }
                    }
                }
            }
            check_budget(&work, options.max_work)?;

            // Populate and verify checkpoint decisions
            for (decide, entity, dt) in &state.decisions {
                let dec = SettledDecision {
                    entity_id: entity.clone(),
                    candidate_name: dt.candidate_name.clone(),
                    priority: dt.priority,
                    phase: dt.phase,
                    value: dt.value.clone(),
                    calendar_key: soc_core::calendar::Key {
                        phase: dt.phase,
                        priority: dt.priority,
                        tiebreak: dt.tiebreak,
                    },
                };
                prev_settlements
                    .entry(decide.clone())
                    .or_default()
                    .insert(entity.clone(), dec);
            }
            let computed_root = if ref_prog.is_some() || !prev_settlements.is_empty() {
                Some(compute_decision_root(&prev_settlements))
            } else {
                None
            };
            if computed_root != state.record.decision_root {
                return Err(VerifyError::DecisionRootMismatch {
                    seq: *seq,
                    expected: state.record.decision_root,
                    found: computed_root,
                });
            }

            if let Some(ref prog) = ref_prog {
                let mut ref_meter = reference::ReferenceWorkMeter::default();
                let ref_state = reference::evaluate_with_budget(
                    prog,
                    &relations,
                    &mut ref_meter,
                    options.max_work,
                    work.total_work(),
                )?;
                work.tuples_decoded += ref_meter.tuples_scanned;
                work.rows_evaluated += ref_meter.rows_evaluated;
                work.candidates_evaluated += ref_meter.candidates_evaluated;
                work.settlements_computed += ref_meter.settlements_computed;
                work.expressions_evaluated += ref_meter.expressions_evaluated;
                work.join_pairs_evaluated += ref_meter.join_pairs_evaluated;
                check_budget(&work, options.max_work)?;
                let mut eval_decisions = Vec::new();
                for (decide, entities) in &ref_state.settlements {
                    for (ent, sett) in entities {
                        let dec = convert_reference_settlement(sett);
                        let dt = DecisionTupleV1 {
                            candidate_name: dec.candidate_name,
                            priority: dec.priority,
                            phase: dec.phase,
                            value: dec.value,
                            tiebreak: dec.calendar_key.tiebreak,
                        };
                        eval_decisions.push((decide.clone(), ent.clone(), dt));
                    }
                }
                eval_decisions.sort_by(|a, b| {
                    (a.0.as_str(), a.1.as_str()).cmp(&(b.0.as_str(), b.1.as_str()))
                });
                let mut sorted_state_decisions = state.decisions.clone();
                sorted_state_decisions.sort_by(|a, b| {
                    (a.0.as_str(), a.1.as_str()).cmp(&(b.0.as_str(), b.1.as_str()))
                });
                if sorted_state_decisions != eval_decisions {
                    return Err(VerifyError::DecisionDeltaMismatch { seq: *seq });
                }
            }

            let trust_basis = if trusted == state.digest() {
                "verified-by-this-tool"
            } else {
                "caller-pin"
            };
            (
                format!(
                    "checkpoint-suffix ({seq}..HEAD) trust={trust_basis}:{}",
                    trusted.to_hex()
                ),
                *seq,
                Some(*revision_digest),
            )
        }
    };

    // Check HEAD pin
    if bundle.head.revision_digest != options.expect_head {
        return Err(VerifyError::HeadPinMismatch {
            expected: options.expect_head,
            found: bundle.head.revision_digest,
        });
    }

    let mut verified_settlements = Vec::new();

    // 3. Replay and verify each revision
    for entry in &bundle.revisions {
        check_budget(&work, options.max_work)?;
        let record = &entry.record;

        // Verify revision digest matches record fields
        if record.compute_digest_for_record() != record.revision_digest {
            return Err(VerifyError::RevisionDigestMismatch { seq: record.seq });
        }

        // Sequence & continuity
        let expected_seq = prev_seq + 1;
        if record.seq != expected_seq {
            return Err(VerifyError::NonContiguousSeq {
                expected: expected_seq,
                found: record.seq,
            });
        }
        if record.previous_revision_digest != prev_digest {
            return Err(VerifyError::PreviousDigestMismatch { seq: record.seq });
        }
        if record.status != SettlementStatus::Committed {
            return Err(VerifyError::UncommittedRevision { seq: record.seq });
        }
        if let Some(prog) = record.program_digest {
            if prog != options.expect_program {
                return Err(VerifyError::ProgramPinMismatch {
                    expected: options.expect_program,
                    found: prog,
                });
            }
        }
        if let Some(bound_profile) = record.exec_profile_digest {
            if bundle.exec_profile.digest() != bound_profile {
                return Err(VerifyError::ExecProfileMismatch {
                    expected: bound_profile,
                    found: bundle.exec_profile.digest(),
                });
            }
        }

        // Check for unknown relations in revision record
        for rel in record.relation_roots.keys() {
            if !bundle.world_manifest.relations.contains_key(rel) {
                return Err(VerifyError::Usage(format!(
                    "unknown relation in revision record: {rel}"
                )));
            }
        }

        // Apply source delta
        let mut delta_changed_keys: BTreeMap<String, BTreeSet<WorldKey>> = BTreeMap::new();
        for (rel, key, val_opt) in &entry.source_delta {
            if !bundle.world_manifest.relations.contains_key(rel) {
                return Err(VerifyError::Usage(format!(
                    "unknown relation in source delta: {rel}"
                )));
            }
            work.tuples_decoded += 1;
            check_budget(&work, options.max_work)?;
            delta_changed_keys
                .entry(rel.clone())
                .or_default()
                .insert(key.clone());
            let rel_map = relations.entry(rel.clone()).or_default();
            let mut trie = tries.remove(rel).unwrap_or_default();

            match val_opt {
                Some(tuple) => {
                    rel_map.insert(key.clone(), tuple.clone());
                    trie = trie.insert(key.clone(), tuple.clone());
                    work.trie_nodes_built += 1;
                    check_budget(&work, options.max_work)?;
                }
                None => {
                    rel_map.remove(key);
                    trie = trie.remove(key);
                }
            }
            tries.insert(rel.clone(), trie);
        }

        // Check relation roots and cardinalities
        for (rel, decl) in &bundle.world_manifest.relations {
            let _ = decl;
            let expected_root = record.relation_roots.get(rel);
            let expected_card = record.relation_cardinalities.get(rel).copied().unwrap_or(0);
            let trie = tries.get(rel).expect("relation trie exists");

            if let Some(exp_root) = expected_root {
                if trie.root_digest() != *exp_root {
                    return Err(VerifyError::RelationRootMismatch {
                        seq: record.seq,
                        relation: rel.clone(),
                        expected: *exp_root,
                        found: trie.root_digest(),
                    });
                }
            }
            if trie.len() != expected_card {
                return Err(VerifyError::RelationCardinalityMismatch {
                    seq: record.seq,
                    relation: rel.clone(),
                    expected: expected_card,
                    found: trie.len(),
                });
            }

            // Check changed keys
            let empty_set = BTreeSet::new();
            let actual_changed = delta_changed_keys.get(rel).unwrap_or(&empty_set);
            let recorded_changed: BTreeSet<WorldKey> = record
                .changed_keys
                .get(rel)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .collect();
            if actual_changed != &recorded_changed {
                return Err(VerifyError::ChangedKeysMismatch {
                    seq: record.seq,
                    relation: rel.clone(),
                });
            }
        }

        // Secondary index rebuild and verification
        for (rel, decl) in &bundle.world_manifest.relations {
            let rel_map = &relations[rel];
            for sec_field in &decl.secondary_indexes {
                let sec_key_name = format!("{rel}:{sec_field}");
                let mut groups: BTreeMap<WorldKey, BTreeSet<WorldKey>> = BTreeMap::new();
                for (key, tuple) in rel_map {
                    if let Ok(sec_val) = extract_indexed_field(rel, decl, sec_field, tuple) {
                        let sec_key = encode_secondary_key(&sec_val);
                        groups.entry(sec_key).or_default().insert(key.clone());
                    }
                }
                let mut sec_trie = TrieMap::new();
                for (sec_key, primary_keys) in groups {
                    let mut inner_set = TrieMap::new();
                    for pk in primary_keys {
                        inner_set = inner_set.insert(pk, WorldTuple::new(vec![]));
                        work.index_rows_rebuilt += 1;
                        check_budget(&work, options.max_work)?;
                    }
                    sec_trie = sec_trie.insert(
                        sec_key,
                        WorldTuple::new(inner_set.root_digest().as_bytes().to_vec()),
                    );
                }
                if let Some(expected_sec_root) = record.secondary_index_roots.get(&sec_key_name) {
                    if sec_trie.root_digest() != *expected_sec_root {
                        return Err(VerifyError::SecondaryIndexRootMismatch {
                            seq: record.seq,
                            index: sec_key_name,
                            expected: *expected_sec_root,
                            found: sec_trie.root_digest(),
                        });
                    }
                }
            }
        }

        // Decision recomputation with Reference Evaluator
        if let Some(ref prog) = ref_prog {
            let mut ref_meter = reference::ReferenceWorkMeter::default();
            let ref_state = reference::evaluate_with_budget(
                prog,
                &relations,
                &mut ref_meter,
                options.max_work,
                work.total_work(),
            )?;
            work.tuples_decoded += ref_meter.tuples_scanned;
            work.rows_evaluated += ref_meter.rows_evaluated;
            work.candidates_evaluated += ref_meter.candidates_evaluated;
            work.settlements_computed += ref_meter.settlements_computed;
            work.expressions_evaluated += ref_meter.expressions_evaluated;
            work.join_pairs_evaluated += ref_meter.join_pairs_evaluated;
            check_budget(&work, options.max_work)?;

            // Convert settlements to SettledDecision map
            let mut curr_settlements: BTreeMap<String, BTreeMap<String, SettledDecision>> =
                BTreeMap::new();
            for (decide, entities) in &ref_state.settlements {
                let mut ent_map = BTreeMap::new();
                for (ent, sett) in entities {
                    let dec = convert_reference_settlement(sett);
                    verified_settlements.push((
                        record.seq,
                        decide.clone(),
                        ent.clone(),
                        dec.candidate_name.clone(),
                    ));
                    ent_map.insert(ent.clone(), dec);
                }
                curr_settlements.insert(decide.clone(), ent_map);
            }

            let computed_root = Some(compute_decision_root(&curr_settlements));

            if computed_root != record.decision_root {
                return Err(VerifyError::DecisionRootMismatch {
                    seq: record.seq,
                    expected: record.decision_root,
                    found: computed_root,
                });
            }

            // Verify decision delta matches curr_settlements △ prev_settlements
            let mut expected_delta = Vec::new();
            let mut all_decides: BTreeSet<String> = curr_settlements.keys().cloned().collect();
            all_decides.extend(prev_settlements.keys().cloned());

            for decide in all_decides {
                let curr_ents = curr_settlements.get(&decide);
                let prev_ents = prev_settlements.get(&decide);
                let mut all_entities: BTreeSet<String> = BTreeSet::new();
                if let Some(e) = curr_ents {
                    all_entities.extend(e.keys().cloned());
                }
                if let Some(e) = prev_ents {
                    all_entities.extend(e.keys().cloned());
                }

                for entity in all_entities {
                    let curr_val = curr_ents.and_then(|e| e.get(&entity));
                    let prev_val = prev_ents.and_then(|e| e.get(&entity));
                    if curr_val != prev_val {
                        let dt = curr_val.map(|s| DecisionTupleV1 {
                            candidate_name: s.candidate_name.clone(),
                            priority: s.priority,
                            phase: s.phase,
                            value: s.value.clone(),
                            tiebreak: s.calendar_key.tiebreak,
                        });
                        expected_delta.push((decide.clone(), entity, dt));
                    }
                }
            }
            expected_delta
                .sort_by(|a, b| (a.0.as_str(), a.1.as_str()).cmp(&(b.0.as_str(), b.1.as_str())));

            if entry.decision_delta != expected_delta {
                return Err(VerifyError::DecisionDeltaMismatch { seq: record.seq });
            }

            if record.decision_root.is_some() || record.decision_delta_digest.is_some() {
                let mut added: BTreeMap<String, BTreeMap<String, SettledDecision>> =
                    BTreeMap::new();
                let mut removed: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
                for (decide, entity, opt_dt) in &expected_delta {
                    if let Some(dt) = opt_dt {
                        let sd = SettledDecision {
                            entity_id: entity.clone(),
                            candidate_name: dt.candidate_name.clone(),
                            priority: dt.priority,
                            phase: dt.phase,
                            value: dt.value.clone(),
                            calendar_key: Key::new(dt.phase, dt.priority, dt.tiebreak),
                        };
                        added
                            .entry(decide.clone())
                            .or_default()
                            .insert(entity.clone(), sd);
                    } else {
                        removed
                            .entry(decide.clone())
                            .or_default()
                            .insert(entity.clone());
                    }
                }
                let body = encode_decision_delta(&added, &removed);
                let computed_delta_digest = Digest::of(Domain::Value, &body);
                if let Some(expected_digest) = record.decision_delta_digest {
                    if computed_delta_digest != expected_digest {
                        return Err(VerifyError::DecisionDeltaMismatch { seq: record.seq });
                    }
                } else if record.schema == REVISION_SCHEMA_V2 {
                    return Err(VerifyError::DecisionDeltaMismatch { seq: record.seq });
                }
            }

            prev_settlements = curr_settlements;
        } else if record.decision_root.is_some() {
            return Err(VerifyError::DecisionRootMismatch {
                seq: record.seq,
                expected: record.decision_root,
                found: None,
            });
        }

        prev_seq = record.seq;
        prev_digest = Some(record.revision_digest);
        work.revisions_replayed += 1;

        check_budget(&work, options.max_work)?;
    }

    if prev_seq != bundle.head.seq {
        return Err(VerifyError::NonContiguousSeq {
            expected: bundle.head.seq,
            found: prev_seq,
        });
    }
    if prev_digest != Some(bundle.head.revision_digest) {
        return Err(VerifyError::HeadPinMismatch {
            expected: options.expect_head,
            found: prev_digest.unwrap_or_else(|| Digest::from_bytes([0u8; 32])),
        });
    }

    Ok(VerifyReport {
        scope: scope_str,
        head_revision: bundle.head.seq,
        head_digest: bundle.head.revision_digest,
        program_digest: options.expect_program,
        work,
        verified_settlements,
    })
}
