//! Host-authorized, context-bound scope handles for named read queries.
//!
//! These types sit on the embedding-host side of the transport boundary. A
//! decoded request never creates an [`AuthorizedHostScope`]; the host validates
//! its independently selected context and profile first.

use std::collections::{BTreeMap, BTreeSet};

use brix_canon::{CanonReader, CanonWriter, Digest, Domain};
use brix_semantic::{
    validate_context_model_v2, AssumptionValidatorV2, CheckedContextV2, ContextDimensionsV2,
    ContextId, ContextResolverV2, ModelError, ModelLimits, ScenarioModeV2,
};
use brix_syntax::ast::{QueryDecl, Ty};

pub const HOST_SCOPE_PROFILE_PARTITION_V1: &str = "brix.host-scope.partition@1";
pub const QUERY_WORLD_ARTIFACT_V1: &str = "world.revision@1";
pub const QUERY_PROGRAM_ARTIFACT_V1: &str = "program";
pub const SCOPED_QUERY_ORDERING_V1: u64 = 1;
/// Named reads after typed collection ordering was introduced.
pub const SCOPED_QUERY_ORDERING_V2: u64 = 2;
pub const MAX_SCOPED_QUERY_PAGE_ROWS: usize = 1024;
pub const MAX_SCOPED_QUERY_INPUT_ROWS: usize = 100_000;
pub const MAX_SCOPED_QUERY_INPUT_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_SCOPED_QUERY_EVAL_WORK: u64 = 10_000_000;
pub const MAX_SCOPED_QUERY_EVAL_MATCHES: u64 = 1_000_000;
pub const MAX_SCOPED_QUERY_OUTPUT_ROWS: usize = 100_000;
pub const MAX_SCOPED_QUERY_CURSOR_TOKEN_BYTES: usize = 8 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostQueryLimitsV1 {
    pub page_rows: usize,
    pub max_input_rows: usize,
    pub max_input_bytes: usize,
    pub max_eval_work: u64,
    pub max_eval_matches: u64,
    pub max_output_rows: usize,
}

pub fn named_query_identity_v1(program_digest: Digest, query_name: &str) -> Digest {
    let mut w = CanonWriter::new();
    w.write_tag("brix.named-query-identity@1");
    w.write_bytes(program_digest.as_bytes());
    w.write_str(query_name);
    w.digest(Domain::Value)
}

/// Canonical artifact preimage for the exact world/program revision a query
/// context authorizes. Hosts expose this through their `ContextResolverV2`.
pub fn world_revision_artifact_preimage_v1(
    world_id: &str,
    revision_seq: u64,
    revision_digest: Digest,
    program_digest: Digest,
) -> Vec<u8> {
    let mut w = CanonWriter::new();
    w.write_tag("brix.world-revision-artifact@1");
    w.write_str(world_id);
    w.write_uint(revision_seq);
    w.write_bytes(revision_digest.as_bytes());
    w.write_bytes(program_digest.as_bytes());
    w.finish()
}

pub fn world_revision_artifact_digest_v1(
    world_id: &str,
    revision_seq: u64,
    revision_digest: Digest,
    program_digest: Digest,
) -> Digest {
    Digest::of(
        Domain::Value,
        &world_revision_artifact_preimage_v1(
            world_id,
            revision_seq,
            revision_digest,
            program_digest,
        ),
    )
}

/// Versioned host policy: each source is either partitioned by a declared field
/// or explicitly approved as global. Derived and decision relations are not
/// accepted as profile entries in the initial release profile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostScopeProfileV1 {
    pub profile_id: String,
    pub partition_fields: BTreeMap<String, String>,
    pub global_relations: BTreeSet<String>,
    pub query_limits: HostQueryLimitsV1,
}

