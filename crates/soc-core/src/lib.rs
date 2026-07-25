//! `soc-core` — the SOC settlement-side engine core (ADR-0002 — SOC
//! Constitution — `spec/adr/ADR-0002_SOC_Constitution.md`).
//!
//! Build order (ADR-0002 §9.3; `spec/Build_Plan_v3_SOC.md` Steps 2–3):
//!
//! - **E1** ([`intern`], [`store`], [`history`]): canonical-digest → dense
//!   `u32` interner; a persistent key→value store behind a swappable trait
//!   ([`store::PersistentMap`]; the HAMT-shaped ADR target, `BTreeMap`/`Arc`
//!   v1 — see that module's docs for the dependency-policy rationale); the
//!   chained history digest `h' = H(h_digest, step)`, O(1)/step.
//! - **S2⋈E2** ([`exec`], [`regime`], [`adm`], [`oracle`]): the execution
//!   config `e = ⟨x, p, h⟩`; the realization interface
//!   ([`regime::Regime::candidates`], the deliberately-naive enumerate form —
//!   see that module's docs); the admissibility judgment ([`adm::Adm`]); the
//!   naive reference oracle ([`oracle::cand`]/[`oracle::succ`]) —
//!   `brix-oracle`'s role reborn at the SOC layer (ADR-0002 §3, §9.2).
//!
//! This crate depends on `brix-canon` (canonical digests/encoding) and
//! `brix-semantic` (the `ContextId` world identity, [`exec::intern_context`])
//! **only** — no heavier dependency, per ADR-0002 §3 substrate discipline and
//! the workspace Ring-0 whitelist (root `Cargo.toml`
//! `[workspace.dependencies]`).
//!
//! **What this crate deliberately does not do yet:** the calendar/commit
//! step (keyed determinization, `Derived`/`Audited` publication —
//! `Build_Plan_v3_SOC.md` Step 4) and the incremental delta-driven regime
//! form (`footprint`/`apply(delta)`, ADR-0002 §9.2, `Build_Plan_v3_SOC.md`
//! Step 6). Both are later engine steps; this crate's oracle is
//! intentionally the slow, obviously-correct baseline they get
//! differential-tested against.
//!
//! **Gate.** The executable governance-conservation law — tightening `Adm`
//! shrinks `cand(e)`/`Succ(e)` pointwise over every reachable `e`
//! (ADR-0002 §5 point 5, §5.5) — lives in
//! `tests/governance_conservation.rs`.

pub mod adm;
pub mod exec;
pub mod history;
pub mod intern;
pub mod oracle;
pub mod regime;
pub mod store;

pub use adm::{Adm, AdmAll, AdmNone, AdmRegimeAllowlist, AdmSuccessorFilter, AndAdm};
pub use exec::{intern_context, ExecConfig};
pub use history::History;
pub use intern::{Handle, Interner};
pub use oracle::{cand, succ};
pub use regime::{Candidate, Regime};
pub use store::{ArcMap, PersistentMap};
