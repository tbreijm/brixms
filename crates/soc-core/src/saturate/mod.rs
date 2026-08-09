//! Divergence-sensitive saturation — Stage A (ADR-0014 §9, tracked by #61).
//!
//! # What saturation is, and where it sits
//!
//! `commit_tick` is the underlying transition `γ`. Its [`Committed`] value is
//! `D_O`-shaped only because the unsaturated system happens to have no
//! administrative work. **`F_O` is the functor of the *saturated* behavior, not
//! of the immediate step** (ADR-0002 §1.3; CJ-1 speaks of a world "whose
//! *saturated* realizing semantics is an `F_O`-coalgebra").
//!
//! This module layers *above* `γ`: [`sat_step`] consumes a run of γ-ticks and
//! exports exactly one [`SaturatedStep`]. Nothing here changes `F_O`,
//! `O_min`, `select_K`, the calendar key, [`Committed`], [`Observation`], or
//! the [`CommittedStep`]/[`Journal`] ABI (ADR-0014 §12).
//!
//! # τ is a declared projection, not a new summand (⟨D-TAU⟩)
//!
//! Every step the calendar commits — administrative or realizing — is a full
//! `Committed::Step`: it carries a real `Observation`, mints its `Derived`
//! judgement through `try_commit_selected` alone, and is appended to the
//! journal. [`StepLabel::Administrative`] means only that the declared
//! observation boundary does **not export** that step's observation across
//! saturation. It does not mean the step was uncommitted, unjournaled,
//! ungraded, or unauditable.
//!
//! Two consequences worth internalizing: the **same journal under two profiles
//! has two visible traces and one identical committed trace**; and a profile
//! whose administrative partition is empty makes saturation degenerate exactly
//! to today's behavior, so every existing regime keeps its current meaning.
//!
//! # Divergence, and what a lasso is worth (Stage B)
//!
//! [`sat_step`] now detects administrative lassos: if the orbit returns to an
//! [`ObservableState`] it has already visited, it never reaches a realizing
//! step, and that is certified as [`SaturatedStep::Divergent`] — a fourth
//! summand, `Unknown`-graded for the completion question and **never**
//! `Refuted`, **never** the `1` summand (⟨D-DIV⟩).
//!
//! A lasso is a divergence proof only under two hypotheses the presentation
//! *declares* and this module *bounded-checks*, never proves: **P1**, that
//! candidate enumeration and admissibility read the config only through
//! [`project`]; and **P6**, that keying does not read the calendar phase.
//! Without both, returning to the same `(world, policy)` says nothing about
//! what the engine will do next. So the certificate records which mode it was
//! minted under, an undeclared hypothesis yields
//! [`SaturationUnknown::UndeclaredAssumption`] rather than a certificate, and a
//! declaration the bounded check falsifies yields
//! [`SaturationUnknown::AssumptionViolated`].
//!
//! Budget exhaustion remains what it always was: an honest "we do not know",
//! structurally distinct from certified divergence. Distinguishing the two is
//! the whole point of ADR-0014 §5.1.
//!
//! # Driving and comparing (Stage C)
//!
//! [`driver::run_saturated`] drives saturation to a stop, and every stop is a
//! *claim*: certified quiescence, certified divergence, or an explicit
//! `Unknown`. [`bisimulation::check_saturated`] holds two
//! [`driver::SaturatedSystem`]s to a [`bisimulation::Contract`] — symmetric
//! weak bisimulation, or directional refinement whose sole asymmetry is that a
//! specification's divergence imposes no obligation while its quiescence
//! forbids the implementation from spinning.
//!
//! Because `F_O` is deterministic, comparison is a lockstep walk and its
//! counterexample is the *unique shortest* disagreeing visible trace, with no
//! search and no shrinking.
//!
//! # Safety and adequacy (Stage D)
//!
//! [`closure::check_closure`] decides one-step closure for a safety predicate,
//! under a [`closure::ClosureMode`] the caller must state: `Visible` constrains
//! only what an observer at the boundary can see, `Raw` constrains every
//! committed γ-state. The two genuinely differ, which is the operational proof
//! that saturation hides something semantically consequential.
//!
//! [`adequacy`] states the CJ-1 adequacy interface — total, effective, returns
//! the encoded `F_O`-structure, explicit certificates, honest `⊥` — and makes
//! the `F_O` sub-carrier computable via [`adequacy::fo_definedness`]. It does
//! **not** prove CJ-1; that is Build Plan Step 12.

