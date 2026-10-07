//! Transactional batch mutation envelope schema `brix.world.batch@1` (ADR-0046 §3.6, P3).

use brix_canon::{CanonWriter, Canonical, Digest, Domain};
use serde_json::{json, Value as JsonValue};
use std::collections::BTreeMap;

use super::codec::TupleRecord;
use super::error::WorldError;
use super::types::{WorldKey, WorldTuple};

pub const BATCH_SCHEMA: &str = "brix.world.batch@1";
const BATCH_TAG: &str = "brix.world.batch@1";

/// A single atomic mutation operation inside a batch.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum WorldBatchOp {
    /// Upsert a record into a relation with primary key and tuple value.
    Upsert {
        relation: String,
        key: WorldKey,
        tuple: WorldTuple,
    },
    /// Remove a record from a relation by primary key.
    Remove { relation: String, key: WorldKey },
}

impl WorldBatchOp {
    pub fn relation(&self) -> &str {
        match self {
            Self::Upsert { relation, .. } => relation,
            Self::Remove { relation, .. } => relation,
        }
    }

    pub fn key(&self) -> &WorldKey {
        match self {
            Self::Upsert { key, .. } => key,
            Self::Remove { key, .. } => key,
        }
    }

    pub fn canon_write(&self, w: &mut CanonWriter) {
        match self {
            Self::Upsert {
                relation,
                key,
                tuple,
            } => {
                w.write_uint(1); // 1 = Upsert
                w.write_ident(relation);
                key.canon_write(w);
                tuple.canon_write(w);
            }
            Self::Remove { relation, key } => {
                w.write_uint(2); // 2 = Remove
                w.write_ident(relation);
                key.canon_write(w);
            }
        }
    }

    pub fn to_json(&self) -> JsonValue {
        match self {
            Self::Upsert {
                relation,
                key,
                tuple,
            } => json!({
                "op": "upsert",
                "relation": relation,
                "key": key.to_hex(),
                "tuple": tuple.to_hex(),
            }),
            Self::Remove { relation, key } => json!({
                "op": "remove",
                "relation": relation,
                "key": key.to_hex(),
            }),
        }
    }

    pub fn from_json(v: &JsonValue) -> Result<Self, WorldError> {
        let op = v
            .get("op")
            .and_then(|x| x.as_str())
            .ok_or_else(|| WorldError::Json("batch op missing 'op'".to_string()))?;
        let relation = v
            .get("relation")
            .and_then(|x| x.as_str())
            .ok_or_else(|| WorldError::Json("batch op missing 'relation'".to_string()))?
            .to_string();
        let key_str = v
            .get("key")
            .and_then(|x| x.as_str())
            .ok_or_else(|| WorldError::Json("batch op missing 'key'".to_string()))?;
        let key = WorldKey::from_hex(key_str).unwrap_or_else(|_| WorldKey::from_str(key_str));

        match op {
            "upsert" => {
                let tuple_val = v
                    .get("tuple")
                    .ok_or_else(|| WorldError::Json("upsert op missing 'tuple'".to_string()))?;
                let tuple = match tuple_val {
                    JsonValue::String(tuple_hex) => {
                        if tuple_hex.len() % 2 == 0 {
                            if let Ok(k) = WorldKey::from_hex(tuple_hex) {
                                WorldTuple::new(k.0)
                            } else {
                                WorldTuple::from_str(tuple_hex)
                            }
                        } else {
                            WorldTuple::from_str(tuple_hex)
                        }
                    }
                    JsonValue::Object(map) => {
                        let mut rec = TupleRecord::new();
                        for (k, val) in map {
                            let s = match val {
                                JsonValue::String(s) => s.clone(),
                                JsonValue::Number(n) => n.to_string(),
                                JsonValue::Bool(b) => b.to_string(),
                                JsonValue::Null => String::new(),
                                _ => val.to_string(),
                            };
                            rec.set_str(k, &s);
                        }
                        rec.to_tuple()
                    }
                    _ => {
                        return Err(WorldError::Json(
                            "upsert op 'tuple' must be string or object".to_string(),
                        ))
                    }
                };
                Ok(Self::Upsert {
                    relation,
                    key,
                    tuple,
                })
            }
            "remove" => Ok(Self::Remove { relation, key }),
            other => Err(WorldError::Json(format!("unknown batch op kind '{other}'"))),
        }
    }
}

