//! Immutable revision records schema `brix.world.revision@1` (ADR-0046 §3.6, P3).

use brix_canon::{CanonWriter, Canonical, Digest, Domain};
use serde_json::{json, Value as JsonValue};
use std::collections::BTreeMap;

use super::error::WorldError;
use super::types::WorldKey;

pub const REVISION_SCHEMA: &str = "brix.world.revision@1";
const REVISION_TAG: &str = "brix.world.revision@1";

/// Settlement status of the world state following batch application.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SettlementStatus {
    /// Batch successfully committed into a new consistent revision.
    Committed,
    /// Batch evaluation exhausted or failed, leaving previous revision intact.
    Unknown { reason: String, attempt_id: Digest },
}

impl SettlementStatus {
    pub fn canon_write(&self, w: &mut CanonWriter) {
        match self {
            Self::Committed => w.write_uint(1),
            Self::Unknown { reason, attempt_id } => {
                w.write_uint(2);
                w.write_str(reason);
                w.write_bytes(attempt_id.as_bytes());
            }
        }
    }

    pub fn to_json(&self) -> JsonValue {
        match self {
            Self::Committed => json!({"status": "committed"}),
            Self::Unknown { reason, attempt_id } => json!({
                "status": "unknown",
                "reason": reason,
                "attempt_id": attempt_id.to_hex(),
            }),
        }
    }

    pub fn from_json(v: &JsonValue) -> Result<Self, WorldError> {
        let status = v
            .get("status")
            .and_then(|x| x.as_str())
            .ok_or_else(|| WorldError::Json("status missing 'status'".to_string()))?;
        match status {
            "committed" => Ok(Self::Committed),
            "unknown" => {
                let reason = v
                    .get("reason")
                    .and_then(|x| x.as_str())
                    .ok_or_else(|| WorldError::Json("unknown status missing 'reason'".to_string()))?
                    .to_string();
                let att_hex = v
                    .get("attempt_id")
                    .and_then(|x| x.as_str())
                    .ok_or_else(|| {
                        WorldError::Json("unknown status missing 'attempt_id'".to_string())
                    })?;
                if att_hex.len() != 64 {
                    return Err(WorldError::Json(
                        "attempt_id must be 64-char hex".to_string(),
                    ));
                }
                let mut b = [0u8; 32];
                let chars: Vec<char> = att_hex.chars().collect();
                for i in (0..64).step_by(2) {
                    let high = chars[i]
                        .to_digit(16)
                        .ok_or_else(|| WorldError::Json("invalid hex in attempt_id".to_string()))?;
                    let low = chars[i + 1]
                        .to_digit(16)
                        .ok_or_else(|| WorldError::Json("invalid hex in attempt_id".to_string()))?;
                    b[i / 2] = ((high << 4) | low) as u8;
                }
                Ok(Self::Unknown {
                    reason,
                    attempt_id: Digest::from_bytes(b),
                })
            }
            other => Err(WorldError::Json(format!("unknown status kind '{other}'"))),
        }
    }
}

/// An immutable, content-addressed revision journal record.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct WorldRevision {
    pub schema: String,
    pub seq: u64,
    pub timestamp: String,
    pub expected_base_revision: u64,
    pub idempotency_key: String,
    pub batch_digest: Option<Digest>,
    /// Program closure identity pinned by this revision, when the world is executable.
    pub program_digest: Option<Digest>,
    pub previous_revision_digest: Option<Digest>,
    pub relation_roots: BTreeMap<String, Digest>,
    pub relation_cardinalities: BTreeMap<String, usize>,
    pub secondary_index_roots: BTreeMap<String, Digest>,
    pub decision_root: Option<Digest>,
    pub changed_keys: BTreeMap<String, Vec<WorldKey>>,
    pub status: SettlementStatus,
    pub revision_digest: Digest,
}

