//! ADR-0012 Stage C — the incremental settlement adapter, the
//! `saturate::run_saturated` integration, and the semantic/diagnostic result
//! split (§2 item 4, §4.3, §5).
//!
//! This module is what makes `brix run` possible, without adding it (§9
//! Stage C: "No CLI `brix run` command is added before Stages A-C have the
//! above fixtures"). It builds on Stage A ([`crate::l3`], [`crate::l3_canon`])
//! and Stage B ([`crate::l3_regime`]) without re-deriving any of their
//! identities, and consumes `soc-core`'s saturated interface (ADR-0014) as a
//! *client*: the total stepping/selection/stop-vocabulary machinery is
//! [`soc_core::saturate::run_saturated`]'s, never this module's own.
//!
//! # What this module does, and does not, decide
//!
//! - It builds one [`soc_core::saturate::PresentationV1`] per §2 item 8 and
//!   §3.4, and one calendar keyer per §3.4's `Key` formula.
//! - The keyer ([`L3StepAdapter`]) is the *only* place this module hooks into
//!   stepping: it is invoked by `commit_tick`'s δ-phase (inside
//!   `sat_step`/`run_saturated`) once per admissible candidate — at most once
//!   per step for this profile's head-only regime — and uses that one call to
//!   perform every "before every selection" obligation §4.3 steps 4-5
//!   describe: comparing the incrementally maintained candidate view (a real
//!   `soc_core::engine::IncrementalEngine` over a *second*,
//!   `Rc`-shared [`crate::l3_regime::L3Regime`] instance, per Stage B's dual-
//!   regime design) against the naive relation, cross-checking the pure
//!   `prospective_successor`, and maintaining a keyed
//!   `soc_core::calendar::Frontier` transactionally.
//! - It never mints a `Derived` judgement (that is `try_commit_selected`'s
//!   sole job, reached only through `commit_tick`/`sat_step`) and never
//!   decides quiescence itself (that is `run_saturated`'s `SaturatedStop`,
//!   carried through verbatim — §5, ⟨D-STATUS⟩).
//! - An adapter-local integrity failure — a differential mismatch, a staged
//!   update failure — downgrades the eventual result to `Unknown`
//!   regardless of what `run_saturated` itself reported, and is never
//!   "dressed as" `SaturatedStop::Quiescent` (§2 item 4).
//!
//! # The semantic/diagnostic split (§5)
//!
//! [`SettlementRunV1`] is the *only* canonically-identified result: program,
//! context, presentation and observation-profile identities, initial/final
//! world and policy identities, the exact [`PlanLimitsV1`], the total stop
//! reason (a stable versioned [`L3UnknownReasonV1`] code, never human text),
//! the [`soc_core::saturate::QuiescenceCertificateId`] on `Quiescent`, the
//! ordered committed step digests, and the journal chain digest. It
//! deliberately excludes the [`SaturationBudget`], `agenda_residue`, cost
//! records, and the certificate/journal bodies themselves — those ride on
//! [`L3RunReport`] as non-semantic diagnostics (⟨D-LIM⟩, §3.3: excluding the
//! budget is what lets two runs under different *sufficient* budgets share
//! one certificate, ADR-0014 §6.2).

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::rc::Rc;

use brix_canon::{CanonWriter, Canonical, Digest, Domain};
use brix_semantic::{ConfigId, ContextId};

use soc_core::adm::Adm;
use soc_core::calendar::{Frontier, FrontierDeltaError, Key};
use soc_core::commit::{prospective_successor, CommitError, SettlementRegime};
use soc_core::cost::CostRecord;
use soc_core::delta::Delta;
use soc_core::engine::IncrementalEngine;
use soc_core::exec::ExecConfig;
use soc_core::history::History;
use soc_core::intern::{Handle, Interner};
use soc_core::journal::Journal;
use soc_core::regime::{Candidate, Regime};
use soc_core::saturate::{
    quiescence_certificate_id, run_saturated, DeclaredAssumptions, DivergenceCertificateV1,
    GeneratorPartitionProfile, ObservationProfile, ObservationProfileId,
    PresentationIdV1 as SocPresentationIdV1, PresentationV1, QuiescenceCertificateId,
    QuiescenceCertificateV1, SaturatedRun, SaturatedStop, SaturationBudget, SaturationUnknown,
};

use crate::l3::{L3PlanV1, PlanLimitsV1, L3_PROFILE_MARKER_V1};
use crate::l3_canon::{context_id, policy_id, program_id, ProgramIdV1, RunContextV1};
use crate::l3_regime::{
    build_l3_observation_profile, build_l3_transition_table, l3_adm, l3_policy, L3Regime,
    L3TransitionTable,
};

// ---------------------------------------------------------------------------
// The stable, versioned Unknown-reason vocabulary (ADR-0012 §5).
// ---------------------------------------------------------------------------

/// A stable, versioned reason code for a post-plan `Unknown` result (ADR-0012
/// §5: "`Unknown` additionally carries a stable versioned reason code, while
/// human diagnostic text remains outside the semantic identity"). Ordinals
/// are ABI: append-only, never reordered.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum L3UnknownReasonV1 {
    /// `SaturationUnknown::AdministrativeBudgetExhausted`. Structurally
    /// unreachable under this profile's `𝒢_τ = ∅` (ADR-0012 §4.1) — carried
    /// for totality, the same discipline `SaturatedStep::Divergent` uses.
    AdministrativeBudgetExhausted,
    /// `SaturationUnknown::AdministrativeStateBudgetExhausted`. Also
    /// structurally unreachable under `𝒢_τ = ∅`.
    AdministrativeStateBudgetExhausted,
    /// `SaturationUnknown::VisibleBudgetExhausted` — the honest replacement
    /// for the retired draft's `CommitBudgetExhausted` (ADR-0012 §5).
    VisibleBudgetExhausted,
    /// `SaturationUnknown::ProfileError`.
    ProfileError,
    /// `SaturationUnknown::CommitFailed`, or the adapter's own detection of
    /// an equivalent [`CommitError`] before `commit_tick`'s panicking
    /// reference path would have been reached.
    CommitFailed,
    /// `SaturationUnknown::UndeclaredAssumption`.
    UndeclaredAssumption,
    /// `SaturationUnknown::AssumptionViolated`.
    AssumptionViolated,
    /// `SaturationUnknown::KeyConflict`, or the adapter's own keyed-frontier
    /// maintenance detecting the `B^uk` unique-key discipline violated
    /// ([`FrontierDeltaError::InsertConflict`]).
    KeyConflict,
    /// An adapter-local integrity failure: the incrementally maintained
    /// candidate/frontier view disagreed with the naive `Regime` relation, a
    /// staged update failed, or a committed successor did not equal its
    /// pre-checked prospect (ADR-0012 §2 item 4, §4.3 steps 4-5). Never a
    /// commit, never a certificate. [`L3RunReport::adapter_failure`] carries
    /// the fine-grained (non-semantic) detail.
    AdapterIntegrityFailure,
    /// `SaturatedStop::Divergent` was observed. Structurally impossible under
    /// `𝒢_τ = ∅` (ADR-0012 §4.1, §9 Stage C fixture 13); if it is ever seen,
    /// ADR-0012 §5 requires treating it as an integrity failure, never a
    /// settlement result. The certificate itself is retained as an
    /// operational diagnostic ([`L3RunReport::divergence_certificate`]),
    /// never discarded.
    DivergenceObserved,
}

impl L3UnknownReasonV1 {
    /// Canonical ABI ordinal. Append-only; never reorder.
    pub const fn ordinal(self) -> u64 {
        match self {
            Self::AdministrativeBudgetExhausted => 0,
            Self::AdministrativeStateBudgetExhausted => 1,
            Self::VisibleBudgetExhausted => 2,
            Self::ProfileError => 3,
            Self::CommitFailed => 4,
            Self::UndeclaredAssumption => 5,
            Self::AssumptionViolated => 6,
            Self::KeyConflict => 7,
            Self::AdapterIntegrityFailure => 8,
            Self::DivergenceObserved => 9,
        }
    }
}

