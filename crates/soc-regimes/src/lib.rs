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
//! [`soc_core::Regime`] / [`soc_core::SettlementRegime`] traits (and, for the
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
pub mod literal;
pub mod native;
pub mod type_realization;

pub use literal::{LiteralEqualityRegime, LiteralEqualitySemantics};
pub use type_realization::{
    audited_type_check, g_lit, g_var, type_check, Expr, Ty, TyCtx, TypeError,
};
