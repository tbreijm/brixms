//! Bounded, versioned context model. V1 encodings remain untouched.

use std::collections::{BTreeMap, BTreeSet};

use brix_canon::{CanonError, CanonReader, CanonWriter, Canonical, Digest, Domain};

use crate::ContextId;

pub const CONTEXT_MODEL_MARKER_V2: &[u8] = b"brix.semantic.context-model";
pub const CONTEXT_MODEL_VERSION_V2: u64 = 2;
pub const MAX_MODEL_BYTES: usize = 65_536;
pub const MAX_MODEL_ASSUMPTIONS: usize = 256;
pub const MAX_MODEL_DEPTH: usize = 128;
pub const MAX_MODEL_TEXT: usize = 256;
pub const MAX_MODEL_VALUE: usize = 4_096;
pub const MAX_MODEL_ARTIFACTS: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelLimits {
    pub bytes: usize,
    pub assumptions: usize,
    pub depth: usize,
    pub artifact_bytes: usize,
}
impl Default for ModelLimits {
    fn default() -> Self {
        Self {
            bytes: MAX_MODEL_BYTES,
            assumptions: MAX_MODEL_ASSUMPTIONS,
            depth: MAX_MODEL_DEPTH,
            artifact_bytes: MAX_MODEL_BYTES,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ArtifactRefV2 {
    /// Resolver-defined artifact class; this tag does not establish authority.
    pub kind: String,
    /// Content digest under `Domain::Value`; the resolver owns schema/authority checks.
    pub digest: Digest,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypedAssumptionV2 {
    pub name: String,
    pub schema: String,
    pub canonical_value: Vec<u8>,
}

/// All non-assumption context dimensions. The whole value is caller-bound at validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextDimensionsV2 {
    pub parent: Option<ContextId>,
    pub program: Option<ArtifactRefV2>,
    pub world: Option<ArtifactRefV2>,
    pub namespace: String,
    pub tenant: String,
    pub principal: String,
    pub capability_view: String,
    pub scenario_mode: ScenarioModeV2,
    pub branch: Option<Digest>,
    pub valid_time_start: u64,
    pub valid_time_end: u64,
    pub transaction_time: Option<u64>,
    pub correction_time: Option<u64>,
    pub simulation_time: Option<u64>,
    pub observation_profile: String,
    pub redaction_profile: String,
    pub kernel_profile: String,
    pub regime_profile: String,
    pub axiom_profile: String,
    pub validity_start: u64,
    pub validity_end: u64,
    pub step_budget: u64,
    pub depth_budget: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScenarioModeV2 {
    Production,
    Simulation,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextModelV2 {
    dimensions: ContextDimensionsV2,
    assumptions: Vec<TypedAssumptionV2>,
}
impl ContextModelV2 {
    pub fn new(
        dimensions: ContextDimensionsV2,
        mut assumptions: Vec<TypedAssumptionV2>,
    ) -> Result<Self, ModelError> {
        if assumptions.len() > MAX_MODEL_ASSUMPTIONS {
            return Err(ModelError::TooManyAssumptions);
        }
        assumptions.sort_by(|a, b| a.name.cmp(&b.name));
        validate_shape(&dimensions, &assumptions)?;
        let model = Self {
            dimensions,
            assumptions,
        };
        if encode_context_model_v2(&model).len() > MAX_MODEL_BYTES {
            return Err(ModelError::TooLarge);
        }
        Ok(model)
    }
    pub fn dimensions(&self) -> &ContextDimensionsV2 {
        &self.dimensions
    }
    pub fn assumptions(&self) -> &[TypedAssumptionV2] {
        &self.assumptions
    }
    pub fn id(&self) -> ContextId {
        ContextId::from_canon(&encode_context_model_v2(self))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelError {
    TooLarge,
    TooManyAssumptions,
    TooDeep,
    BadLength,
    Malformed,
    NonCanonicalOrder,
    DuplicateName,
    InvalidField,
    IdentityMismatch,
    BindingMismatch,
    MissingParent(ContextId),
    MissingArtifact(ArtifactRefV2),
    ArtifactMismatch(ArtifactRefV2),
    ArtifactBudgetExceeded,
    AssumptionRejected(String),
    ParentDimensionMismatch,
    ModeMismatch,
    MissingUsedAssumption(String),
    TransportMismatch,
}
impl From<CanonError> for ModelError {
    fn from(_: CanonError) -> Self {
        Self::Malformed
    }
}

/// Application-owned semantic typing hook. The model validates structure only.
/// The implementation is part of the trusted boundary; it must verify the named
/// schema and charge consumed payload bytes against `budget`.
pub trait AssumptionValidatorV2 {
    fn validate(&self, schema: &str, value: &[u8], budget: &mut usize) -> bool;
}
pub trait ContextResolverV2 {
    /// Return raw canonical V2 bytes for this exact content-addressed parent.
    fn context_bytes(&self, id: &ContextId) -> Option<Vec<u8>>;
    /// Return the canonical preimage for an artifact. Digest matching establishes
    /// content identity and availability only; the resolver must enforce ownership
    /// and artifact-kind policy independently.
    fn artifact_bytes(&self, reference: &ArtifactRefV2) -> Option<Vec<u8>>;
}

/// Checked handles cannot be supplied as untrusted assertions.
/// ```compile_fail
/// use brix_semantic::CheckedContextV2;
/// let forged = CheckedContextV2 { model: todo!(), ancestors: vec![] };
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckedContextV2 {
    model: ContextModelV2,
    ancestors: Vec<ContextModelV2>,
}
impl CheckedContextV2 {
    pub fn id(&self) -> ContextId {
        self.model.id()
    }
    pub fn model(&self) -> &ContextModelV2 {
        &self.model
    }
    /// Effective bindings after applying ancestors root-to-child; a child binding shadows a parent binding.
    pub fn assumptions_in_scope(&self) -> BTreeMap<String, TypedAssumptionV2> {
        let mut out = BTreeMap::new();
        for scope in self
            .ancestors
            .iter()
            .rev()
            .chain(std::iter::once(&self.model))
        {
            for a in &scope.assumptions {
                out.insert(a.name.clone(), a.clone());
            }
        }
        out
    }
    pub fn same_context(&self, other: &Self) -> bool {
        self.id() == other.id()
    }
    /// Add a local binding, shadowing an inherited same-name binding when present.
    /// This returns a checked child only after resolver and assumption validation.
    pub fn extend_typed<R: ContextResolverV2, A: AssumptionValidatorV2>(
        &self,
        assumption: TypedAssumptionV2,
        limits: ModelLimits,
        resolver: &R,
        validator: &A,
    ) -> Result<CheckedContextV2, ModelError> {
        let mut dims = self.model.dimensions.clone();
        dims.parent = Some(self.id());
        let model = ContextModelV2::new(dims.clone(), vec![assumption])?;
        let bytes = encode_context_model_v2(&model);
        let layered = self.layered_resolver(resolver);
        validate_context_model_v2(&bytes, model.id(), &dims, limits, &layered, validator)
    }
    pub fn rebind_typed<R: ContextResolverV2, A: AssumptionValidatorV2>(
        &self,
        assumption: TypedAssumptionV2,
        limits: ModelLimits,
        resolver: &R,
        validator: &A,
    ) -> Result<CheckedContextV2, ModelError> {
        self.extend_typed(assumption, limits, resolver, validator)
    }
    /// Projection may remove assumptions only; every non-assumption dimension is preserved.
    pub fn restrict<R: ContextResolverV2, A: AssumptionValidatorV2>(
        &self,
        keep: &BTreeSet<String>,
        limits: ModelLimits,
        resolver: &R,
        validator: &A,
    ) -> Result<CheckedContextV2, ModelError> {
        let effective = self.assumptions_in_scope();
        if effective.keys().any(|name| {
            !keep.contains(name)
                && self
                    .ancestors
                    .iter()
                    .any(|p| p.assumptions.iter().any(|a| &a.name == name))
        }) {
            return Err(ModelError::MissingUsedAssumption(
                "inherited binding cannot be detached".into(),
            ));
        }
        let assumptions = self
            .model
            .assumptions
            .iter()
            .filter(|a| keep.contains(&a.name))
            .cloned()
            .collect();
        let model = ContextModelV2::new(self.model.dimensions.clone(), assumptions)?;
        let layered = self.layered_resolver(resolver);
        validate_context_model_v2(
            &encode_context_model_v2(&model),
            model.id(),
            &model.dimensions,
            limits,
            &layered,
            validator,
        )
    }
    fn layered_resolver<'a, R: ContextResolverV2>(&self, resolver: &'a R) -> ParentLayer<'a, R> {
        let mut contexts = vec![(self.id(), encode_context_model_v2(&self.model))];
        contexts.extend(
            self.ancestors
                .iter()
                .map(|m| (m.id(), encode_context_model_v2(m))),
        );
        ParentLayer {
            contexts,
            inner: resolver,
        }
    }
    pub fn transport_to(
        &self,
        destination: &CheckedContextV2,
        used: &BTreeSet<String>,
    ) -> Result<ContextTransportReceiptV2, ModelError> {
        if immutable_dimensions(&self.model.dimensions)
            != immutable_dimensions(&destination.model.dimensions)
        {
            return Err(ModelError::TransportMismatch);
        }
        for name in used {
            let src = self.assumptions_in_scope().get(name).cloned();
            let dst = destination.assumptions_in_scope().get(name).cloned();
            if src.is_none() || src != dst {
                return Err(ModelError::MissingUsedAssumption(name.clone()));
            }
        }
        let mut w = CanonWriter::new();
        w.write_tag("brix.context.used-assumptions@2");
        for name in used {
            w.write_ident(name);
        }
        Ok(ContextTransportReceiptV2 {
            source: self.id(),
            destination: destination.id(),
            used_assumptions: Digest::of(Domain::Value, &w.finish()),
        })
    }

    /// Project the effective bindings into a standalone context and retain the
    /// complete evaluation context in a receipt. `used` is a dependency claim,
    /// not proof: the consuming checker must recheck the actual term in the result.
    pub fn project_assumptions<R: ContextResolverV2, A: AssumptionValidatorV2>(
        &self,
        used: &BTreeSet<String>,
        limits: ModelLimits,
        resolver: &R,
        validator: &A,
    ) -> Result<(CheckedContextV2, ContextTransportReceiptV2), ModelError> {
        let effective = self.assumptions_in_scope();
        let mut retained = Vec::new();
        for name in used {
            retained.push(
                effective
                    .get(name)
                    .cloned()
                    .ok_or_else(|| ModelError::MissingUsedAssumption(name.clone()))?,
            );
        }
        let mut dimensions = self.model.dimensions.clone();
        dimensions.parent = None;
        let model = ContextModelV2::new(dimensions.clone(), retained)?;
        let projected = validate_context_model_v2(
            &encode_context_model_v2(&model),
            model.id(),
            &dimensions,
            limits,
            resolver,
            validator,
        )?;
        let receipt = self.transport_to(&projected, used)?;
        Ok((projected, receipt))
    }
}

/// A receipt binds checked source/destination identities and used assumption bindings.
/// It does not upgrade evidence or authorize a theorem; consumers must recheck
/// the actual term and read dependencies.
/// ```compile_fail
/// use brix_semantic::ContextTransportReceiptV2;
/// let forged = ContextTransportReceiptV2 { source: todo!(), destination: todo!(), used_assumptions: todo!() };
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextTransportReceiptV2 {
    source: ContextId,
    destination: ContextId,
    used_assumptions: Digest,
}
impl ContextTransportReceiptV2 {
    /// Content identity of the receipt, separate from any proof certificate.
    pub fn id(&self) -> Digest {
        Digest::of(Domain::Value, &self.canon_bytes())
    }
    pub fn source(&self) -> ContextId {
        self.source
    }
    pub fn destination(&self) -> ContextId {
        self.destination
    }
    pub fn used_assumptions_digest(&self) -> Digest {
        self.used_assumptions
    }
}

impl Canonical for ContextTransportReceiptV2 {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_tag("brix.context.transport-receipt@2");
        self.source.canon_write(w);
        self.destination.canon_write(w);
        w.write_bytes(self.used_assumptions.as_bytes());
    }
}

fn immutable_dimensions(d: &ContextDimensionsV2) -> ContextDimensionsV2 {
    let mut x = d.clone();
    x.parent = None;
    x
}

struct ParentLayer<'a, R> {
    contexts: Vec<(ContextId, Vec<u8>)>,
    inner: &'a R,
}
impl<R: ContextResolverV2> ContextResolverV2 for ParentLayer<'_, R> {
    fn context_bytes(&self, id: &ContextId) -> Option<Vec<u8>> {
        self.contexts
            .iter()
            .find(|(known, _)| known == id)
            .map(|(_, bytes)| bytes.clone())
            .or_else(|| self.inner.context_bytes(id))
    }
    fn artifact_bytes(&self, a: &ArtifactRefV2) -> Option<Vec<u8>> {
        self.inner.artifact_bytes(a)
    }
}

pub fn validate_context_model_v2<R: ContextResolverV2, A: AssumptionValidatorV2>(
    bytes: &[u8],
    expected: ContextId,
    expected_dimensions: &ContextDimensionsV2,
    limits: ModelLimits,
    resolver: &R,
    assumptions: &A,
) -> Result<CheckedContextV2, ModelError> {
    let limits = ModelLimits {
        bytes: limits.bytes.min(MAX_MODEL_BYTES),
        assumptions: limits.assumptions.min(MAX_MODEL_ASSUMPTIONS),
        depth: limits.depth.min(MAX_MODEL_DEPTH),
        artifact_bytes: limits.artifact_bytes.min(MAX_MODEL_BYTES),
    };
    if bytes.len() > limits.bytes {
        return Err(ModelError::TooLarge);
    }
    let model = decode_context_model_v2(bytes, limits)?;
    if model.id() != expected {
        return Err(ModelError::IdentityMismatch);
    }
    if &model.dimensions != expected_dimensions {
        return Err(ModelError::BindingMismatch);
    }
    let mut work = limits.artifact_bytes;
    let mut artifact_count = 0;
    let mut chain_bytes = bytes.len();
    validate_artifacts(&model, resolver, &mut work, &mut artifact_count)?;
    let mut assumption_budget = limits.bytes;
    let mut chain_assumption_count = model.assumptions.len();
    for a in &model.assumptions {
        if !assumptions.validate(&a.schema, &a.canonical_value, &mut assumption_budget) {
            return Err(ModelError::AssumptionRejected(a.name.clone()));
        }
    }
    // Iterative parent traversal. Every parent must preserve all non-assumption dimensions.
    let mut seen = BTreeSet::new();
    let mut parent = model.dimensions.parent;
    let mut depth = 0;
    let mut child_mode = model.dimensions.scenario_mode;
    let mut ancestors = Vec::new();
    while let Some(id) = parent {
        if !seen.insert(id) {
            return Err(ModelError::Malformed);
        }
        depth += 1;
        if depth > limits.depth {
            return Err(ModelError::TooDeep);
        }
        let raw = resolver
            .context_bytes(&id)
            .ok_or(ModelError::MissingParent(id))?;
        if raw.len() > limits.bytes.saturating_sub(chain_bytes) {
            return Err(ModelError::TooLarge);
        }
        chain_bytes += raw.len();
        let p = decode_context_model_v2(&raw, limits)?;
        if p.id() != id {
            return Err(ModelError::IdentityMismatch);
        }
        if p.dimensions.scenario_mode == ScenarioModeV2::Simulation
            && child_mode == ScenarioModeV2::Production
        {
            return Err(ModelError::ModeMismatch);
        }
        if immutable_dimensions(&p.dimensions) != immutable_dimensions(&model.dimensions) {
            return Err(ModelError::ParentDimensionMismatch);
        }
        validate_artifacts(&p, resolver, &mut work, &mut artifact_count)?;
        chain_assumption_count = chain_assumption_count.saturating_add(p.assumptions.len());
        if chain_assumption_count > limits.assumptions {
            return Err(ModelError::TooManyAssumptions);
        }
        for a in &p.assumptions {
            if !assumptions.validate(&a.schema, &a.canonical_value, &mut assumption_budget) {
                return Err(ModelError::AssumptionRejected(a.name.clone()));
            }
        }
        child_mode = p.dimensions.scenario_mode;
        parent = p.dimensions.parent;
        ancestors.push(p);
    }
    Ok(CheckedContextV2 { model, ancestors })
}

fn validate_artifacts<R: ContextResolverV2>(
    m: &ContextModelV2,
    r: &R,
    remaining: &mut usize,
    count: &mut usize,
) -> Result<(), ModelError> {
    for a in [&m.dimensions.program, &m.dimensions.world]
        .into_iter()
        .flatten()
    {
        *count += 1;
        if *count > MAX_MODEL_ARTIFACTS {
            return Err(ModelError::ArtifactBudgetExceeded);
        }
        let raw = r
            .artifact_bytes(a)
            .ok_or_else(|| ModelError::MissingArtifact(a.clone()))?;
        if raw.len() > *remaining {
            return Err(ModelError::ArtifactBudgetExceeded);
        }
        *remaining -= raw.len();
        if Digest::of(Domain::Value, &raw) != a.digest {
            return Err(ModelError::ArtifactMismatch(a.clone()));
        }
    }
    Ok(())
}

fn validate_shape(d: &ContextDimensionsV2, a: &[TypedAssumptionV2]) -> Result<(), ModelError> {
    if a.len() > MAX_MODEL_ASSUMPTIONS {
        return Err(ModelError::TooManyAssumptions);
    }
    for s in [
        &d.namespace,
        &d.tenant,
        &d.principal,
        &d.capability_view,
        &d.observation_profile,
        &d.redaction_profile,
        &d.kernel_profile,
        &d.regime_profile,
        &d.axiom_profile,
    ] {
        if s.is_empty() || s.len() > MAX_MODEL_TEXT || !s.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(ModelError::InvalidField);
        }
    }
    for a in [&d.program, &d.world].into_iter().flatten() {
        if a.kind.is_empty()
            || a.kind.len() > MAX_MODEL_TEXT
            || !a.kind.bytes().all(|b| b.is_ascii_graphic())
        {
            return Err(ModelError::InvalidField);
        }
    }
    if d.valid_time_start > d.valid_time_end
        || d.validity_start > d.validity_end
        || d.step_budget == 0
        || d.depth_budget == 0
    {
        return Err(ModelError::InvalidField);
    }
    match (d.scenario_mode, d.branch) {
        (ScenarioModeV2::Production, None) | (ScenarioModeV2::Simulation, Some(_)) => {}
        _ => return Err(ModelError::InvalidField),
    }
    for n in [d.transaction_time, d.correction_time, d.simulation_time]
        .into_iter()
        .flatten()
    {
        if n < d.validity_start || n > d.validity_end {
            return Err(ModelError::InvalidField);
        }
    }
    let mut names = BTreeSet::new();
    for x in a {
        if x.name.is_empty()
            || x.schema.is_empty()
            || x.name.len() > MAX_MODEL_TEXT
            || x.schema.len() > MAX_MODEL_TEXT
            || x.canonical_value.is_empty()
            || x.canonical_value.len() > MAX_MODEL_VALUE
            || !x.name.bytes().all(|b| b.is_ascii_graphic())
            || !x.schema.bytes().all(|b| b.is_ascii_graphic())
            || !names.insert(&x.name)
        {
            return Err(ModelError::DuplicateName);
        }
    }
    Ok(())
}

pub fn encode_context_model_v2(m: &ContextModelV2) -> Vec<u8> {
    let mut w = CanonWriter::new();
    w.write_bytes(CONTEXT_MODEL_MARKER_V2);
    w.write_uint(CONTEXT_MODEL_VERSION_V2);
    let d = &m.dimensions;
    opt_id(&mut w, d.parent);
    for a in [&d.program, &d.world] {
        match a {
            None => w.write_enum(0, |_| {}),
            Some(x) => w.write_enum(1, |w| {
                w.write_ident(&x.kind);
                w.write_bytes(x.digest.as_bytes())
            }),
        }
    }
    for s in [&d.namespace, &d.tenant, &d.principal, &d.capability_view] {
        w.write_ident(s)
    }
    w.write_uint(match d.scenario_mode {
        ScenarioModeV2::Production => 0,
        ScenarioModeV2::Simulation => 1,
    });
    opt_digest(&mut w, d.branch);
    for n in [d.valid_time_start, d.valid_time_end] {
        w.write_uint(n)
    }
    for n in [d.transaction_time, d.correction_time, d.simulation_time] {
        opt_uint(&mut w, n);
    }
    for s in [
        &d.observation_profile,
        &d.redaction_profile,
        &d.kernel_profile,
        &d.regime_profile,
        &d.axiom_profile,
    ] {
        w.write_ident(s)
    }
    for n in [
        d.validity_start,
        d.validity_end,
        d.step_budget,
        d.depth_budget,
    ] {
        w.write_uint(n)
    }
    w.write_uint(m.assumptions.len() as u64);
    for a in &m.assumptions {
        w.write_ident(&a.name);
        w.write_ident(&a.schema);
        w.write_bytes(&a.canonical_value)
    }
    w.finish()
}
fn opt_digest(w: &mut CanonWriter, d: Option<Digest>) {
    match d {
        None => w.write_enum(0, |_| {}),
        Some(x) => w.write_enum(1, |w| w.write_bytes(x.as_bytes())),
    }
}
fn opt_uint(w: &mut CanonWriter, n: Option<u64>) {
    match n {
        None => w.write_enum(0, |_| {}),
        Some(v) => w.write_enum(1, |w| w.write_uint(v)),
    }
}
fn get_opt_uint(r: &mut CanonReader<'_>) -> Result<Option<u64>, ModelError> {
    match r.read_uint()? {
        0 => Ok(None),
        1 => Ok(Some(r.read_uint()?)),
        _ => Err(ModelError::Malformed),
    }
}
fn opt_id(w: &mut CanonWriter, d: Option<ContextId>) {
    opt_digest(w, d.map(|x| x.digest()))
}
fn get_opt_digest(r: &mut CanonReader<'_>) -> Result<Option<Digest>, ModelError> {
    match r.read_uint()? {
        0 => Ok(None),
        1 => Ok(Some(Digest::from_bytes(
            r.read_bytes()?
                .try_into()
                .map_err(|_| ModelError::BadLength)?,
        ))),
        _ => Err(ModelError::Malformed),
    }
}
fn read_text(r: &mut CanonReader<'_>) -> Result<String, ModelError> {
    let b = r.read_bytes()?;
    if b.len() > MAX_MODEL_TEXT {
        return Err(ModelError::InvalidField);
    };
    Ok(std::str::from_utf8(b)
        .map_err(|_| ModelError::InvalidField)?
        .to_owned())
}
pub fn decode_context_model_v2(
    bytes: &[u8],
    limits: ModelLimits,
) -> Result<ContextModelV2, ModelError> {
    if bytes.len() > limits.bytes.min(MAX_MODEL_BYTES) {
        return Err(ModelError::TooLarge);
    }
    let mut r = CanonReader::new(bytes);
    if r.read_bytes()? != CONTEXT_MODEL_MARKER_V2 || r.read_uint()? != CONTEXT_MODEL_VERSION_V2 {
        return Err(ModelError::Malformed);
    }
    let parent = get_opt_digest(&mut r)?.map(ContextId);
    let mut artifact = || -> Result<Option<ArtifactRefV2>, ModelError> {
        match r.read_uint()? {
            0 => Ok(None),
            1 => {
                let kind = read_text(&mut r)?;
                let digest = Digest::from_bytes(
                    r.read_bytes()?
                        .try_into()
                        .map_err(|_| ModelError::BadLength)?,
                );
                Ok(Some(ArtifactRefV2 { kind, digest }))
            }
            _ => Err(ModelError::Malformed),
        }
    };
    let program = artifact()?;
    let world = artifact()?;
    let namespace = read_text(&mut r)?;
    let tenant = read_text(&mut r)?;
    let principal = read_text(&mut r)?;
    let capability_view = read_text(&mut r)?;
    let scenario_mode = match r.read_uint()? {
        0 => ScenarioModeV2::Production,
        1 => ScenarioModeV2::Simulation,
        _ => return Err(ModelError::Malformed),
    };
    let branch = get_opt_digest(&mut r)?;
    let valid_time_start = r.read_uint()?;
    let valid_time_end = r.read_uint()?;
    let transaction_time = get_opt_uint(&mut r)?;
    let correction_time = get_opt_uint(&mut r)?;
    let simulation_time = get_opt_uint(&mut r)?;
    let observation_profile = read_text(&mut r)?;
    let redaction_profile = read_text(&mut r)?;
    let kernel_profile = read_text(&mut r)?;
    let regime_profile = read_text(&mut r)?;
    let axiom_profile = read_text(&mut r)?;
    let validity_start = r.read_uint()?;
    let validity_end = r.read_uint()?;
    let step_budget = r.read_uint()?;
    let depth_budget = r.read_uint()?;
    let n = usize::try_from(r.read_uint()?).map_err(|_| ModelError::BadLength)?;
    if n > limits.assumptions.min(MAX_MODEL_ASSUMPTIONS) {
        return Err(ModelError::TooManyAssumptions);
    }
    let mut assumptions = Vec::with_capacity(n);
    let mut last: Option<String> = None;
    for _ in 0..n {
        let name = read_text(&mut r)?;
        let schema = read_text(&mut r)?;
        let v = r.read_bytes()?;
        if v.len() > MAX_MODEL_VALUE {
            return Err(ModelError::InvalidField);
        };
        if last.as_ref().is_some_and(|x| x >= &name) {
            return Err(ModelError::NonCanonicalOrder);
        };
        last = Some(name.clone());
        assumptions.push(TypedAssumptionV2 {
            name,
            schema,
            canonical_value: v.to_vec(),
        });
    }
    if !r.is_empty() {
        return Err(ModelError::Malformed);
    }
    let d = ContextDimensionsV2 {
        parent,
        program,
        world,
        namespace,
        tenant,
        principal,
        capability_view,
        scenario_mode,
        branch,
        valid_time_start,
        valid_time_end,
        transaction_time,
        correction_time,
        simulation_time,
        observation_profile,
        redaction_profile,
        kernel_profile,
        regime_profile,
        axiom_profile,
        validity_start,
        validity_end,
        step_budget,
        depth_budget,
    };
    validate_shape(&d, &assumptions)?;
    let model = ContextModelV2 {
        dimensions: d,
        assumptions,
    };
    if encode_context_model_v2(&model).as_slice() != bytes {
        return Err(ModelError::NonCanonicalOrder);
    }
    if encode_context_model_v2(&model).len() > MAX_MODEL_BYTES {
        return Err(ModelError::TooLarge);
    }
    Ok(model)
}