fn map_saturation_unknown(u: &SaturationUnknown) -> L3UnknownReasonV1 {
    match u {
        SaturationUnknown::AdministrativeBudgetExhausted { .. } => {
            L3UnknownReasonV1::AdministrativeBudgetExhausted
        }
        SaturationUnknown::AdministrativeStateBudgetExhausted { .. } => {
            L3UnknownReasonV1::AdministrativeStateBudgetExhausted
        }
        SaturationUnknown::VisibleBudgetExhausted { .. } => {
            L3UnknownReasonV1::VisibleBudgetExhausted
        }
        SaturationUnknown::ProfileError { .. } => L3UnknownReasonV1::ProfileError,
        SaturationUnknown::CommitFailed { .. } => L3UnknownReasonV1::CommitFailed,
        SaturationUnknown::UndeclaredAssumption(_) => L3UnknownReasonV1::UndeclaredAssumption,
        SaturationUnknown::AssumptionViolated { .. } => L3UnknownReasonV1::AssumptionViolated,
        SaturationUnknown::KeyConflict { .. } => L3UnknownReasonV1::KeyConflict,
    }
}

/// Map a [`CommitError`] the adapter observed directly (never through the
/// panicking `commit_tick` reference path — see [`L3StepAdapter`]'s module
/// doc) onto the stable Unknown vocabulary. Every `CommitError` variant maps
/// to the same public reason (`CommitFailed`); the exact condition is a
/// non-semantic diagnostic (the `CommitError` value itself), per §5's
/// "human diagnostic text remains outside the semantic identity."
pub fn commit_error_reason(_e: &CommitError) -> L3UnknownReasonV1 {
    L3UnknownReasonV1::CommitFailed
}

/// Map a [`FrontierDeltaError`] the adapter's own keyed-frontier maintenance
/// observed onto the stable Unknown vocabulary. An `InsertConflict` is
/// exactly the `B^uk` unique-key discipline violated — `KeyConflict`, ADR-0012
/// §9 Stage C fixture 7. A stale `RemoveMismatch`/`RemoveMissing` indicates
/// the adapter's own bookkeeping fell out of sync with reality — a genuine
/// adapter-local integrity failure, not a keying collision.
pub fn frontier_conflict_reason<V>(e: &FrontierDeltaError<V>) -> L3UnknownReasonV1 {
    match e {
        FrontierDeltaError::InsertConflict(_) => L3UnknownReasonV1::KeyConflict,
        FrontierDeltaError::RemoveMismatch(_) | FrontierDeltaError::RemoveMissing(_) => {
            L3UnknownReasonV1::AdapterIntegrityFailure
        }
    }
}

// ---------------------------------------------------------------------------
// The semantic result envelope (ADR-0012 §5).
// ---------------------------------------------------------------------------

/// The post-plan status this driver reports (ADR-0012 §5, ⟨D-STATUS⟩):
/// `SaturatedStop` carried through, with `Divergent` remapped to `Unknown`
/// per §5's normative rule and every adapter-local integrity failure also
/// downgrading to `Unknown` regardless of what `run_saturated` reported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettlementStopV1 {
    /// The **only** decided negative (ADR-0012 §5).
    Quiescent {
        /// The identity of the independently re-derivable certificate.
        certificate: QuiescenceCertificateId,
    },
    /// Every other post-plan outcome, named by a stable reason code.
    Unknown {
        /// Why nothing was established.
        reason: L3UnknownReasonV1,
    },
}

/// The frozen marker opening a [`SettlementRunV1`] preimage. Not pinned by
/// ADR-0012's text; chosen here following this crate's marker+version house
/// style (`l3_canon`'s module doc).
pub const L3_RUN_MARKER: &[u8] = b"brix.l3.run";
/// Frozen format version for [`SettlementRunV1`]'s preimage.
pub const L3_RUN_FORMAT_V1: u64 = 1;

/// `SettlementRunV1` (ADR-0012 §5): the required canonical result envelope.
///
/// Field order follows §5's bullet list literally: program, context,
/// presentation, observation profile, initial world/policy, `PlanLimitsV1`,
/// final world/policy, total stop reason (with the `QuiescenceCertificateId`
/// riding inside [`SettlementStopV1::Quiescent`] rather than as a redundant
/// sibling field), ordered committed step digests, and the journal chain
/// digest.
///
/// **Deliberately excluded** (§5): raw-handle `ExecConfig.history`,
/// `SaturationBudget`, `agenda_residue`, cost records, audit reports. See
/// [`L3RunReport`] for all of those, carried as non-semantic diagnostics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettlementRunV1 {
    /// The executable plan revision.
    pub program: ProgramIdV1,
    /// The complete run context's identity (ADR-0012 §3.3).
    pub context: ContextId,
    /// The program/world revision this saturated run was taken against.
    pub presentation: SocPresentationIdV1,
    /// The declared observation boundary's identity.
    pub observation_profile: ObservationProfileId,
    /// The world the run started from.
    pub initial_world: ConfigId,
    /// The policy in force at the start (and, for this profile, throughout —
    /// policy never changes mid-run).
    pub initial_policy: ConfigId,
    /// The exact plan-validation limits (ADR-0012 §3.3 ⟨D-LIM⟩).
    pub limits: PlanLimitsV1,
    /// The world the run ended at.
    pub final_world: ConfigId,
    /// The policy the run ended under.
    pub final_policy: ConfigId,
    /// The total stop reason.
    pub stop: SettlementStopV1,
    /// Every committed step's canonical digest, administrative and
    /// realizing alike, in commit order (this profile never has an
    /// administrative one — §4.1 ⟨D-TAUZERO⟩).
    pub step_digests: Vec<Digest>,
    /// `Journal::chain_digest()` — used only to check that *this* run
    /// replays deterministically, never to decide behavioral agreement with
    /// another run (ADR-0014 risk 3; ADR-0012 §5).
    pub chain_digest: Digest,
}

impl Canonical for SettlementRunV1 {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(L3_RUN_MARKER);
        w.write_uint(L3_RUN_FORMAT_V1);
        w.write_bytes(self.program.digest().as_bytes());
        w.write_bytes(self.context.digest().as_bytes());
        w.write_bytes(self.presentation.digest().as_bytes());
        w.write_bytes(self.observation_profile.digest().as_bytes());
        w.write_bytes(self.initial_world.digest().as_bytes());
        w.write_bytes(self.initial_policy.digest().as_bytes());
        self.limits.canon_write(w);
        w.write_bytes(self.final_world.digest().as_bytes());
        w.write_bytes(self.final_policy.digest().as_bytes());
        match &self.stop {
            SettlementStopV1::Quiescent { certificate } => {
                w.write_enum(0, |w| w.write_bytes(certificate.digest().as_bytes()));
            }
            SettlementStopV1::Unknown { reason } => {
                w.write_enum(1, |w| w.write_uint(reason.ordinal()));
            }
        }
        w.write_list(self.step_digests.iter().map(|d| d.as_bytes().to_vec()));
        w.write_bytes(self.chain_digest.as_bytes());
    }
}

/// Content-addressed identity of a [`SettlementRunV1`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct SettlementRunId(pub Digest);

impl SettlementRunId {
    /// The underlying digest.
    pub fn digest(&self) -> Digest {
        self.0
    }
}

/// The canonical identity of a [`SettlementRunV1`] (ADR-0012 §5, fixture 14:
/// two runs of the same plan under different *sufficient* budgets MUST
/// produce the same identity).
pub fn settlement_run_id(run: &SettlementRunV1) -> SettlementRunId {
    SettlementRunId(run.canon_digest(Domain::Value))
}

// ---------------------------------------------------------------------------
// Admissibility policy choice (ADR-0012 §3.4).
// ---------------------------------------------------------------------------

