//! `brix-semantic` — the canonical proof-substrate artifacts (ADR-0001).
//!
//! The **one substrate** shared by BrixMS's two trusted kernels — the
//! settlement kernel (`brix-rt`/`brix-oracle`) and the dependent proof kernel
//! (`brix-kernel`) — and by every resolver (`brix.type`, `brix.proof`,
//! `brix.complexity`, …). It holds *only* canonical artifacts and their
//! validation: no parser, no search, no settlement, no compiler IR, no
//! proof-checking algorithm. It depends on **`brix-canon` only** (ADR §3), so
//! the proof kernel can be built on it without pulling in the resolver stack.
//!
//! Landed so far (ADR stage 1, first slice):
//! - [`Outcome`] / [`Authority`] — the single epistemic outcome lattice (§4)
//!   with the one-authority-per-route table (§4.1).
//! - [`ContextId`] — the content-addressed assumption-context identity (§5.1),
//!   including the **root migration anchor** whose digest equals today's
//!   `reflect::ScopeId::root()` so `brix.type`'s `FactId`s survive the move to
//!   real scoped contexts.
//!
//! Next slices add `PropositionId`, `EvidenceId` (with the durability axis),
//! `JudgementId`, `Dependency` (typed edge kinds), `DiscoveryRun`, and the
//! observational cost records (§5).

mod context;
mod outcome;

pub use context::ContextId;
pub use outcome::{Authority, Outcome};
