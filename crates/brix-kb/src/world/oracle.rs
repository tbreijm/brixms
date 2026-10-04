//! In-memory bounded full-recompute correctness oracle (ADR-0046 §3.6, P3).

use std::collections::{BTreeMap, BTreeSet};

use super::batch::{WorldBatch, WorldBatchOp};
use super::error::WorldError;
use super::manifest::WorldManifest;
use super::types::{WorldKey, WorldTuple};

/// Change event emitted when diffing revisions.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DiffEvent {
    Upserted { key: WorldKey, tuple: WorldTuple },
    Removed { key: WorldKey },
}

impl DiffEvent {
    pub fn key(&self) -> &WorldKey {
        match self {
            Self::Upserted { key, .. } | Self::Removed { key } => key,
        }
    }
}

/// A snapshot of all relations at a particular revision in the oracle.
#[derive(Clone, Debug, Default)]
pub struct OracleRevisionState {
    pub relations: BTreeMap<String, BTreeMap<WorldKey, WorldTuple>>,
    pub secondary_indexes: BTreeMap<String, BTreeMap<WorldKey, BTreeSet<WorldKey>>>,
}

/// The independent reference oracle maintaining full recomputed state in memory.
#[derive(Clone, Debug)]
pub struct WorldOracle {
    pub manifest: WorldManifest,
    pub current_revision: u64,
    pub history: BTreeMap<u64, OracleRevisionState>,
    pub relations: BTreeMap<String, BTreeMap<WorldKey, WorldTuple>>,
    pub secondary_indexes: BTreeMap<String, BTreeMap<WorldKey, BTreeSet<WorldKey>>>,
}

impl WorldOracle {
    pub fn new(manifest: WorldManifest) -> Self {
        let mut rels = BTreeMap::new();
        let mut sec_idxs = BTreeMap::new();
        for (name, decl) in &manifest.relations {
            rels.insert(name.clone(), BTreeMap::new());
            for idx in &decl.secondary_indexes {
                sec_idxs.insert(format!("{}:{}", name, idx), BTreeMap::new());
            }
        }

        let mut history = BTreeMap::new();
        history.insert(
            0,
            OracleRevisionState {
                relations: rels.clone(),
                secondary_indexes: sec_idxs.clone(),
            },
        );

        Self {
            manifest,
            current_revision: 0,
            history,
            relations: rels,
            secondary_indexes: sec_idxs,
        }
    }

    pub fn apply_batch(&mut self, batch: &WorldBatch) -> Result<u64, WorldError> {
        if batch.expected_base_revision != self.current_revision {
            return Err(WorldError::StaleBaseRevision {
                expected: batch.expected_base_revision,
                current: self.current_revision,
            });
        }

        let normalized = batch.validate_and_normalize()?;
        let mut new_rels = self.relations.clone();

        for op in normalized {
            let rel = new_rels
                .get_mut(op.relation())
                .ok_or_else(|| WorldError::UnknownRelation(op.relation().to_string()))?;

            match op {
                WorldBatchOp::Upsert { key, tuple, .. } => {
                    rel.insert(key, tuple);
                }
                WorldBatchOp::Remove { key, .. } => {
                    rel.remove(&key);
                }
            }
        }

        self.current_revision += 1;
        self.relations = new_rels.clone();
        self.history.insert(
            self.current_revision,
            OracleRevisionState {
                relations: new_rels,
                secondary_indexes: self.secondary_indexes.clone(),
            },
        );

        Ok(self.current_revision)
    }

    pub fn get(&self, relation: &str, key: &WorldKey) -> Option<&WorldTuple> {
        self.relations.get(relation).and_then(|r| r.get(key))
    }

    pub fn diff_revisions(
        &self,
        from_rev: u64,
        to_rev: u64,
        relation: &str,
    ) -> Result<Vec<DiffEvent>, WorldError> {
        let from_state = self
            .history
            .get(&from_rev)
            .ok_or(WorldError::RevisionNotFound(from_rev))?;
        let to_state = self
            .history
            .get(&to_rev)
            .ok_or(WorldError::RevisionNotFound(to_rev))?;

        let empty = BTreeMap::new();
        let from_rel = from_state.relations.get(relation).unwrap_or(&empty);
        let to_rel = to_state.relations.get(relation).unwrap_or(&empty);

        let mut events = Vec::new();

        // Check for upserts/modifications
        for (k, v) in to_rel {
            match from_rel.get(k) {
                Some(old_v) if old_v == v => {}
                _ => events.push(DiffEvent::Upserted {
                    key: k.clone(),
                    tuple: v.clone(),
                }),
            }
        }

        // Check for removes
        for k in from_rel.keys() {
            if !to_rel.contains_key(k) {
                events.push(DiffEvent::Removed { key: k.clone() });
            }
        }

        events.sort_by(|a, b| match (a, b) {
            (DiffEvent::Upserted { key: k1, .. }, DiffEvent::Upserted { key: k2, .. }) => {
                k1.cmp(k2)
            }
            (DiffEvent::Removed { key: k1 }, DiffEvent::Removed { key: k2 }) => k1.cmp(k2),
            (DiffEvent::Upserted { key: k1, .. }, DiffEvent::Removed { key: k2 }) => k1.cmp(k2),
            (DiffEvent::Removed { key: k1 }, DiffEvent::Upserted { key: k2, .. }) => k1.cmp(k2),
        });

        Ok(events)
    }
}
