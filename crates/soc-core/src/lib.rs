//! `soc-core` — the SOC settlement-side engine core (ADR-0002 — SOC
//! Constitution — `spec/adr/ADR-0002_SOC_Constitution.md`).
//!
//! Build order (ADR-0002 §9.3; `spec/Build_Plan_v3_SOC.md` Steps 2–3):
//!
//! - **E1** ([`intern`], [`store`], [`history`]): canonical-digest → dense
//!   `u32` interner; a persistent key→value store behind a swappable trait
//!   ([`store::PersistentMap`]; the HAMT-shaped ADR target, `BTreeMap`/`Arc`
//!   v1 — see that module's docs for the dependency-policy rationale); the
//!   chained history digest `h' = H(h_digest, step)`, O(1)/step.
//! - **S2⋈E2** ([`exec`], [`regime`], [`adm`], [`oracle`]): the execution
//!   config `e = ⟨x, p, h⟩`; the realization interface
//!   ([`regime::Regime::candidates`], the deliberately-naive enumerate form —
//!   see that module's docs); the admissibility judgment ([`adm::Adm`]); the
//!   naive reference oracle ([`oracle::cand`]/[`oracle::succ`]) —
//!   `brix-oracle`'s role reborn at the SOC layer (ADR-0002 §3, §9.2).
//!
//! This crate depends on `brix-canon` (canonical digests/encoding) and
//! `brix-semantic` (the `ContextId` world identity, [`exec::intern_context`])
//! **only** — no heavier dependency, per ADR-0002 §3 substrate discipline and
//! the workspace Ring-0 whitelist (root `Cargo.toml`
//! `[workspace.dependencies]`).
//!
//! - **S3⋈E4** ([`calendar`], [`commit`], [`journal`]): the keyed calendar
//!   `K = (phase/time, priority, canonical-digest tie-break)` and the
//!   unique-key deliberation frontier `B^uk_{K,O}` ([`calendar::Frontier`]);
//!   the committed coalgebra `γ = select_K ∘ δ` into `D_O = 1 + O×X`
//!   ([`commit::Committed`], [`commit::Observation`] = `O_min`,
//!   [`commit::run`]); the append-only [`journal::Journal`] chaining
//!   [`journal::CommittedStep`]s through [`history::History`]; and
//!   deterministic replay ([`journal::Journal::replay_chain`]). ADR-0002 §1,
//!   §8 (⟨D-FO⟩ ratified), §9.2; `Build_Plan_v3_SOC.md` Step 4.
//!
//! - **Lane 2** ([`audit`]): the audit-factorization checker — the sole
//!   authority for `Outcome::Audited` (ADR-0002 §4.1). Replays each committed
//!   step's recorded [`brix_semantic::Decomposition`] against the log,
//!   verifies the exact relational composition `ρ_k = ρ_gn ∘ … ∘ ρ_g1` over
//!   the intermediate-configuration chain, and — iff it composes exactly —
//!   publishes a new `Audited` judgement linked to the pre-existing `Derived`
//!   one by a `Dependency` edge ([`audit::audit_step`],
//!   [`audit::audit_journal`]). A replay that does not complete exactly
//!   yields `Unknown(reason)`, never a pass (`Build_Plan_v3_SOC.md` Step 4
//!   gate; PD-1's operational discharge).
//!
//! - **E3⋈E5** ([`delta`], [`engine`]): the incremental delta-driven regime
//!   form. [`delta::Delta`]/[`delta::CandidateDelta`]/[`delta::Footprint`] are
//!   the delta protocol over content-addressed world-config handles;
//!   [`engine::IncrementalRegime`] is the regime as a **dataflow operator**
//!   (`footprint`/`apply(delta) → candidate delta`, ADR-0002 §9.2); and
//!   [`engine::IncrementalEngine`] maintains the materialized candidate view
//!   via a footprint index, so a committed step's cost is `∝ |Δ| × fanout`,
//!   never `∝ |world|` (ADR-0002 §9.1). It lands **beside** the naive oracle,
//!   which is retained verbatim as the reference oracle the fast engine is
//!   differential-tested against ([`engine::naive_view_over`];
//!   `Build_Plan_v3_SOC.md` Step 6). The committed loop still only *records*
//!   decompositions, never verifies them (ADR-0002 §5.1) — verification is
//!   [`audit`]'s job, off the hot path.
//!
//! **Gate.** The executable governance-conservation law — tightening `Adm`
//! shrinks `cand(e)`/`Succ(e)` pointwise over every reachable `e`
//! (ADR-0002 §5 point 5, §5.5) — lives in
//! `tests/governance_conservation.rs`. The calendar/commit gates (`select_K`
//! totality, the B^uk unique-key discipline, the committed coalgebra, and
//! deterministic replay) live in `tests/calendar_commit.rs`.
//!
//! **Also E5-scaffolded here** ([`cost`], [`oracle::cand_instrumented`]): the
//! O(Δ) benchmark gate (ADR-0002 §9.1/§9.3, `Build_Plan_v3_SOC.md` Step 6) —
//! per-step [`cost::CostRecord`]s (ADR-0001 stage-4a) wired against the
//! naive oracle *before* the fast incremental engine (E3/E4) exists, so the
//! invariant is measurable from the first delta-driven candidate. The naive
//! oracle is *expected* to fail this gate by design; the armed, currently
//! `#[ignore]`d future gate lives in `tests/o_delta_gate.rs`.

pub mod adm;
pub mod audit;
pub mod calendar;
pub mod commit;
pub mod cost;
pub mod delta;
pub mod engine;
pub mod exec;
pub mod history;
pub mod intern;
pub mod journal;
pub mod oracle;
pub mod regime;
pub mod store;

pub use adm::{Adm, AdmAll, AdmNone, AdmRegimeAllowlist, AdmSuccessorFilter, AndAdm};
pub use audit::{audit_journal, audit_step, AuditResult, AuditedStep, GeneratorSemantics};
pub use calendar::{Frontier, FrontierDeltaError, Key, KeyConflict};
pub use commit::{
    commit_tick, prospective_successor, run, step_world_delta, try_commit_selected, CommitError,
    Committed, Observation, SettlementRegime,
};
pub use cost::CostRecord;
pub use delta::{CandidateDelta, Delta, Footprint};
pub use engine::{
    naive_view_over, naive_view_over_instrumented, IncrementalEngine, IncrementalRegime, StepReport,
};
pub use exec::{intern_context, ExecConfig};
pub use history::History;
pub use intern::{Handle, Interner};
pub use journal::{CommittedStep, Journal};
pub use oracle::{cand, cand_instrumented, succ};
pub use regime::{Candidate, Regime};
pub use store::{ArcMap, PersistentMap};
