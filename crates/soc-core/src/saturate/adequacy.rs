//! The CJ-1 adequacy interface (ADR-0002 §10; ADR-0014 §9, Stage D).
//!
//! # What this module is, and firmly is not
//!
//! ADR-0002 §10's conjecture **CJ-1** speaks of a world *"whose **saturated**
//! realizing semantics is an `F_O`-coalgebra"*. #61's job was never to prove
//! that — proving it is `Build_Plan_v3_SOC.md` Step 12. #61's job was to build
//! **the interface CJ-1 will be stated against**, and this module states it
//! explicitly so a later proof has a fixed target rather than a moving one.
//!
//! Nothing here searches for proofs, elaborates a program, or renders anything.
//!
//! # The interface, stated
//!
//! The saturated settlement interface `sat = ` [`super::sat_step`] is adequate
//! for CJ-1 in exactly this sense:
//!
//! 1. **Total.** `sat` returns a value for every `(presentation, config, phase,
//!    budget)`. There is no input on which it fails to answer, and no partial
//!    function to lift.
//! 2. **Effective.** Every call terminates. The administrative loop is bounded
//!    three ways ([`super::SaturationBudget`]) and lasso detection closes the
//!    remaining case, so there is no input that runs forever.
//! 3. **Returns the encoded `F_O`-structure.** The answer is a
//!    [`super::SaturatedStep`], whose first two summands are exactly `O × X`
//!    and `1` — the two summands of `F_O` — and whose remaining two are
//!    explicitly outside it.
//! 4. **Explicit certificates on the decided summands.** The `1` summand
//!    carries a [`super::QuiescenceCertificateV1`] and certified divergence a
//!    [`super::DivergenceCertificateV1`], each independently re-derivable by
//!    its checker. A decided answer is never asserted on the engine's authority
//!    alone.
//! 5. **Honest `⊥`.** Everything else is [`super::SaturationUnknown`], with a
//!    reason. Never a pass, never `Refuted`, never silence.
//!
//! # The sub-carrier, made computable
//!
//! ADR-0014 §5 says it plainly: *"the `F_O`-coalgebra is defined exactly on the
//! sub-carrier where this returns an `F_O`-value — that partiality is the honest
//! content of the interface."*
//!
//! That sentence is the whole of CJ-1's difficulty, so this module makes the
//! sub-carrier something you can *compute* rather than something you reason
//! about in prose. [`fo_definedness`] decides, for one saturated step, whether
//! the coalgebra is defined there — and the two ways it can be undefined are
//! kept apart, because they are not the same fact:
//!
//! - [`FoUndefined::CertifiedDivergence`] is a **positive finite observation of
//!   an infinite behavior**. The system genuinely has no `F_O`-value here, and
//!   we know it.
//! - [`FoUndefined::Unestablished`] means we simply did not find out. The
//!   system may well have an `F_O`-value here.
//!
//! A future CJ-1 statement must quantify over the first and exclude the second.
//! Collapsing them would let a resource limit masquerade as a semantic fact,
//! which is the confusion ADR-0014 exists to remove.

use super::driver::{SaturatedRun, SaturatedStop};
use super::SaturatedStep;

/// An `F_O`-value: the coalgebra is defined at this state.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum FoValue {
    /// The `O × X` summand — one observation and a successor.
    Realizing,
    /// The `1` summand, certified.
    Quiescent,
}

/// Why the coalgebra is not defined at a state.
///
/// The two variants are **not interchangeable**, and keeping them apart is the
/// point of the whole ADR: one is knowledge, the other is its absence.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum FoUndefined {
    /// A certified administrative lasso. The system has no `F_O`-value here and
    /// that is a *fact about the system*, established by a checkable
    /// certificate.
    CertifiedDivergence,
    /// Nothing was established. A *fact about the analysis*, not about the
    /// system: an `F_O`-value may well exist here.
    Unestablished,
}

/// Whether the `F_O`-coalgebra is defined at a state.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum FoDefinedness {
    /// Inside the sub-carrier.
    Defined(FoValue),
    /// Outside it.
    Undefined(FoUndefined),
}

impl FoDefinedness {
    /// Whether this state is in the `F_O` sub-carrier.
    pub fn is_defined(&self) -> bool {
        matches!(self, FoDefinedness::Defined(_))
    }
}

/// Decide whether one saturated step lies in the `F_O` sub-carrier.
///
/// Total by construction: every [`SaturatedStep`] variant maps somewhere, which
/// is interface property 1 discharged at the type level.
pub fn fo_definedness(step: &SaturatedStep) -> FoDefinedness {
    match step {
        SaturatedStep::Realizing { .. } => FoDefinedness::Defined(FoValue::Realizing),
        SaturatedStep::Quiescent(_) => FoDefinedness::Defined(FoValue::Quiescent),
        SaturatedStep::Divergent(_) => FoDefinedness::Undefined(FoUndefined::CertifiedDivergence),
        SaturatedStep::Unknown(_) => FoDefinedness::Undefined(FoUndefined::Unestablished),
    }
}

/// Where a whole run stood relative to the `F_O` sub-carrier.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct AdequacyReport {
    /// The definedness of the step that ended the run.
    pub outcome: FoDefinedness,
    /// Whether the run stayed inside the sub-carrier from start to stop.
    ///
    /// A run only continues on a realizing step, so every step before the stop
    /// is already an `F_O`-value; this is therefore decided entirely by the
    /// stop.
    pub defined_throughout: bool,
    /// The visible depth at which the run left the sub-carrier, if it did.
    pub left_at_visible_depth: Option<u64>,
}

/// Classify a finished run against the `F_O` sub-carrier.
pub fn adequacy_of(run: &SaturatedRun) -> AdequacyReport {
    let outcome = match &run.stop {
        SaturatedStop::Quiescent(_) => FoDefinedness::Defined(FoValue::Quiescent),
        SaturatedStop::Divergent(_) => FoDefinedness::Undefined(FoUndefined::CertifiedDivergence),
        SaturatedStop::Unknown(_) => FoDefinedness::Undefined(FoUndefined::Unestablished),
    };
    let defined_throughout = outcome.is_defined();
    AdequacyReport {
        outcome,
        defined_throughout,
        left_at_visible_depth: if defined_throughout {
            None
        } else {
            Some(run.visible.len() as u64)
        },
    }
}
