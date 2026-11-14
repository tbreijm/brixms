//! Materialized-context admission for SOC discovery and certificate replay.
//!
//! This is the production consumer seam for #52. Providers still enumerate;
//! context admission does not certify completeness or upgrade an outcome.

use std::collections::BTreeSet;

use brix_canon::{CanonWriter, Digest, Domain};

use brix_semantic::{
    validate_context_model_v2, AssumptionValidatorV2, CheckedContextV2, ContextDimensionsV2,
    ContextId, ContextResolverV2, ModelError, ModelLimits,
};

use crate::{
    resolution::{ResolutionQuery, ResolutionReport, ResolutionUnknown, VerifiedCompleteness},
    Adm, Candidate, ExecConfig, Interner, WitnessProvider,
};

/// Canonical snapshot artifact binding portable world/policy identities and history.
/// Store these bytes under the `soc.exec@1` artifact kind in `dimensions.world`.
pub fn execution_context_preimage(
    exec: &ExecConfig,
    interner: &Interner,
) -> Result<Vec<u8>, ModelError> {
    let world = interner
        .try_resolve(exec.world)
        .ok_or(ModelError::BindingMismatch)?;
    let policy = interner
        .try_resolve(exec.policy)
        .ok_or(ModelError::BindingMismatch)?;
    let mut w = CanonWriter::new();
    w.write_tag("brix.soc.context-execution@1");
    for digest in [world, policy, exec.history] {
        w.write_bytes(digest.as_bytes());
    }
    Ok(w.finish())
}

/// Exact context retained with discovered candidates, independently of selection.
#[derive(Clone, Debug)]
pub struct ContextualCandidates {
    context: ContextId,
    policy: Digest,
    candidates: BTreeSet<Candidate>,
}

impl ContextualCandidates {
    /// The exact admitted context under which provider discovery ran.
    pub fn context(&self) -> ContextId {
        self.context
    }

    /// Policy identity used during candidate discovery.
    pub fn policy(&self) -> Digest {
        self.policy
    }

    /// Discovered candidates, before resolution admissibility and selection.
    pub fn candidates(&self) -> &BTreeSet<Candidate> {
        &self.candidates
    }
}

/// An admitted request. There is no constructor from an unvalidated ID.
pub struct ResolutionContextV2 {
    checked: CheckedContextV2,
}

impl ResolutionContextV2 {
    /// The host supplies independent expected bindings and an owning schema validator.
    pub fn validate<R: ContextResolverV2, A: AssumptionValidatorV2>(
        bytes: &[u8],
        expected: ContextId,
        bindings: &ContextDimensionsV2,
        limits: ModelLimits,
        resolver: &R,
        validator: &A,
    ) -> Result<Self, ModelError> {
        Ok(Self {
            checked: validate_context_model_v2(
                bytes, expected, bindings, limits, resolver, validator,
            )?,
        })
    }

    pub fn checked(&self) -> &CheckedContextV2 {
        &self.checked
    }

    /// Enumerate using the existing oracle only after context admission.
    /// Provider semantics and completeness remain the owning regime's responsibility.
    pub fn candidates(
        &self,
        providers: &[&dyn WitnessProvider],
        adm: &dyn Adm,
        exec: &ExecConfig,
        interner: &Interner,
    ) -> Result<ContextualCandidates, ModelError> {
        let binding = self
            .checked
            .model()
            .dimensions()
            .world
            .as_ref()
            .ok_or(ModelError::BindingMismatch)?;
        if binding.kind != "soc.exec@1"
            || binding.digest
                != Digest::of(Domain::Value, &execution_context_preimage(exec, interner)?)
        {
            return Err(ModelError::BindingMismatch);
        }
        Ok(ContextualCandidates {
            context: self.checked.id(),
            policy: interner
                .try_resolve(exec.policy)
                .ok_or(ModelError::BindingMismatch)?,
            candidates: crate::oracle::cand(providers, adm, exec),
        })
    }

    /// Classify candidates discovered under this admitted context and retain the exact query.
    ///
    /// This is the production #52/#59 bridge: a report cannot be created from a candidate
    /// set discovered under another context, nor from a query scoped to another context.
    /// Completeness evidence remains independently checked and query-bound by
    /// `ResolutionReport::classify`. The current `WitnessProvider` API discovers from an
    /// `ExecConfig`, not a goal, so this method does not establish that candidates solve
    /// `query.goal`; callers must not interpret the report as a goal-aware resolution result.
    pub fn resolution_report(
        &self,
        query: ResolutionQuery,
        discovered: ContextualCandidates,
        evidence: Option<&VerifiedCompleteness<Candidate>>,
        selected: Option<Candidate>,
        fallback_reason: ResolutionUnknown,
    ) -> Result<ResolutionReport<Candidate>, ModelError> {
        let admitted = self.checked.id();
        if query.context != admitted
            || discovered.context != admitted
            || query.policy != discovered.policy
        {
            return Err(ModelError::BindingMismatch);
        }
        Ok(ResolutionReport::classify(
            query,
            discovered.candidates,
            evidence,
            selected,
            fallback_reason,
        ))
    }
}

