//! `brix-kernel` — the dependent proof kernel for BrixMS (ADR-0003 Profile 1).
//!
//! Evaluates explicit proof terms against propositions in an assumption context
//! to produce an authoritative [`Verdict`].

mod check;
mod term;
mod verdict;

pub use check::{acceptance, Budget};
pub use term::{ExplicitTerm, Prop, TermKind, Var};
pub use verdict::{
    Certificate, RejectionReason, ResourceBudgetReason, UnsupportedConstruct, Verdict,
};