use std::collections::{BTreeMap, BTreeSet};

use brix_canon::{CanonWriter, Canonical, Digest, Domain};
use brix_semantic::{ConfigId, ContextId, GeneratorId, Outcome};

use crate::adm::Adm;
use crate::commit::{
    try_commit_tick, CommitError, CommitTickError, Committed, Observation, SettlementRegime,
};
use crate::cost::CostRecord;
use crate::exec::ExecConfig;
use crate::intern::{Handle, Interner};
use crate::journal::CommittedStep;
use crate::regime::Candidate;

pub mod adequacy;
pub mod bisimulation;
pub mod certificate;
pub mod closure;
pub mod driver;

pub use adequacy::{
    adequacy_of, fo_definedness, AdequacyReport, FoDefinedness, FoUndefined, FoValue,
};
pub use bisimulation::{
    check_saturated, ComparisonUnknown, Contract, MismatchKind, SaturatedComparison,
    SaturatedCounterexample, Summand,
};
pub use closure::{
    check_closure, ClosureMode, ClosureResult, ClosureUnknown, ClosureViolation, SafetyState,
    ViolationSite,
};
pub use driver::{
    run_saturated, PresentedSystem, SaturatedRun, SaturatedStop, SaturatedSystem, SystemBoundary,
};

pub use certificate::{
    check_divergence_certificate, check_quiescence_certificate, decode_divergence_v1,
    decode_quiescence_v1, divergence_certificate_id, encode_divergence_v1, encode_quiescence_v1,
    quiescence_certificate_id, quiescence_judgement, validate_divergence_v1,
    validate_quiescence_v1, AssumptionMode, CertEnvelopeError, CertificateCheck,
    CertificateCheckError, DivergenceCertificateId, DivergenceCertificateV1,
    EnumerationCompleteness, QuiescenceCertificateId, QuiescenceCertificateV1,
    CERTIFICATE_FORMAT_V1, DIVERGENCE_MARKER, QUIESCENCE_MARKER, SATURATION_PROFILE_V1,
};

/// Whether a committed step is visible at the declared observation boundary.
///
/// A property of the step under one profile, never intrinsic to the step: the
/// same journal classified by two profiles yields two different visible traces.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum StepLabel {
    /// `τ`. Committed, journaled, `Derived`, auditable — but its `Observation`
    /// is not exported across the saturation boundary.
    Administrative,
    /// `o ∈ O`. Its `Observation` is the exported `O_min` value.
    Realizing,
}

/// Why a profile could not classify a committed step.
///
/// Fail closed: a profile never defaults to `Realizing` (which would fabricate
/// an observation) nor to `Administrative` (which would silently hide one).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ProfileError {
    /// The step's decomposition mixes generators from both partitions, so the
    /// step is neither wholly administrative nor wholly realizing.
    MixedDecomposition,
    /// A generator in the step's decomposition is in neither partition.
    UnregisteredGenerator,
    /// The decomposition had no generators. Defensive: `Decomposition`'s
    /// constructor already forbids this, and `try_commit_selected` rejects it
    /// as [`CommitError::EmptyDecomposition`].
    EmptyDecomposition,
}

/// Canonical identity of an observation profile.
///
/// Opaque everywhere it appears in a certificate or counterexample. #59 may
/// define richer profiles under new kind strings; it MUST NOT reinterpret the
/// v1 generator-partition preimage (ADR-0014 §4.1).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ObservationProfileId(pub Digest);

impl ObservationProfileId {
    /// Hash a canon-encoded profile payload under the value domain.
    pub fn from_canon(payload: &[u8]) -> Self {
        Self(Digest::of(Domain::Value, payload))
    }

    /// The underlying digest.
    pub fn digest(&self) -> Digest {
        self.0
    }

    /// Lowercase-hex rendering (diagnostics).
    pub fn to_hex(&self) -> String {
        self.0.to_hex()
    }
}

impl Canonical for ObservationProfileId {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(self.0.as_bytes());
    }
}

/// Opaque canonical identity of the program/world revision a saturated run is
/// taken against.
///
/// `soc-core` has no lowering dependency and therefore **cannot compute this**;
/// the caller supplies it and MUST derive it from canonical artifacts
/// (ADR-0012's `ProgramIdV1` qualifies) and never from source text, file paths,
/// or interner handles. soc-core cannot enforce that — see ADR-0014 §11.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct PresentationIdV1(pub Digest);

