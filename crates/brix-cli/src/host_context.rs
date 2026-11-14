//! Trusted host configuration for authorizing named world queries.
//!
//! This module parses an explicitly supplied host-owned configuration. It does
//! not read environment variables and does not make a wire payload trusted.
//! The caller must establish that the config came from the embedding host,
//! provide a resolver for the selected world/program artifacts, and pass the
//! resulting sealed [`AuthorizedHostScope`] separately from query arguments.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::path::Path;

use brix_canon::Digest;
use brix_kb::world::scoped_query::{
    world_revision_artifact_digest_v1, world_revision_artifact_preimage_v1, AuthorizedHostScope,
    HostContextExpectationV2, HostQueryLimitsV1, HostScopeProfileV1,
    HOST_SCOPE_PROFILE_PARTITION_V1, MAX_SCOPED_QUERY_EVAL_MATCHES, MAX_SCOPED_QUERY_EVAL_WORK,
    MAX_SCOPED_QUERY_INPUT_BYTES, MAX_SCOPED_QUERY_INPUT_ROWS, MAX_SCOPED_QUERY_OUTPUT_ROWS,
    MAX_SCOPED_QUERY_PAGE_ROWS, QUERY_PROGRAM_ARTIFACT_V1, QUERY_WORLD_ARTIFACT_V1,
};
use brix_kb::world::session::WorldSession;
use brix_semantic::{
    encode_context_model_v2, ArtifactRefV2, AssumptionValidatorV2, ContextDimensionsV2, ContextId,
    ContextModelV2, ContextResolverV2, ModelLimits, ScenarioModeV2,
};
use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Number, Value};

pub const HOST_QUERY_CONFIG_SCHEMA: &str = "brix.host-query-config@1";
pub const MAX_HOST_QUERY_CONFIG_BYTES: usize = 256 * 1024;

/// Load an explicitly named host-owned config file.
pub fn load_host_query_config(path: &Path) -> Result<HostQueryConfig, String> {
    HostQueryConfig::load_host_query_config(path)
}

/// Load high-level host policy from an explicitly named host-owned file.
pub fn load_host_query_policy(path: &Path) -> Result<HostQueryPolicy, String> {
    let value = read_unique_json_file(path)?;
    HostQueryPolicy::from_json(&value)
}