/// The transactional mutation batch envelope `brix.world.batch@1`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct WorldBatch {
    pub schema: String,
    pub expected_base_revision: u64,
    pub idempotency_key: String,
    pub operations: Vec<WorldBatchOp>,
}

impl WorldBatch {
    pub fn new(
        expected_base_revision: u64,
        idempotency_key: impl Into<String>,
        operations: Vec<WorldBatchOp>,
    ) -> Self {
        Self {
            schema: BATCH_SCHEMA.to_string(),
            expected_base_revision,
            idempotency_key: idempotency_key.into(),
            operations,
        }
    }

    pub fn digest(&self) -> Digest {
        let mut w = CanonWriter::new();
        w.write_tag(BATCH_TAG);
        w.write_ident(&self.schema);
        w.write_uint(self.expected_base_revision);
        w.write_str(&self.idempotency_key);
        w.write_uint(self.operations.len() as u64);
        for op in &self.operations {
            op.canon_write(&mut w);
        }
        w.digest(Domain::Value)
    }

    /// Normalize operations deterministically and reject conflicting duplicate operations
    /// (ADR-0046 §3.6: "Normalize operations deterministically; reject conflicting
    /// duplicate operations rather than taking transport order as an implicit override.")
    pub fn validate_and_normalize(&self) -> Result<Vec<WorldBatchOp>, WorldError> {
        let mut seen: BTreeMap<(&str, &WorldKey), &WorldBatchOp> = BTreeMap::new();
        let mut normalized = Vec::with_capacity(self.operations.len());

        for op in &self.operations {
            let rel_key = (op.relation(), op.key());
            if let Some(existing) = seen.get(&rel_key) {
                match (existing, op) {
                    (
                        WorldBatchOp::Upsert { tuple: t1, .. },
                        WorldBatchOp::Upsert { tuple: t2, .. },
                    ) => {
                        if t1 != t2 {
                            return Err(WorldError::BatchConflict {
                                relation: op.relation().to_string(),
                                key: op.key().clone(),
                                reason: "conflicting duplicate upsert with differing tuple values"
                                    .to_string(),
                            });
                        }
                        // Identical duplicate upsert is idempotent; skip redundant copy
                        continue;
                    }
                    (WorldBatchOp::Remove { .. }, WorldBatchOp::Remove { .. }) => {
                        // Identical duplicate remove is idempotent; skip redundant copy
                        continue;
                    }
                    _ => {
                        return Err(WorldError::BatchConflict {
                            relation: op.relation().to_string(),
                            key: op.key().clone(),
                            reason: "contradictory operations (upsert and remove in same batch)"
                                .to_string(),
                        });
                    }
                }
            } else {
                seen.insert(rel_key, op);
                normalized.push(op.clone());
            }
        }

        Ok(normalized)
    }

    pub fn to_json(&self) -> JsonValue {
        let ops: Vec<JsonValue> = self.operations.iter().map(|op| op.to_json()).collect();
        json!({
            "schema": self.schema,
            "expected_base_revision": self.expected_base_revision,
            "idempotency_key": self.idempotency_key,
            "operations": ops,
        })
    }

    pub fn from_json(v: &JsonValue) -> Result<Self, WorldError> {
        let schema = v
            .get("schema")
            .and_then(|x| x.as_str())
            .ok_or_else(|| WorldError::Json("batch missing 'schema'".to_string()))?;
        if schema != BATCH_SCHEMA {
            return Err(WorldError::InvalidSchema(format!(
                "expected batch schema {BATCH_SCHEMA}, got {schema}"
            )));
        }

        let expected_base_revision = v
            .get("expected_base_revision")
            .and_then(|x| x.as_u64())
            .ok_or_else(|| {
                WorldError::Json("batch missing 'expected_base_revision'".to_string())
            })?;

        let idempotency_key = v
            .get("idempotency_key")
            .and_then(|x| x.as_str())
            .ok_or_else(|| WorldError::Json("batch missing 'idempotency_key'".to_string()))?
            .to_string();

        let ops_arr = v
            .get("operations")
            .and_then(|x| x.as_array())
            .ok_or_else(|| WorldError::Json("batch missing 'operations'".to_string()))?;

        let mut operations = Vec::with_capacity(ops_arr.len());
        for op_val in ops_arr {
            operations.push(WorldBatchOp::from_json(op_val)?);
        }

        Ok(Self {
            schema: schema.to_string(),
            expected_base_revision,
            idempotency_key,
            operations,
        })
    }
}
