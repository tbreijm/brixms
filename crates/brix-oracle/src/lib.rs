//! brix-oracle — Naive reference evaluator. The semantic authority; frozen at G1.
//! Ring 0 lane owner: see OWNER.md. Spec: ../../spec/BrixMS_v9_0.md
//!
//! # What this crate is
//!
//! The single-threaded reference implementation of Part III (the semantic
//! kernel): `Settled(P, r) = least fixpoint of the rules of program revision
//! P, evaluated phase by phase over Base(r)`. Design goal is **boring** — no
//! caching, no incremental maintenance, `BTreeMap` everywhere a semantic
//! order is observable, full recompute every revision. Every clever idea
//! belongs in `brix-rt` and gets checked against this.
//!
//! # The IR-facing interface
//!
//! This crate defines its own minimal typed IR — see [`program`] — general
//! enough that a `brix-ir -> Program` lowering is a thin adapter, and small
//! enough to hand-build directly in Rust for tests (see `tests/` and the
//! fixtures `brix-conformance` builds against it). That adapter is
//! [`frontend::program_from_source`] (issue #24, Ring 0 G1): the flagship
//! program parses, lowers, and settles on the oracle through it, replacing
//! hand-built `dsl.rs` construction as the *only* path a real program takes
//! to reach the oracle. [`phase::infer_phases`] runs Appendix F's phase
//! inference directly over `Program` so the oracle proves itself standalone
//! of `brix-phase`'s own lane-neutral input shape.
//!
//! # Module map
//!
//! - [`value`] / [`row`] — the value and row/extent representation
//!   (`Extent = BTreeMap<CanonBytes, Row>`, keyed by each row's own
//!   canonical bytes so iteration order is canonical row order).
//! - [`identity`] — `NodeId`/`EdgeId`/key-bytes computation from a
//!   `RelationDef` + `Row` (Part III §3, Appendix G).
//! - [`program`] — the IR-facing interface: relations, rules-as-patterns,
//!   constraints.
//! - [`frontend`] — the `brix-ir` `FrontendSource -> Program` adapter.
//! - [`phase`] — Appendix F phase inference.
//! - [`eval`] — the settlement fixpoint: positive recursion, stratified
//!   negation, masks, key-conflict withdrawal, error edges, constraints.
//! - [`provenance`] — `Support`/`Claim` and the other sealed kernel edges
//!   (`Masked`, `KeyConflict`, `RuleError`, `Violation`).
//! - [`txn`] / [`store`] — transactions (snapshot-isolated commit against a
//!   `Store`) and the top-level revision-by-revision engine.

pub mod dsl;
pub mod dump;
pub mod eval;
pub mod frontend;
pub mod identity;
pub mod phase;
pub mod program;
pub mod provenance;
pub mod row;
pub mod store;
pub mod txn;
pub mod value;
