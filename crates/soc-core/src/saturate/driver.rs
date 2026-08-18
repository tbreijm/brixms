//! The saturated driver and the system abstraction bisimulation walks
//! (ADR-0014 §9, Stage C).
//!
//! # `run_saturated` versus `run`
//!
//! [`crate::commit::run_reason`] drives the *unsaturated* engine: one γ-tick per
//! iteration, every committed step visible. [`run_saturated`] drives the
//! *saturated* one: one [`crate::saturate::sat_step`] per iteration, each
//! hiding a finite administrative prefix and exporting at most one observation.
//!
//! The difference that matters is the stop reason. `run` had exactly one exit
//! value for two situations — settled, and out of budget — which is the defect
//! ADR-0014 §1 opens with. `run_reason` fixed that for the unsaturated driver
//! with two variants. [`SaturatedStop`] carries the fix through to saturation
//! with three, and **every one of them is a claim about why stepping ended**:
//! certified quiescence, certified divergence, or an explicit `Unknown` that
//! establishes nothing. There is no fall-out-of-the-loop path.
//!
//! # What a run returns, and what it does not
//!
//! [`SaturatedRun::visible`] is the exported trace: one [`Observation`] per
//! saturated realizing step, administrative steps excluded by construction.
//! [`SaturatedRun::journal`] is the full committed log — administrative steps
//! included, because they are real committed evidence that must stay auditable
//! (⟨D-TAU⟩). Two runs with identical visible traces may have entirely
//! different journals; that is the point of saturation and a #61 non-goal, so
//! **never compare journals or chain digests to decide behavioral agreement**
//! (ADR-0014 risk 3).

use brix_canon::Digest;

use crate::commit::Observation;
use crate::cost::CostRecord;
use crate::exec::ExecConfig;
use crate::journal::Journal;

use super::certificate::{DivergenceCertificateV1, QuiescenceCertificateV1};
use super::{
    sat_step, DeclaredAssumptions, ObservationProfileId, PresentationIdV1, PresentationV1,
    SaturatedStep, SaturationBudget, SaturationUnknown,
};
use brix_semantic::ContextId;

/// Why a saturated run stopped.
///
/// Three variants, one per way stepping can end, and **no silent exit**. Two of
/// them carry certificates; the third carries an explicit reason and
/// establishes nothing. Compare [`crate::commit::UnsaturatedStop`], which does
/// the same job one layer down.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SaturatedStop {
    /// The run reached certified quiescence — the `1` summand of `F_O`.
    Quiescent(Box<QuiescenceCertificateV1>),
    /// The run reached a certified administrative lasso. `Unknown`-graded for
    /// the completion question; never `Refuted`, never the `1` summand.
    Divergent(Box<DivergenceCertificateV1>),
    /// A saturated step established nothing — including the whole-run visible
    /// budget running out ([`SaturationUnknown::VisibleBudgetExhausted`]).
    Unknown(SaturationUnknown),
}

impl SaturatedStop {
    /// Whether this stop is a decided negative — the **only** honest way to
    /// report "settled". Divergence and every `Unknown` return `false`.
    pub fn is_quiescent(&self) -> bool {
        matches!(self, SaturatedStop::Quiescent(_))
    }
}

/// One saturated run's full result.
#[derive(Clone, Debug)]
pub struct SaturatedRun {
    /// Every committed step, administrative and realizing alike, in order.
    pub journal: Journal,
    /// The exported visible trace: one observation per saturated realizing
    /// step. **Administrative observations never appear here.**
    pub visible: Vec<Observation>,
    /// One cost record per saturated step, including the step that stopped the
    /// run. `costs.len()` is `visible.len() + 1` unless the run took no steps.
    pub costs: Vec<CostRecord>,
    /// The configuration the run ended at.
    pub final_config: ExecConfig,
    /// Why it stopped.
    pub stop: SaturatedStop,
}

impl SaturatedRun {
    /// How many saturated realizing steps the run exported.
    pub fn visible_len(&self) -> usize {
        self.visible.len()
    }

    /// The journal's chain digest.
    ///
    /// **Not a behavioral identity.** Two weakly bisimilar runs have different
    /// journals whenever their administrative layouts differ, so this must
    /// never be used to decide agreement — only to check that *one* run
    /// replays deterministically (ADR-0014 risk 3).
    pub fn chain_digest(&self) -> Digest {
        self.journal.chain_digest()
    }
}

