//! Divergence-sensitive weak bisimulation and directional refinement
//! (ADR-0014 §7, ⟨D-REF⟩; Stage C).
//!
//! # Why this is a walk, not a partition refinement
//!
//! `F_O` is **partial deterministic** (⟨D-FO⟩ Candidate A), so saturated
//! behavior is a partial *function*, not a relation. The usual bisimulation
//! clause — *for every move of one side there **exists** a matching move of the
//! other* — collapses: there is only ever one move. What remains is a lockstep
//! walk over pairs, and that is the whole algorithm.
//!
//! Three consequences follow, and each is load-bearing:
//!
//! 1. **Divergence-sensitivity is one clause.** The two sides must inhabit the
//!    same summand of `{O×X, 1, ↑}`. `↑` never matches `1`. That single
//!    requirement is the entire content of "divergence-sensitive", and
//!    [`MismatchKind::DivergenceVsQuiescence`] is the report it produces.
//! 2. **Minimality is free** (§7.3). Determinism means exactly one path leaves
//!    each start pair, so the visible prefix at the first mismatch *is* the
//!    unique shortest disagreeing visible trace — by construction, with no
//!    search, no breadth-first-by-length, and no shrinking. A counterexample
//!    here is minimal because it could not have been anything else.
//! 3. **The coinductive close is a set membership test.** Revisiting a pair of
//!    [`ObservableState`]s means both sides will repeat identically forever, so
//!    agreement holds coinductively and the walk stops with [`Holds`].
//!
//! [`Holds`]: SaturatedComparison::Holds
//!
//! # What is compared, and what is deliberately not
//!
//! Observations, summands, and successor states. **Never** journals, chain
//! digests, administrative step counts, costs, or the τ-traces themselves —
//! all of which two behaviorally-identical systems are permitted to differ on
//! (#61's explicit non-goal, and ADR-0014 risk 3, which warns that existing
//! fixtures train the reader toward chain equality).
//!
//! # The soundness precondition
//!
//! The coinductive close is sound only because [`project`] loses nothing that
//! affects behavior — that is P1 — and because keying does not drift with the
//! phase — that is P6. Either undeclared and the checker returns
//! [`ComparisonUnknown::UndeclaredAssumption`] rather than a verdict. ADR-0014
//! §7.3 states P1 normatively; P6 is required here for the same reason and by
//! the same argument, matching how [`super::sat_step`] already treats them as a
//! pair when closing a lasso.
//!
//! [`project`]: super::project
//! [`ObservableState`]: super::ObservableState

use std::collections::BTreeSet;

use brix_canon::{CanonWriter, Canonical, Digest, Domain};
use brix_semantic::ContextId;

use crate::commit::Observation;

use super::driver::SaturatedSystem;
use super::{
    project, AssumptionId, ObservableState, ObservationProfileId, PresentationIdV1, SaturatedStep,
    SaturationUnknown,
};

/// Which agreement two systems are being held to.
///
/// The direction is **a field of the result, never an implicit default**
/// (SOC-LAW-10, via ADR-0014 §7.2): a reader of a counterexample must be able
/// to tell which asymmetry was in force without consulting the call site.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Contract {
    /// Symmetric divergence-sensitive weak bisimulation: the two systems must
    /// agree on every summand and every observation.
    ///
    /// **This is the correct contract for SOC-LAW-08's naive-versus-incremental
    /// parity** (ADR-0014 §7.2, normative). The fast engine must be *identical*
    /// to the reference oracle, not merely a refinement of it.
    Bisimilar,
    /// Directional: the implementation refines the specification.
    ///
    /// With no committed nondeterminism the only asymmetry available is
    /// definedness. The specification's divergence imposes no obligation; its
    /// quiescence forbids the implementation from spinning. In a sentence:
    /// **replacing a loop with a stop is legal; replacing a stop with a loop is
    /// not.**
    ///
    /// Right when the specification is a *partial* reference — a reference
    /// oracle that loops administratively over a region the fast engine
    /// short-circuits.
    Refines,
}

impl Contract {
    /// Canonical ABI ordinal. Append-only; never reorder.
    pub const fn ordinal(self) -> u64 {
        match self {
            Contract::Bisimilar => 0,
            Contract::Refines => 1,
        }
    }
}

impl Canonical for Contract {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_uint(self.ordinal());
    }
}

