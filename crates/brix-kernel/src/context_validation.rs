//! Kernel-owned consumer for materialized semantic contexts (context model v2).
//!
//! Context proofs use a distinct verifier identity and certificate domain. The
//! legacy `acceptance` / certificate-v1 ABI is intentionally unchanged.

use brix_canon::{CanonReader, CanonWriter, Canonical};
use brix_semantic::{
    validate_context_model_v2, AssumptionValidatorV2, CheckedContextV2, ContextDimensionsV2,
    ContextId, ContextResolverV2, ModelError, ModelLimits, PropositionId, ScenarioModeV2,
    VerifierId,
};

use crate::check::acceptance_with_gamma;
use crate::term::{ExplicitTerm, ObjectTerm, Prop};
use crate::{Budget, Verdict, KERNEL_PROFILE_V1};

/// Schema tag accepted by the built-in validator for canonical kernel propositions.
pub const KERNEL_PROP_SCHEMA_V1: &str = "brix.kernel.prop@1";
/// Regime profile whose assumptions use the proof kernel's closed proposition schema.
pub const PROOF_REGIME_PROFILE_V1: &str = "brix.proof@1";
/// Axiom profile supported by this consumer; all proof premises are explicit context facts.
pub const NO_AXIOMS_PROFILE_V1: &str = "none";
/// Separate verifier identity for certificates that rely on context assumptions.
pub const CONTEXT_VERIFIER_NAME_V1: &str = "brix.kernel.context@1";
const CONTEXT_CERTIFICATE_MARKER_V1: &[u8] = b"brix.kernel.context-certificate";
const MAX_PROP_BYTES: usize = 4_096;
const MAX_PROP_DEPTH: usize = 128;
const MAX_APPLIED_ARGS: usize = 256;

/// A result tied to the semantically checked context that supplied Gamma.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextAcceptance {
    checked_context: CheckedContextV2,
    verdict: Verdict,
}

impl ContextAcceptance {
    pub fn checked_context(&self) -> &CheckedContextV2 {
        &self.checked_context
    }
    pub fn verdict(&self) -> &Verdict {
        &self.verdict
    }
    pub fn into_verdict(self) -> Verdict {
        self.verdict
    }
}

/// Failure while validating the context before proof checking starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContextAcceptanceError {
    Model(ModelError),
    UnsupportedKernelProfile,
    UnsupportedRegimeProfile,
    UnsupportedAxiomProfile,
    ModeMismatch,
}

/// Revalidate materialized v2 bytes against caller-supplied identity and all
/// expected dimensions, then type-check the proof with validated named Gamma.
#[allow(clippy::too_many_arguments)]
pub fn acceptance_in_context<R: ContextResolverV2>(
    bytes: &[u8],
    expected_id: ContextId,
    expected_dimensions: &ContextDimensionsV2,
    requested_mode: ScenarioModeV2,
    limits: ModelLimits,
    resolver: &R,
    proposition: &Prop,
    term: &ExplicitTerm,
    budget: Budget,
) -> Result<ContextAcceptance, ContextAcceptanceError> {
    if expected_dimensions.kernel_profile != KERNEL_PROFILE_V1 {
        return Err(ContextAcceptanceError::UnsupportedKernelProfile);
    }
    if expected_dimensions.regime_profile != PROOF_REGIME_PROFILE_V1 {
        return Err(ContextAcceptanceError::UnsupportedRegimeProfile);
    }
    if expected_dimensions.axiom_profile != NO_AXIOMS_PROFILE_V1 {
        return Err(ContextAcceptanceError::UnsupportedAxiomProfile);
    }
    if expected_dimensions.scenario_mode != requested_mode {
        return Err(ContextAcceptanceError::ModeMismatch);
    }
    let checked = validate_context_model_v2(
        bytes,
        expected_id,
        expected_dimensions,
        limits,
        resolver,
        &KernelAssumptionValidator,
    )
    .map_err(ContextAcceptanceError::Model)?;

    let id = checked.id();
    let gamma = checked
        .assumptions_in_scope()
        .into_iter()
        .map(|(name, assumption)| {
            // Validation above guarantees this exact schema and a bounded Prop.
            let prop = decode_prop(&assumption.canonical_value)
                .expect("validated kernel assumption must decode identically");
            (Some(name), prop)
        })
        .collect();
    let verdict = acceptance_with_gamma(
        &id,
        proposition,
        term,
        effective_budget(budget, expected_dimensions),
        gamma,
        context_verifier(),
        || context_certificate_id(&id, proposition, term),
    );
    Ok(ContextAcceptance {
        checked_context: checked,
        verdict,
    })
}