impl PresentationIdV1 {
    /// Hash a canon-encoded presentation payload under the value domain.
    pub fn from_canon(payload: &[u8]) -> Self {
        Self(Digest::of(Domain::Value, payload))
    }

    /// The underlying digest.
    pub fn digest(&self) -> Digest {
        self.0
    }
}

impl Canonical for PresentationIdV1 {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(self.0.as_bytes());
    }
}

/// Classifies each committed step as administrative or realizing at one
/// declared observation boundary (SOC-LAW-10's domain clause).
pub trait ObservationProfile {
    /// The canonical identity carried in every certificate and counterexample.
    fn id(&self) -> ObservationProfileId;

    /// Classify `step`.
    ///
    /// Classification reads **durable canonical material only** — the
    /// decomposition, endpoints, key, and observation — never a raw
    /// [`Handle`], so a label is replayable from the journal alone.
    fn label(&self, step: &CommittedStep) -> Result<StepLabel, ProfileError>;
}

/// The v1 observation profile: the generator partition `𝒢 = 𝒢_τ ⊎ 𝒢_o`.
///
/// A step is [`StepLabel::Administrative`] iff **every** generator of its
/// decomposition lies in the administrative partition, [`StepLabel::Realizing`]
/// iff every one lies in the realizing partition, and a [`ProfileError`]
/// otherwise. The partitions are held as `BTreeSet`s — a partition is not a
/// `𝒢`-membership registry, and canonical set order is observable here.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GeneratorPartitionProfile {
    administrative: BTreeSet<GeneratorId>,
    realizing: BTreeSet<GeneratorId>,
}

/// The two partitions of a [`GeneratorPartitionProfile`] overlapped.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct OverlappingPartitions;

impl GeneratorPartitionProfile {
    /// The frozen kind string for this profile's canonical preimage.
    pub const KIND: &'static str = "brix.soc.obs-profile.generator-partition@1";
    /// The frozen envelope marker for observation-profile identities.
    pub const MARKER: &'static [u8] = b"brix.soc.obs-profile";
    /// The frozen format version.
    pub const VERSION: u64 = 1;

    /// Build a profile from two disjoint generator partitions.
    ///
    /// Fallible on overlap: a generator that is both administrative and
    /// realizing has no well-defined visibility.
    pub fn new(
        administrative: BTreeSet<GeneratorId>,
        realizing: BTreeSet<GeneratorId>,
    ) -> Result<Self, OverlappingPartitions> {
        if administrative.intersection(&realizing).next().is_some() {
            return Err(OverlappingPartitions);
        }
        Ok(Self {
            administrative,
            realizing,
        })
    }

    /// A profile that classifies every registered generator as realizing —
    /// the degenerate case under which `sat_step` behaves exactly like
    /// `commit_tick` (ADR-0014 §3.2).
    pub fn all_realizing(realizing: BTreeSet<GeneratorId>) -> Self {
        Self {
            administrative: BTreeSet::new(),
            realizing,
        }
    }

    /// The frozen canonical preimage: marker, version, kind, `𝒢_τ`, `𝒢_o`.
    pub fn canon_preimage(&self) -> Vec<u8> {
        let mut w = CanonWriter::new();
        w.write_bytes(Self::MARKER);
        w.write_uint(Self::VERSION);
        w.write_str(Self::KIND);
        w.write_set(self.administrative.iter().map(|g| g.canon_bytes()));
        w.write_set(self.realizing.iter().map(|g| g.canon_bytes()));
        w.finish()
    }
}

impl ObservationProfile for GeneratorPartitionProfile {
    fn id(&self) -> ObservationProfileId {
        ObservationProfileId::from_canon(&self.canon_preimage())
    }

    fn label(&self, step: &CommittedStep) -> Result<StepLabel, ProfileError> {
        let generators = step.decomposition.generators();
        if generators.is_empty() {
            return Err(ProfileError::EmptyDecomposition);
        }

        let mut administrative = 0usize;
        let mut realizing = 0usize;
        for g in generators {
            if self.administrative.contains(g) {
                administrative += 1;
            } else if self.realizing.contains(g) {
                realizing += 1;
            } else {
                return Err(ProfileError::UnregisteredGenerator);
            }
        }

        match (administrative, realizing) {
            (_, 0) => Ok(StepLabel::Administrative),
            (0, _) => Ok(StepLabel::Realizing),
            _ => Err(ProfileError::MixedDecomposition),
        }
    }
}

