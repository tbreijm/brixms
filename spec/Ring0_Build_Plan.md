# BrixMS Ring 0 — Toolchain Build Plan

**The concrete engineering plan for the core: crates, decisions, order, gates.**
**Date:** 16 July 2026

## 0. Workspace

```text
brixms/
  Cargo.toml            workspace; toolchain pinned via rust-toolchain.toml
  crates/
    brix-canon          canonical encoding + identity (App. G)      [no deps on siblings]
    brix-diag           diagnostic types, BRX codes, JSON/SARIF     [canon]
    brix-ast            lexer, parser, CST, AST, spans              [canon, diag]
    brix-ir             types, effects, traits, Core IR, checking   [ast, canon, diag]
    brix-phase          dependency graph, SCC, phase assignment     [ir, diag]
    brix-oracle         naive reference evaluator                   [ir, phase, canon]
    brix-rt             runtime: revisions, deltas, provenance,
                        lifecycles, sim clock, WASM host, caps      [canon, diag]
    brixc               pipeline + Rust codegen                     [everything above]
    brixpkg             manifest, lockfile, resolve, local registry [canon, diag]
    brix-cli            new/build/run/test/sim/fmt/why/repl         [all]
    brix-conformance    fixture format, CONF runner, differential
                        harness, random program generator           [oracle, rt, brixc]
  sdk/
    driver-wit/         WIT worlds: delta ABI + host capabilities
    brix-driver-rs/     Rust guest SDK for WASM Drivers
  vectors/              frozen canon golden vectors (Day-1 artifact)
  spec/                 v9 carved per Part + errata/
```

**Dependency whitelist (Ring 0):** blake3, logos, petgraph, wasmtime, pubgrub,
proptest, insta, miette, quote+syn+prettyplease, indexmap, camino, serde(+json for
diag only). Nothing else without an Opus-written justification in `DEPS.md`.

**Determinism discipline, enforced mechanically:** `std::collections::HashMap` and
`HashSet` are clippy-denied in brix-canon, brix-oracle, brix-rt, and all generated
code — semantic paths use BTreeMap or sorted IndexMap; iteration order is canon byte
order everywhere it can be observed. No floats in any Ring 0 semantic path except
behind the strict-IEEE ops module. `unsafe` denied workspace-wide except an
allowlisted arena module.

## 1. Build order and crate briefs

### 1.1 brix-canon — Day 1 morning; frozen Day 1 evening (G0)

The serialization point: every hash, identity, log entry, aggregation order, and
cross-runtime vector flows through it, so it freezes first and hardest.

- API: `trait Canonical { fn canon_write(&self, w: &mut CanonWriter); }`,
  `CanonReader`, `Digest = blake3(domain_tag ++ bytes)`, typed wrappers `NodeId`,
  `EdgeId`, `ClaimId`, `SnapshotId`. Version tag `canon/1` in every digest domain.
- Implements App. G exactly: minimal-length ints, normalized decimals, NFC
  identifiers, sorted record fields by field-name bytes, sorted set/map entries,
  base-unit quantities, currency minor units, enum ordinals as ABI, float exclusion
  from key positions with totalOrder tiebreak bytes for aggregation-order use.
- Tests: proptest roundtrip + ordering laws; `insta` golden vectors for every type;
  **Codex's independent Python implementation replays the vectors byte-for-byte
  before freeze**. After freeze, vectors/ is append-only; any change is a spec
  erratum plus a new canon version tag.
- The revision log format is defined as canon-encoded entries — the log needs no
  second serializer, ever.

### 1.2 brix-diag — Day 1, parallel

`Diagnostic { code: BrxCode, severity, site: Span, message, structure: CanonValue }`;
stable `BRX0xxx–BRX8xxx` ranges from the spec; miette rendering for humans, JSON for
agents, SARIF projection. Every later crate reports only through this — agents debug
by diag code, so this exists before the parser does.

