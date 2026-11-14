//! Immutable host authorization for finite-decision request evidence.
//!
//! Evidence bytes and expected pins are separate inputs. Only a host-created
//! [`RequestEvidencePolicy`] can produce a checked V2 request context; the
//! legacy environment path remains explicitly V1/unscoped.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use brix_canon::{CanonWriter, Digest, Domain};
use brix_lower::finite_decision::{
    finite_decision_program_id, finite_decision_program_preimage, FiniteDecisionPlan,
    FINITE_DECISION_PROFILE,
};
use brix_semantic::{
    encode_context_model_v2, validate_context_model_v2, ArtifactRefV2, AssumptionValidatorV2,
    CheckedContextV2, ContextDimensionsV2, ContextId, ContextModelV2, ContextResolverV2,
    ModelLimits, ScenarioModeV2,
};
use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{json, Map, Value};

pub const REQUEST_EVIDENCE_POLICY_SCHEMA: &str = "brix.request-evidence-policy@1";
pub const REQUEST_EVIDENCE_PROGRAM_ARTIFACT_V1: &str = "brix.l3.finite-decision.program@1";
pub const REQUEST_EVIDENCE_VERIFIER_V1: &str = "brix.world.decision-evidence-replay@1";
pub const MAX_REQUEST_EVIDENCE_POLICY_BYTES: usize = 65_536;

/// Parse evidence envelopes while rejecting duplicate keys at every depth.
/// Unlike policy JSON, evidence artifacts may be larger than the host-config
/// parser's 256 KiB transport bound; the caller supplies the physical limit.
pub fn parse_unique_evidence_json(bytes: &[u8]) -> Result<Value, String> {
    let text = std::str::from_utf8(bytes)
        .map_err(|error| format!("evidence JSON is not UTF-8: {error}"))?;
    serde_json::from_str::<UniqueJson>(text)
        .map(|value| value.0)
        .map_err(|error| format!("invalid evidence JSON: {error}"))
}