/// The state identity saturation, cycle detection, and bisimulation use.
///
/// [`ExecConfig::history`] is **deliberately excluded**. `oracle`'s
/// `CandidateStep` folds `Handle::raw()` into the history chain — so history is
/// allocation-order dependent — and it strictly grows on every applied
/// candidate. Including it would make every configuration along an
/// administrative loop distinct, rendering cycle detection vacuous and every
/// state space infinite (ADR-0014 §1.1, ⟨D-PROJ⟩).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ObservableState {
    /// `x` — the world handle.
    pub world: Handle,
    /// `p` — the policy handle.
    pub policy: Handle,
}

/// Project an exec config onto its observable state, discarding history.
pub fn project(e: &ExecConfig) -> ObservableState {
    ObservableState {
        world: e.world,
        policy: e.policy,
    }
}

/// Which presentation-level hypothesis a result depended on.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum AssumptionId {
    /// **P1** — `Regime::candidates` and `Adm::admits` depend on the exec
    /// config only through [`project`].
    HistoryIndependence,
    /// **P6** — the keyer's `priority` and `tiebreak` components do not depend
    /// on `Key::phase`.
    PhaseStableKeying,
}

/// The presentation-level hypotheses an analysis is conditional on.
///
/// These are **declared** by the presentation and bounded-checked by the
/// conformance harness. `soc-core` never proves them (ADR-0014 §4.2, risk 2).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct DeclaredAssumptions {
    /// P1 — see [`AssumptionId::HistoryIndependence`].
    pub history_independent: bool,
    /// P6 — see [`AssumptionId::PhaseStableKeying`].
    pub phase_stable_keying: bool,
}

impl DeclaredAssumptions {
    /// Declare both hypotheses — the shape lasso detection requires.
    pub fn all() -> Self {
        Self {
            history_independent: true,
            phase_stable_keying: true,
        }
    }

    /// Whether `id` is declared.
    pub fn declares(&self, id: AssumptionId) -> bool {
        match id {
            AssumptionId::HistoryIndependence => self.history_independent,
            AssumptionId::PhaseStableKeying => self.phase_stable_keying,
        }
    }
}

/// The `Pres = (C₀, W₀, Real, Adm, D, e₀)` interface, as much of it as exists
/// at Step 8. Build Plan Step 10 (S6) must supply a presentation that maps onto
/// this (ADR-0014 §11 item 4).
pub struct PresentationV1<'a> {
    /// Opaque canonical program/world revision identity, caller-supplied.
    pub id: PresentationIdV1,
    /// The regimes in play.
    pub regimes: &'a [&'a dyn SettlementRegime],
    /// Caller-supplied canonical identity of the ordered regime set.
    pub regime_set: Digest,
    /// The governance predicate.
    pub adm: &'a dyn Adm,
    /// Caller-supplied canonical identity of the admissibility policy.
    pub adm_id: Digest,
    /// The declared observation boundary.
    pub profile: &'a dyn ObservationProfile,
    /// The interner that minted every handle in play.
    pub interner: &'a Interner,
    /// The exact context every committed judgement is indexed by.
    pub context: ContextId,
    /// The hypotheses this presentation declares.
    pub assumptions: DeclaredAssumptions,
}

/// Resource bounds for one saturated step.
///
/// No `Default` impl on purpose: a caller must state its budget, following
/// [`CostRecord`]'s honesty discipline. `brix_kernel::Budget` is unavailable —
/// `soc-core` depends only on `brix-canon` and `brix-semantic`, and adding that
/// edge would violate the trusted-boundary dependency policy.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct SaturationBudget {
    /// Maximum administrative steps hidden inside **one** saturated step.
    pub max_hidden_steps: u64,
    /// Maximum distinct [`ObservableState`]s retained for lasso detection.
    /// Unused in Stage A (divergence detection arrives with Stage B).
    pub max_administrative_states: u64,
    /// Maximum saturated (visible) steps for a whole run.
    pub max_visible_steps: u64,
}

impl SaturationBudget {
    /// A budget with all three bounds set to `n`.
    pub fn uniform(n: u64) -> Self {
        Self {
            max_hidden_steps: n,
            max_administrative_states: n,
            max_visible_steps: n,
        }
    }
}