impl HostScopeProfileV1 {
    /// Canonical identity of the host-owned scope configuration. This digest
    /// is embedded in the validated V2 context's `capability_view` dimension.
    pub fn profile_digest(&self) -> Result<Digest, ScopeError> {
        self.validate()?;
        let mut w = CanonWriter::new();
        w.write_tag("brix.host-scope-profile@1");
        w.write_str(&self.profile_id);
        w.write_uint(self.partition_fields.len() as u64);
        for (relation, field) in &self.partition_fields {
            w.write_str(relation);
            w.write_str(field);
        }
        w.write_uint(self.global_relations.len() as u64);
        for relation in &self.global_relations {
            w.write_str(relation);
        }
        w.write_uint(self.query_limits.page_rows as u64);
        w.write_uint(self.query_limits.max_input_rows as u64);
        w.write_uint(self.query_limits.max_input_bytes as u64);
        w.write_uint(self.query_limits.max_eval_work);
        w.write_uint(self.query_limits.max_eval_matches);
        w.write_uint(self.query_limits.max_output_rows as u64);
        Ok(w.digest(Domain::Value))
    }

    pub fn context_capability_view(&self) -> Result<String, ScopeError> {
        Ok(format!(
            "{}:{}",
            self.profile_id,
            self.profile_digest()?.to_hex()
        ))
    }

    pub fn validate(&self) -> Result<(), ScopeError> {
        if self.profile_id != HOST_SCOPE_PROFILE_PARTITION_V1 {
            return Err(ScopeError::InvalidProfile(
                "unsupported host-scope profile version",
            ));
        }
        if self
            .partition_fields
            .keys()
            .any(|r| self.global_relations.contains(r))
        {
            return Err(ScopeError::InvalidProfile(
                "relation is both partitioned and global",
            ));
        }
        let q = self.query_limits;
        if q.page_rows == 0
            || q.page_rows > MAX_SCOPED_QUERY_PAGE_ROWS
            || q.max_input_rows == 0
            || q.max_input_rows > MAX_SCOPED_QUERY_INPUT_ROWS
            || q.max_input_bytes == 0
            || q.max_input_bytes > MAX_SCOPED_QUERY_INPUT_BYTES
            || q.max_eval_work == 0
            || q.max_eval_work > MAX_SCOPED_QUERY_EVAL_WORK
            || q.max_eval_matches == 0
            || q.max_eval_matches > MAX_SCOPED_QUERY_EVAL_MATCHES
            || q.max_output_rows == 0
            || q.max_output_rows > MAX_SCOPED_QUERY_OUTPUT_ROWS
        {
            return Err(ScopeError::InvalidProfile(
                "query limit outside supported bounds",
            ));
        }
        if self
            .partition_fields
            .iter()
            .any(|(r, f)| r.is_empty() || f.is_empty())
            || self.global_relations.iter().any(String::is_empty)
        {
            return Err(ScopeError::InvalidProfile("empty relation or field"));
        }
        Ok(())
    }
}

/// Independently selected host expectation. `bytes` are untrusted evidence;
/// `id` and `dimensions` must come from host request state, not the envelope.
pub struct HostContextExpectationV2<'a> {
    pub bytes: &'a [u8],
    pub id: ContextId,
    pub dimensions: &'a ContextDimensionsV2,
    pub limits: ModelLimits,
}

/// Private checked capability passed to the scoped query evaluator.
#[derive(Clone, Debug)]
pub struct AuthorizedHostScope {
    profile: HostScopeProfileV1,
    context: CheckedContextV2,
    identity: Digest,
    tenant: String,
}

/// Every non-positional input that gives meaning to a scoped continuation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopedCursorBindingsV1 {
    pub query_identity: Digest,
    pub program_identity: Digest,
    pub typed_arguments: Digest,
    pub scope_identity: Digest,
    pub context_identity: ContextId,
    pub world_identity: String,
    pub revision_seq: u64,
    pub revision_digest: Digest,
    pub ordering_version: u64,
}

/// Canonical continuation for one named query at one pinned world revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopedQueryCursorV1 {
    bindings: ScopedCursorBindingsV1,
    position: u64,
}

/// Deliberately small scalar fragment supported by the first named-query
/// profile. Values are canonical data and never parsed as Brix source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueryValueV1 {
    Str(String),
    Int(i64),
    Bool(bool),
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct NamedQueryArgsV1(pub BTreeMap<String, QueryValueV1>);

