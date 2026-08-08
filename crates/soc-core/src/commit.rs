//! The committed coalgebra `γ = select_K ∘ δ` into `D_O = 1 + O×X`
//! (ADR-0002 §1 "Dynamics"; §8, ⟨D-FO⟩ ratified: `F_O = D_O = 1 + O×X`
//! committed, `O = O_min`; `Build_Plan_v3_SOC.md` Step 4).
//!
//! One committed realizing step per tick: `δ` enumerates the keyed
//! deliberation frontier from `e` — reusing the same candidate-enumeration
//! shape as [`crate::oracle::cand`]/[`crate::oracle::cand_instrumented`], so
//! **oracle and committed loop share candidate enumeration** (ADR-0002 §9.2)
//! — and `γ = select_K ∘ δ` ([`crate::calendar::Frontier::select_least`])
//! commits the least-key one into [`Committed`].
//!
//! **Enumeration-sharing note (a documented design choice).** [`run`]/
//! [`commit_tick`] do not call [`crate::oracle::cand_instrumented`] directly.
//! Doing so would need `regimes: &[&dyn SettlementRegime]` converted to a
//! fresh `Vec<&dyn Regime>` (an extra allocation) and then a *second*,
//! redundant enumeration pass to recover which concrete regime produced the
//! selected candidate (needed to call [`SettlementRegime::try_decompose`] on the
//! right regime — [`crate::regime::Candidate::regime`] is only a bare
//! interned [`crate::intern::Handle`], not a way back to the `&dyn
//! SettlementRegime` that produced it). Instead, `commit_tick` enumerates
//! inline, **mirroring `cand`/`cand_instrumented`'s exact algorithm and cost
//! accounting** (one work unit per regime scanned, unconditionally; one more
//! per raw candidate scanned for admissibility) while keeping each
//! candidate's originating regime index alongside it in the frontier. The
//! enumeration *algorithm* is therefore identical to the oracle's; only the
//! call site differs, for the reason above.

use brix_canon::{CanonWriter, Canonical, Digest};
use brix_semantic::{
    Authority, ConfigId, ContextId, Decomposition, Judgement, Outcome, Realizes, Support,
};

use crate::adm::Adm;
use crate::calendar::{Frontier, Key, KeyConflict};
use crate::cost::CostRecord;
use crate::delta::Delta;
use crate::exec::ExecConfig;
use crate::intern::Interner;
use crate::journal::{CommittedStep, Journal};
use crate::oracle;
use crate::regime::{Candidate, Regime};

/// `O_min` (ADR-0002 §8.3): "a small finite set of settlement-event *tags* —
/// the committed outcome class + a digest of the committed `JudgementId`."
/// This is the **entire** observation alphabet the `soc-core` encoders
/// freeze against — deliberately exactly these two fields, nothing richer
/// (`O_rich` is a future, separately-versioned behavior signature, ADR-0002
/// §8.3 — not this one).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Observation {
    /// The committed outcome class. Always [`Outcome::Derived`] for a step
    /// this loop commits (only the audit-factorization checker may later
    /// publish `Audited` for the *same* proposition under different
    /// evidence — a different judgement, ADR-0002 §5 point 1).
    pub outcome_class: Outcome,
    /// A digest of the committed step's `JudgementId`.
    pub judgement_digest: Digest,
}

impl Canonical for Observation {
    fn canon_write(&self, w: &mut CanonWriter) {
        // Field order is ABI: outcome_class, judgement_digest.
        self.outcome_class.canon_write(w);
        w.write_bytes(self.judgement_digest.as_bytes());
    }
}

/// `D_O = 1 + O×X`, the committed coalgebra's codomain (ADR-0002 §8.2
/// Candidate A, ratified ⟨D-FO⟩): `Quiescent` is the `1` summand (`inl(*)` —
/// no admissible candidate this tick); `Step` is the `O×X` summand (one
/// committed [`Observation`] plus the successor [`ExecConfig`]). `O = O_min`
/// per §8.3.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Committed {
    /// The `1` summand: the keyed frontier was empty — no admissible
    /// candidate **this tick**.
    ///
    /// This is the *unsaturated, uncertified* notion of quiescence: it says the
    /// oracle-shared enumeration found nothing to commit right here, and
    /// nothing more. It is **not** a quiescence certificate, and it does not
    /// distinguish a terminal configuration from one whose administrative
    /// search has not finished.
    ///
    /// The certified notion — divergence-sensitive, profile/context/revision
    /// bound, independently checkable — is
    /// [`crate::saturate::SaturatedStep::Quiescent`] (ADR-0014, tracked by
    /// #61). Do not report this variant as "quiescent", "settled", or "at a
    /// fixpoint".
    Quiescent,
    /// The `O×X` summand: `select_K` committed exactly one observation and
    /// advanced to exactly one successor configuration.
    Step {
        observation: Observation,
        successor: ExecConfig,
    },
}

/// The committed-path extension of [`Regime`] (ADR-0002 §6 `Decomposition`;
/// §5.1 "the hot loop records a compact support record plus the
/// (unverified) `Decomposition`"). [`Candidate`] stays lean (`Copy`, lives in
/// a `BTreeSet` in the naive oracle) — this trait is where a regime supplies
/// the tight `𝒢`-decomposition realizing one *specific* committed candidate,
/// called only at the commit boundary (once per tick, on the single
/// selected candidate), never in the `Ord`-set hot enumeration path.
pub trait SettlementRegime: Regime {
    /// The tight `𝒢`-decomposition realizing `c`'s witness, in RECORDED
    /// (unverified) form (ADR-0002 §5.1 — the hot loop records, never
    /// verifies). Called at the commit boundary, not in the `Ord`-set hot
    /// path.
    ///
    /// Fallible (ADR-0012 §2 item 6, §6.3): a regime driven from untrusted
    /// source-derived state (a malformed plan, a missing interner entry, an
    /// empty decomposition) cannot honour an infallible signature without
    /// panicking or fabricating a `Decomposition`. Rejecting is reported
    /// through the same [`CommitError`] vocabulary [`try_commit_selected`]
    /// already uses at the commit boundary — `SettlementRegime` is already
    /// commit-boundary-specific (see the trait doc above), so there is no
    /// need for a second, regime-local error type; extending `CommitError`
    /// keeps exactly one rejection vocabulary instead of two that would need
    /// to be kept in sync.
    ///
    /// This is deliberately the trait's only decomposition method — no
    /// infallible `decompose` survives beside it. A blanket default
    /// (`decompose` kept, `try_decompose` wrapping it in `Ok`) would let a
    /// future regime silently opt out of the fail-closed contract by
    /// implementing only the infallible half; every current implementor in
    /// this workspace already constructs a fixed, valid `Decomposition`, so
    /// migrating them is mechanical (ADR-0012 §2 item 6).
    fn try_decompose(&self, e: &ExecConfig, c: &Candidate) -> Result<Decomposition, CommitError>;
}