### 1.3 brix-ast — Day 1–2

- Lexer with logos; **hand-written recursive-descent parser** (not a generator):
  error recovery and diagnostic quality are the product here, since Ring 1 agents
  live on these messages. Tree-sitter is a separate Codex deliverable for editors,
  never load-bearing.
- CST with full spans → AST. `brix fmt` v0 = canonical AST pretty-printer
  (idempotent; format-then-parse fixture on the whole corpus).
- Fixture corpus: **every ```brix block in the spec, extracted mechanically**, plus
  error-recovery fixtures per diagnostic code. Grammar questions found here become
  errata against Appendix D on Day 2, not Week 4.

### 1.4 brix-ir — Day 2–3 (the semantic fixpoint)

Everything downstream consumes Core IR; codegen must be semantics-free translation.

- Name resolution, type inference (HM-style with rows for relation patterns; keep
  trait solving minimal-coherent: no specialization, no overlapping impls, plain
  associated types), effect-row inference, purity/determinism/nondivergence checks
  for rule bodies, Canonical-in-key checking, pattern → binding/read-set analysis,
  authority-constraint generation (Part XII §5), `partial`/`?` site assignment
  (stable SiteIds).
- IR is a small, closed set of nodes with explicit types on every node; `Display`
  for IR is a debugging deliverable, not an afterthought.
- Scope cuts for v0, recorded in errata as deferred-not-dropped: no user-defined
  operators (already true), regions/borrows minimal (affine check on capabilities
  and ClaimRef only), `optional` lowering via Option joins.

### 1.5 brix-phase — Day 3

Direct App. F transcription over petgraph: positive edges, strict edges (without/
aggregate/witness), mask edges per the three-part rule, SCC condensation, phase
assignment, and — the part worth the extra day of care — **minimal offending path
extraction** for cycle errors, emitted as diag structure. Property test: phase
assignment is invariant under rule declaration order.

### 1.6 brix-oracle — Day 2–3, parallel with 1.4 tail (G1)

The semantic authority. Design goal: *boring*. Single-threaded; extents as
`BTreeMap<CanonBytes, Row>`; each revision recomputes the full fixpoint phase by
phase; supports as sorted sets; masks, key conflicts (per-kind rules), error edges,
constraints, snapshot-isolated transactions, naive protocol lifecycle, sim clock as
plain state. No caching, no cleverness — every clever idea goes in brix-rt and gets
*checked* against this. Exit G1: the flagship parses, checks, and runs end-to-end on
the oracle; `why` answers from oracle provenance.
After G1 the oracle is frozen except through spec errata.

### 1.7 brix-rt + the delta ABI — Day 3–5

**Where the hypergraph lives — explicit, because it's easy to lose:** logically, the
hypergraph is the whole model; physically, pass-1 compiles it away per relation into
monomorphized columnar stores (a hyperedge is a row; roles are columns; incidence is
the per-role indices). The *generic* hypergraph exists in exactly three places: the
oracle (the reference implementation — canon-keyed BTreeMaps, generic roles, naive
incidence, the literal data structure), the `GraphCore` substrate below, and the
`RelationStore` trait view. Anything needing to see the graph *as a graph* —
reflection over `meta.*`, cross-relation `why` traversal, tier-B WASM rules, BGIF
export, path expressions over heterogeneous edges, Studio — goes through those, never
through a second copy of the data.

- `GraphCore`: node interner + arenas (`NodeIdx`), edge identity resolution, and the
  **global incidence index** — `NodeIdx → posting list of (relation, row)`, updated
  by generated emission/retraction code. The one cross-relation structure the runtime
  maintains; it powers "every edge touching this node" for `why`, erasure
  propagation, path evaluation, and visualization. (Lineage note: incidence detection
  was the original Brix primitive — the thesis's collision loop — and this index is
  its production form.)
- `RelationStore` trait: the generic view every generated store implements — iterate
  rows as canon values, look up by role, resolve `EdgeRef`s — a zero-copy trait
  projection over the specialized columns, used by reflection, tooling, the delta
  ABI, and tier B.
- Owns additionally: revision log (canon-encoded, append-only, mmap),
  snapshot/MVCC bookkeeping, the settle scheduler (phase-by-phase, single settler),
  support counting, provenance store with compaction hooks, KeyConflict detection
  service, transaction pipeline with intent identity + conflict validation, protocol
  lifecycle engine (App. H state machine, typed), sim clock + event calendar,
  wasmtime Driver host with capability imports, `BudgetExceeded` logical limits.
- **The delta ABI is the contract to design carefully on Day 3** (Opus task): typed
  batches in, emissions + support ops out; it is what generated code, tier-B WASM,
  and the Driver SDK all compile against. One Rust trait + one WIT world, generated
  from a single definition.

### 1.8 brixc — Day 4–5

Pipeline: ast → ir → phase → plan → emit. Plans v0: heuristic join order
(bound-variables-first, prefer key/index access, cross-product requires the explicit
`cross`), recorded in `meta.Plan`. Codegen via quote + prettyplease into a generated
cargo workspace: one module per relation (store + indices) and per rule (one delta
fn per delta source), `#[deny(...)]` headers matching the determinism discipline,
`brix explain --rust` prints the module. Golden tests: generated code for the
flagship is an insta snapshot — codegen drift is visible in review.