impl NamedQueryArgsV1 {
    /// Require an exact name/type match with the declaration before execution.
    pub fn validate(&self, declaration: &QueryDecl) -> Result<Digest, ScopeError> {
        if self.0.len() != declaration.params.len() {
            return Err(ScopeError::QueryArgumentSetMismatch);
        }
        let mut w = CanonWriter::new();
        w.write_tag("brix.named-query-args@1");
        w.write_uint(self.0.len() as u64);
        for parameter in &declaration.params {
            let value = self
                .0
                .get(&parameter.name)
                .ok_or(ScopeError::QueryArgumentSetMismatch)?;
            match (&parameter.ty, value) {
                (Ty::Named(expected), QueryValueV1::Str(_)) if expected == "Str" => {}
                (Ty::Named(expected), QueryValueV1::Int(_)) if expected == "Int" => {}
                (Ty::Named(expected), QueryValueV1::Bool(_)) if expected == "Bool" => {}
                (Ty::Named(expected), _) if matches!(expected.as_str(), "Str" | "Int" | "Bool") => {
                    return Err(ScopeError::QueryArgumentTypeMismatch(
                        parameter.name.clone(),
                    ))
                }
                _ => return Err(ScopeError::UnsupportedQueryType(parameter.name.clone())),
            }
            w.write_str(&parameter.name);
            match value {
                QueryValueV1::Str(v) => {
                    w.write_uint(1);
                    w.write_str(v);
                }
                QueryValueV1::Int(v) => {
                    w.write_uint(2);
                    w.write_int(*v);
                }
                QueryValueV1::Bool(v) => {
                    w.write_uint(3);
                    w.write_bool(*v);
                }
            }
        }
        Ok(w.digest(Domain::Value))
    }

    pub fn lower_scalars(&self) -> BTreeMap<String, brix_lower::QueryScalarV1> {
        self.0
            .iter()
            .map(|(name, value)| {
                (
                    name.clone(),
                    match value {
                        QueryValueV1::Str(v) => brix_lower::QueryScalarV1::Str(v.clone()),
                        QueryValueV1::Int(v) => brix_lower::QueryScalarV1::Int(*v),
                        QueryValueV1::Bool(v) => brix_lower::QueryScalarV1::Bool(*v),
                    },
                )
            })
            .collect()
    }
}

/// Transport-neutral first-page/continuation request model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopedQueryPageRequestV1 {
    pub query_name: String,
    pub arguments: NamedQueryArgsV1,
    pub revision: Option<u64>,
    pub limit: usize,
    pub max_input_rows: usize,
    pub max_input_bytes: usize,
    pub max_eval_work: u64,
    pub max_eval_matches: u64,
    pub max_output_rows: usize,
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopedQueryPageV1 {
    pub query_identity: Digest,
    pub revision_seq: u64,
    pub revision_digest: Digest,
    pub rows: Vec<super::types::WorldTuple>,
    pub result_schema: BTreeMap<String, Ty>,
    pub typed_rows: Vec<serde_json::Value>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
    /// Observation-only counters from bounded ephemeral evaluation. They do not
    /// contribute to query identity, cursor bindings, or storage state.
    pub work_units: u64,
    pub matches: u64,
    pub expressions_evaluated: u64,
}

impl ScopedQueryCursorV1 {
    pub fn new(bindings: ScopedCursorBindingsV1, position: u64) -> Self {
        Self { bindings, position }
    }

    pub fn position(&self) -> u64 {
        self.position
    }
    pub fn revision_seq(&self) -> u64 {
        self.bindings.revision_seq
    }

    pub fn to_token(&self) -> String {
        let body = self.encode_body();
        let digest = Digest::of(Domain::Value, &body);
        let mut writer = CanonWriter::new();
        writer.write_tag("brix.scoped-query-cursor-envelope@1");
        writer.write_bytes(&body);
        writer.write_bytes(digest.as_bytes());
        hex(&writer.finish())
    }

    pub fn decode_token(token: &str) -> Result<Self, ScopeError> {
        if token.len() > MAX_SCOPED_QUERY_CURSOR_TOKEN_BYTES {
            return Err(ScopeError::InvalidCursor);
        }
        let bytes = unhex(token).ok_or(ScopeError::InvalidCursor)?;
        let mut reader = CanonReader::new(&bytes);
        if read_string(&mut reader)? != "brix.scoped-query-cursor-envelope@1" {
            return Err(ScopeError::InvalidCursor);
        }
        let body = reader.read_bytes().map_err(|_| ScopeError::InvalidCursor)?;
        let digest_bytes = reader.read_bytes().map_err(|_| ScopeError::InvalidCursor)?;
        if !reader.is_empty() || digest_bytes != Digest::of(Domain::Value, body).as_bytes() {
            return Err(ScopeError::InvalidCursor);
        }
        let (bindings, position) = decode_cursor_body(body)?;
        Ok(Self { bindings, position })
    }