/// Which summand of the saturated behavior a step landed in.
///
/// A projection of [`SaturatedStep`] that drops all payloads, so a
/// counterexample can name *where* the two sides diverged without embedding
/// certificates or successors.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Summand {
    /// `O × X` — a realizing step.
    Realizing,
    /// `1` — certified quiescence.
    Quiescent,
    /// `↑` — certified divergence.
    Divergent,
    /// `⊥` — nothing established.
    Unknown,
}

impl Summand {
    /// The summand a saturated step inhabits.
    pub fn of(step: &SaturatedStep) -> Self {
        match step {
            SaturatedStep::Realizing { .. } => Summand::Realizing,
            SaturatedStep::Quiescent(_) => Summand::Quiescent,
            SaturatedStep::Divergent(_) => Summand::Divergent,
            SaturatedStep::Unknown(_) => Summand::Unknown,
        }
    }

    /// Canonical ABI ordinal. Append-only; never reorder.
    pub const fn ordinal(self) -> u64 {
        match self {
            Summand::Realizing => 0,
            Summand::Quiescent => 1,
            Summand::Divergent => 2,
            Summand::Unknown => 3,
        }
    }
}

impl Canonical for Summand {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_uint(self.ordinal());
    }
}

/// How the two systems disagreed.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum MismatchKind {
    /// The two sides landed in different summands, in a way that is not the
    /// divergence-versus-quiescence case.
    SummandMismatch,
    /// Both realizing, but with different `O_min` observations.
    ObservationMismatch,
    /// One side certified divergence where the other certified quiescence.
    ///
    /// **The divergence-sensitivity clause**, and the reason
    /// `Build_Plan_v3_SOC.md` Step 8 demands a terminal state and an
    /// infinitely-searching state be distinguished. Under [`Contract::Refines`]
    /// this is reported only in the forbidden direction — a specification that
    /// stops against an implementation that spins; the reverse imposes no
    /// obligation and is not a mismatch at all.
    DivergenceVsQuiescence,
}

impl MismatchKind {
    /// Canonical ABI ordinal. Append-only; never reorder.
    pub const fn ordinal(self) -> u64 {
        match self {
            MismatchKind::SummandMismatch => 0,
            MismatchKind::ObservationMismatch => 1,
            MismatchKind::DivergenceVsQuiescence => 2,
        }
    }
}

impl Canonical for MismatchKind {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_uint(self.ordinal());
    }
}

/// The unique shortest disagreeing visible trace, with the disagreement.
///
/// `visible_prefix` holds **observations only** — never administrative steps,
/// per #61's non-goal. Two implementations with entirely different τ-layouts
/// that disagree at the same visible depth produce the *same* counterexample,
/// which is exactly right: the τ-layout is not part of the behavior being
/// compared.
///
/// That exclusion is also what lets this type have a stable canonical identity
/// at all. Replay aids — journals, chain digests, costs, hidden-step counts —
/// are deliberately absent from both the struct and the encoding.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SaturatedCounterexample {
    /// Which agreement was in force.
    pub contract: Contract,
    /// The shared context.
    pub context: ContextId,
    /// The shared observation boundary.
    pub profile: ObservationProfileId,
    /// The implementation's revision.
    pub implementation: PresentationIdV1,
    /// The specification's revision. Under [`Contract::Bisimilar`] the roles
    /// are symmetric and this is simply the other system.
    pub specification: PresentationIdV1,
    /// The agreed observations preceding the disagreement — minimal by
    /// construction (§7.3). Its length is the visible depth of the mismatch.
    pub visible_prefix: Vec<Observation>,
    /// The summand the implementation landed in.
    pub implementation_summand: Summand,
    /// The summand the specification landed in.
    pub specification_summand: Summand,
    /// How they disagreed.
    pub kind: MismatchKind,
}

impl SaturatedCounterexample {
    /// The visible depth at which the two systems first disagreed.
    pub fn visible_depth(&self) -> usize {
        self.visible_prefix.len()
    }

    /// The canonical preimage. Frozen field order.
    pub fn canon_preimage(&self) -> Vec<u8> {
        let mut w = CanonWriter::new();
        w.write_bytes(b"brix.soc.saturated-counterexample");
        w.write_uint(1);
        self.contract.canon_write(&mut w);
        w.write_bytes(self.context.digest().as_bytes());
        w.write_bytes(self.profile.digest().as_bytes());
        w.write_bytes(self.implementation.digest().as_bytes());
        w.write_bytes(self.specification.digest().as_bytes());
        w.write_list(self.visible_prefix.iter().map(|o| o.canon_bytes()));
        self.implementation_summand.canon_write(&mut w);
        self.specification_summand.canon_write(&mut w);
        self.kind.canon_write(&mut w);
        w.finish()
    }