### 1.9 brix-cli + brixpkg — Day 5–6 (the Ring 1 surface)

- `brix new` (package skeleton + OWNER.md template), `build` (pass 1 + cargo),
  `run` (debug profile opt-0; content-hash cache keyed by canonical source + lock +
  toolchain — cache hit must be <100 ms to REPL-feel; Cranelift rustc backend is a
  later optimization, not a Day 6 dependency), `test` (brix.test runner over
  scenarios + doctests), `sim`, `why/whynot`, `fmt`, `repl` (v0: re-run on oracle
  per input — correct first, fast later).
- `brixpkg`: TOML manifest, lockfile with exact digests, pubgrub resolution, local
  registry = content-addressed directory + index file, publish/yank. Signatures and
  OCI are post-G4.

### 1.10 brix-conformance — starts Day 1, never stops

- Fixture format: `(program, txn-stream, expected: canonical settled dump per
  revision + provenance answers)`, IDs mapped to spec conformance categories.
- **Differential harness**: run fixture on oracle and engine, compare canon-encoded
  dumps bit-for-bit; on mismatch, auto-shrink the txn stream and file with the
  divergence revision.
- **Random generator** (Opus design, Day 3): typed schema gen → stratification-
  respecting rule gen (positive recursion allowed, cycles through strict edges
  avoided by construction) → txn stream gen with retracts/supersedes/masks/conflict
  bait. Runs in CI from the first merged delta function; G2 = sustained clean fuzz.

### 1.11 Driver SDK — Day 6

WIT world from the delta-ABI definition + capability host functions; `brix-driver-rs`
guest crate with `on_request` ergonomics, typed outcomes, lease/cancel plumbing;
one example Driver (HTTP notify) shipped as the template Ring 1 copies.

## 2. Deferred from Ring 0 v0 (deliberate, recorded)

Tier-B WASM rule activation (post-G3; REPL uses the oracle meanwhile). brixd
LSP/DAP (week 2–3; diagnostics-as-JSON carries agents until then). `serve` + APIs +
websocket host (post-G4; `run/sim/test` are the blitz products). Lattices,
witnesses beyond a minimal Complete check, info-flow, distribution: edition-gated
as specced. fmt beauty, plan adaptivity, checkpoint compaction: post-G3 polish.

## 3. Orchestration