/// One tick of the committed coalgebra `γ = select_K ∘ δ`:
///
/// 1. **`δ`** — enumerate every regime's candidates at `e`, filter by `adm`
///    (mirroring [`crate::oracle::cand`]/[`crate::oracle::cand_instrumented`],
///    see module docs), key each admissible candidate via `keyer`, and
///    insert it into a fresh [`crate::calendar::Frontier`] — enforcing the
///    B^uk unique-key discipline (a keyer bug producing two different
///    values at the same key is a hard error, since silently dropping or
///    misordering a candidate would violate `cand`'s completeness).
/// 2. **`select_K`** — pop the frontier's least key. Empty ⇒
///    [`Committed::Quiescent`] (the `1` summand); otherwise the selected
///    `(Candidate, regime)` commits.
/// 3. **Commit boundary** (ADR-0002 §9.2: "digests computed at boundaries,
///    not in the hot loop") — resolve `e.world`/`candidate.successor` through
///    `interner` to digests, obtain the regime's recorded (unverified)
///    [`Decomposition`], set `witness` to the canonical composition of its
///    generators ([`brix_semantic::compose_chain`]), build
///    `Realizes(witness, src, dst)`'s `PropositionId`, and publish the
///    committed `Derived` judgement through the ADR-0016 §4 fence —
///    `Judgement::publish(Authority::SettlementKernel, …, Support::Settlement(&decomposition))`,
///    whose route additionally requires the chain to be in the `Recorded`
///    form and derives the `Evidence::SettlementReplay` id from it.
///    The [`Observation`] is `{ outcome_class: Derived, judgement_digest }`.
///    The successor `ExecConfig` is produced by [`crate::oracle::apply`] —
///    reused verbatim so the committed successor's history component folds
///    exactly like the oracle's own deliberation successors.
///
/// Returns the abstract `D_O` value ([`Committed`]), the full
/// [`CommittedStep`] to log (`None` on `Quiescent`), and this tick's
/// [`CostRecord`] (measuring the `δ` enumeration — always `Steps`, never
/// omitted, matching [`crate::oracle::cand_instrumented`]'s work-unit
/// shape).
pub fn commit_tick<F>(
    regimes: &[&dyn SettlementRegime],
    adm: &dyn Adm,
    interner: &Interner,
    e: &ExecConfig,
    context: ContextId,
    phase: u64,
    keyer: &mut F,
) -> (Committed, Option<CommittedStep>, CostRecord)
where
    F: FnMut(&Candidate, u64) -> Key,
{
    match try_commit_tick(regimes, adm, interner, e, context, phase, keyer) {
        Ok(committed) => committed,
        // The reference driver's contract is unchanged (ADR-0012 §2.5): it
        // commits a valid selected candidate under a keyer whose tie-break is
        // unique, so neither condition can arise here, and either one is an
        // internal-consistency bug rather than a state a caller should handle.
        // The messages are the ones this function raised before the fallible
        // sibling was factored out.
        Err(CommitTickError::KeyConflict(conflict)) => panic!(
            "B^uk unique-key discipline violated at {:?}: two candidates with \
             different observed successors were assigned the same calendar key \
             (existing={:?}, attempted={:?}) — the keyer's tie-break is not \
             actually unique for these candidates",
            conflict.key, conflict.existing, conflict.attempted
        ),
        Err(CommitTickError::Commit(error)) => {
            panic!("commit_tick: reference driver committing a valid selected candidate: {error:?}")
        }
    }
}

/// Why a [`try_commit_tick`] tick could not complete (ADR-0012 §4.3 step 5,
/// §6.3; issue #254).
///
/// Both conditions were previously panics inside [`commit_tick`], and both are
/// already named in the saturation stop vocabulary —
/// [`crate::saturate::SaturationUnknown::KeyConflict`] and
/// [`crate::saturate::SaturationUnknown::CommitFailed`]. This type is what
/// makes those two variants reachable from a run instead of merely declared.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CommitTickError {
    /// Two admissible candidates with different observed successors were
    /// assigned the same calendar key: the `B^uk` unique-key discipline
    /// (ADR-0002 §1/§8.1) was violated by the keyer.
    ///
    /// The frontier is left exactly as it was ([`Frontier::insert`]), so no
    /// partially-built tick escapes.
    KeyConflict(KeyConflict<(Candidate, usize)>),
    /// The commit boundary rejected the selected candidate — an unresolved
    /// handle, a malformed or endpoint-mismatched decomposition, or any other
    /// [`CommitError`] from [`try_commit_selected`].
    Commit(CommitError),
}

impl From<CommitError> for CommitTickError {
    fn from(e: CommitError) -> Self {
        CommitTickError::Commit(e)
    }
}