    /// The content-addressed identity of this counterexample.
    pub fn digest(&self) -> Digest {
        Digest::of(Domain::Value, &self.canon_preimage())
    }
}

/// Why a comparison established nothing.
///
/// Never a pass, never a counterexample, never `Refuted`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ComparisonUnknown {
    /// One of the systems does not declare a hypothesis the coinductive close
    /// depends on.
    UndeclaredAssumption(AssumptionId),
    /// The two systems are bound to different contexts or observation
    /// profiles, so "same behavior" has no meaning between them.
    BoundaryMismatch,
    /// The implementation established nothing at some visible depth, so there
    /// is nothing to compare against.
    ImplementationUnknown {
        /// The visible depth reached before this.
        visible_depth: u64,
        /// Why the step established nothing.
        reason: SaturationUnknown,
    },
    /// The specification established nothing at some visible depth.
    SpecificationUnknown {
        /// The visible depth reached before this.
        visible_depth: u64,
        /// Why the step established nothing.
        reason: SaturationUnknown,
    },
    /// The pair-walk budget ran out before the walk closed or disagreed.
    PairBudgetExhausted {
        /// Distinct state pairs visited.
        pairs: u64,
        /// The bound that was hit.
        budget: u64,
    },
}

/// The result of holding two systems to a contract.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SaturatedComparison {
    /// The contract holds over every reachable pair.
    ///
    /// For a walk that terminated by revisiting a pair, this is a *coinductive*
    /// conclusion: the two systems repeat identically forever.
    Holds {
        /// Visible steps the walk agreed on.
        visible_steps: u64,
        /// Distinct state pairs visited.
        pairs: u64,
    },
    /// The contract fails, with the unique shortest disagreeing visible trace.
    Fails(Box<SaturatedCounterexample>),
    /// Neither established. Never a pass, never `Refuted`.
    Unknown(ComparisonUnknown),
}

impl SaturatedComparison {
    /// Whether the contract was established.
    pub fn holds(&self) -> bool {
        matches!(self, SaturatedComparison::Holds { .. })
    }

    /// The counterexample, if the contract failed.
    pub fn counterexample(&self) -> Option<&SaturatedCounterexample> {
        match self {
            SaturatedComparison::Fails(cx) => Some(cx),
            _ => None,
        }
    }
}

