//! ADR-0012 Stage B — the compiler-owned dual regime, the precomputed
//! transition table, fallible decomposition, and the observation profile.
//!
//! This module implements exactly:
//!
//! - **§3.3**'s precomputed transition table: during bounded setup, the
//!   plan's `N + 1` deterministic prefix worlds and `N` head candidate
//!   triples are built and interned once ([`build_l3_transition_table`]).
//!   `Candidate` has no canonical identity of its own — its witness and
//!   successor *constituents* get interned — so the hot provider maps each
//!   nonterminal world handle **directly** to one
//!   preconstructed candidate, never scanning the pending agenda or hashing
//!   source-sized data.
//! - **§2 item 3**'s compiler-owned dual regime ([`L3Regime`]): one type
//!   implementing both the retained naive [`WitnessProvider`] and the incremental
//!   [`IncrementalWitnessIndex`] over the *same* immutable
//!   [`L3TransitionTable`]. Because [`soc_core::engine::IncrementalEngine`]
//!   owns its regime mutably, the naive differential oracle is expected to
//!   run a **separate** `L3Regime` instance sharing the same `Rc`-held table
//!   — see that struct's doc.
//! - **§6.3**'s four required `try_decompose` conditions
//!   ([`L3Regime::try_decompose`], [`check_l3_decomposition`]).
//! - **§3.4**'s policy compilation ([`l3_policy`], [`l3_adm`]): precisely
//!   `RegimeId::named("brix.l3.rule-agenda-saturated@1")`'s interned handle,
//!   never [`soc_core::adm::AdmAll`].
//! - **§2 item 8 / §4.1**'s all-realizing observation profile
//!   ([`build_l3_observation_profile`]): `all_realizing(G)` where `G` is
//!   *exactly* the plan's `N` generators.
//!
//! **Not this module's job (ADR-0012 Stages C/D, out of scope here):** no
//! incremental settlement adapter, no bounded driver, no `run_saturated`
//! integration, no CLI. This module supplies the *ingredients* a Stage C
//! adapter will assemble; it does not assemble them into a stepping loop.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use brix_semantic::{ConfigId, Decomposition, GeneratorId, RegimeId};

use soc_core::adm::AdmWitnessAllowlist;
use soc_core::commit::{CommitError, SettlementWitnessProvider};
use soc_core::delta::{CandidateDelta, Delta, Footprint};
use soc_core::engine::IncrementalWitnessIndex;
use soc_core::exec::ExecConfig;
use soc_core::intern::{Handle, Interner};
use soc_core::saturate::GeneratorPartitionProfile;
use soc_core::witness_provider::{Candidate, WitnessProvider};

use crate::l3::{L3PlanItem, L3PlanV1, L3ValueV1, L3_PROFILE_MARKER_V1};
use crate::l3_canon::{
    fact_id, l3_generator_id, l3_value_id, l3_witness_id, rule_id, world_id, FactChainIdV1, FactV1,
    L3PolicyV1, L3WorldV1, PendingIdV1, ProgramIdV1, RuleId,
};

// ---------------------------------------------------------------------------
// The precomputed transition table (ADR-0012 §3.3).
// ---------------------------------------------------------------------------

/// One rule's precomputed decomposition data: the single generator this
/// transition's committed witness must compose from, the interned handle of
/// that generator's primitive `WitnessId` (cached at setup so
/// [`L3Regime::try_decompose`] can check §6.3 condition 2 by `Handle`
/// equality — same-interner handle equality *is* resolved-digest equality,
/// exactly the discipline [`crate::l3_canon`]'s module doc and ADR-0012 §3.4
/// describe for `resolved(...)` — without a second interner round-trip), and
/// the exact pre/post canonical [`L3WorldV1`] identities this rule's
/// transition connects.
#[derive(Clone, Copy, Debug)]
struct L3DecompositionData {
    generator: GeneratorId,
    witness_handle: Handle,
    expected_src: ConfigId,
    expected_dst: ConfigId,
}

/// The bounded, one-time-computed transition table of ADR-0012 §3.3: the
/// plan's `N + 1` deterministic prefix worlds and `N` head candidate triples,
/// precomputed and interned during setup so the hot [`L3Regime`] never scans
/// the pending agenda or hashes source-sized data per call. Building this
/// table is `O(N)` and is a **non-semantic setup diagnostic** — never a
/// committed [`soc_core::cost::CostRecord`] (ADR-0012 §3.3).
///
/// Immutable once built. [`L3Regime`] wraps it in an [`Rc`] rather than
/// owning it directly: ADR-0012 §2 item 3 requires "the naive differential
/// oracle [to use] a separate regime instance over the same immutable
/// precomputed transition table," and `Rc::clone` is exactly the cheap,
/// no-duplicate-recompute way to hand two independent `L3Regime` values
/// (one driven mutably by `IncrementalEngine`, one held by a naive
/// differential-test oracle) a handle to the same table.
pub struct L3TransitionTable {
    regime_id: RegimeId,
    regime_handle: Handle,
    /// The `N + 1` world handles `W0..WN`, in order. This is exactly the
    /// declared [`IncrementalWitnessIndex::footprint`] (ADR-0012 §3.3: "its
    /// footprint is exactly those N + 1 world handles").
    worlds: Vec<Handle>,
    /// The `N + 1` canonical world identities, index-aligned with `worlds`.
    world_configs: Vec<ConfigId>,
    /// Nonterminal world handle -> its index in `worlds`/`candidates`/
    /// `decomposition_data`. Covers indices `0..N` only: `WN` (the terminal
    /// world) is deliberately absent because it proposes no candidate, and a
    /// present-but-candidate-less entry would just be a longer way to express
    /// the same "no candidate here" answer `candidate_for` already gives for
    /// any handle absent from this map.
    world_index: BTreeMap<Handle, usize>,
    /// The `N` precomputed head candidates; `candidates[i]` is the one
    /// candidate proposed at `worlds[i]`.
    candidates: Vec<Candidate>,
    /// The `N` decomposition data entries, index-aligned with `candidates`.
    decomposition_data: Vec<L3DecompositionData>,
}