fn read_unique_json_file(path: &Path) -> Result<Value, String> {
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|f| {
            f.take((MAX_HOST_QUERY_CONFIG_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
        })
        .map_err(|e| format!("failed to read host query config: {e}"))?;
    if bytes.len() > MAX_HOST_QUERY_CONFIG_BYTES {
        return Err("host query config exceeds byte limit".into());
    }
    parse_unique_json(
        std::str::from_utf8(&bytes).map_err(|e| format!("host query config is not UTF-8: {e}"))?,
    )
}

/// Parse bounded JSON while rejecting duplicate object keys at every nesting
/// level. Shared by host policy, scoped-query arguments, and transport decoding.
pub fn parse_unique_json(text: &str) -> Result<Value, String> {
    if text.len() > MAX_HOST_QUERY_CONFIG_BYTES {
        return Err("JSON input exceeds 256 KiB limit".into());
    }
    serde_json::from_str::<UniqueJson>(text)
        .map(|parsed| parsed.0)
        .map_err(|e| format!("invalid JSON: {e}"))
}

/// Immutable host-selected expectation used to create an authorized query
/// scope. Its fields are private so ordinary query arguments cannot mutate the
/// bindings after parsing.
#[derive(Clone, Debug)]
pub struct HostQueryConfig {
    profile: HostScopeProfileV1,
    context_bytes: Vec<u8>,
    context_id: ContextId,
    dimensions: ContextDimensionsV2,
    limits: ModelLimits,
}

impl HostQueryConfig {
    /// Load a host-owned JSON config with a hard byte bound and duplicate-key
    /// rejection. The caller is responsible for choosing a trusted path.
    pub fn load_host_query_config(path: &Path) -> Result<Self, String> {
        let value = read_unique_json_file(path)?;
        Self::from_json(&value)
    }

    /// Parse an already decoded config at the trusted-host seam. Unknown fields,
    /// unsupported profiles, malformed bindings, and widened limits are refused.
    pub fn from_json(value: &Value) -> Result<Self, String> {
        let root = object(
            value,
            "host query config",
            &["schema", "profile", "context", "limits", "query_limits"],
        )?;
        if string(required(root, "schema")?, "schema")? != HOST_QUERY_CONFIG_SCHEMA {
            return Err("unsupported host query config schema".into());
        }

        let query_limits = parse_query_limits(required(root, "query_limits")?)?;
        let profile = parse_profile(required(root, "profile")?, query_limits)?;

        let c = object(
            required(root, "context")?,
            "context",
            &["id", "bytes", "dimensions"],
        )?;
        let context_id = ContextId(parse_digest(string(required(c, "id")?, "context.id")?)?);
        let context_bytes = decode_hex(string(required(c, "bytes")?, "context.bytes")?)?;
        if context_bytes.len() > ModelLimits::default().bytes {
            return Err("context bytes exceed V2 model limit".into());
        }
        let dimensions = parse_dimensions(required(c, "dimensions")?)?;
        let expected_capability_view = profile
            .context_capability_view()
            .map_err(|e| format!("invalid host-scope profile: {e:?}"))?;
        if dimensions.capability_view != expected_capability_view {
            return Err(
                "context capability_view does not bind the configured host-scope profile".into(),
            );
        }
        let limits = parse_limits(required(root, "limits")?)?;

        Ok(Self {
            profile,
            context_bytes,
            context_id,
            dimensions,
            limits,
        })
    }

    /// Validate the context against independent host expectations and the
    /// caller-provided artifact store. The resolver and assumption validator
    /// are explicit so this layer does not duplicate artifact encodings.
    pub fn authorize<R: ContextResolverV2, A: AssumptionValidatorV2>(
        &self,
        resolver: &R,
        assumptions: &A,
    ) -> Result<AuthorizedHostScope, String> {
        AuthorizedHostScope::authorize(
            self.profile.clone(),
            HostContextExpectationV2 {
                bytes: &self.context_bytes,
                id: self.context_id,
                dimensions: &self.dimensions,
                limits: self.limits,
            },
            resolver,
            assumptions,
        )
        .map_err(|e| format!("host query context rejected: {e:?}"))
    }

    /// Build and admit a canonical request context from trusted host policy,
    /// bound to one actual pinned revision of `session`. Resolve `revision_seq`
    /// before calling this method if it came from a cursor; HEAD is never read
    /// again inside the query operation.
    pub fn from_host_policy_json(
        value: &Value,
        session: &WorldSession,
        revision_seq: u64,
    ) -> Result<AuthorizedHostScope, String> {
        HostQueryPolicy::from_json(value)?.authorize(session, revision_seq)
    }

    pub fn expected_context_id(&self) -> ContextId {
        self.context_id
    }

    pub fn dimensions(&self) -> &ContextDimensionsV2 {
        &self.dimensions
    }
}

/// High-level policy supplied by the trusted embedding host. Revision and
/// linked-program identities are intentionally absent: the builder takes them
/// from the actual pinned session, never from policy JSON or query parameters.
#[derive(Clone, Debug)]
pub struct HostQueryPolicy {
    profile: HostScopeProfileV1,
    namespace: String,
    tenant: String,
    principal: String,
    valid_time_start: u64,
    valid_time_end: u64,
    validity_start: u64,
    validity_end: u64,
    step_budget: u64,
    depth_budget: u64,
    limits: ModelLimits,
}

impl HostQueryPolicy {
    /// Parse a strict host-owned policy value. Supply this separately from
    /// named-query arguments at the trusted embedding boundary.
    pub fn from_json(value: &Value) -> Result<Self, String> {
        let root = object(
            value,
            "host query policy",
            &[
                "schema",
                "profile",
                "namespace",
                "tenant",
                "principal",
                "valid_time_start",
                "valid_time_end",
                "validity_start",
                "validity_end",
                "step_budget",
                "depth_budget",
                "limits",
                "query_limits",
            ],
        )?;
        if string(required(root, "schema")?, "schema")? != "brix.host-query-policy@1" {
            return Err("unsupported host query policy schema".into());
        }
        let query_limits = parse_query_limits(required(root, "query_limits")?)?;
        let profile = parse_profile(required(root, "profile")?, query_limits)?;
        let policy = Self {
            profile,
            namespace: string(required(root, "namespace")?, "namespace")?.to_owned(),
            tenant: string(required(root, "tenant")?, "tenant")?.to_owned(),
            principal: string(required(root, "principal")?, "principal")?.to_owned(),
            valid_time_start: uint(root, "valid_time_start")?,
            valid_time_end: uint(root, "valid_time_end")?,
            validity_start: uint(root, "validity_start")?,
            validity_end: uint(root, "validity_end")?,
            step_budget: uint(root, "step_budget")?,
            depth_budget: uint(root, "depth_budget")?,
            limits: parse_limits(required(root, "limits")?)?,
        };
        if policy.namespace.is_empty() || policy.tenant.is_empty() || policy.principal.is_empty() {
            return Err("namespace, tenant, and principal must not be empty".into());
        }
        Ok(policy)
    }

    pub fn query_limits(&self) -> HostQueryLimitsV1 {
        self.profile.query_limits
    }

    /// Build and admit a canonical request context from trusted host policy,
    /// bound to one actual pinned revision of `session`. Resolve `revision_seq`
    /// before calling this method if it came from a cursor; HEAD is never read
    /// again inside the query operation.
    pub fn authorize(
        &self,
        session: &WorldSession,
        revision_seq: u64,
    ) -> Result<AuthorizedHostScope, String> {
        let snapshot = session
            .pin_revision(revision_seq)
            .map_err(|e| format!("host query revision is unavailable: {e}"))?;
        let world_id = snapshot.world_id.clone();
        let revision_digest = snapshot.revision_digest();
        let program_digest = snapshot
            .revision
            .program_digest
            .ok_or("pinned world revision has no program binding")?;
        if program_digest != session.manifest.program_digest {
            return Err("pinned revision program differs from the available linked program".into());
        }
        let world_artifact_digest = world_revision_artifact_digest_v1(
            &world_id,
            revision_seq,
            revision_digest,
            program_digest,
        );
        let dimensions = ContextDimensionsV2 {
            parent: None,
            program: Some(ArtifactRefV2 {
                kind: QUERY_PROGRAM_ARTIFACT_V1.to_owned(),
                digest: program_digest,
            }),
            world: Some(ArtifactRefV2 {
                kind: QUERY_WORLD_ARTIFACT_V1.to_owned(),
                digest: world_artifact_digest,
            }),
            namespace: self.namespace.clone(),
            tenant: self.tenant.clone(),
            principal: self.principal.clone(),
            capability_view: self
                .profile
                .context_capability_view()
                .map_err(|e| format!("invalid host-scope profile: {e:?}"))?,
            scenario_mode: ScenarioModeV2::Production,
            branch: None,
            valid_time_start: self.valid_time_start,
            valid_time_end: self.valid_time_end,
            transaction_time: None,
            correction_time: None,
            simulation_time: None,
            observation_profile: "none@1".to_owned(),
            redaction_profile: "none@1".to_owned(),
            kernel_profile: "none".to_owned(),
            regime_profile: "none".to_owned(),
            axiom_profile: "none".to_owned(),
            validity_start: self.validity_start,
            validity_end: self.validity_end,
            step_budget: self.step_budget,
            depth_budget: self.depth_budget,
        };
        let model = ContextModelV2::new(dimensions.clone(), Vec::new())
            .map_err(|e| format!("invalid host query context dimensions: {e:?}"))?;
        let context_bytes = encode_context_model_v2(&model);
        let config = HostQueryConfig {
            profile: self.profile.clone(),
            context_bytes,
            context_id: model.id(),
            dimensions,
            limits: self.limits,
        };

        let program_preimage = session
            .program_artifact_preimage()
            .map_err(|e| format!("failed to load linked program artifact: {e}"))?;
        let resolver = SessionArtifactResolver {
            world_id: &world_id,
            revision_seq,
            revision_digest,
            program_digest,
            program_preimage,
        };
        let authorized = config.authorize(&resolver, &NoTypedAssumptions)?;
        authorized
            .check_execution_bindings(&world_id, revision_seq, revision_digest, program_digest)
            .map_err(|e| format!("host query pin mismatch: {e:?}"))?;
        Ok(authorized)
    }
}

fn parse_profile(
    value: &Value,
    query_limits: HostQueryLimitsV1,
) -> Result<HostScopeProfileV1, String> {
    let p = object(
        value,
        "profile",
        &["profile_id", "partition_fields", "global_relations"],
    )?;
    let profile_id = string(required(p, "profile_id")?, "profile.profile_id")?.to_owned();
    if profile_id != HOST_SCOPE_PROFILE_PARTITION_V1 {
        return Err("unsupported host-scope profile version".into());
    }
    let fields = required(p, "partition_fields")?
        .as_object()
        .ok_or("profile.partition_fields must be an object")?;
    let mut partition_fields = BTreeMap::new();
    for (relation, field) in fields {
        let field = string(field, "partition field")?;
        if relation.is_empty() || field.is_empty() {
            return Err("partition relation and field names must not be empty".into());
        }
        partition_fields.insert(relation.clone(), field.to_owned());
    }
    let profile = HostScopeProfileV1 {
        profile_id,
        partition_fields,
        global_relations: string_set(required(p, "global_relations")?, "profile.global_relations")?,
        query_limits,
    };
    profile
        .validate()
        .map_err(|e| format!("invalid host-scope profile: {e:?}"))?;
    Ok(profile)
}

struct SessionArtifactResolver<'a> {
    world_id: &'a str,
    revision_seq: u64,
    revision_digest: Digest,
    program_digest: Digest,
    program_preimage: Vec<u8>,
}