    pub fn from_token(token: &str, expected: &ScopedCursorBindingsV1) -> Result<Self, ScopeError> {
        let cursor = Self::decode_token(token)?;
        if &cursor.bindings != expected {
            return Err(ScopeError::CursorBindingMismatch);
        }
        Ok(cursor)
    }

    fn encode_body(&self) -> Vec<u8> {
        let b = &self.bindings;
        let mut w = CanonWriter::new();
        w.write_tag("brix.scoped-query-cursor@1");
        w.write_bytes(b.query_identity.as_bytes());
        w.write_bytes(b.program_identity.as_bytes());
        w.write_bytes(b.typed_arguments.as_bytes());
        w.write_bytes(b.scope_identity.as_bytes());
        w.write_bytes(b.context_identity.digest().as_bytes());
        w.write_str(&b.world_identity);
        w.write_uint(b.revision_seq);
        w.write_bytes(b.revision_digest.as_bytes());
        w.write_uint(b.ordering_version);
        w.write_uint(self.position);
        w.finish()
    }
}

fn decode_cursor_body(bytes: &[u8]) -> Result<(ScopedCursorBindingsV1, u64), ScopeError> {
    fn digest(r: &mut CanonReader<'_>) -> Result<Digest, ScopeError> {
        let bytes = r.read_bytes().map_err(|_| ScopeError::InvalidCursor)?;
        let bytes: [u8; 32] = bytes.try_into().map_err(|_| ScopeError::InvalidCursor)?;
        Ok(Digest::from_bytes(bytes))
    }
    let mut r = CanonReader::new(bytes);
    if read_string(&mut r)? != "brix.scoped-query-cursor@1" {
        return Err(ScopeError::InvalidCursor);
    }
    let query_identity = digest(&mut r)?;
    let program_identity = digest(&mut r)?;
    let typed_arguments = digest(&mut r)?;
    let scope_identity = digest(&mut r)?;
    let context_identity = ContextId(digest(&mut r)?);
    let world_identity = read_string(&mut r)?.to_owned();
    let revision_seq = r.read_uint().map_err(|_| ScopeError::InvalidCursor)?;
    let revision_digest = digest(&mut r)?;
    let ordering_version = r.read_uint().map_err(|_| ScopeError::InvalidCursor)?;
    let position = r.read_uint().map_err(|_| ScopeError::InvalidCursor)?;
    if !r.is_empty() {
        return Err(ScopeError::InvalidCursor);
    }
    Ok((
        ScopedCursorBindingsV1 {
            query_identity,
            program_identity,
            typed_arguments,
            scope_identity,
            context_identity,
            world_identity,
            revision_seq,
            revision_digest,
            ordering_version,
        },
        position,
    ))
}

fn read_string<'a>(reader: &mut CanonReader<'a>) -> Result<&'a str, ScopeError> {
    std::str::from_utf8(reader.read_bytes().map_err(|_| ScopeError::InvalidCursor)?)
        .map_err(|_| ScopeError::InvalidCursor)
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 15) as usize] as char);
    }
    out
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let n = |b: u8| match b {
                b'0'..=b'9' => Some(b - b'0'),
                b'a'..=b'f' => Some(b - b'a' + 10),
                b'A'..=b'F' => Some(b - b'A' + 10),
                _ => None,
            };
            Some((n(pair[0])? << 4) | n(pair[1])?)
        })
        .collect()
}

