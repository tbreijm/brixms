//! `soc-regimes` — client realization regimes over `soc-core` (ADR-0002 §7
//! "Realization regimes"; `spec/Build_Plan_v3_SOC.md` Step 5, "First
//! regimes").
//!
//! ADR-0002 §7 is explicit about where a regime lives architecturally:
//!
//! > A **realization regime** (formerly "resolver": `brix.type`,
//! > `brix.proof`, `brix.complexity`, compatibility, authorization, …) is an
//! > ordinary sealed BrixMS package presenting a class of witnesses under
//! > one `ρ_w` interpretation.
//!
//! This crate is that package layer: it implements `soc_core`'s
//! [`soc_core::WitnessProvider`] / [`soc_core::SettlementWitnessProvider`] traits (and, for the
//! audit boundary, [`soc_core::audit::GeneratorSemantics`]) **without
//! modifying `soc-core` itself** — a regime proposes candidates and records
//! decompositions; it never publishes `Derived` (only the calendar/commit
//! loop does) or `Proven`/`Refuted` (only `brix-kernel` does), per ADR-0002
//! §5 point 4 / §7.
//!
//! Landed so far (Build Plan Step 5(a), the first vertical slice):
//!
//! - [`literal`] — the **literal-equality** regime: the simplest possible
//!   `ρ_w`, the diagonal relation `x ⊨_w y ⟺ x == y`. See
//!   `tests/literal_vertical_slice.rs` for the full round trip — candidates
//!   → `Adm` → calendar/commit → a `Derived` `Realizes` judgement → the
//!   audit-factorization checker's upgrade to `Audited` — exercising the
//!   whole SOC loop end to end for the first time.
//!
//! - [`native`] — the **native Brix type checker (SOC paradigm)** (ADR-0009): conflict
//!   detection (all 8 type-inference categories + all 6 rule-side-conditions)
//!   and, via [`type_realization`], `HasType` judgements that elaborate to
//!   `Proven` (ADR-0005/0007/0008). The legacy `brix-ir` differential oracle
//!   it was built against was deleted at zero-legacy (N9); native is now the
//!   sole checker.

pub mod coverage;
pub mod finite_frontier;
pub mod literal;
pub mod native;
pub mod tree_audit;
// A generator's discharge ground under the type-realization contract §9.2(1)
// lives in its doc comment; §9.1 is explicit that the `generator_is_tight`
// entry alone is not a discharge. So an undocumented item in this module is a
// missing normative artifact, not a missing convenience.
//
// This is scoped here rather than crate-wide because it guards a specific
// failure that has happened twice: inserting a new generator between an
// existing doc comment and the `pub fn` it belongs to. #296 did it to
// `g_arith_split` and #317 did it to `g_bool_lit`, each time leaving the
// displaced item bare and stacking its ground onto the newcomer — which is how
// `g_fix`, deliberately NOT discharged, came to carry a doc opening
// "Discharged tight on the same grounds as `g_lit`". Verified to fire on that
// exact shape rather than assumed to: reinserting a documented generator ahead
// of `g_bool_lit`'s `pub fn` fails the build.
//
// The residual gap, stated rather than glossed: this catches an insertion
// between a doc BLOCK and its item, which is what both instances were. An
// insertion *inside* a block leaves the displaced item holding the block's
// tail, and a partial doc is still a doc. Nothing here catches that.
#[deny(missing_docs)]
pub mod type_realization;

pub use finite_frontier::{
    explain_why, explain_why_not, AdmissionDecision, AdmissionPolicy, AdmitAllPolicy,
    CandidateStatus, CanonicalCandidateV1, DeliberationOutcome, DenyAllPolicy, EvaluatedFrontier,
    EvaluationFault, FiniteCandidateRegime, FnPolicy, GuardPolicy, NamedCandidate,
    PolicyToAdmAdapter, ReasonCode, WhyExplanation, WhyNotExplanation, CANONICAL_TIEBREAK_TAG,
    FINITE_DECISION_PROFILE_MARKER, FINITE_FRONTIER_GENERATOR_NAME, FINITE_FRONTIER_REGIME_NAME,
};
pub use literal::{literal_equality_semantics, LiteralEqualityRegime};
pub use type_realization::{g_lit, g_var, Expr, Ty, TyCtx, TypeError};