/// Hold `implementation` and `specification` to `contract`.
///
/// Under [`Contract::Bisimilar`] the two roles are symmetric and the argument
/// order is immaterial. Under [`Contract::Refines`] it is not: the claim is
/// `implementation ⊑ specification`.
///
/// Walks pairs in lockstep from both initial configurations, closing
/// coinductively when a pair of [`ObservableState`]s repeats and stopping at
/// the first disagreement. `max_pairs` bounds the walk; hitting it is an
/// explicit `Unknown`, never a pass.
///
/// [`ObservableState`]: super::ObservableState
pub fn check_saturated(
    implementation: &mut dyn SaturatedSystem,
    specification: &mut dyn SaturatedSystem,
    contract: Contract,
    max_pairs: u64,
) -> SaturatedComparison {
    let impl_boundary = implementation.boundary();
    let spec_boundary = specification.boundary();
    if impl_boundary.context != spec_boundary.context
        || impl_boundary.profile != spec_boundary.profile
    {
        return SaturatedComparison::Unknown(ComparisonUnknown::BoundaryMismatch);
    }

    // The coinductive close is only sound under both hypotheses — see the
    // module docs. Fail closed before walking a single step.
    for assumption in [
        AssumptionId::HistoryIndependence,
        AssumptionId::PhaseStableKeying,
    ] {
        if !implementation.assumptions().declares(assumption)
            || !specification.assumptions().declares(assumption)
        {
            return SaturatedComparison::Unknown(ComparisonUnknown::UndeclaredAssumption(
                assumption,
            ));
        }
    }

    let mut seen: BTreeSet<(ObservableState, ObservableState)> = BTreeSet::new();
    let mut visible_prefix: Vec<Observation> = Vec::new();
    let mut impl_config = implementation.initial();
    let mut spec_config = specification.initial();
    let mut impl_phase: u64 = 0;
    let mut spec_phase: u64 = 0;

    let fail = |kind: MismatchKind,
                visible_prefix: &[Observation],
                implementation_summand: Summand,
                specification_summand: Summand| {
        SaturatedComparison::Fails(Box::new(SaturatedCounterexample {
            contract,
            context: impl_boundary.context,
            profile: impl_boundary.profile,
            implementation: impl_boundary.presentation,
            specification: spec_boundary.presentation,
            visible_prefix: visible_prefix.to_vec(),
            implementation_summand,
            specification_summand,
            kind,
        }))
    };

    loop {
        let pair = (project(&impl_config), project(&spec_config));
        if !seen.insert(pair) {
            // Both sides have been here before, and both are deterministic, so
            // from here they repeat exactly what they already agreed on.
            return SaturatedComparison::Holds {
                visible_steps: visible_prefix.len() as u64,
                pairs: seen.len() as u64,
            };
        }
        if seen.len() as u64 > max_pairs {
            return SaturatedComparison::Unknown(ComparisonUnknown::PairBudgetExhausted {
                pairs: seen.len() as u64,
                budget: max_pairs,
            });
        }

        let (impl_step, impl_consumed) = implementation.saturated_step(&impl_config, impl_phase);
        let (spec_step, spec_consumed) = specification.saturated_step(&spec_config, spec_phase);
        impl_phase = impl_phase.saturating_add(impl_consumed);
        spec_phase = spec_phase.saturating_add(spec_consumed);

        let visible_depth = visible_prefix.len() as u64;
        let impl_summand = Summand::of(&impl_step);
        let spec_summand = Summand::of(&spec_step);

        // `⊥` on either side is `Unknown` for both contracts (§7.1 clause 3):
        // a step that established nothing cannot witness agreement *or*
        // disagreement.
        if let SaturatedStep::Unknown(reason) = &impl_step {
            return SaturatedComparison::Unknown(ComparisonUnknown::ImplementationUnknown {
                visible_depth,
                reason: reason.clone(),
            });
        }
        if let SaturatedStep::Unknown(reason) = &spec_step {
            return SaturatedComparison::Unknown(ComparisonUnknown::SpecificationUnknown {
                visible_depth,
                reason: reason.clone(),
            });
        }

        // Under `Refines`, a diverging specification imposes no obligation
        // whatever, so the walk ends satisfied without inspecting the
        // implementation's summand at all (§7.2). This clause is checked before
        // summand agreement precisely because it is the one place the two
        // contracts genuinely differ.
        if contract == Contract::Refines && spec_summand == Summand::Divergent {
            return SaturatedComparison::Holds {
                visible_steps: visible_prefix.len() as u64,
                pairs: seen.len() as u64,
            };
        }

        match (&impl_step, &spec_step) {
            (
                SaturatedStep::Realizing {
                    observation: impl_observation,
                    successor: impl_successor,
                    ..
                },
                SaturatedStep::Realizing {
                    observation: spec_observation,
                    successor: spec_successor,
                    ..
                },
            ) => {
                if impl_observation != spec_observation {
                    return fail(
                        MismatchKind::ObservationMismatch,
                        &visible_prefix,
                        impl_summand,
                        spec_summand,
                    );
                }
                visible_prefix.push(*impl_observation);
                impl_config = *impl_successor;
                spec_config = *spec_successor;
            }
            (SaturatedStep::Quiescent(_), SaturatedStep::Quiescent(_)) => {
                return SaturatedComparison::Holds {
                    visible_steps: visible_prefix.len() as u64,
                    pairs: seen.len() as u64,
                }
            }
            (SaturatedStep::Divergent(_), SaturatedStep::Divergent(_)) => {
                return SaturatedComparison::Holds {
                    visible_steps: visible_prefix.len() as u64,
                    pairs: seen.len() as u64,
                }
            }
            // The divergence-sensitivity clause, in both directions under
            // `Bisimilar` and in the forbidden direction under `Refines` (the
            // permitted direction returned `Holds` above).
            (SaturatedStep::Divergent(_), SaturatedStep::Quiescent(_))
            | (SaturatedStep::Quiescent(_), SaturatedStep::Divergent(_)) => {
                return fail(
                    MismatchKind::DivergenceVsQuiescence,
                    &visible_prefix,
                    impl_summand,
                    spec_summand,
                )
            }
            _ => {
                return fail(
                    MismatchKind::SummandMismatch,
                    &visible_prefix,
                    impl_summand,
                    spec_summand,
                )
            }
        }
    }
}