struct UniqueJson(Value);
impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UniqueVisitor;
        impl<'de> Visitor<'de> for UniqueVisitor {
            type Value = UniqueJson;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("JSON without duplicate object keys")
            }
            fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Bool(value)))
            }
            fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Number(value.into())))
            }
            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Number(value.into())))
            }
            fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Self::Value, E> {
                serde_json::Number::from_f64(value)
                    .map(|number| UniqueJson(Value::Number(number)))
                    .ok_or_else(|| E::custom("non-finite number"))
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::String(value.to_owned())))
            }
            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::String(value)))
            }
            fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Null))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = sequence.next_element::<UniqueJson>()? {
                    values.push(value.0);
                }
                Ok(UniqueJson(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut values = Map::new();
                while let Some((key, value)) = map.next_entry::<String, UniqueJson>()? {
                    if values.contains_key(&key) {
                        return Err(serde::de::Error::custom(format!(
                            "duplicate object key '{key}'"
                        )));
                    }
                    values.insert(key, value.0);
                }
                Ok(UniqueJson(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(UniqueVisitor)
    }
}

const HARD_MAX_INPUT_FILES: usize = 16;
const HARD_MAX_INPUT_FILE_BYTES: usize = 1_048_576;
const HARD_MAX_INPUT_AGGREGATE_BYTES: usize = 4_194_304;
const HARD_MAX_RUNTIME_CLAIMS: usize = 64;
const HARD_MAX_EVIDENCE_ARTIFACT_BYTES: usize = 16_777_216;
const HARD_MAX_EVIDENCE_AGGREGATE_BYTES: usize = 67_108_864;
const HARD_MAX_EVIDENCE_BUNDLE_BYTES: usize = 8_388_608;
const HARD_MAX_EVIDENCE_PROOF_NODES: usize = 512;
const HARD_MAX_EVIDENCE_NODE_BYTES: usize = 65_536;
const HARD_MAX_REPLAY_WORK: u64 = 500_000;

/// Resource ceilings selected by the host for one request. Every value is
/// explicit in the policy and can only narrow the underlying verifier bounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RequestEvidenceLimits {
    max_input_files: usize,
    max_input_file_bytes: usize,
    max_input_aggregate_bytes: usize,
    max_runtime_claims: usize,
    max_evidence_artifact_bytes: usize,
    max_evidence_aggregate_bytes: usize,
    max_evidence_bundle_bytes: usize,
    max_evidence_proof_nodes: usize,
    max_evidence_node_bytes: usize,
    max_replay_work: u64,
    context_bytes: usize,
    context_artifact_bytes: usize,
    context_depth: usize,
}

impl RequestEvidenceLimits {
    pub fn max_input_files(&self) -> usize {
        self.max_input_files
    }
    pub fn max_input_file_bytes(&self) -> usize {
        self.max_input_file_bytes
    }
    pub fn max_input_aggregate_bytes(&self) -> usize {
        self.max_input_aggregate_bytes
    }
    pub fn max_runtime_claims(&self) -> usize {
        self.max_runtime_claims
    }
    pub fn max_evidence_artifact_bytes(&self) -> usize {
        self.max_evidence_artifact_bytes
    }
    pub fn max_evidence_aggregate_bytes(&self) -> usize {
        self.max_evidence_aggregate_bytes
    }
    pub fn max_evidence_bundle_bytes(&self) -> usize {
        self.max_evidence_bundle_bytes
    }
    pub fn max_evidence_proof_nodes(&self) -> usize {
        self.max_evidence_proof_nodes
    }
    pub fn max_evidence_node_bytes(&self) -> usize {
        self.max_evidence_node_bytes
    }
    pub fn max_replay_work(&self) -> u64 {
        self.max_replay_work
    }
    pub fn context_bytes(&self) -> usize {
        self.context_bytes
    }
    pub fn context_artifact_bytes(&self) -> usize {
        self.context_artifact_bytes
    }
    pub fn context_depth(&self) -> usize {
        self.context_depth
    }

    fn model_limits(self) -> ModelLimits {
        ModelLimits {
            bytes: self.context_bytes,
            assumptions: 0,
            depth: self.context_depth,
            artifact_bytes: self.context_artifact_bytes,
        }
    }
}

/// Strict host-owned policy. The policy itself is not a checked context until
/// it is authorized against the exact lowered finite-decision program.
#[derive(Clone, Debug)]
pub struct RequestEvidencePolicy {
    expected_program: Digest,
    runtime_claim_ids: BTreeSet<Digest>,
    documentary_digest: Option<Digest>,
    verifier: String,
    profile: String,
    namespace: String,
    tenant: String,
    principal: String,
    limits: RequestEvidenceLimits,
}

impl RequestEvidencePolicy {
    pub fn load(path: &Path) -> Result<Self, String> {
        let mut bytes = Vec::new();
        File::open(path)
            .and_then(|file| {
                file.take((MAX_REQUEST_EVIDENCE_POLICY_BYTES + 1) as u64)
                    .read_to_end(&mut bytes)
            })
            .map_err(|error| format!("failed to read request evidence policy: {error}"))?;
        if bytes.len() > MAX_REQUEST_EVIDENCE_POLICY_BYTES {
            return Err("request evidence policy exceeds byte limit".into());
        }
        let text = std::str::from_utf8(&bytes)
            .map_err(|error| format!("request evidence policy is not UTF-8: {error}"))?;
        let value = crate::host_context::parse_unique_json(text)?;
        Self::from_json(&value)
    }

    /// Parse a trusted-host policy object. Unknown keys, missing keys, bad
    /// pins, duplicate runtime IDs, and widened bounds fail closed.
    pub fn from_json(value: &Value) -> Result<Self, String> {
        let root = exact_object(
            value,
            "request evidence policy",
            &[
                "schema",
                "expected_program",
                "runtime_claim_ids",
                "documentary_digest",
                "verifier",
                "profile",
                "namespace",
                "tenant",
                "principal",
                "limits",
            ],
        )?;
        if string(root, "schema")? != REQUEST_EVIDENCE_POLICY_SCHEMA {
            return Err("unsupported request evidence policy schema".into());
        }
        let expected_program = digest(string(root, "expected_program")?, "expected_program")?;
        let mut runtime_claim_ids = BTreeSet::new();
        let claims = root["runtime_claim_ids"]
            .as_array()
            .ok_or("runtime_claim_ids must be an array")?;
        if claims.len() > HARD_MAX_RUNTIME_CLAIMS {
            return Err("runtime_claim_ids exceeds hard maximum".into());
        }
        for claim in claims {
            let id = digest(
                claim.as_str().ok_or("runtime claim ID must be a string")?,
                "runtime claim ID",
            )?;
            if !runtime_claim_ids.insert(id) {
                return Err("runtime_claim_ids contains a duplicate".into());
            }
        }
        let documentary_digest = match &root["documentary_digest"] {
            Value::Null => None,
            Value::String(hex) => Some(digest(hex, "documentary_digest")?),
            _ => return Err("documentary_digest must be null or a digest string".into()),
        };
        let policy = Self {
            expected_program,
            runtime_claim_ids,
            documentary_digest,
            verifier: string(root, "verifier")?.to_owned(),
            profile: string(root, "profile")?.to_owned(),
            namespace: string(root, "namespace")?.to_owned(),
            tenant: string(root, "tenant")?.to_owned(),
            principal: string(root, "principal")?.to_owned(),
            limits: parse_limits(&root["limits"])?,
        };
        if policy.verifier != REQUEST_EVIDENCE_VERIFIER_V1 {
            return Err("unsupported request evidence verifier profile".into());
        }
        if policy.profile != FINITE_DECISION_PROFILE {
            return Err("unsupported request evidence profile".into());
        }
        if policy.namespace.is_empty() || policy.tenant.is_empty() || policy.principal.is_empty() {
            return Err("namespace, tenant, and principal must not be empty".into());
        }
        if policy.runtime_claim_ids.len() > policy.limits.max_runtime_claims {
            return Err("runtime claim pins exceed max_runtime_claims".into());
        }
        Ok(policy)
    }

    pub fn expected_program(&self) -> Digest {
        self.expected_program
    }
    pub fn runtime_claim_ids(&self) -> &BTreeSet<Digest> {
        &self.runtime_claim_ids
    }
    pub fn documentary_digest(&self) -> Option<Digest> {
        self.documentary_digest
    }
    pub fn limits(&self) -> RequestEvidenceLimits {
        self.limits
    }

    pub fn to_json(&self) -> Value {
        json!({
            "schema": REQUEST_EVIDENCE_POLICY_SCHEMA,
            "expected_program": self.expected_program.to_hex(),
            "runtime_claim_ids": self.runtime_claim_ids.iter().map(Digest::to_hex).collect::<Vec<_>>(),
            "documentary_digest": self.documentary_digest.map(|digest| digest.to_hex()),
            "verifier": self.verifier,
            "profile": self.profile,
            "namespace": self.namespace,
            "tenant": self.tenant,
            "principal": self.principal,
            "limits": limits_json(self.limits),
        })
    }

    pub fn authorize_program(
        &self,
        plan: &FiniteDecisionPlan,
    ) -> Result<RequestEvidenceContext, String> {
        if plan.profile != FINITE_DECISION_PROFILE || self.profile != plan.profile {
            return Err(
                "request evidence policy is not authorized for this program profile".into(),
            );
        }
        let program_id = finite_decision_program_id(plan).digest();
        if program_id != self.expected_program {
            return Err(
                "request evidence expected_program does not match the lowered program".into(),
            );
        }
        let policy_digest = policy_digest(self);
        let dimensions = ContextDimensionsV2 {
            parent: None,
            program: Some(ArtifactRefV2 {
                kind: REQUEST_EVIDENCE_PROGRAM_ARTIFACT_V1.into(),
                digest: program_id,
            }),
            world: None,
            namespace: self.namespace.clone(),
            tenant: self.tenant.clone(),
            principal: self.principal.clone(),
            capability_view: format!(
                "{}:{}",
                REQUEST_EVIDENCE_POLICY_SCHEMA,
                policy_digest.to_hex()
            ),
            scenario_mode: ScenarioModeV2::Production,
            branch: None,
            valid_time_start: 0,
            valid_time_end: u64::MAX,
            transaction_time: None,
            correction_time: None,
            simulation_time: None,
            observation_profile: "none@1".into(),
            redaction_profile: "none@1".into(),
            kernel_profile: "none".into(),
            regime_profile: "none".into(),
            axiom_profile: "none".into(),
            validity_start: 0,
            validity_end: u64::MAX,
            step_budget: self.limits.max_replay_work,
            depth_budget: self.limits.context_depth as u64,
        };
        let model = ContextModelV2::new(dimensions.clone(), Vec::new())
            .map_err(|error| format!("request evidence context is invalid: {error:?}"))?;
        let canonical_bytes = encode_context_model_v2(&model);
        let context_id = model.id();
        let checked = validate_context_model_v2(
            &canonical_bytes,
            context_id,
            &dimensions,
            self.limits.model_limits(),
            &ProgramResolver {
                digest: program_id,
                preimage: finite_decision_program_preimage(plan),
            },
            &NoAssumptions,
        )
        .map_err(|error| format!("request evidence V2 context validation failed: {error:?}"))?;
        Ok(RequestEvidenceContext::Host(Box::new(
            HostRequestEvidenceContext {
                policy: self.clone(),
                checked,
                canonical_bytes,
                context_id,
            },
        )))
    }
}

/// A request's admitted evidence context. Legacy CLI pins remain a distinct
/// variant and never acquire a synthetic V2 identity.
#[derive(Clone, Debug)]
pub enum RequestEvidenceContext {
    Host(Box<HostRequestEvidenceContext>),
    Legacy(LegacyEvidencePins),
    Empty,
}

#[derive(Clone, Debug)]
pub struct HostRequestEvidenceContext {
    policy: RequestEvidencePolicy,
    checked: CheckedContextV2,
    canonical_bytes: Vec<u8>,
    context_id: ContextId,
}

#[derive(Clone, Debug, Default)]
pub struct LegacyEvidencePins {
    runtime_claim_ids: BTreeSet<Digest>,
    documentary_digest: Option<Digest>,
}

impl RequestEvidenceContext {
    pub fn empty() -> Self {
        Self::Empty
    }
    pub fn id(&self) -> Option<ContextId> {
        match self {
            Self::Host(context) => Some(context.context_id),
            _ => None,
        }
    }
    pub fn canonical_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Host(context) => Some(&context.canonical_bytes),
            _ => None,
        }
    }
    pub fn is_host(&self) -> bool {
        matches!(self, Self::Host(_))
    }
    pub fn checked_context(&self) -> Option<&CheckedContextV2> {
        match self {
            Self::Host(context) => Some(&context.checked),
            _ => None,
        }
    }
    pub fn policy_json(&self) -> Option<Value> {
        match self {
            Self::Host(context) => Some(context.policy.to_json()),
            _ => None,
        }
    }
    pub fn runtime_claim_ids(&self) -> &BTreeSet<Digest> {
        match self {
            Self::Host(context) => context.policy.runtime_claim_ids(),
            Self::Legacy(pins) => &pins.runtime_claim_ids,
            Self::Empty => empty_digests(),
        }
    }
    pub fn documentary_digest(&self) -> Option<Digest> {
        match self {
            Self::Host(context) => context.policy.documentary_digest(),
            Self::Legacy(pins) => pins.documentary_digest,
            Self::Empty => None,
        }
    }
    pub fn read_limits(&self) -> RequestEvidenceLimits {
        match self {
            Self::Host(context) => context.policy.limits(),
            Self::Legacy(_) | Self::Empty => RequestEvidenceLimits::compatibility(),
        }
    }
    pub fn requires_exact_runtime_pins(&self) -> bool {
        matches!(self, Self::Host(_))
    }
    pub fn requires_documentary_snapshot(&self) -> bool {
        matches!(self, Self::Host(_)) && self.documentary_digest().is_some()
    }
}