/// Kernel-owned semantic typing hook. Values must be exact canonical Prop bytes.
pub struct KernelAssumptionValidator;
impl AssumptionValidatorV2 for KernelAssumptionValidator {
    fn validate(&self, schema: &str, value: &[u8], budget: &mut usize) -> bool {
        if schema != KERNEL_PROP_SCHEMA_V1 || value.len() > MAX_PROP_BYTES || value.len() > *budget
        {
            return false;
        }
        *budget -= value.len();
        decode_prop(value).is_some()
    }
}

fn effective_budget(host: Budget, dimensions: &ContextDimensionsV2) -> Budget {
    Budget::new(
        host.max_steps
            .min(usize::try_from(dimensions.step_budget).unwrap_or(usize::MAX)),
        host.max_depth
            .min(usize::try_from(dimensions.depth_budget).unwrap_or(usize::MAX)),
    )
}

fn context_verifier() -> VerifierId {
    VerifierId::named(CONTEXT_VERIFIER_NAME_V1)
}

fn context_certificate_id(
    context: &ContextId,
    proposition: &Prop,
    term: &ExplicitTerm,
) -> brix_semantic::CertificateId {
    let mut w = CanonWriter::new();
    w.write_bytes(CONTEXT_CERTIFICATE_MARKER_V1);
    w.write_uint(1);
    w.write_str(KERNEL_PROFILE_V1);
    w.write_bytes(context.0.as_bytes());
    w.write_bytes(&proposition.canon_bytes());
    w.write_bytes(&term.canon_bytes());
    brix_semantic::CertificateId::from_canon(&w.finish())
}

fn decode_prop(bytes: &[u8]) -> Option<Prop> {
    if bytes.len() > MAX_PROP_BYTES {
        return None;
    }
    let mut r = CanonReader::new(bytes);
    let p = read_prop(&mut r, 0, 0)?;
    if !r.is_empty() || p.canon_bytes() != bytes {
        return None;
    }
    Some(p)
}

fn read_digest(r: &mut CanonReader<'_>) -> Option<[u8; 32]> {
    r.read_bytes().ok()?.try_into().ok()
}
fn read_id(r: &mut CanonReader<'_>) -> Option<PropositionId> {
    Some(PropositionId(brix_canon::Digest::from_bytes(read_digest(
        r,
    )?)))
}

fn read_obj(r: &mut CanonReader<'_>, depth: usize, binders: usize) -> Option<ObjectTerm> {
    if depth >= MAX_PROP_DEPTH {
        return None;
    }
    Some(match r.read_uint().ok()? {
        0 => ObjectTerm::Const(read_id(r)?),
        1 => {
            let n = usize::try_from(r.read_uint().ok()?).ok()?;
            if n >= binders {
                return None;
            }
            ObjectTerm::BoundVar(n)
        }
        2 => ObjectTerm::Compose(
            Box::new(read_obj(r, depth + 1, binders)?),
            Box::new(read_obj(r, depth + 1, binders)?),
        ),
        3 => ObjectTerm::Tensor(
            Box::new(read_obj(r, depth + 1, binders)?),
            Box::new(read_obj(r, depth + 1, binders)?),
        ),
        4 => ObjectTerm::Id(Box::new(read_obj(r, depth + 1, binders)?)),
        _ => return None,
    })
}

fn read_prop(r: &mut CanonReader<'_>, depth: usize, binders: usize) -> Option<Prop> {
    if depth >= MAX_PROP_DEPTH {
        return None;
    }
    let d = depth + 1;
    Some(match r.read_uint().ok()? {
        0 => Prop::Atom(read_id(r)?),
        1 => Prop::Impl(
            Box::new(read_prop(r, d, binders)?),
            Box::new(read_prop(r, d, binders)?),
        ),
        2 => Prop::Prod(
            Box::new(read_prop(r, d, binders)?),
            Box::new(read_prop(r, d, binders)?),
        ),
        3 => Prop::Sum(
            Box::new(read_prop(r, d, binders)?),
            Box::new(read_prop(r, d, binders)?),
        ),
        4 => Prop::Eq(read_obj(r, d, binders)?, read_obj(r, d, binders)?),
        5 => Prop::Exists(Box::new(read_prop(r, d, binders + 1)?)),
        6 => {
            let sym = read_id(r)?;
            let n = usize::try_from(r.read_uint().ok()?).ok()?;
            if n > MAX_APPLIED_ARGS {
                return None;
            }
            let mut args = Vec::with_capacity(n);
            for _ in 0..n {
                args.push(read_obj(r, d, binders)?);
            }
            Prop::Applied(sym, args)
        }
        7 => Prop::Realizes(
            read_obj(r, d, binders)?,
            read_obj(r, d, binders)?,
            read_obj(r, d, binders)?,
        ),
        8 => Prop::Preserves(
            read_obj(r, d, binders)?,
            Box::new(read_prop(r, d, binders)?),
        ),
        _ => return None,
    })
}