impl ContextResolverV2 for SessionArtifactResolver<'_> {
    fn context_bytes(&self, _id: &ContextId) -> Option<Vec<u8>> {
        None
    }

    fn artifact_bytes(&self, reference: &ArtifactRefV2) -> Option<Vec<u8>> {
        if reference.kind == QUERY_PROGRAM_ARTIFACT_V1 && reference.digest == self.program_digest {
            return Some(self.program_preimage.clone());
        }
        if reference.kind == QUERY_WORLD_ARTIFACT_V1
            && reference.digest
                == world_revision_artifact_digest_v1(
                    self.world_id,
                    self.revision_seq,
                    self.revision_digest,
                    self.program_digest,
                )
        {
            return Some(world_revision_artifact_preimage_v1(
                self.world_id,
                self.revision_seq,
                self.revision_digest,
                self.program_digest,
            ));
        }
        None
    }
}

struct NoTypedAssumptions;
impl AssumptionValidatorV2 for NoTypedAssumptions {
    fn validate(&self, _schema: &str, _value: &[u8], _budget: &mut usize) -> bool {
        false
    }
}

fn parse_dimensions(value: &Value) -> Result<ContextDimensionsV2, String> {
    let keys = [
        "parent",
        "program",
        "world",
        "namespace",
        "tenant",
        "principal",
        "capability_view",
        "scenario_mode",
        "branch",
        "valid_time_start",
        "valid_time_end",
        "transaction_time",
        "correction_time",
        "simulation_time",
        "observation_profile",
        "redaction_profile",
        "kernel_profile",
        "regime_profile",
        "axiom_profile",
        "validity_start",
        "validity_end",
        "step_budget",
        "depth_budget",
    ];
    let o = object(value, "context.dimensions", &keys)?;
    let mode = match string(required(o, "scenario_mode")?, "scenario_mode")? {
        "production" => ScenarioModeV2::Production,
        "simulation" => ScenarioModeV2::Simulation,
        _ => return Err("scenario_mode must be 'production' or 'simulation'".into()),
    };
    Ok(ContextDimensionsV2 {
        parent: optional_digest_id(o, "parent")?,
        program: optional_artifact(o, "program")?,
        world: optional_artifact(o, "world")?,
        namespace: string(required(o, "namespace")?, "namespace")?.to_owned(),
        tenant: string(required(o, "tenant")?, "tenant")?.to_owned(),
        principal: string(required(o, "principal")?, "principal")?.to_owned(),
        capability_view: string(required(o, "capability_view")?, "capability_view")?.to_owned(),
        scenario_mode: mode,
        branch: optional_digest(o, "branch")?,
        valid_time_start: uint(o, "valid_time_start")?,
        valid_time_end: uint(o, "valid_time_end")?,
        transaction_time: optional_uint(o, "transaction_time")?,
        correction_time: optional_uint(o, "correction_time")?,
        simulation_time: optional_uint(o, "simulation_time")?,
        observation_profile: string(required(o, "observation_profile")?, "observation_profile")?
            .to_owned(),
        redaction_profile: string(required(o, "redaction_profile")?, "redaction_profile")?
            .to_owned(),
        kernel_profile: string(required(o, "kernel_profile")?, "kernel_profile")?.to_owned(),
        regime_profile: string(required(o, "regime_profile")?, "regime_profile")?.to_owned(),
        axiom_profile: string(required(o, "axiom_profile")?, "axiom_profile")?.to_owned(),
        validity_start: uint(o, "validity_start")?,
        validity_end: uint(o, "validity_end")?,
        step_budget: uint(o, "step_budget")?,
        depth_budget: uint(o, "depth_budget")?,
    })
}