/// Drive saturation from `e0` until certified quiescence, certified divergence,
/// an explicit `Unknown`, or the whole-run visible budget.
///
/// Every committed step — administrative and realizing — is appended to the
/// journal in order, so the run stays fully auditable and replayable. The
/// calendar phase advances by one per *committed* step, matching
/// [`crate::commit::run`]'s convention, so a saturated run and an unsaturated
/// one over the same trajectory key their candidates identically.
pub fn run_saturated<F>(
    pres: &PresentationV1<'_>,
    e0: ExecConfig,
    keyer: &mut F,
    budget: SaturationBudget,
) -> SaturatedRun
where
    F: FnMut(&crate::witness_provider::Candidate, u64) -> crate::calendar::Key,
{
    let mut journal = Journal::new();
    let mut visible = Vec::new();
    let mut costs = Vec::new();
    let mut current = e0;
    let mut phase: u64 = 0;

    loop {
        let visible_steps = visible.len() as u64;
        if visible_steps >= budget.max_visible_steps {
            return SaturatedRun {
                journal,
                visible,
                costs,
                final_config: current,
                stop: SaturatedStop::Unknown(SaturationUnknown::VisibleBudgetExhausted {
                    visible_steps,
                    budget: budget.max_visible_steps,
                }),
            };
        }

        let (step, consumed, cost) = sat_step(pres, &current, phase, keyer, budget);
        phase = phase.saturating_add(consumed.len() as u64);
        for committed in consumed {
            journal.append(committed);
        }
        costs.push(cost);

        match step {
            SaturatedStep::Realizing {
                observation,
                successor,
                ..
            } => {
                visible.push(observation);
                current = successor;
            }
            SaturatedStep::Quiescent(certificate) => {
                return SaturatedRun {
                    journal,
                    visible,
                    costs,
                    final_config: current,
                    stop: SaturatedStop::Quiescent(certificate),
                }
            }
            SaturatedStep::Divergent(certificate) => {
                return SaturatedRun {
                    journal,
                    visible,
                    costs,
                    final_config: current,
                    stop: SaturatedStop::Divergent(certificate),
                }
            }
            SaturatedStep::Unknown(reason) => {
                return SaturatedRun {
                    journal,
                    visible,
                    costs,
                    final_config: current,
                    stop: SaturatedStop::Unknown(reason),
                }
            }
        }
    }
}

/// The observation boundary a system's results are bound to.
///
/// Two systems may be compared only if they share a `context` and a `profile`:
/// "same behavior" is meaningless across different observation boundaries, and
/// SOC-LAW-10's domain clause is exactly this restriction. Their
/// `presentation` revisions are *expected* to differ — comparing two revisions
/// is the point — so that field is carried into counterexamples rather than
/// required equal.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct SystemBoundary {
    /// The exact context.
    pub context: ContextId,
    /// The declared observation boundary.
    pub profile: ObservationProfileId,
    /// The program/world revision.
    pub presentation: PresentationIdV1,
}

/// A system whose saturated behavior can be walked one step at a time.
///
/// The abstraction [`super::bisimulation::check_saturated`] compares over.
/// Deliberately narrow: a walker needs a start state, a boundary, the declared
/// hypotheses, and a step function — and nothing else. In particular it does
/// **not** expose the journal, because journals are permitted to differ between
/// systems that agree behaviorally (#61 non-goal).
///
/// `saturated_step` returns the number of **committed** steps the saturated
/// step consumed, so the caller can advance the calendar phase the way
/// [`run_saturated`] does. Each system keeps its own phase: two systems with
/// different administrative layouts legitimately reach the same visible depth
/// at different phases.
pub trait SaturatedSystem {
    /// The configuration this system starts from.
    fn initial(&self) -> ExecConfig;

    /// The boundary this system's results are bound to.
    fn boundary(&self) -> SystemBoundary;

    /// The hypotheses this system's presentation declares.
    fn assumptions(&self) -> DeclaredAssumptions;

    /// One saturated step from `e` at `phase`, plus the committed-step count it
    /// consumed.
    fn saturated_step(&mut self, e: &ExecConfig, phase: u64) -> (SaturatedStep, u64);
}

/// The [`SaturatedSystem`] backed by a [`PresentationV1`] and [`sat_step`] —
/// the ordinary way to present a settlement system for comparison.
///
/// A differential fixture builds two of these over the same context and profile
/// with different candidate sources, and compares them under
/// [`super::bisimulation::Contract::Bisimilar`]. That is SOC-LAW-08's
/// naive-versus-incremental parity, stated at the saturated level: the two must
/// be *identical* in visible behavior, not merely related by refinement
/// (ADR-0014 §7.2, normative).
pub struct PresentedSystem<'a, F> {
    presentation: PresentationV1<'a>,
    initial: ExecConfig,
    keyer: F,
    budget: SaturationBudget,
}

impl<'a, F> PresentedSystem<'a, F>
where
    F: FnMut(&crate::witness_provider::Candidate, u64) -> crate::calendar::Key,
{
    /// Present `presentation` starting from `initial`, keyed by `keyer` under
    /// `budget`.
    pub fn new(
        presentation: PresentationV1<'a>,
        initial: ExecConfig,
        keyer: F,
        budget: SaturationBudget,
    ) -> Self {
        Self {
            presentation,
            initial,
            keyer,
            budget,
        }
    }

    /// The underlying presentation.
    pub fn presentation(&self) -> &PresentationV1<'a> {
        &self.presentation
    }

    /// Drive this system to a stop with [`run_saturated`].
    pub fn run(&mut self) -> SaturatedRun {
        run_saturated(
            &self.presentation,
            self.initial,
            &mut self.keyer,
            self.budget,
        )
    }
}

impl<F> SaturatedSystem for PresentedSystem<'_, F>
where
    F: FnMut(&crate::witness_provider::Candidate, u64) -> crate::calendar::Key,
{
    fn initial(&self) -> ExecConfig {
        self.initial
    }

    fn boundary(&self) -> SystemBoundary {
        SystemBoundary {
            context: self.presentation.context,
            profile: self.presentation.profile.id(),
            presentation: self.presentation.id,
        }
    }

    fn assumptions(&self) -> DeclaredAssumptions {
        self.presentation.assumptions
    }

    fn saturated_step(&mut self, e: &ExecConfig, phase: u64) -> (SaturatedStep, u64) {
        let (step, consumed, _) =
            sat_step(&self.presentation, e, phase, &mut self.keyer, self.budget);
        (step, consumed.len() as u64)
    }
}
