//! brixc — Compiler pipeline + Rust codegen (quote + prettyplease).
//! Ring 0 lane owner: see OWNER.md. Spec: ../../spec/BrixMS_v9_0.md
//!
//! # Two-pass compilation (spec Part XXVIII §28.1)
//!
//! ```text
//! world.brix ──brixc──► generated Rust workspace ──rustc──► native world binary
//! (pass 1: BrixMS → Rust)                        (pass 2: Rust → machine code)
//! ```
//!
//! brixc *is* pass 1. All semantics live here (§28.1): backends, opt levels, and
//! targets change cost, never an observable value. The oracle (naive Core IR
//! evaluator, a sibling lane) is the conformance judge, never a deployment target.
//!
//! # Pipeline: `ast → ir → phase → plan → emit`
//!
//! The five stages are trait seams in [`pipeline`]:
//!
//! | Stage | Trait | Owner | Status |
//! |-------|-------|-------|--------|
//! | parse | [`pipeline::Frontend`] | `brix-ast` | **real** ([`lower::AstFrontend`]) |
//! | lower | [`pipeline::Lower`] | `brix-ir` | **real** ([`lower::AstLower`]) |
//! | phase | [`pipeline::PhaseAssign`] | `brix-phase` | **real** ([`phase::AstPhase`]) |
//! | plan  | [`pipeline::Plan`] | `brixc` | **real** ([`plan`]) |
//! | emit  | [`pipeline::Emit`] | `brixc` | **real** ([`emit`]) |
//!
//! The upstream three fail closed with a [`pipeline::PipelineError::Unimplemented`]
//! that names the lane it waits on — a premature call is a legible error, never a
//! silent wrong answer. Artifact types are associated types on the seams, so when
//! `brix-ir` lands its `CoreIr` becomes `Lower::Ir` with no reshaping here.
//!
//! # Codegen (Ring0_Build_Plan §1.8)
//!
//! [`emit`] generates a cargo workspace via `quote` + `prettyplease`: one module
//! per relation (store + canon-ordered `BTreeMap` indices) and one per rule (one
//! semi-naive `delta_from_<source>` fn per delta source), under a `#[deny(...)]`
//! determinism header that makes a generated `HashMap` or stray float a compile
//! error. Generated code is byte-deterministic — the flagship's workspace is an
//! `insta` snapshot so drift is reviewable.
//!
//! # `brix run` cache key (Ring0_Build_Plan §1.9)
//!
//! [`cache::CacheKey`] is `Digest(canonical_source ++ lockfile_digest ++
//! toolchain ++ profile)` through `brix-canon` — the exact determinism basis of
//! Part XXVIII §28.1 and conformance §I "Quality determinism". A warm rebuild is
//! a cache hit (target < 100 ms); the key changes exactly when the binary would.

pub mod cache;
pub mod emit;
pub mod incremental;
pub mod lower;
pub mod phase;
pub mod pipeline;
pub mod plan;
pub mod selfhost;

pub use cache::{CacheInputs, CacheKey, Profile, ToolchainId};
pub use incremental::{IncrementalCompiler, IncrementalProgress, IncrementalUnit};
pub use lower::{lower_file, lower_graph, merge_files, AstFrontend, AstLower, DepPackage, Lowered};
pub use phase::{AstPhase, Phased};
pub use pipeline::{PipelineError, Stage};