/// How the admissibility policy for a run is chosen.
///
/// ADR-0012 §3.4: "There is no surface policy language in v1." [`Compiled`]
/// is therefore the **only** production path: [`l3_adm`] over this plan's own
/// compiled regime. [`Override`] exists solely so a test can inject a denying
/// policy (ADR-0012 §5 ⟨D-RESIDUE⟩, §9 Stage C fixture 5; §13: "denial can
/// only be injected by a test") without inventing a surface policy language
/// this profile does not have.
///
/// An override's `adm_id` MUST name the override's own identity, never the
/// compiled policy's: `PresentationV1` binds `adm` and `adm_id` together
/// (ADR-0014 §11 item 5), and reusing the compiled digest for a behaviorally
/// different predicate would silently mislabel every certificate taken
/// against it.
///
/// [`Compiled`]: L3AdmChoice::Compiled
/// [`Override`]: L3AdmChoice::Override
pub enum L3AdmChoice<'a> {
    /// The production path.
    Compiled,
    /// A test/diagnostic override.
    Override {
        /// The overriding admissibility predicate.
        adm: &'a dyn Adm,
        /// Its own canonical identity — never the compiled policy's.
        adm_id: Digest,
    },
}

// ---------------------------------------------------------------------------
// The incremental settlement adapter / calendar keyer (ADR-0012 §2 item 4,
// §4.3).
// ---------------------------------------------------------------------------

/// Fine-grained, non-semantic detail behind
/// [`L3UnknownReasonV1::AdapterIntegrityFailure`]. Never part of any
/// canonical identity (ADR-0012 §5: "human diagnostic text remains outside
/// the semantic identity") — carried only on [`L3RunReport`] for operators.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdapterFailureDetail {
    /// The naive `Regime` relation at the adapter's tracked world disagreed
    /// with the candidate `commit_tick` is about to key (ADR-0012 §4.3 step
    /// 4).
    DifferentialMismatch,
    /// The incrementally maintained `IncrementalEngine` view disagreed with
    /// the expected singleton candidate set.
    IncrementalViewMismatch,
    /// The pure `prospective_successor` did not equal the candidate's
    /// declared successor.
    ProspectiveSuccessorMismatch,
    /// A handle failed to resolve through the run's interner.
    UnresolvedHandle,
    /// A candidate's witness was not one any rule in this table's transition
    /// table proposes.
    UnknownCandidate,
    /// The adapter's own keyed `Frontier` maintenance
    /// ([`Frontier::apply_delta`]) rejected a staged update. Carries the
    /// `Debug` rendering of the [`FrontierDeltaError`] (not its typed value,
    /// to keep this enum free of a generic parameter).
    FrontierIntegrity(String),
}

/// The Stage C incremental settlement adapter (ADR-0012 §2 item 4, §4.3).
///
/// Wrapped in a closure (see [`run_l3_plan_with_interner`]) and handed to
/// `saturate::run_saturated` as the keyer — the only stepping/selection
/// mechanism this driver uses (§2 item 4: "it does not re-implement
/// stepping, selection semantics, or the stop vocabulary"; the returned
/// `SaturatedStop` is the sole authority on why a run ended).
///
/// `commit_tick`'s δ-phase invokes the keyer once per admissible candidate,
/// before selection. This profile's head-only regime proposes **at most one**
/// candidate per world (Stage B), so a keyer call is always for the candidate
/// that will in fact be selected and committed next — there is no second
/// candidate to choose between. That determinism is what licenses doing the
/// full "before every selection" bookkeeping (§4.3 steps 4-5) *inside* one
/// keyer call rather than needing a genuine post-commit hook the current
/// `commit_tick`/`sat_step` API does not expose:
///
/// - re-derive the naive admissible view at the adapter's own tracked
///   `expected_world` (a *second*, independent [`L3Regime`] instance sharing
///   the same immutable [`L3TransitionTable`] — Stage B's dual-regime design)
///   and require it equal `{candidate}` (§4.3 step 4);
/// - advance a genuine [`IncrementalEngine`] by the world delta this step
///   induces and require its resulting view to agree;
/// - cross-check the pure [`prospective_successor`] against the candidate's
///   declared successor (§4.3 step 5);
/// - maintain a keyed [`Frontier<Candidate>`] transactionally via
///   [`Frontier::apply_delta`] (§2 item 4's "incrementally maintained keyed
///   Frontier").
///
/// A disagreement anywhere sets [`Self::failure`]; the run's eventual result
/// is downgraded to `Unknown` regardless of what `run_saturated` itself
/// reported — never dressed as `Quiescent` (§2 item 4).
struct L3StepAdapter<'a> {
    program: ProgramIdV1,
    interner: &'a Interner,
    naive_regime: L3Regime,
    priority_of: BTreeMap<Handle, u64>,
    incremental_engine: IncrementalEngine,
    frontier: Frontier<Candidate>,
    policy_handle: Handle,
    dummy_history: Digest,
    expected_world: Handle,
    /// The cumulative L3-local `apply_counted` probe count — "one
    /// precomputed world-transition-table lookup per touched handle and no
    /// agenda-element probes" (ADR-0012 §4.3), tracked via a direct call to
    /// [`L3Regime::apply_counted`] rather than through
    /// [`IncrementalEngine::step`]'s own (deliberately richer, ADR-0002
    /// §9.1) generic cost formula, which additionally counts one unit per
    /// regime actually applied and one per produced candidate-delta entry —
    /// a different, non-comparable accounting. `IncrementalEngine` still
    /// backs [`Self::incremental_engine`]'s maintained *view* (§2 item 4's
    /// "integrate IncrementalEngine"); this field is what §9 Stage C fixture
    /// 10 actually asserts over.
    l3_apply_probe_total: u64,
    failure: Option<AdapterFailureDetail>,
}

impl<'a> L3StepAdapter<'a> {
    fn new(
        program: ProgramIdV1,
        table: &Rc<L3TransitionTable>,
        interner: &'a Interner,
        policy_handle: Handle,
    ) -> Self {
        let priority_of: BTreeMap<Handle, u64> = (0..table.rule_count())
            .filter_map(|i| table.candidate_at(i).map(|c| (c.witness, i as u64)))
            .collect();
        let naive_regime = L3Regime::new(Rc::clone(table));
        // §4.3 step 2: initialize the incremental engine and apply the
        // initial world delta — bounded setup, not a committed step.
        let mut incremental_engine =
            IncrementalEngine::new(vec![Box::new(L3Regime::new(Rc::clone(table)))]);
        let initial = table.initial_world();
        incremental_engine.step(&Delta::of_added([initial]));

        L3StepAdapter {
            program,
            interner,
            naive_regime,
            priority_of,
            incremental_engine,
            frontier: Frontier::new(),
            policy_handle,
            dummy_history: History::empty().digest(),
            expected_world: initial,
            l3_apply_probe_total: 0,
            failure: None,
        }
    }

    fn probe_config(&self, world: Handle) -> ExecConfig {
        ExecConfig::new(world, self.policy_handle, self.dummy_history)
    }

    /// Resolve `h` through the run's interner, recording (once) an integrity
    /// failure instead of panicking on a miss (ADR-0012 §6: the adapter must
    /// convert a source-derived malformed condition to a failure result).
    fn resolve(&mut self, h: Handle) -> Digest {
        match self.interner.try_resolve(h) {
            Some(d) => d,
            None => {
                self.failure
                    .get_or_insert(AdapterFailureDetail::UnresolvedHandle);
                Digest::of(Domain::Value, b"brix.l3.adapter.unresolved")
            }
        }
    }

    /// ADR-0012 §3.4's tie-break: `H("brix.l3.key@1", plan, resolved(regime),
    /// resolved(witness), resolved(successor))`.
    fn tiebreak(&mut self, candidate: &Candidate) -> Digest {
        let regime_digest = self.resolve(candidate.regime);
        let witness_digest = self.resolve(candidate.witness);
        let successor_digest = self.resolve(candidate.successor);
        let mut w = CanonWriter::new();
        w.write_tag("brix.l3.key@1");
        w.write_bytes(self.program.digest().as_bytes());
        w.write_bytes(regime_digest.as_bytes());
        w.write_bytes(witness_digest.as_bytes());
        w.write_bytes(successor_digest.as_bytes());
        w.digest(Domain::Value)
    }