impl L3TransitionTable {
    /// This table's one compiler-owned regime identity (ADR-0012 §3.4).
    pub fn regime_id(&self) -> RegimeId {
        self.regime_id
    }

    /// The interned handle of [`Self::regime_id`]. It is retained for frozen
    /// v1 presentation/key material as witness-interpretation provenance;
    /// [`l3_adm`] allow-lists the table's witness handles instead.
    pub fn regime_handle(&self) -> Handle {
        self.regime_handle
    }

    /// `W0`'s handle — the initial world (ADR-0012 §4.3: "the initial input is
    /// `Delta::of_added([W0])`").
    pub fn initial_world(&self) -> Handle {
        self.worlds[0]
    }

    /// `W0`'s canonical identity.
    pub fn initial_world_config(&self) -> ConfigId {
        self.world_configs[0]
    }

    /// `WN`'s handle — the terminal world (no candidate is ever proposed for
    /// it: the pending agenda is empty there).
    pub fn terminal_world(&self) -> Handle {
        *self
            .worlds
            .last()
            .expect("worlds always holds at least W0 (N >= 0 => N + 1 >= 1)")
    }

    /// The full `N + 1` world handle sequence `W0..WN`, in order.
    pub fn worlds(&self) -> &[Handle] {
        &self.worlds
    }

    /// The full `N + 1` canonical world identity sequence, index-aligned with
    /// [`Self::worlds`].
    pub fn world_configs(&self) -> &[ConfigId] {
        &self.world_configs
    }

    /// `N` — the number of selected rules (and thus committable candidates)
    /// this table holds.
    pub fn rule_count(&self) -> usize {
        self.candidates.len()
    }

    /// Exactly the plan's `N` generators `g(program, rᵢ)` (ADR-0012 §4.1):
    /// the realizing partition [`build_l3_observation_profile`] declares.
    pub fn generators(&self) -> BTreeSet<GeneratorId> {
        self.decomposition_data
            .iter()
            .map(|d| d.generator)
            .collect()
    }

    /// The one candidate this table associates with `world`, or `None` if
    /// `world` proposes none (the terminal world, or a handle this table does
    /// not know about at all). The single source of truth both
    /// [`WitnessProvider::candidates`] and [`IncrementalWitnessIndex::apply`] read, so the
    /// two are byte-identical by construction — never two independently
    /// maintained lookups that could drift (the differential-identity anchor,
    /// ADR-0002 §9.2).
    fn candidate_for(&self, world: Handle) -> Option<Candidate> {
        self.world_index
            .get(&world)
            .map(|&idx| self.candidates[idx])
    }

    /// The candidate at module-order rule ordinal `index` (`index <
    /// rule_count()`), or `None` past the end.
    ///
    /// ADR-0012 Stage C addition: the incremental settlement adapter's
    /// calendar keyer (§3.4) needs a `witness handle -> rule ordinal` map to
    /// compute a candidate's `priority` component, and `candidates`/
    /// `decomposition_data` are private to this module. Exposing lookup by
    /// index (rather than the raw vectors) keeps the table's internal layout
    /// free to change without widening its public surface beyond what a
    /// Stage C driver actually needs.
    pub fn candidate_at(&self, index: usize) -> Option<Candidate> {
        self.candidates.get(index).copied()
    }

    /// The index of `world` among the full `N + 1` world sequence
    /// ([`Self::worlds`]), covering the terminal world too (unlike the
    /// internal `world_index` map, which deliberately excludes it — see that
    /// field's doc). `O(N)`: intended for once-per-run bookkeeping (e.g. a
    /// Stage C driver computing `agenda_residue` from the terminal world a
    /// run ended at), never a hot-loop lookup.
    pub fn world_index_of(&self, world: Handle) -> Option<usize> {
        self.worlds.iter().position(|&w| w == world)
    }

    /// The exact pre/post canonical world identities this plan's generator
    /// `g` witnesses (ADR-0012 §2 item 7: "the semantics checks the exact
    /// plan, rule, source world, destination world, and fact identity"), or
    /// `None` if `g` is not one of this table's `N` generators.
    ///
    /// This is a **re-derivation**, not a lookup into anything a caller
    /// supplies: the pair returned here comes only from this table's own
    /// precomputed transition data, built once from the validated,
    /// normalized plan (Stage A/B) — never from a committed step's recorded
    /// `src`/`dst`. An audit semantics that instead trusted a journal
    /// step's own claim about its endpoints would not be auditing anything
    /// (ADR-0012 Stage D task brief; §2 item 7). See
    /// [`crate::l3_audit::L3GeneratorSemantics`], the sole caller.
    ///
    /// `O(N)`, a linear scan of [`Self::decomposition_data`]: intended for
    /// the audit boundary, which ADR-0012 §12 blocker 4 explicitly leaves
    /// unbudgeted — never a hot-loop lookup, and never subject to the
    /// per-committed-step `O(|Delta| × fanout)` gate (§4.3) that governs
    /// settlement, not audit.
    pub fn expected_endpoints(&self, generator: GeneratorId) -> Option<(ConfigId, ConfigId)> {
        self.decomposition_data
            .iter()
            .find(|d| d.generator == generator)
            .map(|d| (d.expected_src, d.expected_dst))
    }
}

