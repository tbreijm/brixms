//! Immutable revision records schema `brix.world.revision@1` (ADR-0046 §3.6, P3).

use brix_canon::{CanonWriter, Canonical, Digest, Domain};
use serde_json::{json, Value as JsonValue};
use std::collections::BTreeMap;

use super::error::WorldError;
use super::types::WorldKey;

pub const REVISION_SCHEMA: &str = "brix.world.revision@1";
/// `brix.world.revision@2` (ADR-0046 P6, decided 2026-10-04): additive over
/// `@1` — binds `decision_delta_digest`, the digest of the per-revision
/// decision delta body (`network::encode_decision_delta`), persisted
/// durably by `session.rs` before the revision record itself. `@1` records
/// decode unchanged (`decision_delta_digest` reads back as `None`), and the
/// revision digest formula for `@1` records is untouched byte-for-byte —
/// the new field only enters the hash for `@2` records (see
/// `WorldRevision::compute_digest`).
pub const REVISION_SCHEMA_V2: &str = "brix.world.revision@2";
/// `brix.world.revision@3` (R0 Phase 2 groundwork): additive over `@2` — binds
/// an optional `network_state_root`, the digest of the persisted operator
/// network state header. Every `@2` field keeps its `@2` position and encoding;
/// the root is appended last, presence-tagged (`0` absent, `1` + 32 bytes).
/// `@1`/`@2` digests are untouched byte-for-byte.
///
/// Old-reader semantics: a reader built before `@3` rejects `@3` records with
/// `WorldError::InvalidSchema` in `from_json` (hard, typed failure; it never
/// misreads them). `@3` is therefore only written when a root is supplied via
/// [`WorldRevision::with_network_state_root`]; worlds that never supply one
/// stay readable by old binaries.
pub const REVISION_SCHEMA_V3: &str = "brix.world.revision@3";
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
    /// Digest of this revision's decision delta body (`brix.world.revision@2`
    /// only, ADR-0046 P6, decided 2026-10-04); `None` for `@1` records and for
    /// any revision with no attached decision network.
    pub decision_delta_digest: Option<Digest>,
    pub exec_profile_digest: Option<Digest>,
    /// Digest of the persisted network state header (`brix.world.revision@3`
    /// only); always `None` for `@1`/`@2` records.
    pub network_state_root: Option<Digest>,
    pub revision_digest: Digest,
}