/// Enumerate the admissible candidates at `e`, returning the set and the work
/// units the scan cost.
///
/// Mirrors [`commit_tick`]'s `δ` exactly — one unit per regime scanned, one per
/// raw candidate tested — which is the point: a quiescence certificate's
/// re-enumeration and the engine's own enumeration must be the *same*
/// enumeration, or the certificate checks something the engine never did.
/// [`commit_tick`] keeps its own inline copy because it keys and frontiers as
/// it goes; this one only needs the set.
pub(crate) fn enumerate_admissible(
    regimes: &[&dyn SettlementRegime],
    adm: &dyn Adm,
    e: &ExecConfig,
) -> (BTreeSet<Candidate>, u64) {
    let mut out = BTreeSet::new();
    let mut work: u64 = 0;
    for regime in regimes {
        work += 1;
        for c in regime.candidates(e) {
            work += 1;
            if adm.admits(e, &c) {
                out.insert(c);
            }
        }
    }
    (out, work)
}

/// The full encoded `F_O`-structure after divergence-sensitive saturation.
///
/// Deliberately a **strictly larger** vocabulary than `F_O`: [`Self::Divergent`]
/// and [`Self::Unknown`] are not `F_O`-values. The `F_O`-coalgebra is defined
/// exactly on the sub-carrier where this returns an `F_O`-value — that
/// partiality is the honest content of the interface, and is what CJ-1 will be
/// stated against (ADR-0014 §5).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SaturatedStep {
    /// `τ* ; o` — the `O × X` summand, after hiding a finite administrative
    /// prefix.
    ///
    /// `hidden_steps` is diagnostic and is **not** part of visible behavior:
    /// two implementations with different administrative layouts may agree
    /// here (#61 non-goal).
    Realizing {
        /// The exported `O_min` value.
        observation: Observation,
        /// The configuration after the realizing step.
        successor: ExecConfig,
        /// How many administrative steps were hidden before it.
        hidden_steps: u64,
    },
    /// The `1` summand, **certified**.
    Quiescent(Box<QuiescenceCertificateV1>),
    /// `↑_τ`, **certified** by a closed lasso. Graded `Unknown` for the
    /// completion/quiescence question — never `Refuted`, and never the `1`
    /// summand.
    Divergent(Box<DivergenceCertificateV1>),
    /// Nothing was established. Never a pass, never a certificate, never
    /// `Refuted`.
    Unknown(SaturationUnknown),
}

/// Why a saturated step established nothing.
///
/// Every variant grades the completion/quiescence question as `Unknown`.
/// Exactly one constructor in this crate yields a decided negative:
/// [`SaturatedStep::Quiescent`].
// Derives match `CommitError`'s, which this embeds: a fieldless-error enum
// there means no `Copy`/`Ord`/`Hash` here.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SaturationUnknown {
    /// The administrative budget was consumed before quiescence, a realizing
    /// step, or a closed lasso. **Distinct from certified divergence: we do
    /// not know whether it diverges.**
    AdministrativeBudgetExhausted {
        /// Administrative steps taken before the bound was hit.
        hidden_steps: u64,
        /// The bound that was hit.
        budget: u64,
    },
    /// The visited-state bound for lasso detection was hit.
    AdministrativeStateBudgetExhausted {
        /// Distinct states retained before the bound was hit.
        states: u64,
        /// The bound that was hit.
        budget: u64,
    },
    /// The whole-run visible-step budget was hit — the honest replacement for
    /// [`crate::commit::run`]'s silent loop exit.
    VisibleBudgetExhausted {
        /// Visible steps taken before the bound was hit.
        visible_steps: u64,
        /// The bound that was hit.
        budget: u64,
    },
    /// The observation profile could not classify a committed step.
    ProfileError {
        /// Index into the returned step vector.
        at_step: u64,
        /// Why classification failed.
        error: ProfileError,
    },
    /// The commit boundary failed, so no trustworthy successor exists.
    ///
    /// Reached from [`sat_step`], which drives the fallible
    /// [`crate::commit::try_commit_tick`] (#254): any [`CommitError`] the
    /// commit boundary raises — an unresolved handle, an empty or
    /// endpoint-mismatched decomposition, a Stage B candidate/witness/generator
    /// mismatch — stops the run here with the underlying error preserved. No
    /// step is committed and no certificate is produced.
    CommitFailed {
        /// Index into the returned step vector.
        at_step: u64,
        /// The commit-boundary failure.
        error: CommitError,
    },
    /// An analysis was requested that needs a hypothesis this presentation
    /// does not declare.
    UndeclaredAssumption(AssumptionId),
    /// A declared hypothesis was falsified by a bounded check — the
    /// presentation's declaration was wrong. Fail closed.
    AssumptionViolated {
        /// Which hypothesis.
        assumption: AssumptionId,
        /// Index into the returned step vector.
        at_step: u64,
    },
    /// The `B^uk` unique-key discipline was violated during saturation: two
    /// admissible candidates with different observed successors were assigned
    /// the same calendar key.
    ///
    /// Reached from [`sat_step`] via [`crate::commit::try_commit_tick`]
    /// (#254). The reference [`crate::commit::commit_tick`] still panics on this — there it
    /// is an internal-consistency bug — but a run driven by a
    /// source-derived keyer stops closed instead. The frontier is left exactly
    /// as it was, so no partially-built tick escapes.
    KeyConflict {
        /// Index into the returned step vector.
        at_step: u64,
    },
}