impl RequestEvidenceLimits {
    fn compatibility() -> Self {
        Self {
            max_input_files: HARD_MAX_INPUT_FILES,
            max_input_file_bytes: HARD_MAX_INPUT_FILE_BYTES,
            max_input_aggregate_bytes: HARD_MAX_INPUT_AGGREGATE_BYTES,
            max_runtime_claims: HARD_MAX_RUNTIME_CLAIMS,
            max_evidence_artifact_bytes: HARD_MAX_EVIDENCE_ARTIFACT_BYTES,
            max_evidence_aggregate_bytes: HARD_MAX_EVIDENCE_AGGREGATE_BYTES,
            max_evidence_bundle_bytes: HARD_MAX_EVIDENCE_BUNDLE_BYTES,
            max_evidence_proof_nodes: HARD_MAX_EVIDENCE_PROOF_NODES,
            max_evidence_node_bytes: HARD_MAX_EVIDENCE_NODE_BYTES,
            max_replay_work: HARD_MAX_REPLAY_WORK,
            context_bytes: 65_536,
            context_artifact_bytes: 65_536,
            context_depth: 128,
        }
    }
}

/// Invocation-level source of expected evidence pins. The legacy variant is
/// only available to standalone CLI code that deliberately snapshots env.
#[derive(Clone, Debug)]
pub enum InvocationEvidenceContext {
    HostPolicy(Box<RequestEvidencePolicy>),
    LegacyCliEnvironment(LegacyEvidencePins),
    Empty,
}