- **Six parallel Claude Code sessions on git worktrees**, one per lane: (1) canon+
  diag, (2) ast+fmt, (3) ir, (4) oracle, (5) rt+ABI, (6) brixc+cli. Codex runs two
  lanes: Python canon + tree-sitter, and standing adversarial review on canon/
  oracle/ir merges. Opus holds: delta-ABI design, phase algorithm, random-generator
  design, every kernel-semantics review, and the erratum drafting queue.
- Merge queue with CI green-gate; PRs ≤ ~500 generated lines; insta snapshots make
  codegen and vector drift reviewable at a glance.
- **Tony's queue is errata only.** Every ambiguity an agent hits becomes a drafted
  erratum with a proposed ruling and the affected conformance IDs; ruling merges to
  spec/ and unblocks the lane. Expect dozens in week one; that queue's latency — not
  token throughput — is the critical path.

## 4. Gate map

```text
G0  Day 1   canon vectors frozen, Python cross-check green
G1  Day 2–3 flagship end-to-end on the oracle; oracle freezes
G1.5 Day 4  first generated delta fn passes differential vs oracle
G4  Day 7   Developer Day: fresh agent ships a package with public tools only
G2  conv.   sustained clean fuzz, engine = oracle across CONF categories
G3  conv.   RushWeek bit-identical on two machines; backend parity
```

## 5. Ring 0 completeness against v9 (the audit)

**Principle the v0 scope missed:** Ring 0 is defined by *capability*, not by week-one
scope. Ring 1 agents write BrixMS and Drivers; they cannot add grammar or reach into
stores. Therefore Ring 0 permanently owns (a) the entire v9 declaration surface and
its lowerings — even where semantics are library — (b) every engine hook named in
sealed schemas, and (c) all host surfaces. The blitz builds Ring 0's core; the rest
is Ring 0 backlog shipped in waves that lead each Ring 1 wave by one release.

**Ring 0 backlog (post-blitz, wave-ordered):**

```text
Compiler surface   policy lowering (Part XII: lifecycle relations, shadow
                   unconsumability, authority→constraints); language task /
                   prompt / model requirement (protocol gen, context patterns,
                   output-schema derivation); principal / access policy /
                   config / secret / binding; retention / legal hold;
                   export api; language test/benchmark; FOL surface forms
                   (no macros exist — logic syntax is grammar or nothing)
Engine hooks       Frame<S>↔Arrow zero-copy bridge over columnar stores;
                   canon/digest intrinsics exposed to the language (BGIF/BRDP
                   exporters depend on them); scenario adapter framework
                   (script/capture/replay/fixed/succeed) as rt surface;
                   crypto-erasure key scoping in the log format; staged
                   activation + shadow settlement; process-sandbox capability
                   (Python/R Drivers); serving host + API/socket codegen;
                   client engine = brix-rt on a WASM target + BRDP (a second
                   runtime target, Ring 0 owned, feeds Part XXIII)
Toolchain          brix quality verb + brixpkg gate hooks; brixd LSP/DAP;
                   tier-B activation; fmt polish; checkpoint compaction
```

**Confirmed pure Ring 1** (needs only the above): formalism packages, ontology
stacks, brix.learn + model Drivers, quality rule-packs over meta.*, lifecycle
tooling, i18n, Studio queries, interchange exporters, all domain libraries.

**Capability families — placement (math, logic, simulation, data science):**