/// Fold a γ-tick's cost into a running total.
///
/// `None` means some tick was unmeasured, and the whole saturated step is then
/// reported as [`CostRecord::UnknownCost`] — never a partial sum, never zero
/// (ADR-0014 §5.2).
fn fold_cost(running: Option<u64>, tick: &CostRecord) -> Option<u64> {
    match (running, tick.work_units()) {
        (Some(total), Some(units)) => Some(total.saturating_add(units)),
        _ => None,
    }
}

/// One saturated step: hide a finite administrative prefix, then export the
/// realizing observation that follows it, or certify quiescence.
///
/// Returns the [`SaturatedStep`], the committed steps consumed (the
/// administrative prefix **plus** the realizing step, in order — all of which
/// the caller must journal), and the folded cost.
///
/// `phase` is the calendar phase of the first inner γ-tick; the `i`-th inner
/// tick uses `phase + i`, matching [`crate::commit::run`]'s one-phase-per-
/// committed-step convention. A caller driving successive saturated steps
/// advances `phase` by the number of steps returned.
///
/// **Divergence versus exhaustion.** An administrative orbit that revisits an
/// [`ObservableState`] is a closed lasso and yields
/// [`SaturatedStep::Divergent`] — but only when the presentation declares P1
/// and P6 *and* the bounded checks at the revisited state hold. Otherwise the
/// orbit simply exhausts `budget.max_hidden_steps` and is reported as
/// [`SaturationUnknown::AdministrativeBudgetExhausted`] — an honest "we do not
/// know", never a quiescence certificate and never `Refuted`.
pub fn sat_step<F>(
    pres: &PresentationV1<'_>,
    e: &ExecConfig,
    phase: u64,
    keyer: &mut F,
    budget: SaturationBudget,
) -> (SaturatedStep, Vec<CommittedStep>, CostRecord)
where
    F: FnMut(&crate::regime::Candidate, u64) -> crate::calendar::Key,
{
    let mut consumed: Vec<CommittedStep> = Vec::new();
    let mut current = *e;
    let mut work: Option<u64> = Some(0);
    let mut hidden: u64 = 0;
    // Observable state → the index in `consumed` at which the orbit was in that
    // state. Index 0 is the start state, before any administrative step. This
    // is the *whole* reason `project` exists: `ExecConfig` equality could never
    // populate a repeat, because `history` grows monotonically (§1.1).
    let mut visited: BTreeMap<ObservableState, VisitRecord> = BTreeMap::new();
    visited.insert(project(&current), record_visit(pres, &current, 0));

    loop {
        let tick_phase = phase.saturating_add(consumed.len() as u64);
        // The fallible driver (#254): a key conflict or a rejected commit
        // boundary is a stop, not a panic. Both land on already-declared
        // `SaturationUnknown` variants whose `at_step` is the index into the
        // steps consumed so far — the step that would have been next.
        let (committed, step, cost) = match try_commit_tick(
            pres.regimes,
            pres.adm,
            pres.interner,
            &current,
            pres.context,
            tick_phase,
            keyer,
        ) {
            Ok(tick) => tick,
            Err(CommitTickError::KeyConflict(_)) => {
                let at_step = consumed.len() as u64;
                return (
                    SaturatedStep::Unknown(SaturationUnknown::KeyConflict { at_step }),
                    consumed,
                    finish_cost(work),
                );
            }
            Err(CommitTickError::Commit(error)) => {
                let at_step = consumed.len() as u64;
                return (
                    SaturatedStep::Unknown(SaturationUnknown::CommitFailed { at_step, error }),
                    consumed,
                    finish_cost(work),
                );
            }
        };
        work = fold_cost(work, &cost);

        match committed {
            Committed::Quiescent => {
                let certificate = quiescence_certificate(pres, e, &current, &consumed);
                return (
                    SaturatedStep::Quiescent(Box::new(certificate)),
                    consumed,
                    finish_cost(work),
                );
            }
            Committed::Step {
                observation,
                successor,
            } => {
                let step = step.expect("Committed::Step always carries Some(CommittedStep)");
                match pres.profile.label(&step) {
                    Err(error) => {
                        let at_step = consumed.len() as u64;
                        consumed.push(step);
                        return (
                            SaturatedStep::Unknown(SaturationUnknown::ProfileError {
                                at_step,
                                error,
                            }),
                            consumed,
                            finish_cost(work),
                        );
                    }
                    Ok(StepLabel::Realizing) => {
                        consumed.push(step);
                        return (
                            SaturatedStep::Realizing {
                                observation,
                                successor,
                                hidden_steps: hidden,
                            },
                            consumed,
                            finish_cost(work),
                        );
                    }
                    Ok(StepLabel::Administrative) => {
                        consumed.push(step);
                        hidden += 1;
                        current = successor;

                        // Lasso check, before the budget check: a closed cycle
                        // is a *result*, and reporting exhaustion for an orbit
                        // we can already certify would throw information away.
                        let state = project(&current);
                        if let Some(previous) = visited.get(&state) {
                            let outcome =
                                close_lasso(pres, e, &current, &consumed, previous, phase, keyer);
                            return (outcome, consumed, finish_cost(work));
                        }

                        if visited.len() as u64 >= budget.max_administrative_states {
                            return (
                                SaturatedStep::Unknown(
                                    SaturationUnknown::AdministrativeStateBudgetExhausted {
                                        states: visited.len() as u64,
                                        budget: budget.max_administrative_states,
                                    },
                                ),
                                consumed,
                                finish_cost(work),
                            );
                        }
                        visited.insert(state, record_visit(pres, &current, consumed.len() as u64));

                        if hidden > budget.max_hidden_steps {
                            return (
                                SaturatedStep::Unknown(
                                    SaturationUnknown::AdministrativeBudgetExhausted {
                                        hidden_steps: hidden,
                                        budget: budget.max_hidden_steps,
                                    },
                                ),
                                consumed,
                                finish_cost(work),
                            );
                        }
                    }
                }
            }
        }
    }
}