    /// Compute this step's [`Key`] (ADR-0012 §3.4), performing every required
    /// differential/incremental check as a side effect.
    fn key_for(&mut self, candidate: &Candidate, phase: u64) -> Key {
        // §4.3 step 4: the naive relation at the world this call is *for*
        // (tracked by the adapter — see the struct doc's determinism
        // argument) must offer exactly this one candidate.
        let naive = self
            .naive_regime
            .candidates(&self.probe_config(self.expected_world));
        if naive != vec![*candidate] {
            self.failure
                .get_or_insert(AdapterFailureDetail::DifferentialMismatch);
        }

        let expected_view: BTreeSet<Candidate> = BTreeSet::from([*candidate]);
        if self.incremental_engine.view() != &expected_view {
            self.failure
                .get_or_insert(AdapterFailureDetail::IncrementalViewMismatch);
        }

        let priority = match self.priority_of.get(&candidate.witness) {
            Some(p) => *p,
            None => {
                self.failure
                    .get_or_insert(AdapterFailureDetail::UnknownCandidate);
                u64::MAX
            }
        };
        let tiebreak = self.tiebreak(candidate);
        let key = Key::new(phase, priority, tiebreak);

        // §4.3 step 5: cross-check the pure prospective successor.
        let prospective = prospective_successor(&self.probe_config(self.expected_world), candidate);
        if prospective.world != candidate.successor {
            self.failure
                .get_or_insert(AdapterFailureDetail::ProspectiveSuccessorMismatch);
        }

        // Maintain the keyed frontier transactionally: the previous call's
        // entry (if any) is now stale — its candidate has just been
        // committed by the time `commit_tick` calls us again — and this
        // candidate takes its place.
        let removals: Vec<(Key, Candidate)> = self
            .frontier
            .peek_least()
            .map(|(k, c)| vec![(*k, *c)])
            .unwrap_or_default();
        if let Err(e) = self.frontier.apply_delta(&removals, &[(key, *candidate)]) {
            self.failure
                .get_or_insert(AdapterFailureDetail::FrontierIntegrity(format!("{e:?}")));
        }

        // Advance the incremental engine by the delta this step induces —
        // eagerly, because a head-only regime's sole candidate is always the
        // one `select_K` goes on to commit (there is never a second to
        // choose between), so there is no later hook at which to do this
        // instead (see the struct doc).
        let delta = Delta::between_worlds(self.expected_world, candidate.successor);

        // The L3-local probe count (ADR-0012 §4.3), computed directly via
        // `L3Regime::apply_counted` against a read-only regime instance
        // sharing the same immutable table — independent of, and compared
        // against, `IncrementalEngine::step`'s own generic bookkeeping below.
        let (l3_delta, probes) = self.naive_regime.apply_counted(&delta);
        self.l3_apply_probe_total = self.l3_apply_probe_total.saturating_add(probes);

        let report = self.incremental_engine.step(&delta);
        if report.candidate_delta != l3_delta {
            self.failure
                .get_or_insert(AdapterFailureDetail::IncrementalViewMismatch);
        }

        self.expected_world = candidate.successor;
        key
    }
}

// ---------------------------------------------------------------------------
// The driver (ADR-0012 §4.3).
// ---------------------------------------------------------------------------

/// The canonical digest of the ordered one-element regime set this profile's
/// presentation declares (ADR-0012 §3.4: "`PresentationV1.regime_set` [is]
/// the canonical digest of the ordered one-element regime set").
///
/// ADR-0012 §3.1 pins that the implementation "MAY choose a concrete encoder
/// shape, but MUST freeze it with independent vectors" — the byte layout
/// itself is not pinned by the ADR text for this specific field; this module
/// fixes marker + version + an ordered list of one `RegimeId` digest.
fn l3_regime_set_digest(table: &L3TransitionTable) -> Digest {
    let mut w = CanonWriter::new();
    w.write_bytes(b"brix.l3.regime-set");
    w.write_uint(1u64);
    w.write_list(std::iter::once(
        table.regime_id().digest().as_bytes().to_vec(),
    ));
    w.digest(Domain::Value)
}

fn finalize_stop(
    stop: SaturatedStop,
    failure: Option<AdapterFailureDetail>,
) -> (
    SettlementStopV1,
    Option<QuiescenceCertificateV1>,
    Option<DivergenceCertificateV1>,
) {
    if failure.is_some() {
        // ADR-0012 §2 item 4: an adapter-local integrity failure is reported
        // as its own Unknown reason and MUST NOT be dressed as Quiescent,
        // regardless of what `run_saturated` itself concluded.
        return (
            SettlementStopV1::Unknown {
                reason: L3UnknownReasonV1::AdapterIntegrityFailure,
            },
            None,
            None,
        );
    }
    match stop {
        SaturatedStop::Quiescent(cert) => {
            let id = quiescence_certificate_id(&cert);
            (
                SettlementStopV1::Quiescent { certificate: id },
                Some(*cert),
                None,
            )
        }
        SaturatedStop::Divergent(cert) => (
            // ADR-0012 §5: "If Divergent is ever observed, the adapter MUST
            // report an adapter integrity failure ... MUST NOT discard or
            // suppress the certificate; it is retained as a diagnostic."
            SettlementStopV1::Unknown {
                reason: L3UnknownReasonV1::DivergenceObserved,
            },
            None,
            Some(*cert),
        ),
        SaturatedStop::Unknown(reason) => (
            SettlementStopV1::Unknown {
                reason: map_saturation_unknown(&reason),
            },
            None,
            None,
        ),
    }
}

/// The operational result of one Stage C run: the semantic [`SettlementRunV1`]
/// plus every non-semantic diagnostic ADR-0012 §5 excludes from it.
pub struct L3RunReport {
    /// The canonically-identified semantic result (ADR-0012 §5).
    pub run: SettlementRunV1,
    /// The full, untranslated `SaturatedStop` `run_saturated` returned —
    /// carries the certificate/budget/step-count detail `SettlementRunV1`
    /// deliberately excludes.
    pub raw_stop: SaturatedStop,
    /// The execution budget this run executed under. Excluded from
    /// `SettlementRunV1`'s identity by ⟨D-LIM⟩.
    pub budget: SaturationBudget,
    /// The count of rules still pending at the terminal world (ADR-0012 §5
    /// ⟨D-RESIDUE⟩). Nonzero only when the agenda was denied, not exhausted.
    pub agenda_residue: u64,
    /// One [`CostRecord`] per saturated step `run_saturated` took.
    pub costs: Vec<CostRecord>,
    /// The quiescence certificate itself, when the run reached `Quiescent`
    /// (and no adapter integrity failure was detected).
    pub quiescence_certificate: Option<QuiescenceCertificateV1>,
    /// A divergence certificate, retained as a diagnostic, if one was ever
    /// observed (ADR-0012 §5 — structurally unreachable for a valid plan,
    /// §9 Stage C fixture 13).
    pub divergence_certificate: Option<DivergenceCertificateV1>,
    /// Fine-grained adapter-integrity-failure detail, if any.
    pub adapter_failure: Option<AdapterFailureDetail>,
    /// The full committed journal (administrative and realizing steps
    /// alike — this profile never has an administrative one).
    pub journal: Journal,
    /// The configuration the run ended at (raw handles — non-canonical,
    /// valid only relative to the interner this run used).
    pub final_config: ExecConfig,
    /// The precomputed transition table this run executed over, retained so
    /// a caller can re-derive an equivalent presentation for independent
    /// checks (ADR-0012 §9 Stage C fixture 12).
    pub table: Rc<L3TransitionTable>,
    /// The presentation's declared regime-set identity.
    pub regime_set: Digest,
    /// The presentation's declared admissibility-policy identity.
    pub adm_id: Digest,
    /// The exact context every committed judgement in this run is indexed
    /// by.
    pub context: ContextId,
    /// The declared observation boundary this run executed under.
    pub observation_profile: GeneratorPartitionProfile,
}

