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
//! # Stage A scope
//!
//! Divergence detection is **not** enabled yet: a non-terminating
//! administrative orbit is reported as
//! [`SaturationUnknown::AdministrativeBudgetExhausted`], never as certified
//! divergence. Lasso detection arrives with Stage B, which also adds the
//! `Canonical` impls, the certificate identities, and the fail-closed readers.

use std::collections::BTreeSet;

use brix_canon::{CanonWriter, Canonical, Digest, Domain};
use brix_semantic::{ConfigId, ContextId, GeneratorId, Outcome};

use crate::adm::Adm;
use crate::commit::{commit_tick, CommitError, Committed, Observation, SettlementRegime};
use crate::cost::CostRecord;
use crate::exec::ExecConfig;
use crate::intern::{Handle, Interner};
use crate::journal::{CommittedStep, Journal};

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
        let generators = &step.decomposition.generators;
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

/// Whether the frontier enumeration backing a quiescence claim was exhaustive.
///
/// The load-bearing honesty field of the certificate: "the frontier is empty"
/// is a decided negative **only if** enumeration was complete. That holds in v1
/// solely because `Regime::candidates -> Vec<Candidate>` is unbounded and
/// total. A bounded or fallible regime API requires a v2 certificate and MUST
/// NOT emit v1 (ADR-0014 §6.2, risk 1).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum EnumerationCompleteness {
    /// The whole admissible frontier was enumerated.
    Complete,
}

/// A certified claim that no admissible candidate exists at the terminal world.
///
/// **Stage A declares this type; Stage B adds its `Canonical` impl, its
/// identity, the quiescence-proposition binding ⟨D-QP⟩, and the fail-closed
/// reader.** Until then a value of this type is an in-memory claim, not yet an
/// independently checkable artifact.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct QuiescenceCertificateV1 {
    /// The declared observation boundary.
    pub profile: ObservationProfileId,
    /// The exact context.
    pub context: ContextId,
    /// The program/world revision.
    pub presentation: PresentationIdV1,
    /// The policy in force.
    pub policy: ConfigId,
    /// The world saturation started from.
    pub src_world: ConfigId,
    /// The world at which the frontier was found empty.
    pub terminal_world: ConfigId,
    /// The hidden administrative prefix, as committed-step digests in order.
    pub hidden: Vec<Digest>,
    /// The chain digest of the hidden prefix, replayed from scratch.
    pub prefix_chain: Option<Digest>,
    /// The ordered regime-set identity.
    pub regime_set: Digest,
    /// The admissibility-policy identity.
    pub adm_id: Digest,
    /// Whether enumeration was exhaustive.
    pub enumeration: EnumerationCompleteness,
    /// The grade this certificate claims. Always [`Outcome::Derived`]: a
    /// settlement-kernel certificate is never a proof-kernel theorem.
    pub grade: Outcome,
}

/// A certified administrative divergence (a closed lasso).
///
/// **Stage A declares this type; it is never constructed before Stage B**,
/// which enables lasso detection. A non-terminating administrative orbit is
/// reported as [`SaturationUnknown::AdministrativeBudgetExhausted`] until then.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DivergenceCertificateV1 {
    /// The declared observation boundary.
    pub profile: ObservationProfileId,
    /// The exact context.
    pub context: ContextId,
    /// The program/world revision.
    pub presentation: PresentationIdV1,
    /// Steps before the cycle closes.
    pub stem: u64,
    /// Length of the closed administrative cycle, at least 1.
    pub cycle: u64,
    /// The lasso, as committed-step digests in order.
    pub lasso: Vec<Digest>,
    /// The observable state the orbit revisits.
    pub revisited: ObservableState,
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
    /// summand. Not constructed before Stage B.
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
    /// The visited-state bound for lasso detection was hit. Not reachable in
    /// Stage A.
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
    /// Not reachable from [`sat_step`], which drives the reference
    /// [`commit_tick`]; it is declared for the fallible driver arriving with
    /// Stage C.
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
    /// The `B^uk` unique-key discipline was violated during saturation.
    ///
    /// Not reachable from [`sat_step`] — the reference `commit_tick` panics on
    /// a key conflict. Declared for the fallible driver (Stage C).
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
/// **Stage A does not detect divergence.** A non-terminating administrative
/// orbit exhausts `budget.max_hidden_steps` and is reported as
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

    loop {
        let tick_phase = phase.saturating_add(consumed.len() as u64);
        let (committed, step, cost) = commit_tick(
            pres.regimes,
            pres.adm,
            pres.interner,
            &current,
            pres.context,
            tick_phase,
            keyer,
        );
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

/// Build the in-memory quiescence claim for a terminal configuration.
///
/// Stage A populates the claim's fields; Stage B gives it a canonical encoding,
/// an identity, and an independent checker.
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
    let prefix_chain = Journal::replay_chain(prefix).last().copied();

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
    }
}