/// Precompute and intern the plan's `N + 1` deterministic prefix worlds and
/// `N` head candidate triples (ADR-0012 §3.3). Bounded `O(N)` setup: every
/// world/candidate is built exactly once, in module order, from the plan's
/// already-validated, already-normalized rules — no rescanning, no re-hashing
/// per call once built.
///
/// **The program id is computed here, not supplied** (ADR-0020 D8). This
/// function used to take a `ProgramIdV1` alongside the plan and document that
/// an inconsistent caller "would silently mint a table for the wrong
/// revision", on the reasoning that `program_id` is a pure function of `plan`
/// so detecting the mismatch was not its job.
///
/// That was harmless while the table only drove execution. It stopped being
/// harmless once the table became the source of a *declared oracle identity*
/// (`l3_generator_semantics`): a `GeneratorSemanticsIdV1` derived from a table
/// built against the wrong program id would authenticate an audit environment
/// that never corresponded to the plan. The unchecked pairing is removed
/// rather than checked, because a seam that cannot be reached cannot be
/// reached inconsistently.
pub fn build_l3_transition_table(interner: &mut Interner, plan: &L3PlanV1) -> L3TransitionTable {
    let program = crate::l3_canon::program_id(plan);
    build_l3_transition_table_with_program(interner, program, plan)
}

/// The former public seam, now private: reached only after
/// [`build_l3_transition_table`] has established `program == program_id(plan)`.
fn build_l3_transition_table_with_program(
    interner: &mut Interner,
    program: ProgramIdV1,
    plan: &L3PlanV1,
) -> L3TransitionTable {
    let rules: Vec<(RuleId, &L3ValueV1)> = plan
        .items
        .iter()
        .filter_map(|item| match item {
            L3PlanItem::Rule {
                ordinal,
                name,
                value,
            } => Some((rule_id(program, *ordinal, name), value)),
            _ => None,
        })
        .collect();
    let n = rules.len();

    let regime_id = RegimeId::named(L3_PROFILE_MARKER_V1);
    let regime_handle = interner.intern(regime_id.digest());

    // Pending suffix identities, built tail-first: pending[n] is the empty
    // suffix; pending[i] conses rule i onto pending[i + 1] (ADR-0012 §3.3).
    // Built backward specifically so the whole table stays O(N) — calling
    // `build_pending(&rules[i..])` once per i would be O(N^2).
    let mut pending: Vec<PendingIdV1> = vec![PendingIdV1::empty(); n + 1];
    for i in (0..n).rev() {
        pending[i] = PendingIdV1::cons(rules[i].0, pending[i + 1]);
    }

    // Fact chain identities, built forward: facts[0] is genesis; facts[i + 1]
    // appends rule i's fact onto facts[i] (ADR-0012 §3.3).
    let mut facts: Vec<FactChainIdV1> = vec![FactChainIdV1::genesis(); n + 1];
    for (i, (rule, value)) in rules.iter().enumerate() {
        let fact = FactV1 {
            rule: *rule,
            payload: l3_value_id(value),
        };
        facts[i + 1] = FactChainIdV1::append(facts[i], fact_id(&fact));
    }

    // The N + 1 canonical worlds and their interned handles.
    let mut world_configs: Vec<ConfigId> = Vec::with_capacity(n + 1);
    let mut worlds: Vec<Handle> = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let world = L3WorldV1 {
            program,
            pending: pending[i],
            facts: facts[i],
            fact_count: i as u64,
        };
        let config = world_id(&world);
        let handle = interner.intern(config.digest());
        world_configs.push(config);
        worlds.push(handle);
    }

    // The N head candidates and their decomposition data.
    let mut world_index: BTreeMap<Handle, usize> = BTreeMap::new();
    let mut candidates: Vec<Candidate> = Vec::with_capacity(n);
    let mut decomposition_data: Vec<L3DecompositionData> = Vec::with_capacity(n);
    for (i, (rule, _)) in rules.iter().enumerate() {
        let src = world_configs[i];
        let dst = world_configs[i + 1];
        let generator = l3_generator_id(program, *rule, src, dst);
        let witness_id = l3_witness_id(generator);
        let witness_handle = interner.intern(witness_id.digest());

        world_index.insert(worlds[i], i);
        candidates.push(Candidate {
            witness: witness_handle,
            successor: worlds[i + 1],
        });
        decomposition_data.push(L3DecompositionData {
            generator,
            witness_handle,
            expected_src: src,
            expected_dst: dst,
        });
    }

    L3TransitionTable {
        regime_id,
        regime_handle,
        worlds,
        world_configs,
        world_index,
        candidates,
        decomposition_data,
    }
}

// ---------------------------------------------------------------------------
// Policy (ADR-0012 §3.4).
// ---------------------------------------------------------------------------

/// The ADR-0012 §3.4 policy envelope over `table`'s one compiler-owned
/// regime identity.
pub fn l3_policy(program: ProgramIdV1, table: &L3TransitionTable) -> L3PolicyV1 {
    L3PolicyV1 {
        plan: program,
        profile: L3_PROFILE_MARKER_V1.to_string(),
        regime: table.regime_id(),
    }
}

/// The compiled policy admits exactly the precomputed primitive witnesses.
/// `RegimeId` remains witness-interpretation provenance in [`l3_policy`]; it
/// is not candidate/provider identity.
pub fn l3_adm(table: &L3TransitionTable) -> AdmWitnessAllowlist {
    AdmWitnessAllowlist::new(table.candidates.iter().map(|c| c.witness))
}

// ---------------------------------------------------------------------------
// The compiler-owned dual regime (ADR-0012 §2 item 3).
// ---------------------------------------------------------------------------

/// The compiler-owned dual regime: one type implementing both the retained
/// naive [`WitnessProvider`] and the incremental [`IncrementalWitnessIndex`] over the same
/// [`L3TransitionTable`]. It derives a [`Candidate`] only from the head of
/// the pending v1 agenda — concretely, from whichever nonterminal world
/// [`L3TransitionTable::candidate_for`] maps the current world handle to.
///
/// Cloning an `L3Regime` clones only the `Rc` pointer, never the table data —
/// this is what makes it cheap to hand the engine one mutably-owned instance
/// and a naive differential oracle a second, independent instance over the
/// *same* immutable table (ADR-0012 §2 item 3).
#[derive(Clone)]
pub struct L3Regime {
    table: Rc<L3TransitionTable>,
}

