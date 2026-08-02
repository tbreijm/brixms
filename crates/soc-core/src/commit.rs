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
//! selected candidate (needed to call [`SettlementRegime::decompose`] on the
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
use brix_semantic::{ConfigId, ContextId, Decomposition, Evidence, Judgement, Outcome, Realizes};

use crate::adm::Adm;
use crate::calendar::{Frontier, Key};
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
    fn decompose(&self, e: &ExecConfig, c: &Candidate) -> Decomposition;
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
///    `Realizes(witness, src, dst)`'s `PropositionId`, wrap the decomposition as
///    `Evidence::SettlementReplay`, and build the committed
///    `Judgement::new(context, proposition, Outcome::Derived, evidence)`.
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
                frontier.insert(key, (c, idx)).unwrap_or_else(|conflict| {
                    panic!(
                        "B^uk unique-key discipline violated at {:?}: two candidates with \
                         different observed successors were assigned the same calendar key \
                         (existing={:?}, attempted={:?}) — the keyer's tie-break is not \
                         actually unique for these candidates",
                        conflict.key, conflict.existing, conflict.attempted
                    )
                });
            }
        }
    }

    let cost = CostRecord::Steps(work);

    // select_K.
    let Some((key, (candidate, regime_idx))) = frontier.select_least() else {
        return (Committed::Quiescent, None, cost);
    };

    // Commit boundary: handles → digests (ADR-0002 §9.2), never earlier.
    // Factored into the shared fallible `try_commit_selected` (ADR-0012 §2.5):
    // this reference driver commits a valid selected candidate, so the fallible
    // conditions cannot arise — an error here is an internal-consistency bug,
    // exactly like the previous `interner.resolve` / `compose_chain` panics.
    let regime = regimes[regime_idx];
    let (committed, step) = try_commit_selected(key, &candidate, regime, interner, e, context)
        .expect("commit_tick: reference driver committing a valid selected candidate");

    (committed, Some(step), cost)
}

/// A recoverable failure at the fallible commit boundary (ADR-0012 §6). These
/// are the conditions the reference [`commit_tick`] previously panicked on;
/// [`try_commit_selected`] surfaces them so the L3 runtime can convert
/// untrusted/source-derived state into a `RuntimeUnknown` result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommitError {
    /// A world or successor handle was not resolvable in the interner.
    UnresolvedHandle,
    /// The committed decomposition had no generators, so no witness can be
    /// composed (`k = g_n ∘ … ∘ g_1` requires at least one generator).
    EmptyDecomposition,
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
/// Fallible where the reference driver panicked: an unresolved handle or an
/// empty decomposition returns [`CommitError`] instead of panicking, so the
/// L3 boundary can fail closed on untrusted source-derived state.
pub fn try_commit_selected(
    key: Key,
    candidate: &Candidate,
    regime: &dyn SettlementRegime,
    interner: &Interner,
    e: &ExecConfig,
    context: ContextId,
) -> Result<(Committed, CommittedStep), CommitError> {
    let decomposition = regime.decompose(e, candidate);

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
    let evidence = Evidence::SettlementReplay {
        body: decomposition.id().digest(),
    }
    .id();
    let judgement_id = Judgement::new(context, proposition, Outcome::Derived, evidence).id();

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
    use brix_semantic::GeneratorId;

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
        fn decompose(&self, _e: &ExecConfig, _c: &Candidate) -> Decomposition {
            Decomposition::recorded(
                vec![GeneratorId::named("fixture.step@1")],
                vec![
                    ConfigId::from_canon(b"fixture-x0"),
                    ConfigId::from_canon(b"fixture-x1"),
                ],
            )
            .unwrap()
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
        let judgement_id = Judgement::new(context, proposition, Outcome::Derived, evidence).id();

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
        fn decompose(&self, _e: &ExecConfig, _c: &Candidate) -> Decomposition {
            Decomposition::recorded(self.generators.clone(), self.configs.clone()).unwrap()
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
        let expected_judgement_id =
            Judgement::new(context, expected_proposition, Outcome::Derived, evidence).id();

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
}