fn finish_cost(work: Option<u64>) -> CostRecord {
    match work {
        Some(units) => CostRecord::Steps(units),
        None => CostRecord::UnknownCost("a γ-tick inside this saturated step was unmeasured"),
    }
}

/// Build the quiescence certificate for a terminal configuration.
///
/// Every field is derived here; nothing is asserted that
/// [`check_quiescence_certificate`] cannot independently re-derive from the
/// presentation and the prefix.
fn quiescence_certificate(
    pres: &PresentationV1<'_>,
    start: &ExecConfig,
    terminal: &ExecConfig,
    prefix: &[CommittedStep],
) -> QuiescenceCertificateV1 {
    let src_world = ConfigId(pres.interner.resolve(start.world));
    let terminal_world = ConfigId(pres.interner.resolve(terminal.world));
    let policy = ConfigId(pres.interner.resolve(terminal.policy));

    // Per-step identities, and separately the running chain digest. These are
    // different folds: `replay_chain` accumulates, `canon_digest` does not.
    let hidden = prefix
        .iter()
        .map(|step| step.canon_digest(Domain::Value))
        .collect();
    let prefix_chain = certificate::replay_chain_digest(prefix);

    QuiescenceCertificateV1 {
        profile: pres.profile.id(),
        context: pres.context,
        presentation: pres.id,
        policy,
        src_world,
        terminal_world,
        hidden,
        prefix_chain,
        regime_set: pres.regime_set,
        adm_id: pres.adm_id,
        enumeration: EnumerationCompleteness::Complete,
        grade: Outcome::Derived,
        // A function of the other fields, never an independent input: a caller
        // cannot mint a certificate whose judgement names something else.
        judgement: certificate::quiescence_judgement_of(
            pres.context,
            terminal_world,
            policy,
            pres.regime_set,
            pres.adm_id,
            prefix_chain,
        ),
    }
}