impl WorldRevision {
    /// Every field is independently load-bearing for the revision digest
    /// (`compute_digest` hashes them in this exact order); a params struct
    /// would just move the same 13 fields one level of indirection away
    /// without reducing the real arity this constructor has to bind.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        seq: u64,
        timestamp: impl Into<String>,
        expected_base_revision: u64,
        idempotency_key: impl Into<String>,
        batch_digest: Option<Digest>,
        program_digest: Option<Digest>,
        previous_revision_digest: Option<Digest>,
        relation_roots: BTreeMap<String, Digest>,
        relation_cardinalities: BTreeMap<String, usize>,
        secondary_index_roots: BTreeMap<String, Digest>,
        decision_root: Option<Digest>,
        changed_keys: BTreeMap<String, Vec<WorldKey>>,
        status: SettlementStatus,
    ) -> Self {
        let ts = timestamp.into();
        let ikey = idempotency_key.into();
        let digest = Self::compute_digest(
            seq,
            &ts,
            expected_base_revision,
            &ikey,
            &batch_digest,
            &program_digest,
            &previous_revision_digest,
            &relation_roots,
            &relation_cardinalities,
            &secondary_index_roots,
            &decision_root,
            &changed_keys,
            &status,
        );

        Self {
            schema: REVISION_SCHEMA.to_string(),
            seq,
            timestamp: ts,
            expected_base_revision,
            idempotency_key: ikey,
            batch_digest,
            program_digest,
            previous_revision_digest,
            relation_roots,
            relation_cardinalities,
            secondary_index_roots,
            decision_root,
            changed_keys,
            status,
            revision_digest: digest,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn compute_digest(
        seq: u64,
        timestamp: &str,
        expected_base_revision: u64,
        idempotency_key: &str,
        batch_digest: &Option<Digest>,
        program_digest: &Option<Digest>,
        previous_revision_digest: &Option<Digest>,
        relation_roots: &BTreeMap<String, Digest>,
        relation_cardinalities: &BTreeMap<String, usize>,
        secondary_index_roots: &BTreeMap<String, Digest>,
        decision_root: &Option<Digest>,
        changed_keys: &BTreeMap<String, Vec<WorldKey>>,
        status: &SettlementStatus,
    ) -> Digest {
        let mut w = CanonWriter::new();
        w.write_tag(REVISION_TAG);
        w.write_ident(REVISION_SCHEMA);
        w.write_uint(seq);
        w.write_str(timestamp);
        w.write_uint(expected_base_revision);
        w.write_str(idempotency_key);
        match batch_digest {
            None => w.write_uint(0),
            Some(b) => {
                w.write_uint(1);
                w.write_bytes(b.as_bytes());
            }
        }
        // Preserve historical storage-only revision digests byte-for-byte.
        if let Some(program) = program_digest {
            w.write_uint(1);
            w.write_bytes(program.as_bytes());
        }
        match previous_revision_digest {
            None => w.write_uint(0),
            Some(prev) => {
                w.write_uint(1);
                w.write_bytes(prev.as_bytes());
            }
        }
        w.write_uint(relation_roots.len() as u64);
        for (rel, root) in relation_roots {
            w.write_ident(rel);
            w.write_bytes(root.as_bytes());
            let card = relation_cardinalities.get(rel).copied().unwrap_or(0);
            w.write_uint(card as u64);
        }
        w.write_uint(secondary_index_roots.len() as u64);
        for (idx, root) in secondary_index_roots {
            w.write_ident(idx);
            w.write_bytes(root.as_bytes());
        }
        match decision_root {
            None => w.write_uint(0),
            Some(d) => {
                w.write_uint(1);
                w.write_bytes(d.as_bytes());
            }
        }
        w.write_uint(changed_keys.len() as u64);
        for (rel, keys) in changed_keys {
            w.write_ident(rel);
            w.write_uint(keys.len() as u64);
            for k in keys {
                k.canon_write(&mut w);
            }
        }
        status.canon_write(&mut w);
        w.digest(Domain::Value)
    }

    pub fn to_json(&self) -> JsonValue {
        let roots_json: BTreeMap<String, String> = self
            .relation_roots
            .iter()
            .map(|(k, v)| (k.clone(), v.to_hex()))
            .collect();
        let cards_json: BTreeMap<String, usize> = self.relation_cardinalities.clone();
        let sec_roots_json: BTreeMap<String, String> = self
            .secondary_index_roots
            .iter()
            .map(|(k, v)| (k.clone(), v.to_hex()))
            .collect();
        let changed_json: BTreeMap<String, Vec<String>> = self
            .changed_keys
            .iter()
            .map(|(k, keys)| (k.clone(), keys.iter().map(|key| key.to_hex()).collect()))
            .collect();

        json!({
            "schema": self.schema,
            "seq": self.seq,
            "timestamp": self.timestamp,
            "expected_base_revision": self.expected_base_revision,
            "idempotency_key": self.idempotency_key,
            "batch_digest": self.batch_digest.map(|d| d.to_hex()),
            "program_digest": self.program_digest.map(|d| d.to_hex()),
            "previous_revision_digest": self.previous_revision_digest.map(|d| d.to_hex()),
            "relation_roots": roots_json,
            "relation_cardinalities": cards_json,
            "secondary_index_roots": sec_roots_json,
            "decision_root": self.decision_root.map(|d| d.to_hex()),
            "changed_keys": changed_json,
            "status": self.status.to_json(),
            "revision_digest": self.revision_digest.to_hex(),
        })
    }

    pub fn from_json(v: &JsonValue) -> Result<Self, WorldError> {
        let schema = v
            .get("schema")
            .and_then(|x| x.as_str())
            .ok_or_else(|| WorldError::Json("revision missing 'schema'".to_string()))?;
        if schema != REVISION_SCHEMA {
            return Err(WorldError::InvalidSchema(format!(
                "expected revision schema {REVISION_SCHEMA}, got {schema}"
            )));
        }

        let seq = v
            .get("seq")
            .and_then(|x| x.as_u64())
            .ok_or_else(|| WorldError::Json("revision missing 'seq'".to_string()))?;

        let timestamp = v
            .get("timestamp")
            .and_then(|x| x.as_str())
            .ok_or_else(|| WorldError::Json("revision missing 'timestamp'".to_string()))?
            .to_string();

        let expected_base_revision = v
            .get("expected_base_revision")
            .and_then(|x| x.as_u64())
            .ok_or_else(|| {
                WorldError::Json("revision missing 'expected_base_revision'".to_string())
            })?;

        let idempotency_key = v
            .get("idempotency_key")
            .and_then(|x| x.as_str())
            .ok_or_else(|| WorldError::Json("revision missing 'idempotency_key'".to_string()))?
            .to_string();

        let batch_digest = match v.get("batch_digest") {
            Some(JsonValue::String(s)) => {
                let mut b = [0u8; 32];
                let chars: Vec<char> = s.chars().collect();
                if chars.len() != 64 {
                    return Err(WorldError::Json(
                        "batch_digest must be 64 hex chars".to_string(),
                    ));
                }
                for i in (0..64).step_by(2) {
                    let high = chars[i].to_digit(16).ok_or_else(|| {
                        WorldError::Json("invalid hex in batch digest".to_string())
                    })?;
                    let low = chars[i + 1].to_digit(16).ok_or_else(|| {
                        WorldError::Json("invalid hex in batch digest".to_string())
                    })?;
                    b[i / 2] = ((high << 4) | low) as u8;
                }
                Some(Digest::from_bytes(b))
            }
            _ => None,
        };

        let previous_revision_digest = match v.get("previous_revision_digest") {
            Some(JsonValue::String(s)) => {
                let mut b = [0u8; 32];
                let chars: Vec<char> = s.chars().collect();
                if chars.len() != 64 {
                    return Err(WorldError::Json(
                        "previous_revision_digest must be 64 hex chars".to_string(),
                    ));
                }
                for i in (0..64).step_by(2) {
                    let high = chars[i].to_digit(16).ok_or_else(|| {
                        WorldError::Json("invalid hex in prev digest".to_string())
                    })?;
                    let low = chars[i + 1].to_digit(16).ok_or_else(|| {
                        WorldError::Json("invalid hex in prev digest".to_string())
                    })?;
                    b[i / 2] = ((high << 4) | low) as u8;
                }
                Some(Digest::from_bytes(b))
            }
            _ => None,
        };

        let program_digest = match v.get("program_digest") {
            None | Some(JsonValue::Null) => None,
            Some(JsonValue::String(s)) => {
                if s.len() != 64 {
                    return Err(WorldError::Json(
                        "program_digest must be 64 hex chars".into(),
                    ));
                }
                let mut bytes = [0u8; 32];
                let chars: Vec<char> = s.chars().collect();
                for i in (0..64).step_by(2) {
                    let high = chars[i]
                        .to_digit(16)
                        .ok_or_else(|| WorldError::Json("invalid hex in program_digest".into()))?;
                    let low = chars[i + 1]
                        .to_digit(16)
                        .ok_or_else(|| WorldError::Json("invalid hex in program_digest".into()))?;
                    bytes[i / 2] = ((high << 4) | low) as u8;
                }
                Some(Digest::from_bytes(bytes))
            }
            _ => {
                return Err(WorldError::Json(
                    "program_digest must be a hex string or null".into(),
                ))
            }
        };

        let mut relation_roots = BTreeMap::new();
        if let Some(obj) = v.get("relation_roots").and_then(|x| x.as_object()) {
            for (rel, hex_val) in obj {
                let s = hex_val.as_str().ok_or_else(|| {
                    WorldError::Json(format!("relation root for '{rel}' must be hex string"))
                })?;
                let mut b = [0u8; 32];
                let chars: Vec<char> = s.chars().collect();
                if chars.len() != 64 {
                    return Err(WorldError::Json(format!(
                        "root for '{rel}' must be 64 hex chars"
                    )));
                }
                for i in (0..64).step_by(2) {
                    let high = chars[i].to_digit(16).ok_or_else(|| {
                        WorldError::Json(format!("invalid hex in root for '{rel}'"))
                    })?;
                    let low = chars[i + 1].to_digit(16).ok_or_else(|| {
                        WorldError::Json(format!("invalid hex in root for '{rel}'"))
                    })?;
                    b[i / 2] = ((high << 4) | low) as u8;
                }
                relation_roots.insert(rel.clone(), Digest::from_bytes(b));
            }
        }

        let mut relation_cardinalities = BTreeMap::new();
        if let Some(obj) = v.get("relation_cardinalities").and_then(|x| x.as_object()) {
            for (rel, c_val) in obj {
                let card = c_val.as_u64().unwrap_or(0) as usize;
                relation_cardinalities.insert(rel.clone(), card);
            }
        }

        let mut secondary_index_roots = BTreeMap::new();
        if let Some(obj) = v.get("secondary_index_roots").and_then(|x| x.as_object()) {
            for (idx, hex_val) in obj {
                let s = hex_val.as_str().ok_or_else(|| {
                    WorldError::Json(format!("secondary index root for '{idx}' must be string"))
                })?;
                let mut b = [0u8; 32];
                let chars: Vec<char> = s.chars().collect();
                if chars.len() != 64 {
                    return Err(WorldError::Json(format!(
                        "sec index root for '{idx}' must be 64 hex chars"
                    )));
                }
                for i in (0..64).step_by(2) {
                    let high = chars[i].to_digit(16).ok_or_else(|| {
                        WorldError::Json(format!("invalid hex in sec root for '{idx}'"))
                    })?;
                    let low = chars[i + 1].to_digit(16).ok_or_else(|| {
                        WorldError::Json(format!("invalid hex in sec root for '{idx}'"))
                    })?;
                    b[i / 2] = ((high << 4) | low) as u8;
                }
                secondary_index_roots.insert(idx.clone(), Digest::from_bytes(b));
            }
        }

        let mut changed_keys = BTreeMap::new();
        if let Some(obj) = v.get("changed_keys").and_then(|x| x.as_object()) {
            for (rel, arr_val) in obj {
                let mut keys = Vec::new();
                if let Some(arr) = arr_val.as_array() {
                    for k_val in arr {
                        if let Some(hex_str) = k_val.as_str() {
                            keys.push(WorldKey::from_hex(hex_str)?);
                        }
                    }
                }
                changed_keys.insert(rel.clone(), keys);
            }
        }

        let decision_root = match v.get("decision_root") {
            Some(JsonValue::String(s)) => {
                let mut b = [0u8; 32];
                let chars: Vec<char> = s.chars().collect();
                if chars.len() != 64 {
                    return Err(WorldError::Json(
                        "decision_root must be 64 hex chars".to_string(),
                    ));
                }
                for i in (0..64).step_by(2) {
                    let high = chars[i].to_digit(16).ok_or_else(|| {
                        WorldError::Json("invalid hex in decision_root".to_string())
                    })?;
                    let low = chars[i + 1].to_digit(16).ok_or_else(|| {
                        WorldError::Json("invalid hex in decision_root".to_string())
                    })?;
                    b[i / 2] = ((high << 4) | low) as u8;
                }
                Some(Digest::from_bytes(b))
            }
            _ => None,
        };

        let status = v
            .get("status")
            .ok_or_else(|| WorldError::Json("revision missing 'status'".to_string()))
            .and_then(SettlementStatus::from_json)?;

        let rev_hex = v
            .get("revision_digest")
            .and_then(|x| x.as_str())
            .ok_or_else(|| WorldError::Json("revision missing 'revision_digest'".to_string()))?;
        let mut d_bytes = [0u8; 32];
        let chars: Vec<char> = rev_hex.chars().collect();
        if chars.len() != 64 {
            return Err(WorldError::Json(
                "revision_digest must be 64 hex chars".to_string(),
            ));
        }
        for i in (0..64).step_by(2) {
            let high = chars[i]
                .to_digit(16)
                .ok_or_else(|| WorldError::Json("invalid hex in revision digest".to_string()))?;
            let low = chars[i + 1]
                .to_digit(16)
                .ok_or_else(|| WorldError::Json("invalid hex in revision digest".to_string()))?;
            d_bytes[i / 2] = ((high << 4) | low) as u8;
        }
        let revision_digest = Digest::from_bytes(d_bytes);

        let computed_digest = Self::compute_digest(
            seq,
            &timestamp,
            expected_base_revision,
            &idempotency_key,
            &batch_digest,
            &program_digest,
            &previous_revision_digest,
            &relation_roots,
            &relation_cardinalities,
            &secondary_index_roots,
            &decision_root,
            &changed_keys,
            &status,
        );

        if computed_digest != revision_digest {
            return Err(WorldError::CorruptedRevision {
                seq,
                expected: revision_digest,
                actual: computed_digest,
            });
        }

        Ok(Self {
            schema: schema.to_string(),
            seq,
            timestamp,
            expected_base_revision,
            idempotency_key,
            batch_digest,
            program_digest,
            previous_revision_digest,
            relation_roots,
            relation_cardinalities,
            secondary_index_roots,
            decision_root,
            changed_keys,
            status,
            revision_digest,
        })
    }
}