impl AuthorizedHostScope {
    /// Host-only authorization seam. Call this after authenticating the caller
    /// and selecting expected context identity/dimensions independently.
    pub fn authorize<R: ContextResolverV2, A: AssumptionValidatorV2>(
        profile: HostScopeProfileV1,
        expected: HostContextExpectationV2<'_>,
        resolver: &R,
        assumptions: &A,
    ) -> Result<Self, ScopeError> {
        profile.validate()?;
        if expected.dimensions.scenario_mode != ScenarioModeV2::Production {
            return Err(ScopeError::UnsupportedMode);
        }
        if expected.dimensions.tenant.is_empty() {
            return Err(ScopeError::MissingTenant);
        }
        if expected.dimensions.namespace.is_empty()
            || expected.dimensions.principal.is_empty()
            || expected.dimensions.kernel_profile != "none"
            || expected.dimensions.regime_profile != "none"
            || expected.dimensions.axiom_profile != "none"
            || expected.dimensions.observation_profile != "none@1"
            || expected.dimensions.redaction_profile != "none@1"
        {
            return Err(ScopeError::UnsupportedContextProfile);
        }
        if expected.dimensions.capability_view != profile.context_capability_view()? {
            return Err(ScopeError::ScopeProfileBindingMismatch);
        }
        let context = validate_context_model_v2(
            expected.bytes,
            expected.id,
            expected.dimensions,
            expected.limits,
            resolver,
            assumptions,
        )?;
        if !context.assumptions_in_scope().is_empty() {
            return Err(ScopeError::UnsupportedContextAssumptions);
        }
        if profile.query_limits.max_eval_work > expected.dimensions.step_budget {
            return Err(ScopeError::ContextBudgetTooSmall);
        }
        let tenant = context.model().dimensions().tenant.clone();
        let mut w = CanonWriter::new();
        w.write_tag("brix.host-scope@1");
        w.write_bytes(profile.profile_digest()?.as_bytes());
        w.write_str(&tenant);
        w.write_bytes(context.id().digest().as_bytes());
        w.write_uint(profile.partition_fields.len() as u64);
        for (relation, field) in &profile.partition_fields {
            w.write_str(relation);
            w.write_str(field);
        }
        w.write_uint(profile.global_relations.len() as u64);
        for relation in &profile.global_relations {
            w.write_str(relation);
        }
        let identity = w.digest(Domain::Value);
        Ok(Self {
            profile,
            context,
            identity,
            tenant,
        })
    }

    pub fn profile(&self) -> &HostScopeProfileV1 {
        &self.profile
    }
    pub fn context_id(&self) -> ContextId {
        self.context.id()
    }
    pub fn identity(&self) -> Digest {
        self.identity
    }
    pub fn tenant(&self) -> &str {
        &self.tenant
    }
    pub fn query_limits(&self) -> HostQueryLimitsV1 {
        self.profile.query_limits
    }

    /// Bind this capability to the actual world snapshot and linked program
    /// selected for execution. The V2 resolver has already checked availability
    /// and digests; this independently compares those references to the session.
    pub fn check_execution_bindings(
        &self,
        world_id: &str,
        revision_seq: u64,
        revision_digest: Digest,
        program_digest: Digest,
    ) -> Result<(), ScopeError> {
        let dimensions = self.context.model().dimensions();
        let world = dimensions
            .world
            .as_ref()
            .ok_or(ScopeError::MissingWorldBinding)?;
        let program = dimensions
            .program
            .as_ref()
            .ok_or(ScopeError::MissingProgramBinding)?;
        if world.kind != QUERY_WORLD_ARTIFACT_V1
            || world.digest
                != world_revision_artifact_digest_v1(
                    world_id,
                    revision_seq,
                    revision_digest,
                    program_digest,
                )
        {
            return Err(ScopeError::WorldBindingMismatch);
        }
        if program.kind != QUERY_PROGRAM_ARTIFACT_V1 || program.digest != program_digest {
            return Err(ScopeError::ProgramBindingMismatch);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScopeError {
    InvalidProfile(&'static str),
    MissingTenant,
    UnsupportedMode,
    InvalidContext(ModelError),
    InvalidCursor,
    CursorBindingMismatch,
    UnsupportedContextProfile,
    UnsupportedContextAssumptions,
    MissingWorldBinding,
    MissingProgramBinding,
    WorldBindingMismatch,
    ProgramBindingMismatch,
    ScopeProfileBindingMismatch,
    QueryArgumentSetMismatch,
    QueryArgumentTypeMismatch(String),
    UnsupportedQueryType(String),
    InvalidReadBudget,
    ContextBudgetTooSmall,
}

impl From<ModelError> for ScopeError {
    fn from(value: ModelError) -> Self {
        Self::InvalidContext(value)
    }
}