```text
Math      Ring 0: numeric tower types, Decimal contexts, strict-IEEE ops module,
                  canonical-order aggregate reductions, units as type checking,
                  directed-rounding primitives, seeded RNG streams
          Ring 1: linalg/stats/distributions/root-finding as interop packages
                  with oracle-liable deterministic claims; solvers as Drivers
          HARD RULE: no BLAS/LAPACK in settlement — strict naive kernels only;
                  fast BLAS at boundaries returning Estimate, or in Drivers
Logic     Ring 0: the kernel IS the engine; FOL grammar forms (backlog);
                  proof objects = provenance subtrees, verified by oracle
                  replay of the subtree (no second checker exists)
          Ring 1: BPEF exporter, temporal/bounded model checking over
                  scenarios + history reads, ontology entailment as rules
Sim       Ring 0: clock; event calendar SUPERDENSE-NATIVE in rt — keys are
                  (Instant, microstep, canonical tiebreak); Zeno = logical
                  budget; timers; scenario lowering + deterministic runner;
                  adapters; seed streams; replay/fork; det. Driver scheduler
          Ring 1: DES/DEVS/SD/ABM/hybrid as vocabularies over relations,
                  rules, and timers; fixed-step SD integrators as BrixMS math
DataSci   Ring 0: Frame<S> type + zero-copy Arrow bridge; snapshot binding;
                  missingness enum in intrinsic stdlib; process-sandbox cap
          Ring 1: cleaning recipes, factors/formulas, estimator workflows
                  (sandbox + ONNX Drivers), registry (reuses Part XII
                  activation), experiments, drift rules — and leakage
                  detection as pure bitemporal provenance queries
```

## 6. Instrumentation — pleasantness and performance as diagnostics

Both residual risks become metrics with owners, thresholds, and release gates.
Sources: toolchain event log (JSONL in v0; migrated onto a BrixMS telemetry world
post-G4 — the toolchain measuring itself in its own vocabulary is the first dogfood
app) plus the `diag.*` and `perf.*` relations, which move from backlog into blitz
scope.

**Ergonomics (workable ≠ pleasant), measured from Ring 1 agent sessions:**

```text
retry-to-green      compile attempts per eventually-green change, per diag code;
                    p90 threshold per code — codes above it enter the DIAG-DEBT
                    queue and block the next toolchain release until triaged
fix-locality        did the next edit touch the span the diagnostic pointed at?
                    low locality = misleading message, auto-filed
recurrence          same code firing ≥N times per package = confusing rule or
                    missing suggestion; suggestion text becomes a deliverable
workaround detector nightly Opus pass over the registry: structural patterns
                    routing around a feature (manual Option joins, boilerplate
                    blocks, cross-as-join) occurring ≥N times across ≥M packages
                    → ergonomics finding → candidate sugar or erratum
ceremony budget     Part II §6 claims as executable corpus checks: ceremony
                    declarations per ordinary package; regression gates release
time-to-first-green fresh `brix new` → passing test, tracked continuously —
                    Developer Day as a metric, not an event
```

**Performance (correct-but-slow), diagnosed rather than discovered:**

```text
BRX9xxx             performance diagnostic range: static findings at compile
                    time (unindexed join on large declared extent, implicit
                    cross-product cost, delta amplification above threshold,
                    unrecognized aggregate on hot path) — slow code gets a
                    diag code and a span, like wrong code
perf fixtures in CI flagship + RushWeek carry budget assertions merged like
                    correctness: settle p95/revision, full-scenario wall time,
                    memory ceiling, cache-hit `brix run` startup < 100 ms —
                    a perf regression is a red build, not a week-3 surprise
oracle-speedup      engine/oracle wall-time ratio per CONF fixture; ≥50× on
                    the flagship by G3, tracked from the first generated rule
                    — catches "compiled but effectively interpreted"
misestimate ratio   plan cardinality estimates vs observed, per join; feeds
                    the recompile-with-stats loop and flags heuristic gaps
```

Weekly surprise report = new DIAG-DEBT entries + perf regressions + workaround
patterns, one page, auto-generated. If it's empty, the residual risks are retired;
if it isn't, they're queue items — which was the point.

**Rolling surface gate (replaces one-shot G4 sufficiency):** before each Ring 1 wave
launches, every declaration and hook named in that wave's OWNER.md files must exist
in a released, lockfile-pinnable toolchain. Developer Day proves Wave-1 sufficiency
only; the surface gate proves it per wave, forever.

First commit: `crates/brix-canon/src/lib.rs`. Same answer since v5.0 — now with the
directory path.

