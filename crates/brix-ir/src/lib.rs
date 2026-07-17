//! brix-ir — Types, effects, traits, name resolution, Core IR, checking.
//! Ring 0 lane owner: see OWNER.md. Spec: ../../spec/BrixMS_v9_0.md
//!
//! # What this crate is
//!
//! The static-semantics core of the BrixMS toolchain (Appendix E is the
//! normative source). It defines:
//!
//! - [`core`] — **Core IR**: a small, closed set of typed nodes ([`core::Expr`],
//!   [`core::Rule`], [`core::Constraint`], [`core::Query`]) with an explicit
//!   type on every expression node and a real [`std::fmt::Display`] for
//!   debugging (a build-plan deliverable).
//! - [`types`] — the **type system representation**: [`types::Ty`], record /
//!   relation-pattern rows ([`types::Row`], row-polymorphism via
//!   [`types::RowTail`]), and the **Canonical-in-key** checks
//!   ([`types::check_key_canonical`] / [`types::check_value_canonical`]).
//! - [`effects`] — **effect rows** ([`effects::EffectRow`]) with set-union
//!   combination and the Appendix E `pure` / `det` / `nondiverge` flags.
//! - [`traits`] — the **minimal-coherent trait solving** design (no
//!   specialization, no overlapping impls, plain associated types); solving is
//!   stubbed, the shape is represented.
//! - [`pattern`] — the IR pattern language and **pattern read-set analysis**
//!   (positive / strict / history / exists classification for the phase lane).
//! - [`site`] — **stable `SiteId` assignment** for `?` / `partial` failure
//!   sites (Part III §9), derived through brix-canon so it is run-stable.
//! - [`authority`] — the **authority-constraint generation** sketch (Part XII
//!   §5): lowering `authority` to ordinary constraints / obligations.
//! - [`check`] — whole-declaration checks that need only resolved schema facts
//!   (Canonical-in-key over schemas, `without`/witness obligations).
//! - [`frontend`] — the **AST-facing interface** the parser/schema lane must
//!   satisfy, so integration is a thin adapter (the AST lane is not on this
//!   branch yet — see the module docs).
//!
//! # Discipline
//!
//! Semantic data is serialized only through `brix-canon`. No `HashMap`/
//! `HashSet` in semantic paths (sorted `Vec`/`BTree*` instead). Inference is
//! stubbed where the bounded deliverable says so, and every stub is labelled.
//! No `f64` *values* are stored anywhere; floats appear only as type tags
//! ([`types::Ty::F64`]) and canonicalized bit patterns
//! ([`pattern::Lit::F64Bits`]).

pub mod authority;
pub mod check;
pub mod core;
pub mod effects;
pub mod frontend;
pub mod ident;
pub mod pattern;
pub mod site;
pub mod traits;
pub mod types;
