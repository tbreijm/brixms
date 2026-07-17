//! Parser-facing re-exports of the workspace diagnostic channel.
//!
//! The parser owns recovery and source order; `brix-diag` owns the diagnostic
//! representation and every rendering.  Keeping this module preserves the
//! established parser API while ensuring later compiler stages receive the
//! same concrete type.

pub use brix_diag::{BrxCode, CanonValue, Diagnostic, Diagnostics, Label, Severity};