/// Saturation's initial materialized profile admits no external logical assumptions.
/// Its existing P1/P6 declarations remain in the bound presentation/certificate.
pub struct NoExternalAssumptions;

impl AssumptionValidatorV2 for NoExternalAssumptions {
    fn validate(&self, _schema: &str, _value: &[u8], _budget: &mut usize) -> bool {
        false
    }
}

fn bind_presentation(
    context: &CheckedContextV2,
    presentation: &crate::saturate::PresentationV1<'_>,
) -> Result<(), ModelError> {
    let dims = context.model().dimensions();
    if context.id() != presentation.context
        || !context.assumptions_in_scope().is_empty()
        || dims.regime_profile != crate::saturate::certificate::SATURATION_PROFILE_V1
        || dims.axiom_profile != "none"
        || dims.kernel_profile != "none"
        || dims.observation_profile != presentation.profile.id().0.to_hex()
        || dims.program.as_ref().map(|r| r.digest) != Some(presentation.id.0)
        || dims.program.as_ref().map(|r| r.kind.as_str()) != Some("soc.presentation@1")
        || dims.world.is_some()
    {
        return Err(ModelError::BindingMismatch);
    }
    Ok(())
}

/// Check quiescence in the exact validated context and observation/presentation binding.
/// Uses the existing independent replay checker and preserves its Derived-only authority.
pub fn check_quiescence_in_context(
    context: &CheckedContextV2,
    certificate: &crate::saturate::QuiescenceCertificateV1,
    presentation: &crate::saturate::PresentationV1<'_>,
    terminal: &ExecConfig,
    consumed: &[crate::CommittedStep],
) -> Result<crate::saturate::CertificateCheck<crate::saturate::QuiescenceCertificateId>, ModelError>
{
    bind_presentation(context, presentation)?;
    if consumed.len() as u64 > context.model().dimensions().step_budget {
        return Err(ModelError::TooLarge);
    }
    Ok(crate::saturate::check_quiescence_certificate(
        certificate,
        presentation,
        terminal,
        consumed,
    ))
}

/// Check divergence under the same materialized context admission rule.
pub fn check_divergence_in_context(
    context: &CheckedContextV2,
    certificate: &crate::saturate::DivergenceCertificateV1,
    presentation: &crate::saturate::PresentationV1<'_>,
    consumed: &[crate::CommittedStep],
) -> Result<crate::saturate::CertificateCheck<crate::saturate::DivergenceCertificateId>, ModelError>
{
    bind_presentation(context, presentation)?;
    if consumed.len() as u64 > context.model().dimensions().step_budget {
        return Err(ModelError::TooLarge);
    }
    Ok(crate::saturate::check_divergence_certificate(
        certificate,
        presentation,
        consumed,
    ))
}

/// Map #59's detailed [`ModelError`] onto #52's coarse context-validation taxonomy.
///
/// The mapping is total and lossy by design: #52 consumers distinguish only whether a
/// context is unavailable, malformed, mismatched, unsupported or over budget.
pub fn resolution_context_error(e: &ModelError) -> crate::resolution::ContextValidationError {
    use crate::resolution::ContextValidationError as E;
    match e {
        ModelError::MissingParent(_) | ModelError::MissingArtifact(_) => E::Unavailable,
        ModelError::IdentityMismatch
        | ModelError::BindingMismatch
        | ModelError::ParentDimensionMismatch
        | ModelError::ModeMismatch
        | ModelError::TransportMismatch
        | ModelError::ArtifactMismatch(_) => E::Mismatched,
        ModelError::AssumptionRejected(_) | ModelError::MissingUsedAssumption(_) => E::Unsupported,
        ModelError::TooLarge
        | ModelError::TooManyAssumptions
        | ModelError::TooDeep
        | ModelError::ArtifactBudgetExceeded => E::Exhausted,
        ModelError::BadLength
        | ModelError::Malformed
        | ModelError::NonCanonicalOrder
        | ModelError::DuplicateName
        | ModelError::InvalidField => E::Malformed,
    }
}

/// Production [`crate::resolution::ContextValidator`]: backed by an admitted
/// [`ResolutionContextV2`], so a context is accepted only if it was fully validated by #59
/// and both identities equal the admitted context's identity.
pub struct AdmittedContextValidator<'a> {
    context: &'a ResolutionContextV2,
}

impl<'a> AdmittedContextValidator<'a> {
    pub fn new(context: &'a ResolutionContextV2) -> Self {
        Self { context }
    }
}

impl crate::resolution::ContextValidator for AdmittedContextValidator<'_> {
    fn validate_context(
        &self,
        expected: ContextId,
        actual: ContextId,
    ) -> Result<(), crate::resolution::ContextValidationError> {
        let id = self.context.checked().id();
        if expected == id && actual == id {
            Ok(())
        } else {
            Err(crate::resolution::ContextValidationError::Mismatched)
        }
    }
}