fn parse_limits(value: &Value) -> Result<ModelLimits, String> {
    let o = object(
        value,
        "limits",
        &["bytes", "assumptions", "depth", "artifact_bytes"],
    )?;
    let limits = ModelLimits {
        bytes: uint(o, "bytes")?
            .try_into()
            .map_err(|_| "limits.bytes out of range")?,
        assumptions: uint(o, "assumptions")?
            .try_into()
            .map_err(|_| "limits.assumptions out of range")?,
        depth: uint(o, "depth")?
            .try_into()
            .map_err(|_| "limits.depth out of range")?,
        artifact_bytes: uint(o, "artifact_bytes")?
            .try_into()
            .map_err(|_| "limits.artifact_bytes out of range")?,
    };
    let max = ModelLimits::default();
    if limits.bytes == 0
        || limits.bytes > max.bytes
        || limits.assumptions == 0
        || limits.assumptions > max.assumptions
        || limits.depth == 0
        || limits.depth > max.depth
        || limits.artifact_bytes == 0
        || limits.artifact_bytes > max.artifact_bytes
    {
        return Err(
            "host context limits must be positive and may only tighten V2 hard limits".into(),
        );
    }
    Ok(limits)
}

fn parse_query_limits(value: &Value) -> Result<HostQueryLimitsV1, String> {
    let o = object(
        value,
        "query_limits",
        &[
            "page_rows",
            "max_input_rows",
            "max_input_bytes",
            "max_eval_work",
            "max_eval_matches",
            "max_output_rows",
        ],
    )?;
    let as_usize = |key| {
        uint(o, key)?
            .try_into()
            .map_err(|_| format!("query_limits.{key} out of range"))
    };
    let limits = HostQueryLimitsV1 {
        page_rows: as_usize("page_rows")?,
        max_input_rows: as_usize("max_input_rows")?,
        max_input_bytes: as_usize("max_input_bytes")?,
        max_eval_work: uint(o, "max_eval_work")?,
        max_eval_matches: uint(o, "max_eval_matches")?,
        max_output_rows: as_usize("max_output_rows")?,
    };
    if limits.page_rows == 0
        || limits.page_rows > MAX_SCOPED_QUERY_PAGE_ROWS
        || limits.max_input_rows == 0
        || limits.max_input_rows > MAX_SCOPED_QUERY_INPUT_ROWS
        || limits.max_input_bytes == 0
        || limits.max_input_bytes > MAX_SCOPED_QUERY_INPUT_BYTES
        || limits.max_eval_work == 0
        || limits.max_eval_work > MAX_SCOPED_QUERY_EVAL_WORK
        || limits.max_eval_matches == 0
        || limits.max_eval_matches > MAX_SCOPED_QUERY_EVAL_MATCHES
        || limits.max_output_rows == 0
        || limits.max_output_rows > MAX_SCOPED_QUERY_OUTPUT_ROWS
    {
        return Err(
            "query limits must be positive and may only tighten scoped-query hard limits".into(),
        );
    }
    Ok(limits)
}

