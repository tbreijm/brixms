//! Native type checker package (ADR-0009).

pub mod analyze;
pub mod syntax;
pub mod translate;

pub use analyze::{analyze, occurs, resolve, rule_flags, unify, zonk, NConflict, NativeReport};
pub use syntax::{
    NArg, NEdge, NEffect, NEffectRow, NExpr, NLit, NRelSchema, NRule, NSig, NTy, NativeQuery,
    NativeSource, Origin, SigTable, Sym,
};
pub use translate::{translate, translate_source, translate_ty};