/// What we remember about an observable state the administrative orbit has
/// already been in.
struct VisitRecord {
    /// Index into the consumed-step vector at which the orbit was in this
    /// state. `0` is the start state, before any administrative step.
    index: u64,
    /// The admissible candidate set enumerated at this state, retained only
    /// when P1 is declared — it is the baseline the bounded check compares
    /// against, and is pure overhead otherwise.
    candidates: Option<BTreeSet<Candidate>>,
}

fn record_visit(pres: &PresentationV1<'_>, e: &ExecConfig, index: u64) -> VisitRecord {
    let candidates = pres
        .assumptions
        .declares(AssumptionId::HistoryIndependence)
        .then(|| enumerate_admissible(pres.regimes, pres.adm, e).0);
    VisitRecord { index, candidates }
}

/// Decide what a revisited observable state means.
///
/// A revisit is only a *proof* of divergence under P1 and P6, so this is where
/// the conditionality is discharged — or refused. Three outcomes, in order of
/// how much they claim: an undeclared hypothesis is
/// [`SaturationUnknown::UndeclaredAssumption`] (we noticed the repeat but may
/// not conclude from it); a declared hypothesis the bounded check falsifies is
/// [`SaturationUnknown::AssumptionViolated`] (the presentation's declaration is
/// *wrong*, which is worse than not declaring); and only with both declared and
/// both checks passing does this mint a certificate.
#[allow(clippy::too_many_arguments)]
fn close_lasso<F>(
    pres: &PresentationV1<'_>,
    start: &ExecConfig,
    revisited: &ExecConfig,
    consumed: &[CommittedStep],
    previous: &VisitRecord,
    phase: u64,
    keyer: &mut F,
) -> SaturatedStep
where
    F: FnMut(&Candidate, u64) -> crate::calendar::Key,
{
    let at_step = consumed.len() as u64;

    if !pres.assumptions.declares(AssumptionId::HistoryIndependence) {
        return SaturatedStep::Unknown(SaturationUnknown::UndeclaredAssumption(
            AssumptionId::HistoryIndependence,
        ));
    }
    if !pres.assumptions.declares(AssumptionId::PhaseStableKeying) {
        return SaturatedStep::Unknown(SaturationUnknown::UndeclaredAssumption(
            AssumptionId::PhaseStableKeying,
        ));
    }

    // P1, bounded: the same observable state must offer the same admissible
    // candidates now as it did on the first visit. A regime that branches on
    // `history` fails exactly here — the two visits differ only in history.
    let (candidates_now, _) = enumerate_admissible(pres.regimes, pres.adm, revisited);
    let baseline = previous
        .candidates
        .as_ref()
        .expect("P1 declared ⇒ the visit record retained its candidate set");
    if *baseline != candidates_now {
        return SaturatedStep::Unknown(SaturationUnknown::AssumptionViolated {
            assumption: AssumptionId::HistoryIndependence,
            at_step,
        });
    }

    // P6, bounded: keying this state's candidates at the phase we are at now
    // must agree — on priority and tie-break, the two components `select_K`
    // orders by within a phase — with keying them at the phase of the first
    // visit. Otherwise the repeat of the candidate *set* need not produce a
    // repeat of the *selection*, and the orbit is not a cycle at all.
    let phase_then = phase.saturating_add(previous.index);
    let phase_now = phase.saturating_add(at_step);
    for candidate in &candidates_now {
        let then = keyer(candidate, phase_then);
        let now = keyer(candidate, phase_now);
        if (then.priority, then.tiebreak) != (now.priority, now.tiebreak) {
            return SaturatedStep::Unknown(SaturationUnknown::AssumptionViolated {
                assumption: AssumptionId::PhaseStableKeying,
                at_step,
            });
        }
    }

    SaturatedStep::Divergent(Box::new(DivergenceCertificateV1 {
        profile: pres.profile.id(),
        context: pres.context,
        presentation: pres.id,
        policy: ConfigId(pres.interner.resolve(start.policy)),
        src_world: ConfigId(pres.interner.resolve(start.world)),
        stem: previous.index,
        cycle: at_step - previous.index,
        lasso: consumed
            .iter()
            .map(|step| step.canon_digest(Domain::Value))
            .collect(),
        cycle_world: ConfigId(pres.interner.resolve(revisited.world)),
        cycle_policy: ConfigId(pres.interner.resolve(revisited.policy)),
        assumptions: AssumptionMode::DeclaredP1P6,
        grade: Outcome::Unknown,
    }))
}
