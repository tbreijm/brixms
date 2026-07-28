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
//! Landed so far (ADR-0001 stage 1, extended by ADR-0002 §6):
//! - [`Outcome`] / [`Authority`] — the single epistemic outcome lattice
//!   (ADR-0001 §4, extended to six members by ADR-0002 §4, ⟨D-AUD⟩'s
//!   `Audited`) with the one-authority-per-route table (§4.1) and the
//!   explicit `lattice_le` partial order.
//! - [`ContextId`] — the content-addressed assumption-context identity (§5.1),
//!   including the **root migration anchor** whose digest equals today's
//!   `reflect::ScopeId::root()` so `brix.type`'s `FactId`s survive the move to
//!   real scoped contexts.
//! - [`PropositionId`] (§5.2), [`Evidence`]/[`EvidenceId`] with the durability
//!   axis + [`VerifierId`] (§5.3), [`Dependency`]/[`EdgeKind`] with typed edge
//!   kinds incl. the elaboration boundary (§5.5), and [`Judgement`]/
//!   [`JudgementId`], the search-invariant capstone (§5.4).
//! - **SOC artifacts (ADR-0002 §6).** [`ConfigId`] — a configuration's
//!   identity, reusing the plain canonical value digest verbatim (no
//!   parallel scheme). [`RegimeId`] — *which* `ρ_w` interpretation a witness
//!   carries. [`Witness`]/[`WitnessId`] — the typed correspondence
//!   `w: src → dst` under a regime. [`GeneratorId`]/[`GeneratorRegistry`] —
//!   the specified, content-addressed class `𝒢` of primitive logged
//!   settlement witnesses (membership is data, not convention).
//!   [`Decomposition`]/[`DecompositionId`] — a finite `𝒢`-factorization with
//!   its intermediate configurations, where [`DecompVerification`]
//!   (`Recorded` vs `ReplayVerified`) is part of the canonical encoding so
//!   the hot loop's unverified record and the audit-factorization checker's
//!   verified replay are *different* evidence with *different* ids — never
//!   confusable, never silently upgraded. [`Realizes`] — the
//!   `Realizes(w, x, y)` proposition kind that makes a realization judgment
//!   a first-class statement.
//!
//! Next slices add `DiscoveryRun`, the observational cost records (§5.7), the
//! `Proven`-provenance validator (the elaboration-boundary rule), and the
//! retraction-closure fixtures (§7).

mod config;
mod context;
mod decomposition;
mod dependency;
mod evidence;
mod generator;
mod id;
mod judgement;
mod outcome;
mod proposition;
mod realizes;
mod regime;
mod witness;

pub use config::ConfigId;
pub use context::ContextId;
pub use decomposition::{DecompVerification, Decomposition, DecompositionError, DecompositionId};
pub use dependency::{Dependency, DependencyId, EdgeKind};
pub use evidence::{CertificateId, Durability, Evidence, EvidenceId, VerifierId};
pub use generator::{GeneratorId, GeneratorRegistry, GeneratorRegistryId};
pub use judgement::{Judgement, JudgementId};
pub use outcome::{Authority, Outcome};
pub use proposition::PropositionId;
pub use realizes::Realizes;
pub use regime::RegimeId;
pub use witness::{compose, compose_chain, Witness, WitnessId, WITNESS_COMPOSE_TAG};