fn optional_artifact(o: &Map<String, Value>, key: &str) -> Result<Option<ArtifactRefV2>, String> {
    match required(o, key)? {
        Value::Null => Ok(None),
        v => {
            let a = object(v, key, &["kind", "digest"])?;
            Ok(Some(ArtifactRefV2 {
                kind: string(required(a, "kind")?, "artifact.kind")?.to_owned(),
                digest: parse_digest(string(required(a, "digest")?, "artifact.digest")?)?,
            }))
        }
    }
}

fn optional_digest_id(o: &Map<String, Value>, key: &str) -> Result<Option<ContextId>, String> {
    Ok(optional_digest(o, key)?.map(ContextId))
}

fn optional_digest(o: &Map<String, Value>, key: &str) -> Result<Option<Digest>, String> {
    match required(o, key)? {
        Value::Null => Ok(None),
        v => Ok(Some(parse_digest(string(v, key)?)?)),
    }
}

fn optional_uint(o: &Map<String, Value>, key: &str) -> Result<Option<u64>, String> {
    match required(o, key)? {
        Value::Null => Ok(None),
        Value::Number(n) => n
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("{key} must be a nonnegative integer or null")),
        _ => Err(format!("{key} must be a nonnegative integer or null")),
    }
}

fn uint(o: &Map<String, Value>, key: &str) -> Result<u64, String> {
    required(o, key)?
        .as_u64()
        .ok_or_else(|| format!("{key} must be a nonnegative integer"))
}