/// The fallible sibling of [`commit_tick`]: one tick of `γ = select_K ∘ δ`
/// that **returns** the two conditions the reference driver panics on
/// (ADR-0012 §4.3 step 5 / §6.3, issue #254).
///
/// Same enumeration, same `select_K`, same commit boundary, same costs —
/// `commit_tick` is now a thin wrapper that unwraps this and panics, so the
/// two drivers cannot drift. A saturated run drives *this* one, which is what
/// lets `SaturationUnknown::{KeyConflict, CommitFailed}` be reached by a run
/// rather than only by calling the primitives directly.
///
/// Fails closed: on either error no `Committed` value, no `CommittedStep`, and
/// no successor is produced. Neither condition is ever `Refuted`; the caller
/// grades both as `Unknown` (ADR-0014 §5.1).
pub fn try_commit_tick<F>(
    regimes: &[&dyn SettlementRegime],
    adm: &dyn Adm,
    interner: &Interner,
    e: &ExecConfig,
    context: ContextId,
    phase: u64,
    keyer: &mut F,
) -> Result<(Committed, Option<CommittedStep>, CostRecord), CommitTickError>
where
    F: FnMut(&Candidate, u64) -> Key,
{
    // δ: oracle-shared enumeration (see module docs for why this mirrors
    // cand_instrumented inline rather than calling it).
    let mut frontier: Frontier<(Candidate, usize)> = Frontier::new();
    let mut work: u64 = 0;

    for (idx, regime) in regimes.iter().enumerate() {
        // One work unit per regime scanned, paid unconditionally — same
        // shape as oracle::cand_instrumented.
        work += 1;
        for c in regime.candidates(e) {
            // One work unit per raw candidate scanned for admissibility.
            work += 1;
            if adm.admits(e, &c) {
                let key = keyer(&c, phase);
                frontier
                    .insert(key, (c, idx))
                    .map_err(CommitTickError::KeyConflict)?;
            }
        }
    }

    let cost = CostRecord::Steps(work);

    // select_K.
    let Some((key, (candidate, regime_idx))) = frontier.select_least() else {
        return Ok((Committed::Quiescent, None, cost));
    };

    // Commit boundary: handles → digests (ADR-0002 §9.2), never earlier.
    // Factored into the shared fallible `try_commit_selected` (ADR-0012 §2.5):
    // this reference driver commits a valid selected candidate, so the fallible
    // conditions cannot arise — an error here is an internal-consistency bug,
    // exactly like the previous `interner.resolve` / `compose_chain` panics.
    let regime = regimes[regime_idx];
    let (committed, step) = try_commit_selected(key, &candidate, regime, interner, e, context)?;

    Ok((committed, Some(step), cost))
}

/// A recoverable failure at the fallible commit boundary (ADR-0012 §6). These
/// are the conditions the reference [`commit_tick`] previously panicked on;
/// [`try_commit_selected`] surfaces them so the L3 runtime can convert
/// untrusted/source-derived state into a `RuntimeUnknown` result.
///
/// This is also the rejection vocabulary [`SettlementRegime::try_decompose`]
/// reports through (ADR-0012 §2 item 6): rather than mint a second,
/// decompose-local error type, the fallible decomposition seam extends this
/// one, since `SettlementRegime` is already commit-boundary-specific.
/// [`UnresolvedHandle`](Self::UnresolvedHandle),
/// [`EmptyDecomposition`](Self::EmptyDecomposition),
/// [`ChainLengthMismatch`](Self::ChainLengthMismatch), and
/// [`EndpointMismatch`](Self::EndpointMismatch) are the "at minimum" set #244
/// called for.
///
/// [`CandidateMismatch`](Self::CandidateMismatch),
/// [`WitnessMismatch`](Self::WitnessMismatch), and
/// [`GeneratorMismatch`](Self::GeneratorMismatch) were added additively for
/// ADR-0012 Stage B (#251): its source-derived regime validates a candidate
/// against a precomputed transition table (§6.3's four required conditions),
/// and three of those four failures have no honest existing variant —
/// `UnresolvedHandle` means the *interner* failed to resolve a handle, which
/// is a different cause from "every handle resolved fine, but this is not
/// the candidate the table expects"; `EmptyDecomposition` means *no*
/// generator is present, which is a different claim from "a generator is
/// present and a decomposition composes, it is just the wrong one." Per
/// ADR-0002 §5.3's discipline against overclaiming, a diagnostic reason code
/// must not describe a non-empty-but-wrong chain as empty, or a
/// wrong-candidate as unresolved — each of §6.3's four conditions now has its
/// own distinct, accurately-named variant, so `Unknown(CommitFailed { error
/// })`'s reason code is never a false statement about which check failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommitError {
    /// A world or successor handle was not resolvable in the interner (or,
    /// from [`SettlementRegime::try_decompose`], some other handle the
    /// regime needed to resolve while building the decomposition).
    UnresolvedHandle,
    /// The committed decomposition had no generators, so no witness can be
    /// composed (`k = g_n ∘ … ∘ g_1` requires at least one generator). Also
    /// returned by a regime whose source-derived plan yielded no rule/step to
    /// decompose at all.
    EmptyDecomposition,
    /// The proposed generator chain and configuration chain are structurally
    /// inconsistent with a finite factorization: `configs.len() !=
    /// generators.len() + 1` (mirrors
    /// [`brix_semantic::DecompositionError::ChainLengthMismatch`], which a
    /// `try_decompose` implementation typically discovers by calling
    /// [`Decomposition::recorded`]/`replay_verified` and propagating its
    /// error via `?`).
    ChainLengthMismatch {
        /// The proposed generator count.
        generators: usize,
        /// The proposed configuration count.
        configs: usize,
    },
    /// The decomposition's endpoints do not match what the candidate/plan
    /// requires — e.g. (ADR-0012 §6.3) `decomposition.configs` is not exactly
    /// `[expected_src, expected_dst]` for the candidate being committed.
    EndpointMismatch,
    /// The candidate a regime was asked to decompose is not the one its own
    /// precomputed transition relation associates with the current world
    /// (ADR-0012 §6.3 condition 1: `candidate != transition_table[current_world]`).
    /// Every handle involved resolves fine — this is deliberately distinct
    /// from [`UnresolvedHandle`](Self::UnresolvedHandle), whose cause (an
    /// interner miss) and fix (repair interning) are both different: here the
    /// candidate itself is simply not the expected one, e.g. a stale or
    /// forged selection reaching the commit boundary.
    CandidateMismatch,
    /// The candidate's witness handle does not equal the one the regime
    /// interned for its expected generator's primitive `WitnessId` (ADR-0012
    /// §6.3 condition 2). A generator *is* present and a decomposition can be
    /// built from it — this is deliberately distinct from
    /// [`EmptyDecomposition`](Self::EmptyDecomposition): the claimed witness
    /// simply does not match the one that generator would produce.
    WitnessMismatch,
    /// The decomposition's generator chain is not exactly the one this
    /// regime expected for the transition being committed (ADR-0012 §6.3
    /// condition 3: `decomposition.generators != [expected_generator]`). The
    /// chain is non-empty and correctly shaped — this is deliberately
    /// distinct from both [`EmptyDecomposition`](Self::EmptyDecomposition)
    /// (no generators at all) and
    /// [`ChainLengthMismatch`](Self::ChainLengthMismatch) (a `configs`/
    /// `generators` length disagreement): the chain composes structurally
    /// fine, it just cites the wrong generator(s).
    GeneratorMismatch,
    /// The `Derived` publication was refused by the ADR-0016 §4 authority
    /// fence — in practice, a decomposition reaching the commit boundary in
    /// the `ReplayVerified` rather than the `Recorded` form, which would be
    /// the hot loop asserting a verification it never performed. Unreachable
    /// on the settled path (`try_decompose` builds a recorded chain); carried
    /// so the boundary stays total and fails closed instead of panicking.
    Publication(brix_semantic::PublicationError),
}