impl WorldRevision {
    /// Every field is independently load-bearing for the revision digest
    /// (`compute_digest` hashes them in this exact order); a params struct
    /// would just move the same 14 fields one level of indirection away
    /// without reducing the real arity this constructor has to bind.
    ///
    /// Always writes `brix.world.revision@2`: see [`REVISION_SCHEMA_V2`].
    /// `@1` records only arise by decoding bytes written before this change
    /// (`from_json`), never from this constructor.
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
        decision_delta_digest: Option<Digest>,
    ) -> Self {
        let ts = timestamp.into();
        let ikey = idempotency_key.into();
        let schema = REVISION_SCHEMA_V2.to_string();
        let digest = Self::compute_digest(
            &schema,
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
            &decision_delta_digest,
            &None,
            &None,
        );

        Self {
            schema,
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
            decision_delta_digest,
            exec_profile_digest: None,
            network_state_root: None,
            revision_digest: digest,
        }
    }

    /// True for schemas that persist decision evidence (`@2` and later).
    pub fn has_decision_evidence(&self) -> bool {
        self.schema == REVISION_SCHEMA_V2 || self.schema == REVISION_SCHEMA_V3
    }

    /// Upgrade this record to `brix.world.revision@3`, binding
    /// `network_state_root` into the revision digest. Builder for the R0
    /// Phase 2 commit path; `new` alone keeps producing `@2`.
    pub fn with_network_state_root(mut self, root: Digest) -> Self {
        self.schema = REVISION_SCHEMA_V3.to_string();
        self.network_state_root = Some(root);
        self.revision_digest = self.compute_digest_for_record();
        self
    }

    /// Version-dispatched digest formula. `schema == REVISION_SCHEMA`
    /// (`@1`) reproduces today's byte-for-byte formula exactly, with
    /// `decision_delta_digest` contributing nothing — not even a presence
    /// tag — so every historical `@1` record still verifies. `@2` appends an
    /// explicit presence-tagged `decision_delta_digest` block after `status`.
    #[allow(clippy::too_many_arguments)]
    fn compute_digest(
        schema: &str,
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
        decision_delta_digest: &Option<Digest>,
        exec_profile_digest: &Option<Digest>,
        network_state_root: &Option<Digest>,
    ) -> Digest {
        let mut w = CanonWriter::new();
        w.write_tag(REVISION_TAG);
        w.write_ident(schema);
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
        let v2_plus = schema == REVISION_SCHEMA_V2 || schema == REVISION_SCHEMA_V3;
        if v2_plus {
            match decision_delta_digest {
                None => w.write_uint(0),
                Some(d) => {
                    w.write_uint(1);
                    w.write_bytes(d.as_bytes());
                }
            }
        }
        if v2_plus {
            match exec_profile_digest {
                None => w.write_uint(0),
                Some(d) => {
                    w.write_uint(1);
                    w.write_bytes(d.as_bytes());
                }
            }
        }
        if schema == REVISION_SCHEMA_V3 {
            match network_state_root {
                None => w.write_uint(0),
                Some(d) => {
                    w.write_uint(1);
                    w.write_bytes(d.as_bytes());
                }
            }
        }
        w.digest(Domain::Value)
    }

    /// Bind the producing execution profile into the revision identity.
    pub fn bind_exec_profile(mut self, digest: Option<Digest>) -> Self {
        self.exec_profile_digest = digest;
        self.revision_digest = Self::compute_digest(
            &self.schema,
            self.seq,
            &self.timestamp,
            self.expected_base_revision,
            &self.idempotency_key,
            &self.batch_digest,
            &self.program_digest,
            &self.previous_revision_digest,
            &self.relation_roots,
            &self.relation_cardinalities,
            &self.secondary_index_roots,
            &self.decision_root,
            &self.changed_keys,
            &self.status,
            &self.decision_delta_digest,
            &self.exec_profile_digest,
            &self.network_state_root,
        );
        self
    }

    /// Recomputes the revision digest from the record fields.
    pub fn compute_digest_for_record(&self) -> Digest {
        Self::compute_digest(
            &self.schema,
            self.seq,
            &self.timestamp,
            self.expected_base_revision,
            &self.idempotency_key,
            &self.batch_digest,
            &self.program_digest,
            &self.previous_revision_digest,
            &self.relation_roots,
            &self.relation_cardinalities,
            &self.secondary_index_roots,
            &self.decision_root,
            &self.changed_keys,
            &self.status,
            &self.decision_delta_digest,
            &self.exec_profile_digest,
            &self.network_state_root,
        )
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

        let mut out = json!({
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
            "decision_delta_digest": self.decision_delta_digest.map(|d| d.to_hex()),
            "exec_profile_digest": self.exec_profile_digest.map(|d| d.to_hex()),
            "revision_digest": self.revision_digest.to_hex(),
        });
        // Emitted only for `@3` so `@1`/`@2` JSON stays byte-identical.
        if self.schema == REVISION_SCHEMA_V3 {
            out["network_state_root"] = json!(self.network_state_root.map(|d| d.to_hex()));
        }
        out
    }

    pub fn from_json(v: &JsonValue) -> Result<Self, WorldError> {
        let schema = v
            .get("schema")
            .and_then(|x| x.as_str())
            .ok_or_else(|| WorldError::Json("revision missing 'schema'".to_string()))?;
        if schema != REVISION_SCHEMA && schema != REVISION_SCHEMA_V2 && schema != REVISION_SCHEMA_V3
        {
            return Err(WorldError::InvalidSchema(format!(
                "expected revision schema {REVISION_SCHEMA}, {REVISION_SCHEMA_V2} or {REVISION_SCHEMA_V3}, got {schema}"
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

        let read_optional_digest = |field: &str| -> Result<Option<Digest>, WorldError> {
            match v.get(field) {
                None | Some(JsonValue::Null) => Ok(None),
                Some(JsonValue::String(hex)) => {
                    if hex.len() != 64 || !hex.is_ascii() {
                        return Err(WorldError::Json(format!("invalid {field}")));
                    }
                    let mut bytes = [0u8; 32];
                    for (i, byte) in bytes.iter_mut().enumerate() {
                        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
                            .map_err(|_| WorldError::Json(format!("invalid {field}")))?;
                    }
                    Ok(Some(Digest::from_bytes(bytes)))
                }
                _ => Err(WorldError::Json(format!("invalid {field}"))),
            }
        };
        let decision_delta_digest = read_optional_digest("decision_delta_digest")?;
        let exec_profile_digest = read_optional_digest("exec_profile_digest")?;
        if schema == REVISION_SCHEMA
            && (decision_delta_digest.is_some() || exec_profile_digest.is_some())
        {
            return Err(WorldError::InvalidSchema(
                "v1 revision cannot bind v2 evidence".into(),
            ));
        }

        let network_state_root = read_optional_digest("network_state_root")?;
        if schema != REVISION_SCHEMA_V3 && network_state_root.is_some() {
            return Err(WorldError::InvalidSchema(
                "only revision@3 can bind a network state root".into(),
            ));
        }

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
            schema,
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
            &decision_delta_digest,
            &exec_profile_digest,
            &network_state_root,
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
            decision_delta_digest,
            exec_profile_digest,
            network_state_root,
            revision_digest,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::type_complexity)]
    fn sample_fields() -> (
        BTreeMap<String, Digest>,
        BTreeMap<String, usize>,
        BTreeMap<String, Digest>,
        BTreeMap<String, Vec<WorldKey>>,
    ) {
        let mut relation_roots = BTreeMap::new();
        relation_roots.insert(
            "orders".to_string(),
            Digest::of(Domain::Value, b"orders-root"),
        );
        let mut relation_cardinalities = BTreeMap::new();
        relation_cardinalities.insert("orders".to_string(), 3usize);
        let secondary_index_roots = BTreeMap::new();
        let mut changed_keys = BTreeMap::new();
        changed_keys.insert("orders".to_string(), vec![WorldKey::from_str("k1")]);
        (
            relation_roots,
            relation_cardinalities,
            secondary_index_roots,
            changed_keys,
        )
    }

    /// A fresh `WorldRevision::new` always writes `@2` and round-trips its
    /// `decision_delta_digest` through JSON unchanged.
    #[test]
    fn v2_round_trips_through_json_with_decision_delta_digest() {
        let (relation_roots, relation_cardinalities, secondary_index_roots, changed_keys) =
            sample_fields();
        let delta_digest = Digest::of(Domain::Value, b"some-decision-delta");
        let rev = WorldRevision::new(
            1,
            "2026-10-04T00:00:00Z",
            0,
            "idem-1",
            None,
            None,
            None,
            relation_roots,
            relation_cardinalities,
            secondary_index_roots,
            Some(Digest::of(Domain::Value, b"decision-root")),
            changed_keys,
            SettlementStatus::Committed,
            Some(delta_digest),
        );
        assert_eq!(rev.schema, REVISION_SCHEMA_V2);
        assert_eq!(rev.decision_delta_digest, Some(delta_digest));
        let json = rev.to_json();
        let decoded = WorldRevision::from_json(&json).expect("v2 round-trip must decode");
        assert_eq!(decoded, rev);
    }

    /// A `brix.world.revision@1` record written before `decision_delta_digest`
    /// existed must still decode, with the field reading back as `None`, and
    /// its digest must match the pre-existing `@1` formula byte-for-byte —
    /// schema `@1` skips the new field entirely, not even a presence tag, so
    /// no historical `@1` record is invalidated by this change.
    #[test]
    fn v1_record_without_the_field_still_decodes_with_unaffected_digest() {
        let (relation_roots, relation_cardinalities, secondary_index_roots, changed_keys) =
            sample_fields();
        let status = SettlementStatus::Committed;
        let decision_root_digest = Digest::of(Domain::Value, b"decision-root");
        let decision_root = Some(decision_root_digest);
        let old_digest = WorldRevision::compute_digest(
            REVISION_SCHEMA,
            1,
            "2026-10-04T00:00:00Z",
            0,
            "idem-1",
            &None,
            &None,
            &None,
            &relation_roots,
            &relation_cardinalities,
            &secondary_index_roots,
            &decision_root,
            &changed_keys,
            &status,
            &None,
            &None,
            &None,
        );
        let v1_json = json!({
            "schema": REVISION_SCHEMA,
            "seq": 1u64,
            "timestamp": "2026-10-04T00:00:00Z",
            "expected_base_revision": 0u64,
            "idempotency_key": "idem-1",
            "batch_digest": JsonValue::Null,
            "program_digest": JsonValue::Null,
            "previous_revision_digest": JsonValue::Null,
            "relation_roots": { "orders": relation_roots["orders"].to_hex() },
            "relation_cardinalities": { "orders": 3u64 },
            "secondary_index_roots": {},
            "decision_root": decision_root_digest.to_hex(),
            "changed_keys": { "orders": [ changed_keys["orders"][0].to_hex() ] },
            "status": status.to_json(),
            "revision_digest": old_digest.to_hex(),
        });
        // Deliberately no "decision_delta_digest" key at all: that is exactly
        // the shape of every `@1` record on disk before this change.
        let decoded = WorldRevision::from_json(&v1_json).expect("v1 record must still decode");
        assert_eq!(decoded.schema, REVISION_SCHEMA);
        assert_eq!(decoded.decision_delta_digest, None);
        assert_eq!(decoded.revision_digest, old_digest);
    }

    fn golden_rev(v1: bool) -> WorldRevision {
        let (rr, rc, sr, ck) = sample_fields();
        let mut r = WorldRevision::new(
            1,
            "2026-10-04T00:00:00Z",
            0,
            "idem-1",
            None,
            None,
            None,
            rr,
            rc,
            sr,
            Some(Digest::of(Domain::Value, b"decision-root")),
            ck,
            SettlementStatus::Committed,
            Some(Digest::of(Domain::Value, b"some-decision-delta")),
        )
        .bind_exec_profile(Some(Digest::of(Domain::Value, b"profile")));
        if v1 {
            r.schema = REVISION_SCHEMA.to_string();
            r.decision_delta_digest = None;
            r.exec_profile_digest = None;
            r.revision_digest = r.compute_digest_for_record();
        }
        r
    }

    /// Golden digests captured on the pre-@3 formula: @1/@2 must never move.
    #[test]
    fn v1_v2_golden_digests_unchanged() {
        assert_eq!(golden_rev(true).revision_digest.to_hex(), GOLDEN_V1);
        assert_eq!(golden_rev(false).revision_digest.to_hex(), GOLDEN_V2);
    }

    #[test]
    fn v3_round_trips_and_binds_root() {
        let root = Digest::of(Domain::Value, b"net-root");
        let v2 = golden_rev(false);
        let v3 = v2.clone().with_network_state_root(root);
        assert_eq!(v3.schema, REVISION_SCHEMA_V3);
        assert_ne!(v3.revision_digest, v2.revision_digest);
        let decoded = WorldRevision::from_json(&v3.to_json()).unwrap();
        assert_eq!(decoded, v3);
        assert_eq!(decoded.network_state_root, Some(root));
        assert!(decoded.has_decision_evidence());
        // @2 JSON carries no network_state_root key at all.
        assert!(v2.to_json().get("network_state_root").is_none());
    }

    #[test]
    fn v3_root_tamper_is_detected() {
        let v3 = golden_rev(false).with_network_state_root(Digest::of(Domain::Value, b"a"));
        let other = golden_rev(false).with_network_state_root(Digest::of(Domain::Value, b"b"));
        assert_ne!(v3.revision_digest, other.revision_digest);
        let mut j = v3.to_json();
        j["network_state_root"] = json!(Digest::of(Domain::Value, b"b").to_hex());
        assert!(matches!(
            WorldRevision::from_json(&j),
            Err(WorldError::CorruptedRevision { .. })
        ));
    }

    /// Presence tag is unambiguous: absent root differs from any present root,
    /// and a @3 record without a root differs from the @2 record.
    #[test]
    fn v3_presence_tag_unambiguous() {
        let mut none3 = golden_rev(false);
        none3.schema = REVISION_SCHEMA_V3.to_string();
        none3.revision_digest = none3.compute_digest_for_record();
        assert_ne!(none3.revision_digest, golden_rev(false).revision_digest);
        let zero = golden_rev(false).with_network_state_root(Digest::from_bytes([0; 32]));
        assert_ne!(none3.revision_digest, zero.revision_digest);
        let rt = WorldRevision::from_json(&none3.to_json()).unwrap();
        assert_eq!(rt.network_state_root, None);
    }

    #[test]
    fn v1_v2_reject_network_state_root() {
        for v1 in [true, false] {
            let mut j = golden_rev(v1).to_json();
            j["network_state_root"] = json!(Digest::of(Domain::Value, b"x").to_hex());
            assert!(matches!(
                WorldRevision::from_json(&j),
                Err(WorldError::InvalidSchema(_))
            ));
        }
    }

    /// Old-reader semantics: a pre-@3 reader accepted only @1/@2 and returned
    /// `InvalidSchema` for anything else; an unknown future schema still does.
    #[test]
    fn unknown_schema_rejected() {
        let mut j = golden_rev(false).to_json();
        j["schema"] = json!("brix.world.revision@4");
        assert!(matches!(
            WorldRevision::from_json(&j),
            Err(WorldError::InvalidSchema(_))
        ));
    }

    const GOLDEN_V1: &str = "361d5946d2349dfd0227ef211f502bcccfe29a97bb6f3d4029408be3924beae4";
    const GOLDEN_V2: &str = "55b862acc82ee0a8b1f22c2a91062e86fcfa8f2cbd0716d598b3128de30170f6";
}