fn string_set(value: &Value, name: &str) -> Result<BTreeSet<String>, String> {
    let values = value
        .as_array()
        .ok_or_else(|| format!("{name} must be an array"))?;
    let mut out = BTreeSet::new();
    for value in values {
        let s = string(value, name)?;
        if !out.insert(s.to_owned()) {
            return Err(format!("{name} contains duplicate entry"));
        }
    }
    Ok(out)
}

fn object<'a>(
    value: &'a Value,
    name: &str,
    allowed: &[&str],
) -> Result<&'a Map<String, Value>, String> {
    let o = value
        .as_object()
        .ok_or_else(|| format!("{name} must be an object"))?;
    for key in o.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!("unknown {name} field '{key}'"));
        }
    }
    Ok(o)
}

fn required<'a>(o: &'a Map<String, Value>, key: &str) -> Result<&'a Value, String> {
    o.get(key)
        .ok_or_else(|| format!("missing required field '{key}'"))
}

fn string<'a>(value: &'a Value, name: &str) -> Result<&'a str, String> {
    value
        .as_str()
        .ok_or_else(|| format!("{name} must be a string"))
}

fn parse_digest(text: &str) -> Result<Digest, String> {
    let bytes = decode_hex(text)?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| "digest must be exactly 64 hex characters")?;
    Ok(Digest::from_bytes(bytes))
}

fn decode_hex(text: &str) -> Result<Vec<u8>, String> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return Err("hex value must contain an even number of ASCII characters".into());
    }
    let mut out = Vec::with_capacity(text.len() / 2);
    for pair in text.as_bytes().chunks_exact(2) {
        let nibble = |b: u8| match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        };
        let hi = nibble(pair[0]).ok_or("invalid hex value")?;
        let lo = nibble(pair[1]).ok_or("invalid hex value")?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

/// serde_json `Value` normally accepts duplicate object keys. Config files are
/// host policy, so retain the usual JSON shape while rejecting duplicates at
/// every nesting level before conversion to `Value`.
#[derive(Debug)]
struct UniqueJson(Value);

impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UniqueVisitor;
        impl<'de> Visitor<'de> for UniqueVisitor {
            type Value = UniqueJson;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("JSON without duplicate object keys")
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Bool(v)))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Number(Number::from(v))))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Number(Number::from(v))))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Self::Value, E> {
                Number::from_f64(v)
                    .map(|n| UniqueJson(Value::Number(n)))
                    .ok_or_else(|| E::custom("non-finite number"))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::String(v.to_owned())))
            }
            fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::String(v)))
            }
            fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Null))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueJson(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element::<UniqueJson>()? {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn query_limits() -> HostQueryLimitsV1 {
        HostQueryLimitsV1 {
            page_rows: 128,
            max_input_rows: 100_000,
            max_input_bytes: 16_777_216,
            max_eval_work: 1_000_000,
            max_eval_matches: 100_000,
            max_output_rows: 10_000,
        }
    }

    fn minimal_config() -> Value {
        let profile = HostScopeProfileV1 {
            profile_id: HOST_SCOPE_PROFILE_PARTITION_V1.to_owned(),
            partition_fields: BTreeMap::from([("records".to_owned(), "tenant_id".to_owned())]),
            global_relations: BTreeSet::new(),
            query_limits: query_limits(),
        };
        let capability_view = profile.context_capability_view().unwrap();
        json!({
            "schema": HOST_QUERY_CONFIG_SCHEMA,
            "profile": {
                "profile_id": HOST_SCOPE_PROFILE_PARTITION_V1,
                "partition_fields": {"records": "tenant_id"},
                "global_relations": []
            },
            "context": {
                "id": "00".repeat(32), "bytes": "00",
                "dimensions": {
                    "parent": null, "program": null, "world": null,
                    "namespace": "sales", "tenant": "acme", "principal": "service",
                    "capability_view": capability_view, "scenario_mode": "production",
                    "branch": null, "valid_time_start": 0, "valid_time_end": 10,
                    "transaction_time": null, "correction_time": null, "simulation_time": null,
                    "observation_profile": "none@1", "redaction_profile": "none@1",
                    "kernel_profile": "none", "regime_profile": "none", "axiom_profile": "none",
                    "validity_start": 0, "validity_end": 10, "step_budget": 1000, "depth_budget": 20
                }
            },
            "limits": {"bytes": 65536, "assumptions": 256, "depth": 128, "artifact_bytes": 65536},
            "query_limits": {
                "page_rows": 128, "max_input_rows": 100000, "max_input_bytes": 16777216,
                "max_eval_work": 1000000, "max_eval_matches": 100000, "max_output_rows": 10000
            }
        })
    }

    fn minimal_policy() -> Value {
        json!({
            "schema": "brix.host-query-policy@1",
            "profile": {
                "profile_id": HOST_SCOPE_PROFILE_PARTITION_V1,
                "partition_fields": {"records": "tenant_id"},
                "global_relations": []
            },
            "namespace": "sales", "tenant": "acme", "principal": "service",
            "valid_time_start": 0, "valid_time_end": 10,
            "validity_start": 0, "validity_end": 10,
            "step_budget": 1000, "depth_budget": 20,
            "limits": {"bytes": 65536, "assumptions": 256, "depth": 128, "artifact_bytes": 65536},
            "query_limits": {
                "page_rows": 128, "max_input_rows": 100000, "max_input_bytes": 16777216,
                "max_eval_work": 1000000, "max_eval_matches": 100000, "max_output_rows": 10000
            }
        })
    }

    #[test]
    fn rejects_unknown_fields_and_unsupported_scope_profiles() {
        let mut config = minimal_config();
        config["context"]["verified"] = json!(true);
        assert!(HostQueryConfig::from_json(&config)
            .unwrap_err()
            .contains("unknown context field"));

        let mut config = minimal_config();
        config["profile"]["profile_id"] = json!("brix.host-scope.partition@2");
        assert!(HostQueryConfig::from_json(&config)
            .unwrap_err()
            .contains("unsupported host-scope"));
    }

    #[test]
    fn rejects_widened_or_invalid_limits_and_conflicting_relations() {
        let mut config = minimal_config();
        config["limits"]["bytes"] = json!(65537);
        assert!(HostQueryConfig::from_json(&config)
            .unwrap_err()
            .contains("only tighten"));

        let mut config = minimal_config();
        config["profile"]["global_relations"] = json!(["records"]);
        assert!(HostQueryConfig::from_json(&config)
            .unwrap_err()
            .contains("both partitioned and global"));
    }

    #[test]
    fn rejects_malformed_digest_and_nonhex_context_material() {
        let mut config = minimal_config();
        config["context"]["id"] = json!("zz".repeat(32));
        assert!(HostQueryConfig::from_json(&config)
            .unwrap_err()
            .contains("invalid hex"));

        let mut config = minimal_config();
        config["context"]["bytes"] = json!("0g");
        assert!(HostQueryConfig::from_json(&config)
            .unwrap_err()
            .contains("invalid hex"));
    }

    #[test]
    fn rejects_duplicate_keys_in_config_files() {
        let parsed = serde_json::from_str::<UniqueJson>(r#"{"schema":"one","schema":"two"}"#);
        assert!(parsed
            .unwrap_err()
            .to_string()
            .contains("duplicate object key"));
    }

    #[test]
    fn parses_policy_with_host_owned_query_limits() {
        let policy = HostQueryPolicy::from_json(&minimal_policy()).unwrap();
        assert_eq!(policy.query_limits().page_rows, 128);
        assert_eq!(policy.query_limits().max_eval_work, 1_000_000);
    }

    #[test]
    fn rejects_query_limits_that_exceed_scoped_query_caps() {
        let mut policy = minimal_policy();
        policy["query_limits"]["max_eval_work"] = json!(MAX_SCOPED_QUERY_EVAL_WORK + 1);
        assert!(HostQueryPolicy::from_json(&policy)
            .unwrap_err()
            .contains("only tighten scoped-query hard limits"));
    }
}