impl InvocationEvidenceContext {
    pub fn empty() -> Self {
        Self::Empty
    }
    pub fn from_host_policy_json(value: &Value) -> Result<Self, String> {
        Ok(Self::HostPolicy(Box::new(
            RequestEvidencePolicy::from_json(value)?,
        )))
    }
    pub fn from_environment() -> Result<Self, String> {
        let mut runtime_claim_ids = BTreeSet::new();
        if let Ok(value) = std::env::var("BRIX_EXPECT_RUNTIME_CLAIMS") {
            for hex in value.split(',').filter(|part| !part.is_empty()) {
                if !runtime_claim_ids.insert(digest(hex, "BRIX_EXPECT_RUNTIME_CLAIMS")?) {
                    return Err("BRIX_EXPECT_RUNTIME_CLAIMS contains a duplicate digest".into());
                }
            }
        }
        let documentary_digest = match std::env::var("BRIX_EXPECT_EVIDENCE_DIGEST") {
            Ok(value) if !value.is_empty() => Some(digest(&value, "BRIX_EXPECT_EVIDENCE_DIGEST")?),
            _ => None,
        };
        Ok(Self::LegacyCliEnvironment(LegacyEvidencePins {
            runtime_claim_ids,
            documentary_digest,
        }))
    }
    pub fn authorize_program(
        &self,
        plan: &FiniteDecisionPlan,
    ) -> Result<RequestEvidenceContext, String> {
        match self {
            Self::HostPolicy(policy) => policy.authorize_program(plan),
            Self::LegacyCliEnvironment(pins) => Ok(RequestEvidenceContext::Legacy(pins.clone())),
            Self::Empty => Ok(RequestEvidenceContext::Empty),
        }
    }
}

