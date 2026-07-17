//! brix-rt — Runtime: revisions, deltas, provenance, lifecycles, sim clock,
//! WASM host, caps. Ring 0 lane owner: see `OWNER.md`. Spec:
//! `../../spec/BrixMS_v9_0.md`, and `../../spec/Ring0_Build_Plan.md` §1.7
//! for this lane's detailed design brief.
//!
//! # Status: Day-1 bounded slice
//!
//! This crate's full `OWNER.md` contract is large (GraphCore, RelationStore,
//! revision log, MVCC, settle scheduler, support counting, provenance store,
//! KeyConflict service, transaction pipeline, protocol lifecycle engine, sim
//! clock, wasmtime Driver host). What ships in this pass — deliberately
//! bounded, per the brief that opened this work:
//!
//! - [`delta`] — the delta ABI: **the** contract generated code, tier-B
//!   WASM, and the Driver SDK all compile against. Designed first, because
//!   everything else downstream compiles against it.
//! - [`graph`] — `GraphCore`: node interner + arenas, edge identity
//!   resolution, and the global incidence index.
//! - [`store`] — `RelationStore`: the generic view every generated store
//!   implements, plus a reference in-memory implementation.
//! - [`revlog`] — the revision log format (canon-encoded, append-only) and
//!   an in-memory implementation; mmap is an explicit follow-up.
//! - [`mvcc`] — snapshot identity and open-snapshot/retention bookkeeping.
//! - [`ids`] — the public reference-type surface (`NodeRef`/`EdgeRef`/
//!   `ClaimRef`/`DataRevision`/`ProgramRevision`) plus provisional
//!   engine-internal identities (`RelationRef`, `RoleRef`, `RuleRef`,
//!   `SiteId`, `MatchDigest`, `SupportRef`) that `brix-ir` will eventually
//!   own more precisely.
//! - [`value`] — `RoleValue`/`EdgeRoleTuple`, the dynamically-typed row
//!   shape reflection/tooling/the ABI boundary use in place of generated
//!   typed columns.
//!
//! **Deliberately not in this slice** (tracked, not forgotten): the settle
//! scheduler, support-count aggregation *engine* (the `SupportOp` vocabulary
//! it consumes is here; the engine that folds those ops into live/dead
//! decisions is not), provenance store + compaction, `KeyConflict` service,
//! full transaction pipeline (intent identity + conflict validation —
//! `TransactionId` and `LogOp` exist; the pipeline does not), protocol
//! lifecycle engine (Appendix H's state machine — the Driver SDK sketch in
//! `sdk/brix-driver-rs` shows the *shape* outcomes take; nothing drives the
//! state machine yet), sim clock + event calendar, and the wasmtime Driver
//! host (`wasmtime` is dependency-whitelisted for this lane in `DEPS.md` but
//! not wired into `Cargo.toml` here — see that crate's notes).
//!
//! # Discipline
//!
//! No `HashMap`/`HashSet` in semantic paths (clippy-denied workspace-wide,
//! `clippy.toml`); this crate uses `BTreeMap`/`BTreeSet` throughout. No
//! `unsafe` (none is used; the arena in [`graph`] is a plain `Vec`-indexed
//! arena and did not need it — see that module). No floats. Every semantic
//! type here serializes exclusively through `brix-canon`'s `Canonical`
//! trait — see `CONTRIBUTING.md`.

pub mod delta;
pub mod graph;
pub mod ids;
pub mod mvcc;
pub mod revlog;
pub mod scheduler;
pub mod store;
pub mod stream;
pub mod value;
