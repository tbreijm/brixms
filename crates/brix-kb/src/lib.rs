//! `brix-kb` — a persistent, revisable knowledge base for Brix finite-decision
//! programs (ADR-0041).
//!
//! Today, `brix run` starts from scratch every time: a program plus input
//! files produce one `@Derived` decision, and nothing persists. This crate
//! makes that decision durable and correctable. A knowledge base is a
//! directory holding content-addressed program sources, content-addressed
//! input snapshots, and an immutable, hash-chained sequence of revision
//! records — one per `init`, `assert`, `retract`, or `program` change. Every
//! revision is re-decided in full (asserted/corrected/retracted, never
//! erased), which is `SOC-LAW-09` ("correction and retraction non-erasure",
//! `spec/SOC_Semantic_Laws.md`) made user-visible: nothing is ever rewritten
//! or deleted, and a later change is a new revision, never an edit of one
//! that came before it.
//!
//! See `spec/adr/ADR-0041_Persistent_Knowledge_Base.md` for the full design.
//! The CLI surface (`brix kb <op>`) lives in `crates/brix-cli/src/commands/kb.rs`.
//!
//! # Modules
//! - [`ops`] — the write/read operations: `init`, `assert_inputs`,
//!   `retract_inputs`, `set_program`, `log`, `show`, `audit_revision`, `verify`.
//! - [`diff`] — `diff`, the two-revision comparison with "why" explanations.
//! - [`revision`] — the immutable revision record (`brix.kb.revision@1`) and
//!   its canonical digest.
//! - [`manifest`] — the knowledge base manifest (`kb.json`) and `HEAD` pointer.
//! - [`pipeline`] — parse → resolve imports → strip `show` → lower → build →
//!   run, independent of `brix-cli`'s copy of the same pipeline.
//! - [`deps`] — the plan's dependency graph, used by `diff`'s "why".
//! - [`snapshot_io`] — encoding an input snapshot back to `brix.input@2` JSON.
//! - [`strict_json`] — a small strict, duplicate-key-rejecting JSON reader for
//!   this crate's own on-disk metadata files.
//! - [`error`] — [`error::KbError`], the unified operation error type.

pub mod deps;
pub mod diff;
pub mod error;
pub mod manifest;
pub mod ops;
pub mod packages;
pub mod paths;
pub mod pipeline;
pub mod revision;
pub mod snapshot_io;
pub mod strict_json;
pub mod world;

#[cfg(test)]
mod lifecycle_tests;

pub use error::KbError;
pub use manifest::{Head, Manifest};
pub use ops::{AuditOutcome, OpOutcome, VerifyReport};
pub use pipeline::ReplayResult;
pub use revision::{Change, RevisionRecord, RevisionResult, Status};