fn empty_digests() -> &'static BTreeSet<Digest> {
    static EMPTY: std::sync::OnceLock<BTreeSet<Digest>> = std::sync::OnceLock::new();
    EMPTY.get_or_init(BTreeSet::new)
}

struct ProgramResolver {
    digest: Digest,
    preimage: Vec<u8>,
}
impl ContextResolverV2 for ProgramResolver {
    fn context_bytes(&self, _: &ContextId) -> Option<Vec<u8>> {
        None
    }
    fn artifact_bytes(&self, reference: &ArtifactRefV2) -> Option<Vec<u8>> {
        (reference.kind == REQUEST_EVIDENCE_PROGRAM_ARTIFACT_V1 && reference.digest == self.digest)
            .then(|| self.preimage.clone())
    }
}
struct NoAssumptions;
impl AssumptionValidatorV2 for NoAssumptions {
    fn validate(&self, _: &str, _: &[u8], _: &mut usize) -> bool {
        false
    }
}

fn policy_digest(policy: &RequestEvidencePolicy) -> Digest {
    let mut writer = CanonWriter::new();
    writer.write_tag(REQUEST_EVIDENCE_POLICY_SCHEMA);
    writer.write_bytes(policy.expected_program.as_bytes());
    writer.write_uint(policy.runtime_claim_ids.len() as u64);
    for id in &policy.runtime_claim_ids {
        writer.write_bytes(id.as_bytes());
    }
    match policy.documentary_digest {
        Some(digest) => {
            writer.write_enum(1, |writer| writer.write_bytes(digest.as_bytes()));
        }
        None => writer.write_enum(0, |_| {}),
    }
    for value in [
        &policy.verifier,
        &policy.profile,
        &policy.namespace,
        &policy.tenant,
        &policy.principal,
    ] {
        writer.write_str(value);
    }
    write_limits(&mut writer, policy.limits);
    Digest::of(Domain::Value, &writer.finish())
}