impl From<brix_semantic::PublicationError> for CommitError {
    fn from(e: brix_semantic::PublicationError) -> Self {
        CommitError::Publication(e)
    }
}

impl From<brix_semantic::DecompositionError> for CommitError {
    fn from(e: brix_semantic::DecompositionError) -> Self {
        match e {
            brix_semantic::DecompositionError::ChainLengthMismatch {
                generators,
                configs,
            } => CommitError::ChainLengthMismatch {
                generators,
                configs,
            },
        }
    }
}

/// The pure prospective successor of committing `candidate` from `e`: the same
/// [`oracle::apply`] fold the deliberation frontier's successors use, computed
/// **without** constructing an observation or committing anything. The L3
/// adapter uses this to peek a candidate's resulting world before deciding to
/// commit; [`try_commit_selected`] uses the very same operation so a committed
/// successor always equals the previously-probed prospect (ADR-0012 §2.5).
pub fn prospective_successor(e: &ExecConfig, candidate: &Candidate) -> ExecConfig {
    oracle::apply(e, candidate)
}

/// The **sole** constructor of a `Derived` settlement judgement, factored out of
/// [`commit_tick`] (ADR-0012 §2.5). Given an already-selected `key`/`candidate`
/// and its `regime`, it validates/decomposes the candidate, resolves the world
/// endpoints, composes the committed witness `k = g_n ∘ … ∘ g_1`, constructs
/// the `Derived` [`Judgement`]/[`Observation`] and [`CommittedStep`], and
/// computes the successor via [`prospective_successor`]. Both the naive
/// `commit_tick` and the incremental L3 adapter call this; the adapter selects
/// and schedules but never mints a settlement judgement itself.
///
/// Fallible where the reference driver panicked: an unresolved handle, a
/// rejected decomposition (malformed, empty, or endpoint-mismatched — see
/// [`SettlementRegime::try_decompose`]), or an empty composed witness each
/// return [`CommitError`] instead of panicking, so the L3 boundary can fail
/// closed on untrusted source-derived state. A rejected decomposition short-
/// circuits before any `CommittedStep` is built and before any `Derived`
/// judgement is minted — this function's only `Ok` path is the one that
/// builds both.
pub fn try_commit_selected(
    key: Key,
    candidate: &Candidate,
    regime: &dyn SettlementRegime,
    interner: &Interner,
    e: &ExecConfig,
    context: ContextId,
) -> Result<(Committed, CommittedStep), CommitError> {
    let decomposition = regime.try_decompose(e, candidate)?;

    let src = ConfigId(
        interner
            .try_resolve(e.world)
            .ok_or(CommitError::UnresolvedHandle)?,
    );
    let dst = ConfigId(
        interner
            .try_resolve(candidate.successor)
            .ok_or(CommitError::UnresolvedHandle)?,
    );
    // Committed witness identity IS the canonical composition of its generators;
    // `candidate.witness` is the regime's proposal, the COMMITTED identity is
    // derived from the factorization.
    let witness = brix_semantic::compose_chain(&decomposition.generators)
        .ok_or(CommitError::EmptyDecomposition)?;

    let proposition = Realizes::new(witness, src, dst).proposition_id();
    // The settlement kernel's own publication, through the ADR-0016 §4 fence:
    // the `(SettlementKernel, Derived, Settlement)` route additionally demands
    // that the chain be `Recorded` — the hot loop records, it never asserts
    // verification (ADR-0002 §4.1/§5.1).
    let judgement_id = Judgement::publish(
        Authority::SettlementKernel,
        context,
        proposition,
        Outcome::Derived,
        Support::Settlement(&decomposition),
    )
    .map_err(CommitError::from)?
    .id();

    let observation = Observation {
        outcome_class: Outcome::Derived,
        judgement_digest: judgement_id.digest(),
    };

    let successor = prospective_successor(e, candidate);

    let step = CommittedStep {
        key,
        observation,
        decomposition,
        src,
        dst,
        witness,
    };

    Ok((
        Committed::Step {
            observation,
            successor,
        },
        step,
    ))
}