impl L3Regime {
    /// Wrap a precomputed, immutable [`L3TransitionTable`].
    pub fn new(table: Rc<L3TransitionTable>) -> Self {
        L3Regime { table }
    }

    /// The underlying transition table.
    pub fn table(&self) -> &L3TransitionTable {
        &self.table
    }

    /// ADR-0012 §4.3's shared counted apply. `IncrementalWitnessIndex::apply`
    /// delegates to this so the deterministic per-touched-handle lookup count
    /// is independently observable in conformance tests —
    /// `IncrementalEngine::StepReport` counts produced entries but cannot see
    /// work hidden inside `apply` (§4.3), which is exactly why this method
    /// exists as its own callable unit rather than being folded silently into
    /// the trait method.
    ///
    /// The deterministic count is **one transition-table lookup per touched
    /// handle and no agenda-element probes**: `delta.touched()` is walked
    /// exactly once, and each handle costs exactly one
    /// [`L3TransitionTable::candidate_for`] lookup — a single `BTreeMap`
    /// probe — regardless of how many rules remain in the plan.
    pub fn apply_counted(&self, delta: &Delta) -> (CandidateDelta, u64) {
        let mut out = CandidateDelta::new();
        let mut probes: u64 = 0;
        for h in delta.touched() {
            probes += 1;
            if let Some(candidate) = self.table.candidate_for(h) {
                if delta.added.contains(&h) {
                    out.added.insert(candidate);
                }
                if delta.removed.contains(&h) {
                    out.removed.insert(candidate);
                }
            }
        }
        (out, probes)
    }
}

impl WitnessProvider for L3Regime {
    /// Naive by trait contract (unbounded, total — ADR-0012 §4.2 ⟨D-CAND⟩),
    /// but structurally free here: this table's `candidate_for` is a single
    /// precomputed lookup, never a scan of the pending agenda, so there is no
    /// enumeration to bound (ADR-0012 §4.2: "An unbounded-total `candidates`
    /// is free here precisely because this profile never enumerates").
    fn candidates(&self, e: &ExecConfig) -> Vec<Candidate> {
        self.table.candidate_for(e.world).into_iter().collect()
    }
}

impl IncrementalWitnessIndex for L3Regime {
    /// Exactly the plan's `N + 1` world handles (ADR-0012 §3.3).
    fn footprint(&self) -> Footprint {
        Footprint::configs(self.table.worlds.iter().copied())
    }

    fn apply(&mut self, delta: &Delta) -> CandidateDelta {
        self.apply_counted(delta).0
    }
}

// ---------------------------------------------------------------------------
// Fallible decomposition (ADR-0012 §6.3).
// ---------------------------------------------------------------------------

/// ADR-0012 §6.3's four required conditions, checked independently, in
/// order, over already-resolved expectations, returning
/// `soc_core::commit::CommitError` directly.
///
/// Each condition maps onto its **own** distinct `CommitError` variant —
/// `CandidateMismatch`, `WitnessMismatch`, `GeneratorMismatch`, and the
/// pre-existing `EndpointMismatch` — added to `soc-core` additively for
/// exactly this purpose (ADR-0012 Stage B, #251). There is no shared or
/// many-to-one mapping here: per ADR-0002 §5.3's overclaiming discipline, the
/// reason code an operator/auditor eventually reads via
/// `Unknown(CommitFailed { error })` must name the condition that actually
/// failed, not one that merely happened to be available.
///
/// A real call through [`L3Regime::try_decompose`] always passes
/// `candidate`/`decomposition` built from the very same `expected_*` values
/// this function is given, so in practice this always succeeds. It exists,
/// and is written as four separate assertions — never short-circuited by
/// the fact that the real caller's construction happens to satisfy all four
/// tautologically — precisely so a future refactor that breaks that
/// invariant (e.g. an off-by-one between `candidates` and
/// `decomposition_data`) is caught here rather than silently producing or
/// committing a wrong [`Decomposition`] (ADR-0012 §6.1: "no silent
/// omission").
fn check_l3_decomposition(
    expected_candidate: &Candidate,
    expected_generator: GeneratorId,
    expected_witness_handle: Handle,
    expected_src: ConfigId,
    expected_dst: ConfigId,
    candidate: &Candidate,
    decomposition: &Decomposition,
) -> Result<(), CommitError> {
    // Condition 1: candidate == transition_table[current_world].
    if candidate != expected_candidate {
        return Err(CommitError::CandidateMismatch);
    }
    // Condition 2: the resolved candidate witness equals the expected
    // generator's primitive WitnessId. "Resolved" (ADR-0012 §3.4) means the
    // canonical digest recovered from the same Interner at the commit
    // boundary; comparing the cached witness handle directly is equivalent
    // here because both `candidate.witness` and `expected_witness_handle`
    // were interned from the same Interner this table was built with (same
    // interner => handle equality iff digest equality).
    if candidate.witness != expected_witness_handle {
        return Err(CommitError::WitnessMismatch);
    }
    // Condition 3: decomposition.generators() == [expected_generator].
    if decomposition.generators() != [expected_generator] {
        return Err(CommitError::GeneratorMismatch);
    }
    // Condition 4: decomposition.configs() == [expected_src, expected_dst].
    if decomposition.configs() != [expected_src, expected_dst] {
        return Err(CommitError::EndpointMismatch);
    }
    Ok(())
}