fn write_limits(writer: &mut CanonWriter, limits: RequestEvidenceLimits) {
    for value in [
        limits.max_input_files as u64,
        limits.max_input_file_bytes as u64,
        limits.max_input_aggregate_bytes as u64,
        limits.max_runtime_claims as u64,
        limits.max_evidence_artifact_bytes as u64,
        limits.max_evidence_aggregate_bytes as u64,
        limits.max_evidence_bundle_bytes as u64,
        limits.max_evidence_proof_nodes as u64,
        limits.max_evidence_node_bytes as u64,
        limits.max_replay_work,
        limits.context_bytes as u64,
        limits.context_artifact_bytes as u64,
        limits.context_depth as u64,
    ] {
        writer.write_uint(value);
    }
}

fn parse_limits(value: &Value) -> Result<RequestEvidenceLimits, String> {
    let object = exact_object(
        value,
        "request evidence limits",
        &[
            "max_input_files",
            "max_input_file_bytes",
            "max_input_aggregate_bytes",
            "max_runtime_claims",
            "max_evidence_artifact_bytes",
            "max_evidence_aggregate_bytes",
            "max_evidence_bundle_bytes",
            "max_evidence_proof_nodes",
            "max_evidence_node_bytes",
            "max_replay_work",
            "context_bytes",
            "context_artifact_bytes",
            "context_depth",
        ],
    )?;
    let limits = RequestEvidenceLimits {
        max_input_files: usize_limit(object, "max_input_files", 1, HARD_MAX_INPUT_FILES)?,
        max_input_file_bytes: usize_limit(
            object,
            "max_input_file_bytes",
            1,
            HARD_MAX_INPUT_FILE_BYTES,
        )?,
        max_input_aggregate_bytes: usize_limit(
            object,
            "max_input_aggregate_bytes",
            1,
            HARD_MAX_INPUT_AGGREGATE_BYTES,
        )?,
        max_runtime_claims: usize_limit(object, "max_runtime_claims", 0, HARD_MAX_RUNTIME_CLAIMS)?,
        max_evidence_artifact_bytes: usize_limit(
            object,
            "max_evidence_artifact_bytes",
            1,
            HARD_MAX_EVIDENCE_ARTIFACT_BYTES,
        )?,
        max_evidence_aggregate_bytes: usize_limit(
            object,
            "max_evidence_aggregate_bytes",
            1,
            HARD_MAX_EVIDENCE_AGGREGATE_BYTES,
        )?,
        max_evidence_bundle_bytes: usize_limit(
            object,
            "max_evidence_bundle_bytes",
            1,
            HARD_MAX_EVIDENCE_BUNDLE_BYTES,
        )?,
        max_evidence_proof_nodes: usize_limit(
            object,
            "max_evidence_proof_nodes",
            1,
            HARD_MAX_EVIDENCE_PROOF_NODES,
        )?,
        max_evidence_node_bytes: usize_limit(
            object,
            "max_evidence_node_bytes",
            1,
            HARD_MAX_EVIDENCE_NODE_BYTES,
        )?,
        max_replay_work: uint_limit(object, "max_replay_work", 1, HARD_MAX_REPLAY_WORK)?,
        context_bytes: usize_limit(object, "context_bytes", 1, 65_536)?,
        context_artifact_bytes: usize_limit(object, "context_artifact_bytes", 1, 65_536)?,
        context_depth: usize_limit(object, "context_depth", 1, 128)?,
    };
    Ok(limits)
}