/// The committed step loop / driver: repeatedly ticks [`commit_tick`],
/// appending every [`Committed::Step`] to a [`Journal`] and advancing `e`,
/// until either quiescence (`Committed::Quiescent`) or `max_ticks` is
/// reached. Returns the built [`Journal`] together with the parallel
/// `Vec<CostRecord>` — **one entry per committed tick** (quiescence itself
/// does not emit a trailing cost record; it simply stops the loop, so
/// `costs.len() == journal.len()` always holds).
///
/// **Signature note (a documented deviation from the design sketch).** An
/// `interner: &Interner` parameter was added beyond the sketch in the task
/// brief — resolving `Handle → Digest` at the commit boundary (ADR-0002
/// §9.2) is not optional, and there is no way to build `ConfigId`/
/// `WitnessId` without the same `Interner` that minted `e0`'s and each
/// regime's handles. The design sketch's generic parameter name `K` was
/// renamed to `F` to avoid reading confusingly next to the unrelated `Key`
/// type.
pub fn run<F>(
    regimes: &[&dyn SettlementRegime],
    adm: &dyn Adm,
    interner: &Interner,
    e0: ExecConfig,
    context: ContextId,
    keyer: F,
    max_ticks: usize,
) -> (Journal, Vec<CostRecord>)
where
    F: FnMut(&Candidate, u64) -> Key,
{
    let (journal, costs, _) = run_reason(regimes, adm, interner, e0, context, keyer, max_ticks);
    (journal, costs)
}

/// Why the unsaturated driver stopped.
///
/// Neither variant is a quiescence certificate: this is the *unsaturated*
/// vocabulary, and [`UnsaturatedStop::ImmediateFrontierEmpty`] carries exactly
/// the weak claim [`Committed::Quiescent`] does. The certified notion lives in
/// [`crate::saturate`] (ADR-0014).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum UnsaturatedStop {
    /// A tick found no admissible candidate. **Not** certified quiescence, and
    /// not a fixpoint claim.
    ImmediateFrontierEmpty,
    /// `max_ticks` was reached with work possibly remaining. Establishes
    /// nothing — never report this as settled.
    TickBudgetExhausted {
        /// The bound that was hit.
        max_ticks: usize,
    },
}

/// [`run`], plus the reason it stopped.
///
/// `run` alone cannot distinguish "the frontier went empty" from "we ran out of
/// ticks" — both simply end its loop and return the same value. Conflating
/// those is exactly the collapse ADR-0002 §5.3 forbids ("a search that has not
/// terminated has proved nothing"), so callers that care about the difference
/// MUST use this entry point.
pub fn run_reason<F>(
    regimes: &[&dyn SettlementRegime],
    adm: &dyn Adm,
    interner: &Interner,
    e0: ExecConfig,
    context: ContextId,
    mut keyer: F,
    max_ticks: usize,
) -> (Journal, Vec<CostRecord>, UnsaturatedStop)
where
    F: FnMut(&Candidate, u64) -> Key,
{
    let mut journal = Journal::new();
    let mut costs = Vec::new();
    let mut e = e0;

    for phase in 0..max_ticks as u64 {
        let (committed, step, cost) =
            commit_tick(regimes, adm, interner, &e, context, phase, &mut keyer);
        match committed {
            Committed::Quiescent => {
                return (journal, costs, UnsaturatedStop::ImmediateFrontierEmpty)
            }
            Committed::Step { successor, .. } => {
                journal.append(step.expect(
                    "Committed::Step always carries Some(CommittedStep) — see commit_tick",
                ));
                costs.push(cost);
                e = successor;
            }
        }
    }

    (
        journal,
        costs,
        UnsaturatedStop::TickBudgetExhausted { max_ticks },
    )
}