impl SettlementWitnessProvider for L3Regime {
    /// The tight decomposition realizing a committed L3 candidate is always a
    /// single generator step `src -[g(program, rule)]-> dst`, per ADR-0012
    /// §3.1's one-generator-per-rule decomposition. Fallible: an `e.world`
    /// this table has no transition for, or a `c` that does not match what
    /// this table associates with `e.world`, is a rejected
    /// [`CommitError`] — never a panic, never a fabricated `Decomposition`
    /// (ADR-0012 §6.3).
    fn try_decompose(&self, e: &ExecConfig, c: &Candidate) -> Result<Decomposition, CommitError> {
        let idx = *self
            .table
            .world_index
            .get(&e.world)
            .ok_or(CommitError::UnresolvedHandle)?;
        let data = &self.table.decomposition_data[idx];
        let expected_candidate = &self.table.candidates[idx];

        let decomposition = Decomposition::recorded(
            vec![data.generator],
            vec![data.expected_src, data.expected_dst],
        )?;

        check_l3_decomposition(
            expected_candidate,
            data.generator,
            data.witness_handle,
            data.expected_src,
            data.expected_dst,
            c,
            &decomposition,
        )?;

        Ok(decomposition)
    }
}

// ---------------------------------------------------------------------------
// The observation profile (ADR-0012 §2 item 8, §4.1).
// ---------------------------------------------------------------------------