/// Drive one Stage C run of `plan` over a fresh [`Interner`] (ADR-0012 §4.3
/// step 1). Convenience wrapper over
/// [`run_l3_plan_with_interner`] for a caller that does not need to control
/// interner insertion order (see that function's doc for why a caller ever
/// would — ADR-0012 §9 Stage C fixture 11).
pub fn run_l3_plan(
    plan: &L3PlanV1,
    adm_choice: L3AdmChoice<'_>,
    budget: SaturationBudget,
) -> L3RunReport {
    let mut interner = Interner::new();
    run_l3_plan_with_interner(&mut interner, plan, adm_choice, budget)
}

/// Drive one Stage C run of `plan` over the caller-supplied `interner`
/// (ADR-0012 §4.3). Implements the whole loop:
///
/// 1. intern plan/world/policy/regime/witness identities and construct
///    `ExecConfig` (§4.3 step 1);
/// 2. initialize the incremental adapter over the initial world delta (§4.3
///    step 2, inside [`L3StepAdapter::new`]);
/// 3. drive `saturate::run_saturated` with the adapter as keyer (§4.3 step
///    3) — the sole stepping/selection/stop-vocabulary authority;
/// 4. steps 4-5 happen inside [`L3StepAdapter::key_for`], called once per
///    admissible candidate by `commit_tick`'s δ-phase;
/// 5. package the returned [`SaturatedRun`] into the semantic/diagnostic
///    split of ADR-0012 §5.
pub fn run_l3_plan_with_interner(
    interner: &mut Interner,
    plan: &L3PlanV1,
    adm_choice: L3AdmChoice<'_>,
    budget: SaturationBudget,
) -> L3RunReport {
    let program = program_id(plan);
    let table = Rc::new(build_l3_transition_table(interner, plan));

    let compiled_policy = l3_policy(program, &table);
    let compiled_adm = l3_adm(&table);
    let (adm, adm_id): (&dyn Adm, Digest) = match &adm_choice {
        L3AdmChoice::Compiled => (&compiled_adm, policy_id(&compiled_policy).digest()),
        L3AdmChoice::Override { adm, adm_id } => (*adm, *adm_id),
    };

    let policy_config = ConfigId(adm_id);
    let policy_handle = interner.intern(policy_config.digest());

    let run_context = RunContextV1 {
        program,
        initial_world: table.initial_world_config(),
        policy: policy_config,
        profile: L3_PROFILE_MARKER_V1.to_string(),
        limits: plan.limits,
    };
    let context = context_id(&run_context);

    let observation_profile = build_l3_observation_profile(&table);
    let observation_profile_id = observation_profile.id();

    let presentation_id = SocPresentationIdV1(program.digest());
    let regime_set = l3_regime_set_digest(&table);

    // No further mutation of `interner` beyond this point: everything else
    // only resolves handles this run has already interned.
    let interner_ref: &Interner = &*interner;

    let presentation_regime = L3Regime::new(Rc::clone(&table));
    let regimes: [&dyn SettlementRegime; 1] = [&presentation_regime];

    let presentation = PresentationV1 {
        id: presentation_id,
        regimes: &regimes,
        regime_set,
        adm,
        adm_id,
        profile: &observation_profile,
        interner: interner_ref,
        context,
        // P1 holds structurally: `L3Regime::candidates` reads only
        // `e.world`, and `AdmRegimeAllowlist::admits` reads only
        // `c.regime` — neither depends on `e.history`. P6 holds
        // structurally too: the key formula's priority/tiebreak read only
        // the plan/candidate, never `phase`.
        assumptions: DeclaredAssumptions::all(),
    };

    let e0 = ExecConfig::new(
        table.initial_world(),
        policy_handle,
        History::empty().digest(),
    );

    let mut adapter = L3StepAdapter::new(program, &table, interner_ref, policy_handle);
    let run: SaturatedRun = {
        let mut keyer = |c: &Candidate, phase: u64| adapter.key_for(c, phase);
        run_saturated(&presentation, e0, &mut keyer, budget)
    };

    let final_world_digest = interner_ref
        .try_resolve(run.final_config.world)
        .unwrap_or_else(|| Digest::of(Domain::Value, b"brix.l3.adapter.unresolved-final-world"));
    let final_policy_digest = interner_ref
        .try_resolve(run.final_config.policy)
        .unwrap_or(adm_id);

    let agenda_residue = table
        .world_index_of(run.final_config.world)
        .map(|idx| table.rule_count().saturating_sub(idx) as u64)
        .unwrap_or(table.rule_count() as u64);

    let stop_for_result = run.stop.clone();
    let (stop, quiescence_certificate, divergence_certificate) =
        finalize_stop(stop_for_result, adapter.failure.clone());

    let step_digests: Vec<Digest> = run
        .journal
        .steps()
        .iter()
        .map(|s| s.canon_digest(Domain::Value))
        .collect();
    let chain_digest = run.journal.chain_digest();

    let settlement = SettlementRunV1 {
        program,
        context,
        presentation: presentation_id,
        observation_profile: observation_profile_id,
        initial_world: table.initial_world_config(),
        initial_policy: policy_config,
        limits: plan.limits,
        final_world: ConfigId(final_world_digest),
        final_policy: ConfigId(final_policy_digest),
        stop,
        step_digests,
        chain_digest,
    };

    L3RunReport {
        run: settlement,
        raw_stop: run.stop,
        budget,
        agenda_residue,
        costs: run.costs,
        quiescence_certificate,
        divergence_certificate,
        adapter_failure: adapter.failure,
        journal: run.journal,
        final_config: run.final_config,
        table,
        regime_set,
        adm_id,
        context,
        observation_profile,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::l3::{lower_l3_plan, L3LowerError};
    use soc_core::adm::AdmNone;
    use soc_core::intern::Interner;
    use soc_core::journal::CommittedStep;
    use soc_core::saturate::{
        check_quiescence_certificate, sat_step, CertificateCheck, CertificateCheckError, StepLabel,
    };

    fn plan(src: &str) -> L3PlanV1 {
        let module = brix_syntax::parse(src).unwrap_or_else(|e| panic!("parse failed: {e}"));
        lower_l3_plan(&module, L3_PROFILE_MARKER_V1, &PlanLimitsV1::generous())
            .unwrap_or_else(|e| panic!("lowering failed: {e:?}"))
    }

    fn generous_budget() -> SaturationBudget {
        SaturationBudget::uniform(1_000)
    }

    /// Build the same presentation [`run_l3_plan_with_interner`] would, over
    /// a caller-owned interner, for tests that need direct access to
    /// `sat_step`/`check_quiescence_certificate`/tamper scenarios. Kept
    /// independent of the production driver's internals (it goes through
    /// only the same public building blocks the driver uses), which is
    /// exactly what "independent" means for fixture 12: independent of the
    /// *run's own claim*, not of the construction code.
    struct Setup {
        interner: Interner,
        table: Rc<L3TransitionTable>,
        program: ProgramIdV1,
        regime: L3Regime,
        profile: GeneratorPartitionProfile,
        adm: AdmChoiceOwned,
        adm_id: Digest,
        context: ContextId,
        e0: ExecConfig,
    }

    enum AdmChoiceOwned {
        Compiled(soc_core::adm::AdmRegimeAllowlist),
        DenyAll,
    }

    impl Adm for AdmChoiceOwned {
        fn admits(&self, e: &ExecConfig, c: &Candidate) -> bool {
            match self {
                AdmChoiceOwned::Compiled(a) => a.admits(e, c),
                AdmChoiceOwned::DenyAll => false,
            }
        }
    }

    fn setup(src: &str, deny: bool) -> Setup {
        let p = plan(src);
        let mut interner = Interner::new();
        let program = program_id(&p);
        let table = Rc::new(build_l3_transition_table(&mut interner, &p));
        let (adm, adm_id) = if deny {
            (
                AdmChoiceOwned::DenyAll,
                Digest::of(Domain::Value, b"test.l3.deny-all@1"),
            )
        } else {
            let compiled_policy = l3_policy(program, &table);
            (
                AdmChoiceOwned::Compiled(l3_adm(&table)),
                policy_id(&compiled_policy).digest(),
            )
        };
        let policy_config = ConfigId(adm_id);
        let policy_handle = interner.intern(policy_config.digest());
        let run_context = RunContextV1 {
            program,
            initial_world: table.initial_world_config(),
            policy: policy_config,
            profile: L3_PROFILE_MARKER_V1.to_string(),
            limits: p.limits,
        };
        let context = context_id(&run_context);
        let profile = build_l3_observation_profile(&table);
        let regime = L3Regime::new(Rc::clone(&table));
        let e0 = ExecConfig::new(
            table.initial_world(),
            policy_handle,
            History::empty().digest(),
        );
        Setup {
            interner,
            table,
            program,
            regime,
            profile,
            adm,
            adm_id,
            context,
            e0,
        }
    }

    /// Build the `PresentationV1` for `s`, over a caller-owned one-element
    /// `regimes` array. Deliberately a free function taking that array by
    /// reference (rather than a `Setup` method constructing it internally):
    /// a `PresentationV1<'a>` borrows its `regimes` slice, so the array must
    /// live in the *caller's* stack frame for at least as long as the
    /// returned presentation — exactly how [`run_l3_plan_with_interner`]
    /// keeps its own `regimes` array and `PresentationV1` in one scope.
    fn build_presentation<'a>(
        s: &'a Setup,
        regimes: &'a [&'a dyn SettlementRegime],
    ) -> PresentationV1<'a> {
        PresentationV1 {
            id: SocPresentationIdV1(s.program.digest()),
            regimes,
            regime_set: l3_regime_set_digest(&s.table),
            adm: &s.adm,
            adm_id: s.adm_id,
            profile: &s.profile,
            interner: &s.interner,
            context: s.context,
            assumptions: DeclaredAssumptions::all(),
        }
    }

    // -----------------------------------------------------------------
    // Fixture 1: empty-rule module returns Quiescent with an empty journal
    // and a certificate that verifies.
    // -----------------------------------------------------------------

    #[test]
    fn fixture_1_empty_plan_is_quiescent_with_empty_journal_and_verifying_certificate() {
        let p = plan("let a = 1\n");
        let report = run_l3_plan(&p, L3AdmChoice::Compiled, generous_budget());
        assert!(report.journal.is_empty());
        let SettlementStopV1::Quiescent { certificate } = report.run.stop.clone() else {
            panic!("expected Quiescent, got {:?}", report.run.stop);
        };
        let cert = report
            .quiescence_certificate
            .clone()
            .expect("Quiescent carries a certificate");
        assert_eq!(quiescence_certificate_id(&cert), certificate);
        assert!(report.adapter_failure.is_none());
    }

    // -----------------------------------------------------------------
    // Fixture 2: two rules with a sufficient budget.
    // -----------------------------------------------------------------

    #[test]
    fn fixture_2_two_rules_quiescent_two_observations_two_step_journal_reproducible_replay() {
        let p = plan("rule a() = 1\nrule b() = 2\n");
        let report = run_l3_plan(&p, L3AdmChoice::Compiled, generous_budget());
        assert!(matches!(
            report.run.stop,
            SettlementStopV1::Quiescent { .. }
        ));
        assert_eq!(report.journal.len(), 2);
        assert_eq!(report.run.step_digests.len(), 2);

        // Reproducible replay chain.
        assert_eq!(
            Journal::replay_chain(report.journal.steps()),
            report.journal.step_digests()
        );
        assert_eq!(
            report.journal.step_digests().last().copied(),
            Some(report.run.chain_digest)
        );
    }

    // -----------------------------------------------------------------
    // Fixture 3: max_visible_steps = 1 yields Unknown(VisibleBudgetExhausted),
    // one committed record, and NO certificate of any kind.
    // -----------------------------------------------------------------

    #[test]
    fn fixture_3_budget_of_one_yields_no_certificate_of_any_kind() {
        let p = plan("rule a() = 1\nrule b() = 2\n");
        let report = run_l3_plan(&p, L3AdmChoice::Compiled, SaturationBudget::uniform(1));
        assert_eq!(
            report.run.stop,
            SettlementStopV1::Unknown {
                reason: L3UnknownReasonV1::VisibleBudgetExhausted
            }
        );
        assert_eq!(report.journal.len(), 1, "one committed record");
        assert!(
            report.quiescence_certificate.is_none(),
            "no quiescence certificate of any kind"
        );
        assert!(
            report.divergence_certificate.is_none(),
            "no divergence certificate of any kind"
        );
        assert!(matches!(
            report.raw_stop,
            SaturatedStop::Unknown(SaturationUnknown::VisibleBudgetExhausted { .. })
        ));
    }

    // -----------------------------------------------------------------
    // Fixture 4: max_visible_steps = 0 establishes nothing, even for an
    // empty agenda.
    // -----------------------------------------------------------------

    #[test]
    fn fixture_4_zero_budget_establishes_nothing_even_for_an_empty_plan() {
        let two_rules = plan("rule a() = 1\nrule b() = 2\n");
        let empty = plan("let a = 1\n");
        for p in [&two_rules, &empty] {
            let report = run_l3_plan(p, L3AdmChoice::Compiled, SaturationBudget::uniform(0));
            assert_eq!(
                report.run.stop,
                SettlementStopV1::Unknown {
                    reason: L3UnknownReasonV1::VisibleBudgetExhausted
                }
            );
            assert!(report.journal.is_empty());
            assert!(report.quiescence_certificate.is_none());
        }
    }

    // -----------------------------------------------------------------
    // Fixture 5: a deliberately denying policy is genuine, qualified
    // quiescence.
    // -----------------------------------------------------------------

    #[test]
    fn fixture_5_denying_policy_is_quiescent_with_positive_agenda_residue() {
        let p = plan("rule a() = 1\nrule b() = 2\n");
        let deny_id = Digest::of(Domain::Value, b"test.l3.deny-all@1");
        let report = run_l3_plan(
            &p,
            L3AdmChoice::Override {
                adm: &AdmNone,
                adm_id: deny_id,
            },
            generous_budget(),
        );
        assert!(matches!(
            report.run.stop,
            SettlementStopV1::Quiescent { .. }
        ));
        let cert = report
            .quiescence_certificate
            .as_ref()
            .expect("Quiescent carries a certificate");
        // Never an unqualified success: the operational report exposes the
        // residue distinctly from the certificate's own (scoped) claim, and
        // the certificate itself still asserts a complete enumeration — it
        // is a genuine, correctly-scoped quiescence claim under this denying
        // policy, not a downgraded or partial one (ADR-0012 §5 ⟨D-RESIDUE⟩).
        assert_eq!(
            cert.enumeration,
            soc_core::saturate::EnumerationCompleteness::Complete
        );
        assert_eq!(report.agenda_residue, 2, "both rules were never admitted");
        assert!(report.journal.is_empty(), "nothing was ever admitted");
    }

    // -----------------------------------------------------------------
    // Fixture 6: unsupported/parameterized/non-static rules reject before
    // any journal is created.
    // -----------------------------------------------------------------

    #[test]
    fn fixture_6_parameterized_rule_rejects_before_any_journal_exists() {
        let module = brix_syntax::parse("rule r(x: Int) = x\n").unwrap();
        let err = lower_l3_plan(&module, L3_PROFILE_MARKER_V1, &PlanLimitsV1::generous())
            .expect_err("a parameterized rule must reject");
        assert!(matches!(err, L3LowerError::ParameterizedRule(_)));
        // No L3PlanV1 exists at all, so no run_l3_plan call is even
        // reachable — Rejected carries no fabricated identity (ADR-0012 §5).
    }

    #[test]
    fn fixture_6_non_static_rule_body_rejects_before_any_journal_exists() {
        let module = brix_syntax::parse("rule r() = 1 + 2\n").unwrap();
        let err = lower_l3_plan(&module, L3_PROFILE_MARKER_V1, &PlanLimitsV1::generous())
            .expect_err("arithmetic must reject");
        assert!(matches!(err, L3LowerError::ArithmeticNotAllowed));
    }

    // -----------------------------------------------------------------
    // Fixture 7: a deliberately colliding calendar key fails closed via the
    // adapter's own keyed-frontier maintenance.
    // -----------------------------------------------------------------

    #[test]
    fn fixture_7_colliding_calendar_keys_fail_closed_as_key_conflict() {
        // Exercise the adapter's own transactional frontier directly: two
        // distinct candidates proposed at the same key is exactly the B^uk
        // discipline `Frontier::apply_delta` (ADR-0012 §2 item 6/§9 Stage C
        // fixture 7) is built to catch, never silently resolved.
        let mut i = Interner::new();
        let regime = i.intern(Digest::of(Domain::Value, b"r"));
        let w0 = i.intern(Digest::of(Domain::Value, b"w0"));
        let w1 = i.intern(Digest::of(Domain::Value, b"w1"));
        let w2 = i.intern(Digest::of(Domain::Value, b"w2"));
        let key = Key::new(0, 0, Digest::of(Domain::Value, b"collide"));
        let c1 = Candidate {
            regime,
            witness: i.intern(Digest::of(Domain::Value, b"wit1")),
            successor: w1,
        };
        let c2 = Candidate {
            regime,
            witness: i.intern(Digest::of(Domain::Value, b"wit2")),
            successor: w2,
        };
        let _ = w0;

        let mut frontier: Frontier<Candidate> = Frontier::new();
        frontier.apply_delta(&[], &[(key, c1)]).unwrap();
        let err = frontier
            .apply_delta(&[], &[(key, c2)])
            .expect_err("two distinct candidates at one key must be rejected");
        assert_eq!(
            frontier_conflict_reason(&err),
            L3UnknownReasonV1::KeyConflict
        );
        match err {
            FrontierDeltaError::InsertConflict(_) => {}
            other => panic!("expected InsertConflict, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // Fixture 8: a candidate-witness/generator mismatch, wrong
    // transition-table candidate, or wrong decomposition endpoint fails
    // before Derived publication.
    // -----------------------------------------------------------------

    #[test]
    fn fixture_8_malformed_conditions_fail_before_derived_publication_via_try_commit_selected() {
        // Stage B's `l3_regime.rs` already proves each of §6.3's four
        // conditions independently through `L3Regime::try_decompose`. Stage
        // C's own obligation is the driver-level one: confirm the resulting
        // `CommitError` (whichever of the four) maps onto this driver's
        // stable `CommitFailed` reason, and that `try_commit_selected` itself
        // never constructs a `CommittedStep`/`Derived` judgement on the
        // rejecting path — i.e. this fails *before* Derived publication, not
        // merely alongside it (`try_commit_selected` short-circuits via `?`
        // before either is built, soc-core `commit.rs`).
        let s = setup("rule a() = 1\n", false);
        let w0 = s.table.initial_world();
        let real_candidate = s
            .table
            .candidate_at(0)
            .expect("rule a proposes one candidate");
        let mut interner = s.interner.clone();
        let mut corrupted = real_candidate;
        // A candidate matching everything except its successor is
        // CandidateMismatch — condition 1 of §6.3 — using a handle freshly
        // interned from the SAME interner, guaranteed distinct from every
        // handle the table already produced.
        corrupted.successor = interner.intern(Digest::of(
            Domain::Value,
            b"fixture-8-not-the-real-successor",
        ));

        let e0 = ExecConfig::new(w0, s.e0.policy, s.e0.history);
        let key = Key::new(0, 0, Digest::of(Domain::Value, b"fixture-8-key"));
        let err = soc_core::commit::try_commit_selected(
            key, &corrupted, &s.regime, &interner, &e0, s.context,
        )
        .expect_err("a candidate not matching the transition table must be rejected");
        assert_eq!(err, CommitError::CandidateMismatch);
        assert_eq!(commit_error_reason(&err), L3UnknownReasonV1::CommitFailed);
    }

    // -----------------------------------------------------------------
    // Fixture 9: incremental candidates/selected key equal the naive
    // relation after every step; hidden_steps == 0 on every step.
    // -----------------------------------------------------------------

    #[test]
    fn fixture_9_differential_equality_and_zero_hidden_steps_on_every_step() {
        let s = setup("rule a() = 1\nrule b() = 2\nrule c() = 3\n", false);
        let regimes: [&dyn SettlementRegime; 1] = [&s.regime];
        let pres = build_presentation(&s, &regimes);
        let mut adapter = L3StepAdapter::new(s.program, &s.table, &s.interner, s.e0.policy);
        let mut keyer = |c: &Candidate, phase: u64| adapter.key_for(c, phase);

        let mut current = s.e0;
        let mut phase = 0u64;
        for _ in 0..s.table.rule_count() {
            let (step, consumed, _cost) =
                sat_step(&pres, &current, phase, &mut keyer, generous_budget());
            phase += consumed.len() as u64;
            match step {
                soc_core::saturate::SaturatedStep::Realizing {
                    successor,
                    hidden_steps,
                    ..
                } => {
                    assert_eq!(hidden_steps, 0, "𝒢_τ = ∅: no step ever hides anything");
                    current = successor;
                }
                other => panic!("expected Realizing, got {other:?}"),
            }
        }
        assert!(
            adapter.failure.is_none(),
            "no differential mismatch ever detected"
        );
    }

    // -----------------------------------------------------------------
    // Fixture 10: trailing agenda ballast leaves one head commit's core
    // work / apply probe count unchanged.
    // -----------------------------------------------------------------

    #[test]
    fn fixture_10_trailing_ballast_leaves_one_head_commit_probe_count_unchanged() {
        let small = setup("rule a() = 1\n", false);
        let mut rules = String::new();
        rules.push_str("rule a() = 1\n");
        for i in 0..200 {
            rules.push_str(&format!("rule ballast{i}() = {i}\n"));
        }
        let large = setup(&rules, false);

        for s in [&small, &large] {
            let regimes: [&dyn SettlementRegime; 1] = [&s.regime];
            let pres = build_presentation(s, &regimes);
            let mut adapter = L3StepAdapter::new(s.program, &s.table, &s.interner, s.e0.policy);
            let mut keyer = |c: &Candidate, phase: u64| adapter.key_for(c, phase);
            let _ = sat_step(&pres, &s.e0, 0, &mut keyer, SaturationBudget::uniform(1));
            // `between_worlds` touches exactly two handles regardless of how
            // many rules remain (ADR-0012 §4.3; mirrors l3_regime.rs's own
            // `apply_counted_pays_exactly_one_probe_per_touched_handle`).
            assert_eq!(
                adapter.l3_apply_probe_total, 2,
                "one head commit's L3-local apply probe count must not scale with trailing ballast"
            );
        }
    }

    // -----------------------------------------------------------------
    // Fixture 11: cross-interner-order identity stability.
    // -----------------------------------------------------------------

    #[test]
    fn fixture_11_cross_interner_order_identities_match() {
        let p = plan("rule a() = 1\nrule b() = 2\n");

        let mut i1 = Interner::new();
        let report1 =
            run_l3_plan_with_interner(&mut i1, &p, L3AdmChoice::Compiled, generous_budget());

        let mut i2 = Interner::new();
        i2.intern(Digest::of(Domain::Value, b"noise-before"));
        let report2 =
            run_l3_plan_with_interner(&mut i2, &p, L3AdmChoice::Compiled, generous_budget());
        i2.intern(Digest::of(Domain::Value, b"noise-after"));

        assert_eq!(report1.run.program, report2.run.program);
        assert_eq!(report1.run.presentation, report2.run.presentation);
        assert_eq!(
            report1.run.observation_profile,
            report2.run.observation_profile
        );
        assert_eq!(
            settlement_run_id(&report1.run),
            settlement_run_id(&report2.run)
        );

        let SettlementStopV1::Quiescent { certificate: c1 } = report1.run.stop else {
            panic!("expected Quiescent");
        };
        let SettlementStopV1::Quiescent { certificate: c2 } = report2.run.stop else {
            panic!("expected Quiescent");
        };
        assert_eq!(c1, c2);
    }

    // -----------------------------------------------------------------
    // Fixture 12: independent certificate verification; the agenda/frontier
    // coincidence is asserted, not assumed; every tamper target yields
    // Unknown, never a pass.
    // -----------------------------------------------------------------

    #[test]
    fn fixture_12_certificate_independently_verifies_and_agenda_frontier_coincide() {
        let s = setup("rule a() = 1\nrule b() = 2\n", false);
        let regimes: [&dyn SettlementRegime; 1] = [&s.regime];
        let pres = build_presentation(&s, &regimes);
        let mut adapter = L3StepAdapter::new(s.program, &s.table, &s.interner, s.e0.policy);
        let run = {
            let mut keyer = |c: &Candidate, phase: u64| adapter.key_for(c, phase);
            run_saturated(&pres, s.e0, &mut keyer, generous_budget())
        };
        let SaturatedStop::Quiescent(cert) = run.stop.clone() else {
            panic!("expected Quiescent, got {:?}", run.stop);
        };

        // Independent observation: the agenda is empty at the terminal
        // world under a FRESH regime instance sharing the same table.
        let fresh_regime = L3Regime::new(Rc::clone(&s.table));
        assert!(fresh_regime.candidates(&run.final_config).is_empty());

        // The certificate re-derives (𝒢_τ = ∅ ⇒ the hidden prefix is always
        // empty — the last sat_step call finds Quiescent on its very first
        // commit_tick call).
        let check = check_quiescence_certificate(&cert, &pres, &run.final_config, &[]);
        assert!(check.is_verified());

        // Tamper target 1: journal prefix (supply a non-empty fake prefix).
        let fake_step: CommittedStep = run.journal.steps()[0].clone();
        let tampered = check_quiescence_certificate(&cert, &pres, &run.final_config, &[fake_step]);
        assert!(!tampered.is_verified());

        // Tamper target 2: terminal world (check against the initial world).
        let tampered = check_quiescence_certificate(&cert, &pres, &s.e0, &[]);
        assert!(!tampered.is_verified());

        // Tamper target 3: regime set.
        let mut bad_regime_set = pres_clone(&pres);
        bad_regime_set.regime_set = Digest::of(Domain::Value, b"wrong-regime-set");
        let tampered = check_quiescence_certificate(&cert, &bad_regime_set, &run.final_config, &[]);
        assert_eq!(
            tampered,
            CertificateCheck::Unknown(CertificateCheckError::RegimeSetMismatch)
        );

        // Tamper target 4: Adm identity.
        let mut bad_adm_id = pres_clone(&pres);
        bad_adm_id.adm_id = Digest::of(Domain::Value, b"wrong-adm-id");
        let tampered = check_quiescence_certificate(&cert, &bad_adm_id, &run.final_config, &[]);
        assert_eq!(
            tampered,
            CertificateCheck::Unknown(CertificateCheckError::AdmMismatch)
        );

        // Tamper target 5: context.
        let mut bad_context = pres_clone(&pres);
        bad_context.context = ContextId::root();
        let tampered = check_quiescence_certificate(&cert, &bad_context, &run.final_config, &[]);
        assert!(!tampered.is_verified());

        // Tamper target 6: observation-profile revision.
        let other_profile =
            GeneratorPartitionProfile::all_realizing(std::collections::BTreeSet::new());
        let mut bad_profile = pres_clone(&pres);
        bad_profile.profile = &other_profile;
        let tampered = check_quiescence_certificate(&cert, &bad_profile, &run.final_config, &[]);
        assert!(!tampered.is_verified());

        // Tamper target 7: presentation revision.
        let mut bad_presentation_id = pres_clone(&pres);
        bad_presentation_id.id = SocPresentationIdV1(Digest::of(Domain::Value, b"wrong-revision"));
        let tampered =
            check_quiescence_certificate(&cert, &bad_presentation_id, &run.final_config, &[]);
        assert!(!tampered.is_verified());

        assert!(adapter.failure.is_none());
    }

    /// A shallow copy of a [`PresentationV1`] for the tamper fixtures — every
    /// field is `Copy` or a shared reference, so this never re-derives
    /// anything; it just lets one test mutate a single field at a time.
    fn pres_clone<'a>(p: &PresentationV1<'a>) -> PresentationV1<'a> {
        PresentationV1 {
            id: p.id,
            regimes: p.regimes,
            regime_set: p.regime_set,
            adm: p.adm,
            adm_id: p.adm_id,
            profile: p.profile,
            interner: p.interner,
            context: p.context,
            assumptions: p.assumptions,
        }
    }

    // -----------------------------------------------------------------
    // Fixture 13: no run of any valid plan returns Divergent, and no
    // committed step ever labels Administrative.
    // -----------------------------------------------------------------

    #[test]
    fn fixture_13_no_run_diverges_and_no_step_is_administrative() {
        for src in [
            "let a = 1\n",
            "rule a() = 1\n",
            "rule a() = 1\nrule b() = 2\n",
            "rule a() = 1\nrule b() = 2\nrule c() = 3\n",
        ] {
            let p = plan(src);
            let report = run_l3_plan(&p, L3AdmChoice::Compiled, generous_budget());
            assert!(
                !matches!(report.raw_stop, SaturatedStop::Divergent(_)),
                "no valid plan may ever diverge"
            );
            for step in report.journal.steps() {
                assert_eq!(
                    report.observation_profile.label(step),
                    Ok(StepLabel::Realizing),
                    "no committed step may ever label Administrative"
                );
            }
        }
    }

    // -----------------------------------------------------------------
    // Fixture 14: two runs under different sufficient budgets share one
    // SettlementRunV1 identity and one QuiescenceCertificateId.
    // -----------------------------------------------------------------

    #[test]
    fn fixture_14_different_sufficient_budgets_share_one_identity() {
        let p = plan("rule a() = 1\nrule b() = 2\n");
        // The minimum *sufficient* budget for N=2 is N+1=3: `run_saturated`
        // checks `max_visible_steps` before each `sat_step` call, so one
        // extra visible-step allowance beyond the two real commits is what
        // lets the final call actually observe (and certify) quiescence
        // (ADR-0012 §4.3's zero-budget-establishes-nothing rule, generalized
        // — see fixture 4).
        let a = run_l3_plan(&p, L3AdmChoice::Compiled, SaturationBudget::uniform(3));
        let b = run_l3_plan(&p, L3AdmChoice::Compiled, SaturationBudget::uniform(500));
        assert_eq!(settlement_run_id(&a.run), settlement_run_id(&b.run));
        assert_eq!(a.run.stop, b.run.stop);
    }

    // -----------------------------------------------------------------
    // Reason-vocabulary mapping sanity.
    // -----------------------------------------------------------------

    #[test]
    fn unknown_reason_ordinals_are_pairwise_distinct() {
        let reasons = [
            L3UnknownReasonV1::AdministrativeBudgetExhausted,
            L3UnknownReasonV1::AdministrativeStateBudgetExhausted,
            L3UnknownReasonV1::VisibleBudgetExhausted,
            L3UnknownReasonV1::ProfileError,
            L3UnknownReasonV1::CommitFailed,
            L3UnknownReasonV1::UndeclaredAssumption,
            L3UnknownReasonV1::AssumptionViolated,
            L3UnknownReasonV1::KeyConflict,
            L3UnknownReasonV1::AdapterIntegrityFailure,
            L3UnknownReasonV1::DivergenceObserved,
        ];
        for i in 0..reasons.len() {
            for j in (i + 1)..reasons.len() {
                assert_ne!(reasons[i].ordinal(), reasons[j].ordinal());
            }
        }
    }
}