/// The world-configuration [`Delta`] a committed step induces (ADR-0002 §9.2;
/// `Build_Plan_v3_SOC.md` Step 6, E3): at the commit boundary, exactly the
/// step's predecessor world-config handle *left* and its successor world-config
/// handle *entered*. This is the emitter that feeds the incremental engine
/// ([`crate::engine::IncrementalEngine::step`]) — a committed step is the unit
/// of world change, and this turns it into the delta the engine consumes.
///
/// - [`Committed::Quiescent`] induces the empty delta (nothing committed,
///   nothing changed).
/// - [`Committed::Step`] induces `{ removed: {before.world}, added:
///   {successor.world} }` — collapsing to empty for a reflexive step whose
///   successor world equals `before.world` (e.g. the literal-equality
///   regime's `x → x`), via [`Delta::between_worlds`].
pub fn step_world_delta(before: &ExecConfig, committed: &Committed) -> Delta {
    match committed {
        Committed::Quiescent => Delta::new(),
        Committed::Step { successor, .. } => Delta::between_worlds(before.world, successor.world),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adm::{AdmAll, AdmNone};
    use crate::history::History;
    use crate::intern::Handle;
    use brix_canon::Domain;
    use brix_semantic::{Evidence, GeneratorId};

    /// A single-candidate fixture regime whose `decompose` always returns
    /// the same fixed, valid recorded `Decomposition` — deterministic and
    /// simple enough for tests to reconstruct independently.
    struct FixtureRegime {
        id: crate::intern::Handle,
        witness: crate::intern::Handle,
        successor: crate::intern::Handle,
    }

    impl Regime for FixtureRegime {
        fn candidates(&self, _e: &ExecConfig) -> Vec<Candidate> {
            vec![Candidate {
                regime: self.id,
                witness: self.witness,
                successor: self.successor,
            }]
        }
    }

    impl SettlementRegime for FixtureRegime {
        fn try_decompose(
            &self,
            _e: &ExecConfig,
            _c: &Candidate,
        ) -> Result<Decomposition, CommitError> {
            Ok(Decomposition::recorded(
                vec![GeneratorId::named("fixture.step@1")],
                vec![
                    ConfigId::from_canon(b"fixture-x0"),
                    ConfigId::from_canon(b"fixture-x1"),
                ],
            )
            .unwrap())
        }
    }

    fn tiebreak_of(c: &Candidate) -> Digest {
        // A canonical digest derived from the candidate's own handles —
        // stable within one run of a fixed Interner (same convention as
        // oracle::CandidateStep), sufficient to make the tie-break unique
        // per distinct candidate in these single-candidate-per-tick fixtures.
        let mut w = CanonWriter::new();
        w.write_uint(c.witness.raw() as u64);
        w.write_uint(c.successor.raw() as u64);
        w.digest(Domain::Value)
    }

    fn setup() -> (Interner, FixtureRegime, ExecConfig) {
        let mut i = Interner::new();
        let world = i.intern(Digest::of(Domain::Value, b"w0"));
        let policy = i.intern(Digest::of(Domain::Value, b"p0"));
        let regime = i.intern(Digest::of(Domain::Value, b"r"));
        let witness = i.intern(Digest::of(Domain::Value, b"wit"));
        let successor = i.intern(Digest::of(Domain::Value, b"w1"));
        let e = ExecConfig::new(world, policy, History::empty().digest());
        (
            i,
            FixtureRegime {
                id: regime,
                witness,
                successor,
            },
            e,
        )
    }

    #[test]
    fn commit_tick_with_no_admissible_candidate_is_quiescent() {
        let (i, regime, e) = setup();
        let regimes: Vec<&dyn SettlementRegime> = vec![&regime];
        let (committed, step, cost) = commit_tick(
            &regimes,
            &AdmNone,
            &i,
            &e,
            ContextId::root(),
            0,
            &mut |c, phase| Key::new(phase, 0, tiebreak_of(c)),
        );
        assert_eq!(committed, Committed::Quiescent);
        assert!(step.is_none());
        assert!(cost.work_units().is_some(), "cost is never omitted");
    }

    #[test]
    fn commit_tick_with_one_admissible_candidate_commits_derived() {
        let (i, regime, e) = setup();
        let regimes: Vec<&dyn SettlementRegime> = vec![&regime];
        let (committed, step, cost) = commit_tick(
            &regimes,
            &AdmAll,
            &i,
            &e,
            ContextId::root(),
            0,
            &mut |c, phase| Key::new(phase, 0, tiebreak_of(c)),
        );
        match committed {
            Committed::Step {
                observation,
                successor,
            } => {
                assert_eq!(observation.outcome_class, Outcome::Derived);
                assert_ne!(successor.history, e.history, "history must advance");
            }
            Committed::Quiescent => panic!("expected a committed step"),
        }
        assert!(step.is_some());
        assert!(cost.work_units().is_some());
    }

    #[test]
    fn observation_judgement_digest_matches_an_independently_rebuilt_judgement() {
        let (i, regime, e) = setup();
        let regimes: Vec<&dyn SettlementRegime> = vec![&regime];
        let context = ContextId::root();
        let (committed, _step, _cost) =
            commit_tick(&regimes, &AdmAll, &i, &e, context, 0, &mut |c, phase| {
                Key::new(phase, 0, tiebreak_of(c))
            });
        let Committed::Step { observation, .. } = committed else {
            panic!("expected a committed step");
        };

        // Independently rebuild the Realizes/Decomposition/Evidence/Judgement
        // chain by hand, using only public constructors and the fixture's
        // known handles — non-vacuous, since this does not call commit_tick.
        let src = ConfigId(i.resolve(e.world));
        let dst = ConfigId(i.resolve(regime.successor));
        let decomposition = Decomposition::recorded(
            vec![GeneratorId::named("fixture.step@1")],
            vec![
                ConfigId::from_canon(b"fixture-x0"),
                ConfigId::from_canon(b"fixture-x1"),
            ],
        )
        .unwrap();
        let witness = brix_semantic::compose_chain(&decomposition.generators).unwrap();
        let proposition = Realizes::new(witness, src, dst).proposition_id();
        let evidence = Evidence::SettlementReplay {
            body: decomposition.id().digest(),
        }
        .id();
        let judgement_id =
            brix_semantic::JudgementId::recompute(context, proposition, Outcome::Derived, evidence);

        assert_eq!(observation.outcome_class, Outcome::Derived);
        assert_eq!(observation.judgement_digest, judgement_id.digest());
    }

    #[test]
    fn cost_is_emitted_for_every_committed_tick_never_omitted() {
        let (i, regime, e) = setup();
        let regimes: Vec<&dyn SettlementRegime> = vec![&regime];
        let (journal, costs) = run(
            &regimes,
            &AdmAll,
            &i,
            e,
            ContextId::root(),
            |c, phase| Key::new(phase, 0, tiebreak_of(c)),
            5,
        );
        assert_eq!(
            costs.len(),
            journal.len(),
            "one CostRecord per committed tick"
        );
        assert!(!costs.is_empty());
        for cost in &costs {
            assert!(cost.work_units().is_some(), "cost is never omitted");
        }
    }

    #[test]
    fn step_world_delta_of_a_committed_step_removes_old_world_adds_successor() {
        let (i, regime, e) = setup();
        let regimes: Vec<&dyn SettlementRegime> = vec![&regime];
        let (committed, _step, _cost) = commit_tick(
            &regimes,
            &AdmAll,
            &i,
            &e,
            ContextId::root(),
            0,
            &mut |c, phase| Key::new(phase, 0, tiebreak_of(c)),
        );
        let Committed::Step { successor, .. } = committed else {
            panic!("expected a committed step");
        };
        let delta = step_world_delta(&e, &committed);
        let expected_removed: std::collections::BTreeSet<Handle> =
            std::collections::BTreeSet::from([e.world]);
        let expected_added: std::collections::BTreeSet<Handle> =
            std::collections::BTreeSet::from([successor.world]);
        assert_eq!(delta.removed, expected_removed);
        assert_eq!(delta.added, expected_added);
    }

    #[test]
    fn step_world_delta_of_quiescence_is_empty() {
        let (i, regime, e) = setup();
        let regimes: Vec<&dyn SettlementRegime> = vec![&regime];
        let (committed, _step, _cost) = commit_tick(
            &regimes,
            &AdmNone,
            &i,
            &e,
            ContextId::root(),
            0,
            &mut |c, phase| Key::new(phase, 0, tiebreak_of(c)),
        );
        assert_eq!(committed, Committed::Quiescent);
        assert!(step_world_delta(&e, &committed).is_empty());
    }

    #[test]
    fn run_is_quiescent_immediately_under_adm_none() {
        let (i, regime, e) = setup();
        let regimes: Vec<&dyn SettlementRegime> = vec![&regime];
        let (journal, costs) = run(
            &regimes,
            &AdmNone,
            &i,
            e,
            ContextId::root(),
            |c, phase| Key::new(phase, 0, tiebreak_of(c)),
            5,
        );
        assert!(journal.is_empty());
        assert!(costs.is_empty());
    }

    #[test]
    fn running_twice_from_the_same_inputs_is_byte_identical_deterministic_replay() {
        let (i, regime, e) = setup();
        let regimes: Vec<&dyn SettlementRegime> = vec![&regime];

        // The fixture regime is a fixed point after one step (its candidate
        // is constant regardless of e), so bound max_ticks to keep the loop
        // finite for this determinism check — one commit, then re-run from
        // scratch and compare.
        let (journal_a, costs_a) = run(
            &regimes,
            &AdmAll,
            &i,
            e,
            ContextId::root(),
            |c, phase| Key::new(phase, 0, tiebreak_of(c)),
            1,
        );
        let (journal_b, costs_b) = run(
            &regimes,
            &AdmAll,
            &i,
            e,
            ContextId::root(),
            |c, phase| Key::new(phase, 0, tiebreak_of(c)),
            1,
        );

        assert_eq!(journal_a.step_digests(), journal_b.step_digests());
        assert_eq!(journal_a.chain_digest(), journal_b.chain_digest());
        assert_eq!(costs_a, costs_b);
        assert_eq!(
            Journal::replay_chain(journal_a.steps()),
            journal_a.step_digests()
        );
    }

    struct MultiGenRegime {
        id: Handle,
        witness: Handle,
        successor: Handle,
        generators: Vec<GeneratorId>,
        configs: Vec<ConfigId>,
    }

    impl Regime for MultiGenRegime {
        fn candidates(&self, _e: &ExecConfig) -> Vec<Candidate> {
            vec![Candidate {
                regime: self.id,
                witness: self.witness,
                successor: self.successor,
            }]
        }
    }

    impl SettlementRegime for MultiGenRegime {
        fn try_decompose(
            &self,
            _e: &ExecConfig,
            _c: &Candidate,
        ) -> Result<Decomposition, CommitError> {
            Ok(Decomposition::recorded(self.generators.clone(), self.configs.clone()).unwrap())
        }
    }

    #[test]
    fn committed_witness_is_generator_composition_over_multi_generator_decomposition() {
        let mut i = Interner::new();
        let world = i.intern(Digest::of(Domain::Value, b"mw0"));
        let policy = i.intern(Digest::of(Domain::Value, b"mp0"));
        let regime = i.intern(Digest::of(Domain::Value, b"mr"));
        let witness_handle = i.intern(Digest::of(Domain::Value, b"mwit"));
        let successor = i.intern(Digest::of(Domain::Value, b"mw1"));
        let e = ExecConfig::new(world, policy, History::empty().digest());

        let generators = vec![
            GeneratorId::named("multi.step@1"),
            GeneratorId::named("multi.step@2"),
            GeneratorId::named("multi.step@3"),
        ];
        let configs = vec![
            ConfigId::from_canon(b"multi-x0"),
            ConfigId::from_canon(b"multi-x1"),
            ConfigId::from_canon(b"multi-x2"),
            ConfigId::from_canon(b"multi-x3"),
        ];

        let multi_regime = MultiGenRegime {
            id: regime,
            witness: witness_handle,
            successor,
            generators: generators.clone(),
            configs: configs.clone(),
        };

        let regimes: Vec<&dyn SettlementRegime> = vec![&multi_regime];
        let context = ContextId::root();
        let (committed, step_opt, _cost) =
            commit_tick(&regimes, &AdmAll, &i, &e, context, 0, &mut |c, phase| {
                Key::new(phase, 0, tiebreak_of(c))
            });

        let step = step_opt.expect("expected a committed step");
        let Committed::Step { observation, .. } = committed else {
            panic!("expected a committed step");
        };

        let expected_witness = brix_semantic::compose_chain(&generators).unwrap();
        assert_eq!(step.witness, expected_witness);

        let src = ConfigId(i.resolve(e.world));
        let dst = ConfigId(i.resolve(multi_regime.successor));
        let expected_proposition = Realizes::new(expected_witness, src, dst).proposition_id();

        let evidence = Evidence::SettlementReplay {
            body: step.decomposition.id().digest(),
        }
        .id();
        let expected_judgement_id = brix_semantic::JudgementId::recompute(
            context,
            expected_proposition,
            Outcome::Derived,
            evidence,
        );

        assert_eq!(observation.judgement_digest, expected_judgement_id.digest());
    }

    #[test]
    fn commit_tick_delegates_byte_identically_to_try_commit_selected() {
        // The seam-factoring guard (ADR-0012 §2.5): commit_tick's committed
        // result MUST be byte-identical to reproducing its select boundary and
        // committing through the factored `try_commit_selected`. CommittedStep
        // and Committed derive Eq over content-addressed fields, so equality is
        // byte-identity.
        let (i, regime, e) = setup();
        let regimes: Vec<&dyn SettlementRegime> = vec![&regime];
        let context = ContextId::root();
        let key_of = |c: &Candidate, phase: u64| Key::new(phase, 0, tiebreak_of(c));

        let (committed_tick, step_tick, _cost) =
            commit_tick(&regimes, &AdmAll, &i, &e, context, 0, &mut |c, p| {
                key_of(c, p)
            });

        // Reproduce commit_tick's δ-enumeration + select boundary by hand.
        let mut frontier: Frontier<(Candidate, usize)> = Frontier::new();
        for (idx, r) in regimes.iter().enumerate() {
            for c in r.candidates(&e) {
                if AdmAll.admits(&e, &c) {
                    frontier.insert(key_of(&c, 0), (c, idx)).unwrap();
                }
            }
        }
        let (key, (candidate, idx)) = frontier.select_least().expect("one admissible candidate");
        let (committed_direct, step_direct) =
            try_commit_selected(key, &candidate, regimes[idx], &i, &e, context)
                .expect("valid candidate commits");

        assert_eq!(committed_tick, committed_direct);
        assert_eq!(step_tick, Some(step_direct));
    }

    #[test]
    fn try_commit_selected_reports_unresolved_handle_instead_of_panicking() {
        // The fallible seam (ADR-0012 §6): a successor handle not resolvable in
        // the commit interner is a recoverable error, not a panic.
        let (i, regime, e) = setup();
        let mut other = Interner::new();
        let bad = (0..64)
            .map(|n| other.intern(Digest::of(Domain::Value, format!("x{n}").as_bytes())))
            .last()
            .unwrap();
        let candidate = Candidate {
            regime: regime.id,
            witness: regime.witness,
            successor: bad,
        };
        let key = Key::new(0, 0, tiebreak_of(&candidate));
        let err = try_commit_selected(key, &candidate, &regime, &i, &e, ContextId::root())
            .expect_err("an unresolvable handle must fail, not panic");
        assert_eq!(err, CommitError::UnresolvedHandle);
    }

    /// A regime whose `try_decompose` always rejects with a fixed
    /// [`CommitError`] — exercising the fail-closed contract
    /// [`SettlementRegime::try_decompose`] documents (ADR-0012 §2 item 6): a
    /// malformed/empty/endpoint-mismatched decomposition must produce a typed
    /// commit failure, never a panic and never a fabricated `Decomposition`.
    struct RejectingRegime {
        id: Handle,
        witness: Handle,
        successor: Handle,
        error: CommitError,
    }

    impl Regime for RejectingRegime {
        fn candidates(&self, _e: &ExecConfig) -> Vec<Candidate> {
            vec![Candidate {
                regime: self.id,
                witness: self.witness,
                successor: self.successor,
            }]
        }
    }

    impl SettlementRegime for RejectingRegime {
        fn try_decompose(
            &self,
            _e: &ExecConfig,
            _c: &Candidate,
        ) -> Result<Decomposition, CommitError> {
            Err(self.error.clone())
        }
    }

    fn rejecting_fixture(error: CommitError) -> (Interner, RejectingRegime, ExecConfig, Candidate) {
        let (i, base, e) = setup();
        let regime = RejectingRegime {
            id: base.id,
            witness: base.witness,
            successor: base.successor,
            error,
        };
        let candidate = Candidate {
            regime: regime.id,
            witness: regime.witness,
            successor: regime.successor,
        };
        (i, regime, e, candidate)
    }

    #[test]
    fn try_commit_selected_reports_empty_decomposition_instead_of_panicking() {
        // Acceptance (#244): an empty decomposition is a typed failure, not a
        // panic and not a fabricated Decomposition — and no CommittedStep or
        // Derived judgement is ever constructed on this path, since
        // try_commit_selected short-circuits on `?` before building either.
        let (i, regime, e, candidate) = rejecting_fixture(CommitError::EmptyDecomposition);
        let key = Key::new(0, 0, tiebreak_of(&candidate));
        let err = try_commit_selected(key, &candidate, &regime, &i, &e, ContextId::root())
            .expect_err("an empty decomposition must fail, not panic");
        assert_eq!(err, CommitError::EmptyDecomposition);
    }

    #[test]
    fn try_commit_selected_reports_endpoint_mismatch_instead_of_panicking() {
        // Acceptance (#244): the interface makes an endpoint-mismatched
        // decomposition expressible (ADR-0012 §6.3 condition 4) even though
        // no regime in this issue's scope produces it yet (that is Stage B's
        // source-derived regime).
        let (i, regime, e, candidate) = rejecting_fixture(CommitError::EndpointMismatch);
        let key = Key::new(0, 0, tiebreak_of(&candidate));
        let err = try_commit_selected(key, &candidate, &regime, &i, &e, ContextId::root())
            .expect_err("an endpoint-mismatched decomposition must fail, not panic");
        assert_eq!(err, CommitError::EndpointMismatch);
    }

    /// A regime whose `try_decompose` builds a genuinely malformed chain (two
    /// generators, only two configs — one short of the three a two-generator
    /// chain needs) via [`Decomposition::recorded`] and propagates its
    /// `DecompositionError` with `?`, exercising the real
    /// `From<DecompositionError> for CommitError` conversion rather than a
    /// hand-constructed error.
    struct MismatchedChainRegime {
        id: Handle,
        witness: Handle,
        successor: Handle,
    }

    impl Regime for MismatchedChainRegime {
        fn candidates(&self, _e: &ExecConfig) -> Vec<Candidate> {
            vec![Candidate {
                regime: self.id,
                witness: self.witness,
                successor: self.successor,
            }]
        }
    }

    impl SettlementRegime for MismatchedChainRegime {
        fn try_decompose(
            &self,
            _e: &ExecConfig,
            _c: &Candidate,
        ) -> Result<Decomposition, CommitError> {
            Ok(Decomposition::recorded(
                vec![
                    GeneratorId::named("mismatched.step@1"),
                    GeneratorId::named("mismatched.step@2"),
                ],
                vec![
                    ConfigId::from_canon(b"mismatched-x0"),
                    ConfigId::from_canon(b"mismatched-x1"),
                ],
            )?)
        }
    }

    #[test]
    fn try_commit_selected_reports_chain_length_mismatch_instead_of_panicking() {
        // Acceptance (#244): a chain-length-mismatched decomposition — the
        // structural half of "malformed" — fails closed via the real
        // DecompositionError -> CommitError conversion, not a panic.
        let (i, base, e) = setup();
        let regime = MismatchedChainRegime {
            id: base.id,
            witness: base.witness,
            successor: base.successor,
        };
        let candidate = Candidate {
            regime: regime.id,
            witness: regime.witness,
            successor: regime.successor,
        };
        let key = Key::new(0, 0, tiebreak_of(&candidate));
        let err = try_commit_selected(key, &candidate, &regime, &i, &e, ContextId::root())
            .expect_err("a chain-length-mismatched decomposition must fail, not panic");
        assert_eq!(
            err,
            CommitError::ChainLengthMismatch {
                generators: 2,
                configs: 2,
            }
        );
    }
}
