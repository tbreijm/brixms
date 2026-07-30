//! Native type checker package (ADR-0009).

pub mod analyze;
pub mod syntax;
pub mod translate;

pub use analyze::{analyze, occurs, resolve, unify, zonk, NConflict, NativeReport};
pub use syntax::{
    NArg, NEdge, NExpr, NLit, NRelSchema, NSig, NTy, NativeQuery, NativeSource, Origin, SigTable,
    Sym,
};
pub use translate::{translate, translate_source, translate_ty};