fn limits_json(limits: RequestEvidenceLimits) -> Value {
    json!({
        "max_input_files": limits.max_input_files,
        "max_input_file_bytes": limits.max_input_file_bytes,
        "max_input_aggregate_bytes": limits.max_input_aggregate_bytes,
        "max_runtime_claims": limits.max_runtime_claims,
        "max_evidence_artifact_bytes": limits.max_evidence_artifact_bytes,
        "max_evidence_aggregate_bytes": limits.max_evidence_aggregate_bytes,
        "max_evidence_bundle_bytes": limits.max_evidence_bundle_bytes,
        "max_evidence_proof_nodes": limits.max_evidence_proof_nodes,
        "max_evidence_node_bytes": limits.max_evidence_node_bytes,
        "max_replay_work": limits.max_replay_work,
        "context_bytes": limits.context_bytes,
        "context_artifact_bytes": limits.context_artifact_bytes,
        "context_depth": limits.context_depth,
    })
}

fn exact_object<'a>(
    value: &'a Value,
    label: &str,
    keys: &[&str],
) -> Result<&'a Map<String, Value>, String> {
    let object = value
        .as_object()
        .ok_or_else(|| format!("{label} must be an object"))?;
    if object.len() != keys.len() || object.keys().any(|key| !keys.contains(&key.as_str())) {
        return Err(format!("{label} has missing or unknown fields"));
    }
    Ok(object)
}
fn string<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a str, String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{key} must be a string"))
}
fn usize_limit(
    object: &Map<String, Value>,
    key: &str,
    min: usize,
    max: usize,
) -> Result<usize, String> {
    let value = object
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{key} must be an unsigned integer"))?;
    let value = usize::try_from(value).map_err(|_| format!("{key} is too large"))?;
    if !(min..=max).contains(&value) {
        return Err(format!("{key} is outside its supported range"));
    }
    Ok(value)
}
fn uint_limit(object: &Map<String, Value>, key: &str, min: u64, max: u64) -> Result<u64, String> {
    let value = object
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{key} must be an unsigned integer"))?;
    if !(min..=max).contains(&value) {
        return Err(format!("{key} is outside its supported range"));
    }
    Ok(value)
}
fn digest(value: &str, name: &str) -> Result<Digest, String> {
    if value.len() != 64 || !value.is_ascii() {
        return Err(format!("{name} must be a 64-character hex digest"));
    }
    let mut bytes = [0; 32];
    for (i, byte) in bytes.iter_mut().enumerate() {
        let hi = (value.as_bytes()[2 * i] as char)
            .to_digit(16)
            .ok_or_else(|| format!("{name} has invalid hex"))?;
        let lo = (value.as_bytes()[2 * i + 1] as char)
            .to_digit(16)
            .ok_or_else(|| format!("{name} has invalid hex"))?;
        *byte = ((hi << 4) | lo) as u8;
    }
    Ok(Digest::from_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use brix_lower::finite_decision::{lower_finite_decision_plan, FINITE_DECISION_PROFILE};

    fn plan() -> FiniteDecisionPlan {
        let module = brix_syntax::parse(
            r#"
config Decision = Ship | Hold
rule r() = only(["a"])
propose p(r) priority 10 when r == "a" = Ship
commit c from (p)
"#,
        )
        .unwrap();
        lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap()
    }

    fn policy_json(program: Digest) -> Value {
        json!({
            "schema": REQUEST_EVIDENCE_POLICY_SCHEMA,
            "expected_program": program.to_hex(),
            "runtime_claim_ids": [],
            "documentary_digest": null,
            "verifier": REQUEST_EVIDENCE_VERIFIER_V1,
            "profile": FINITE_DECISION_PROFILE,
            "namespace": "test",
            "tenant": "tenant-a",
            "principal": "host",
            "limits": {
                "max_input_files": 16,
                "max_input_file_bytes": 1_048_576,
                "max_input_aggregate_bytes": 4_194_304,
                "max_runtime_claims": 64,
                "max_evidence_artifact_bytes": 16_777_216,
                "max_evidence_aggregate_bytes": 67_108_864,
                "max_evidence_bundle_bytes": 8_388_608,
                "max_evidence_proof_nodes": 512,
                "max_evidence_node_bytes": 65_536,
                "max_replay_work": 500_000,
                "context_bytes": 65_536,
                "context_artifact_bytes": 65_536,
                "context_depth": 128
            }
        })
    }

    #[test]
    fn policy_authorizes_only_the_pinned_plan_and_builds_checked_v2_context() {
        let plan = plan();
        let expected = finite_decision_program_id(&plan).digest();
        let policy = RequestEvidencePolicy::from_json(&policy_json(expected)).unwrap();
        let context = policy.authorize_program(&plan).unwrap();
        assert!(context.is_host());
        assert!(context.id().is_some());
        assert!(context.canonical_bytes().is_some());
        assert_eq!(
            context.checked_context().unwrap().id(),
            context.id().unwrap()
        );

        let wrong =
            RequestEvidencePolicy::from_json(&policy_json(Digest::from_bytes([7; 32]))).unwrap();
        assert!(wrong.authorize_program(&plan).is_err());
    }

    #[test]
    fn policy_schema_is_strict_and_empty_context_has_no_v2_identity() {
        let plan = plan();
        let value = policy_json(finite_decision_program_id(&plan).digest());
        let mut unknown = value.clone();
        unknown["unexpected"] = json!(true);
        assert!(RequestEvidencePolicy::from_json(&unknown).is_err());
        let mut bad_limit = value;
        bad_limit["limits"]["max_replay_work"] = json!(HARD_MAX_REPLAY_WORK + 1);
        assert!(RequestEvidencePolicy::from_json(&bad_limit).is_err());

        let empty = InvocationEvidenceContext::empty()
            .authorize_program(&plan)
            .unwrap();
        assert!(!empty.is_host());
        assert_eq!(empty.id(), None);
        assert_eq!(empty.canonical_bytes(), None);
    }

    #[test]
    fn evidence_json_rejects_duplicate_fields_recursively() {
        assert!(parse_unique_evidence_json(br#"{"schema":"x","schema":"y"}"#).is_err());
        assert!(parse_unique_evidence_json(br#"{"outer":{"x":1,"x":2}}"#).is_err());
    }

    #[test]
    fn narrow_evidence_cap_does_not_reject_a_valid_input_shard() {
        let plan = plan();
        let program = finite_decision_program_id(&plan).digest();
        let mut policy_value = policy_json(program);
        policy_value["limits"]["max_evidence_artifact_bytes"] = json!(1);
        policy_value["limits"]["max_evidence_aggregate_bytes"] = json!(1);
        let context = RequestEvidencePolicy::from_json(&policy_value)
            .unwrap()
            .authorize_program(&plan)
            .unwrap();
        let path = std::env::temp_dir().join(format!(
            "brix_request_evidence_input_{}_{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, br#"{"schema":"brix.input@5","values":{}}"#).unwrap();
        let result = crate::commands::load_cli_input_snapshot_with_context(
            std::slice::from_ref(&path),
            &context,
        );
        let _ = std::fs::remove_file(path);
        assert!(
            result.is_ok(),
            "normal input was charged against evidence cap: {result:?}"
        );
    }
}