/// The ADR-0012 §4.1 all-realizing observation profile: `all_realizing(G)`
/// where `G` is **exactly** the plan's `N` generators — no more, no fewer —
/// so a step whose generator lies outside `G` is a fail-closed
/// [`soc_core::saturate::ProfileError::UnregisteredGenerator`] rather than a
/// silent label. The administrative partition is `𝒢_τ = ∅`
/// (`GeneratorPartitionProfile::all_realizing`'s own degenerate case,
/// ADR-0014 §3.2).
pub fn build_l3_observation_profile(table: &L3TransitionTable) -> GeneratorPartitionProfile {
    GeneratorPartitionProfile::all_realizing(table.generators())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::l3::{lower_l3_plan, PlanLimitsV1};
    use crate::l3_canon::program_id;
    use brix_canon::{CanonWriter, Digest, Domain};
    use brix_semantic::{ContextId, Outcome};
    use soc_core::adm::Adm;
    use soc_core::calendar::Key;
    use soc_core::commit::{commit_tick, Committed, Observation};
    use soc_core::history::History;
    use soc_core::journal::CommittedStep;
    use soc_core::saturate::{ObservationProfile, ProfileError, StepLabel};

    fn plan_and_table(src: &str) -> (L3PlanV1, ProgramIdV1, Interner, L3TransitionTable) {
        let module = brix_syntax::parse(src).unwrap_or_else(|e| panic!("parse failed: {e}"));
        let plan = lower_l3_plan(&module, L3_PROFILE_MARKER_V1, &PlanLimitsV1::generous())
            .unwrap_or_else(|e| panic!("lowering failed: {e:?}"));
        let program = program_id(&plan);
        let mut interner = Interner::new();
        let table = build_l3_transition_table(&mut interner, &plan);
        (plan, program, interner, table)
    }

    fn tiebreak_of(c: &Candidate) -> Digest {
        let mut w = CanonWriter::new();
        w.write_uint(c.witness.raw() as u64);
        w.write_uint(c.successor.raw() as u64);
        w.digest(Domain::Value)
    }

    // -----------------------------------------------------------------
    // §9 Stage B fixture: two zero-argument rules in reverse lexical name
    // order. The module-order rule MUST commit first.
    // -----------------------------------------------------------------

    #[test]
    fn module_order_not_lexical_order_governs_candidate_sequence() {
        // "zeta" is declared first, "alpha" second — reverse lexical order.
        let (plan, _program, mut interner, table) =
            plan_and_table("rule zeta() = 1\nrule alpha() = 2\n");
        assert_eq!(table.rule_count(), 2);

        let rule_names: Vec<&str> = plan
            .items
            .iter()
            .filter_map(|it| match it {
                L3PlanItem::Rule { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(rule_names, vec!["zeta", "alpha"]);

        let policy = interner.intern(Digest::of(Domain::Value, b"policy"));
        let regime = L3Regime::new(Rc::new(table));
        let regimes: Vec<&dyn SettlementWitnessProvider> = vec![&regime];
        let adm = l3_adm(regime.table());
        let context = ContextId::root();

        // World 0: committing must decompose exactly W0 -> W1 — the pair the
        // *first-declared* rule ("zeta") connects, not "alpha"'s.
        let w0 = regime.table().initial_world();
        let e0 = ExecConfig::new(w0, policy, History::empty().digest());

        let (committed0, step0, _cost0) = commit_tick(
            &regimes,
            &adm,
            &interner,
            &e0,
            context,
            0,
            &mut |c, phase| Key::new(phase, 0, tiebreak_of(c)),
        );
        let Committed::Step { successor: e1, .. } = committed0 else {
            panic!("expected a committed step at w0");
        };
        let step0 = step0.expect("Committed::Step carries a CommittedStep");
        assert_eq!(step0.src, regime.table().world_configs()[0]);
        assert_eq!(step0.dst, regime.table().world_configs()[1]);
        assert_eq!(step0.decomposition.generators().len(), 1);
        assert!(regime
            .table()
            .generators()
            .contains(&step0.decomposition.generators()[0]));

        // World 1: committing must now decompose exactly W1 -> W2 — "alpha"'s
        // pair, which only becomes eligible once "zeta" has already committed.
        let (committed1, step1, _cost1) = commit_tick(
            &regimes,
            &adm,
            &interner,
            &e1,
            context,
            1,
            &mut |c, phase| Key::new(phase, 0, tiebreak_of(c)),
        );
        let Committed::Step { successor: e2, .. } = committed1 else {
            panic!("expected a committed step at w1");
        };
        let step1 = step1.expect("Committed::Step carries a CommittedStep");
        assert_eq!(step1.src, regime.table().world_configs()[1]);
        assert_eq!(step1.dst, regime.table().world_configs()[2]);

        // Terminal world: no candidate remains — the run is quiescent.
        let (committed2, step2, _cost2) = commit_tick(
            &regimes,
            &adm,
            &interner,
            &e2,
            context,
            2,
            &mut |c, phase| Key::new(phase, 0, tiebreak_of(c)),
        );
        assert_eq!(committed2, Committed::Quiescent);
        assert!(step2.is_none());
    }

    // -----------------------------------------------------------------
    // Precomputed transition table shape.
    // -----------------------------------------------------------------

    #[test]
    fn table_has_n_plus_one_worlds_and_n_candidates() {
        let (_, _, _, table) = plan_and_table("rule a() = 1\nrule b() = 2\nrule c() = 3\n");
        assert_eq!(table.worlds().len(), 4);
        assert_eq!(table.world_configs().len(), 4);
        assert_eq!(table.rule_count(), 3);
        assert_eq!(table.generators().len(), 3);
    }

    #[test]
    fn empty_plan_has_exactly_one_world_and_no_candidates() {
        let (_, _, _, table) = plan_and_table("let a = 1\n");
        assert_eq!(table.worlds().len(), 1);
        assert_eq!(table.rule_count(), 0);
        assert!(table.candidate_for(table.initial_world()).is_none());
    }

    #[test]
    fn candidates_and_apply_agree_on_the_same_precomputed_candidate() {
        // The differential-identity anchor at the unit level: WitnessProvider and
        // IncrementalWitnessIndex must derive the SAME candidate for the same
        // world, because both read through `L3TransitionTable::candidate_for`.
        let (_, _, _, table) = plan_and_table("rule a() = 1\n");
        let table = Rc::new(table);
        let naive = L3Regime::new(Rc::clone(&table));
        let mut incremental = L3Regime::new(Rc::clone(&table));

        let mut i2 = Interner::new();
        let policy = i2.intern(Digest::of(Domain::Value, b"p"));
        let e0 = ExecConfig::new(table.initial_world(), policy, History::empty().digest());

        let naive_candidates = naive.candidates(&e0);
        assert_eq!(naive_candidates.len(), 1);

        let delta = Delta::of_added([table.initial_world()]);
        let cd = incremental.apply(&delta);
        assert_eq!(
            cd.added,
            std::collections::BTreeSet::from([naive_candidates[0]])
        );
    }

    #[test]
    fn apply_counted_pays_exactly_one_probe_per_touched_handle() {
        let (_, _, _, table) = plan_and_table("rule a() = 1\nrule b() = 2\n");
        let regime = L3Regime::new(Rc::new(table));

        let w0 = regime.table().worlds()[0];
        let w1 = regime.table().worlds()[1];
        let (_cd, probes) = regime.apply_counted(&Delta::of_added([w0]));
        assert_eq!(probes, 1);

        let (_cd2, probes2) = regime.apply_counted(&Delta::between_worlds(w0, w1));
        assert_eq!(probes2, 2, "between_worlds touches exactly two handles");

        // Ballast: touching a handle this regime does not know about at all
        // still costs exactly one probe, never more.
        let mut i = Interner::new();
        let unrelated = i.intern(Digest::of(Domain::Value, b"unrelated"));
        let (cd3, probes3) = regime.apply_counted(&Delta::of_added([unrelated]));
        assert_eq!(probes3, 1);
        assert!(cd3.is_empty());
    }

    #[test]
    fn footprint_is_exactly_the_n_plus_one_world_handles() {
        let (_, _, _, table) = plan_and_table("rule a() = 1\nrule b() = 2\n");
        let expected: std::collections::BTreeSet<Handle> = table.worlds().iter().copied().collect();
        let regime = L3Regime::new(Rc::new(table));
        match regime.footprint() {
            Footprint::Configs(set) => assert_eq!(set, expected),
            Footprint::AllConfigs => panic!("L3Regime must declare an explicit config footprint"),
        }
    }

    // -----------------------------------------------------------------
    // Policy (ADR-0012 §3.4).
    // -----------------------------------------------------------------

    #[test]
    fn policy_provenance_and_witness_allowlist_are_distinct() {
        let (_, program, mut interner, table) = plan_and_table("rule a() = 1\n");
        let policy = l3_policy(program, &table);
        assert_eq!(policy.regime, table.regime_id());

        let policy_h = interner.intern(Digest::of(Domain::Value, b"policy"));
        let world_h = interner.intern(Digest::of(Domain::Value, b"world"));
        let e = ExecConfig::new(world_h, policy_h, History::empty().digest());
        let admitted = table.candidate_at(0).unwrap();
        let denied = Candidate {
            witness: interner.intern(Digest::of(Domain::Value, b"other-witness")),
            successor: world_h,
        };
        let adm = l3_adm(&table);
        assert!(adm.admits(&e, &admitted));
        assert!(!adm.admits(&e, &denied));
    }

    // -----------------------------------------------------------------
    // §6.3's four required conditions, each independently violated.
    // -----------------------------------------------------------------

    #[test]
    fn condition_1_wrong_transition_table_candidate_is_rejected() {
        let (_, _, mut interner, table) = plan_and_table("rule a() = 1\nrule b() = 2\n");
        let w0 = table.initial_world();
        let policy = interner.intern(Digest::of(Domain::Value, b"policy"));
        let e0 = ExecConfig::new(w0, policy, History::empty().digest());
        let mut candidate = table.candidate_for(w0).expect("w0 proposes a candidate");
        // Corrupt only the successor, using a handle freshly interned from
        // the SAME interner the table was built with — guaranteed distinct
        // from every handle the table already produced.
        candidate.successor = interner.intern(Digest::of(Domain::Value, b"not-the-real-w1"));

        let regime = L3Regime::new(Rc::new(table));
        let err = regime
            .try_decompose(&e0, &candidate)
            .expect_err("a candidate not matching the transition table must be rejected");
        assert_eq!(err, CommitError::CandidateMismatch);
    }

    #[test]
    fn condition_2_witness_generator_mismatch_is_rejected() {
        let (_, _, mut interner, table) = plan_and_table("rule a() = 1\n");
        let idx = 0usize;
        let expected_candidate = table.candidates[idx];
        let data = table.decomposition_data[idx];

        // Everything about `wrong_candidate` matches `expected_candidate`
        // except its witness — condition 1 (candidate == expected) therefore
        // ALSO fails for this candidate, since `Candidate`'s equality is over
        // all three fields; to isolate condition 2 specifically, check it
        // directly rather than through the full `try_decompose` chain (which
        // checks condition 1 first and would report `CandidateMismatch`
        // instead).
        let mut wrong_candidate = expected_candidate;
        wrong_candidate.witness = interner.intern(Digest::of(Domain::Value, b"wrong-witness"));

        let decomposition = Decomposition::recorded(
            vec![data.generator],
            vec![data.expected_src, data.expected_dst],
        )
        .unwrap();

        let err = check_l3_decomposition(
            &wrong_candidate,
            data.generator,
            data.witness_handle,
            data.expected_src,
            data.expected_dst,
            &wrong_candidate,
            &decomposition,
        )
        .expect_err("a witness not matching the expected generator's WitnessId must be rejected");
        assert_eq!(err, CommitError::WitnessMismatch);
    }

    #[test]
    fn condition_3_wrong_generator_list_is_rejected() {
        let (_, _, _, table) = plan_and_table("rule a() = 1\n");
        let idx = 0usize;
        let expected_candidate = table.candidates[idx];
        let data = table.decomposition_data[idx];

        let wrong_generator = GeneratorId::named("not-the-expected-generator@1");
        let decomposition = Decomposition::recorded(
            vec![wrong_generator],
            vec![data.expected_src, data.expected_dst],
        )
        .unwrap();

        let err = check_l3_decomposition(
            &expected_candidate,
            data.generator,
            data.witness_handle,
            data.expected_src,
            data.expected_dst,
            &expected_candidate,
            &decomposition,
        )
        .expect_err("a decomposition citing the wrong generator must be rejected");
        assert_eq!(err, CommitError::GeneratorMismatch);
    }

    #[test]
    fn condition_4_wrong_endpoint_pair_is_rejected() {
        let (_, _, _, table) = plan_and_table("rule a() = 1\n");
        let idx = 0usize;
        let expected_candidate = table.candidates[idx];
        let data = table.decomposition_data[idx];

        let wrong_dst = ConfigId::from_canon(b"not-the-expected-destination");
        let decomposition =
            Decomposition::recorded(vec![data.generator], vec![data.expected_src, wrong_dst])
                .unwrap();

        let err = check_l3_decomposition(
            &expected_candidate,
            data.generator,
            data.witness_handle,
            data.expected_src,
            data.expected_dst,
            &expected_candidate,
            &decomposition,
        )
        .expect_err("a decomposition with the wrong endpoint pair must be rejected");
        assert_eq!(err, CommitError::EndpointMismatch);
    }

    #[test]
    fn the_four_conditions_map_to_four_pairwise_distinct_commit_errors() {
        // Guards against a future refactor silently re-collapsing two of
        // §6.3's four conditions onto the same `CommitError` variant (exactly
        // the bug this fix corrects): construct one violation of each
        // condition in turn, holding the other three satisfied, and check the
        // four resulting errors are pairwise distinct — not just each
        // individually plausible in isolation.
        let (_, _, mut interner, table) = plan_and_table("rule a() = 1\n");
        let idx = 0usize;
        let expected_candidate = table.candidates[idx];
        let data = table.decomposition_data[idx];
        let ok_decomposition = Decomposition::recorded(
            vec![data.generator],
            vec![data.expected_src, data.expected_dst],
        )
        .unwrap();

        let mut wrong_candidate_successor = expected_candidate;
        wrong_candidate_successor.successor = interner.intern(Digest::of(
            Domain::Value,
            b"distinctness-check-wrong-successor",
        ));
        let err1 = check_l3_decomposition(
            &expected_candidate,
            data.generator,
            data.witness_handle,
            data.expected_src,
            data.expected_dst,
            &wrong_candidate_successor,
            &ok_decomposition,
        )
        .unwrap_err();

        let mut wrong_witness_candidate = expected_candidate;
        wrong_witness_candidate.witness = interner.intern(Digest::of(
            Domain::Value,
            b"distinctness-check-wrong-witness",
        ));
        let err2 = check_l3_decomposition(
            &wrong_witness_candidate,
            data.generator,
            data.witness_handle,
            data.expected_src,
            data.expected_dst,
            &wrong_witness_candidate,
            &ok_decomposition,
        )
        .unwrap_err();

        let wrong_generator_decomposition = Decomposition::recorded(
            vec![GeneratorId::named("distinctness-check-wrong-generator@1")],
            vec![data.expected_src, data.expected_dst],
        )
        .unwrap();
        let err3 = check_l3_decomposition(
            &expected_candidate,
            data.generator,
            data.witness_handle,
            data.expected_src,
            data.expected_dst,
            &expected_candidate,
            &wrong_generator_decomposition,
        )
        .unwrap_err();

        let wrong_endpoint_decomposition = Decomposition::recorded(
            vec![data.generator],
            vec![
                data.expected_src,
                ConfigId::from_canon(b"distinctness-check-wrong-endpoint"),
            ],
        )
        .unwrap();
        let err4 = check_l3_decomposition(
            &expected_candidate,
            data.generator,
            data.witness_handle,
            data.expected_src,
            data.expected_dst,
            &expected_candidate,
            &wrong_endpoint_decomposition,
        )
        .unwrap_err();

        assert_eq!(err1, CommitError::CandidateMismatch);
        assert_eq!(err2, CommitError::WitnessMismatch);
        assert_eq!(err3, CommitError::GeneratorMismatch);
        assert_eq!(err4, CommitError::EndpointMismatch);

        let errors = [err1, err2, err3, err4];
        for i in 0..errors.len() {
            for j in 0..errors.len() {
                if i != j {
                    assert_ne!(
                        errors[i], errors[j],
                        "condition {i} and condition {j} must not share a CommitError variant"
                    );
                }
            }
        }
    }

    #[test]
    fn a_correctly_constructed_decomposition_passes_all_four_conditions() {
        // The positive control for the four negative tests above: with every
        // expectation satisfied, try_decompose succeeds end-to-end.
        let (_, _, mut interner, table) = plan_and_table("rule a() = 1\n");
        let policy = interner.intern(Digest::of(Domain::Value, b"policy"));
        let regime = L3Regime::new(Rc::new(table));
        let e0 = ExecConfig::new(
            regime.table().initial_world(),
            policy,
            History::empty().digest(),
        );
        let real_candidate = regime.table().candidate_for(e0.world).unwrap();
        assert!(regime.try_decompose(&e0, &real_candidate).is_ok());
    }

    #[test]
    fn try_decompose_on_an_unknown_world_is_unresolved_handle() {
        let (_, _, mut i, table) = plan_and_table("rule a() = 1\n");
        let regime = L3Regime::new(Rc::new(table));
        let unknown_world = i.intern(Digest::of(Domain::Value, b"never-in-the-table"));
        let policy = i.intern(Digest::of(Domain::Value, b"policy"));
        let e = ExecConfig::new(unknown_world, policy, History::empty().digest());
        let bogus = Candidate {
            witness: i.intern(Digest::of(Domain::Value, b"w")),
            successor: unknown_world,
        };
        let err = regime.try_decompose(&e, &bogus).unwrap_err();
        assert_eq!(err, CommitError::UnresolvedHandle);
    }

    // -----------------------------------------------------------------
    // Observation profile (ADR-0012 §2 item 8, §4.1).
    // -----------------------------------------------------------------

    #[test]
    fn profile_labels_every_committed_step_realizing() {
        let (_, _, mut interner, table) = plan_and_table("rule a() = 1\nrule b() = 2\n");
        let policy = interner.intern(Digest::of(Domain::Value, b"policy"));
        let table = Rc::new(table);
        let regime = L3Regime::new(Rc::clone(&table));
        let profile = build_l3_observation_profile(&table);

        let regimes: Vec<&dyn SettlementWitnessProvider> = vec![&regime];
        let adm = l3_adm(&table);
        let e0 = ExecConfig::new(table.initial_world(), policy, History::empty().digest());
        let (committed, step, _cost) = commit_tick(
            &regimes,
            &adm,
            &interner,
            &e0,
            ContextId::root(),
            0,
            &mut |c, phase| Key::new(phase, 0, tiebreak_of(c)),
        );
        assert!(matches!(committed, Committed::Step { .. }));
        let step = step.unwrap();
        assert_eq!(profile.label(&step), Ok(StepLabel::Realizing));
    }

    #[test]
    fn profile_rejects_a_generator_outside_the_plan_as_unregistered() {
        let (_, _, _, table) = plan_and_table("rule a() = 1\n");
        let profile = build_l3_observation_profile(&table);

        let foreign_generator = GeneratorId::named("some-other-plan.rule@1");
        let decomposition = Decomposition::recorded(
            vec![foreign_generator],
            vec![
                ConfigId::from_canon(b"foreign-x0"),
                ConfigId::from_canon(b"foreign-x1"),
            ],
        )
        .unwrap();
        let step = fake_step(decomposition);
        assert_eq!(
            profile.label(&step),
            Err(ProfileError::UnregisteredGenerator)
        );
    }

    #[test]
    fn observation_profile_id_is_stable_across_interner_insertion_orders() {
        // Build the same plan through two interners that intern different
        // extra, unrelated digests around building the table: the profile id
        // depends only on canonical GeneratorIds, never on interner
        // allocation order.
        let src = "rule a() = 1\nrule b() = 2\n";
        let module = brix_syntax::parse(src).unwrap();
        let plan = lower_l3_plan(&module, L3_PROFILE_MARKER_V1, &PlanLimitsV1::generous()).unwrap();

        let mut i1 = Interner::new();
        let table1 = build_l3_transition_table(&mut i1, &plan);

        let mut i2 = Interner::new();
        i2.intern(Digest::of(Domain::Value, b"noise-1"));
        let table2 = build_l3_transition_table(&mut i2, &plan);
        i2.intern(Digest::of(Domain::Value, b"noise-2"));

        let p1 = build_l3_observation_profile(&table1);
        let p2 = build_l3_observation_profile(&table2);
        assert_eq!(p1.id(), p2.id());
    }

    /// A `CommittedStep` sufficient for exercising `ObservationProfile::label`,
    /// which reads only `decomposition.generators()` — every other field is a
    /// harmless placeholder.
    fn fake_step(decomposition: Decomposition) -> CommittedStep {
        let src = decomposition.configs()[0];
        let dst = *decomposition.configs().last().unwrap();
        let witness = brix_semantic::compose_chain(decomposition.generators()).unwrap();
        CommittedStep {
            key: Key::new(0, 0, Digest::of(Domain::Value, b"k")),
            observation: Observation {
                outcome_class: Outcome::Derived,
                judgement_digest: Digest::of(Domain::Value, b"j"),
            },
            decomposition,
            src,
            dst,
            witness,
        }
    }
}
