# BrixMS Build Plan v2 — Toolchain First, Agents as Developers

**Supersedes v1. Gates, not dates. Blitz, then swarm.**
**Date:** 16 July 2026

## 0. The reframe

Build the core language tools first; then every agent is not "an AI implementing a
spec" but **an ordinary BrixMS developer building one package of the ecosystem with
those tools**. Two consequences, both structural:

1. **The language governs its builders.** Agent guardrails stop being CLAUDE.md
   prose and become mechanics: capabilities deny ambient access, effect rows expose
   what code does, layer-admission and quality gates reject violations, `brix test`
   and conformance decide correctness. An ecosystem agent *cannot* introduce a second
   truth or private semantics, because the toolchain it develops with refuses them —
   the same containment the spec promises production is the containment the build
   runs under from day 8.
2. **The build is the certification.** Appendix K's completion statement — a wider
   platform team builds optimized tooling *without private semantics*, independent
   authors compose *through published formats and conformance tests* — is executed,
   not asserted: the ecosystem swarm working through public tools alone IS that test
   passing continuously.

## 1. Two rings

**Ring 0 — Toolchain (Rust; small, trusted, Opus-led).** The only code that may
touch engine internals: `brix-canon`, parser/AST, Core IR + typechecker,
`brix-oracle`, `brix-rt`, `brixc` (pass-1 codegen), the CLI verbs (`new build run
repl test sim fmt why whynot explain`), `brixpkg` + local registry, the Driver SDK
(WASM host + `extern rust`), `diag.*` output, the conformance runner, and the
intrinsic stdlib slice that needs engine hooks (`brix.core`, `brix.rel` incremental
aggregates, `brix.time`, `brix.math` numerics).

**Ring 1 — Ecosystem (any number of agents; BrixMS + Driver SDK only, never engine
Rust).** Everything else in v9 is, by the spec's own layer rules, ordinary packages:
`brix.logic`, `brix.data`, `brix.io` Drivers, `brix.arrow` / `brix.delta` /
`brix.postgres`, formalism packages (des/devs/sd/abm/spatial/hybrid/petri/fsm),
`brix.ontology.*`, `brix.learn`, `brix.llm` + gateway Driver, `brix.websocket` /
`brix.http`, `brix.lifecycle`, **the quality engine's rule-packs (BrixMS rules over
`meta.*`)**, `brix.meta` published queries, `brix.i18n`, Studio's query layer, and
the conformance fixtures beyond the kernel (which are BrixMS scenarios).

The interface between rings is exactly the public one: released toolchain versions
(pinned via lockfile), the registry, published formats, and a bug protocol. Ring 1
never blocks on Ring 0 internals; Ring 0 never sees Ring 1 except as its harshest
users.

## 2. The bootstrap blitz (Ring 0, week one — emission is days; gates decide)

- **Day 1 — canon + harness.** `brix-canon` implemented (Sonnet) against Opus's
  design; Codex ships the independent Python canon; golden vectors cross-checked
  byte-for-byte and **frozen by end of day** (G0). Conformance runner + CI green-gate
  live before the second merge. Grammar + parser start in parallel; every ```brix
  block in the spec becomes a fixture.
- **Days 2–3 — oracle.** Core IR, typechecker (App. E), naive settlement with
  phases, masks, key conflicts, error edges, transactions. **Flagship runs
  interpreted** (G1). The oracle is then frozen except via spec errata.
- **Days 3–5 — engine + codegen.** `brix-rt` and `brixc`; the random-program
  differential fuzzer goes live with the first merged delta function and never turns
  off. Protocol lifecycle, WASM Driver host, capabilities.
- **Days 5–6 — the developer surface.** This is the v2 priority shift: `brix new`,
  `build`, `run` (Cranelift + cache), `test`, `fmt` (canonical), `sim`, `why`,
  `brixpkg` with a local registry, the Driver SDK, and `diag.*` with stable codes.
  Rough is fine; *existing and honest* is the bar — Ring 1 runs on these.
- **Day 7 — Developer Day (G4, the flip).** One fresh agent, given only the public
  toolchain, the spec Part for its package, and `brix new` — builds, tests, and
  publishes a real package end-to-end. Every friction it hits is a Ring 0 bug filed
  through the protocol below. When Developer Day passes, the swarm launches.
- **Weeks 2–3, in parallel with the swarm:** the fuzzer burns down engine–oracle
  divergences (G2: CONF-green under sustained fuzz), backend parity, two-machine
  reproducibility (G3: RushWeek deterministic on two machines). These are the serial
  convergence loops parallelism doesn't collapse — canon-downstream invalidation,
  rustc cycle time, and Tony-serial errata rulings — and they run while Ring 1
  produces.

## 3. The swarm (Ring 1, from Day 7)

One agent = one package = one owner file (`OWNER.md`: the spec sections that are its
requirements, its port/format contracts, its conformance IDs). The agent's loop is a
developer's loop: `brix new` → write BrixMS + tests → `brix test` → `brix quality` →
`brixpkg publish --registry local` → CI conformance. PRs are package releases, not
Rust diffs.

Wave order follows the demo slice, not the spec's table of contents:

```text
Wave 1 (demo-critical)   brix.arrow, brix.delta/lakebase Drivers, brix.learn,
                         brix.llm + gateway Driver, brix.data, brix.io.http
Wave 2 (proof-of-breadth) brix.logic, brix.ontology core, formalism.des + devs,
                         brix.websocket/http surfaces, quality rule-packs
Wave 3 (platform)        sd/abm/spatial/hybrid, shacl/owlrl, lifecycle, i18n,
                         Studio queries, remaining conformance scenario suites
```

Model assignment inside Ring 1: Sonnet agents are the developer majority; Codex
agents take TypeScript client generation, Tree-sitter, selected Drivers — and serve
as **cross-family reviewers** on every Wave-1 package, because Sonnet-writes-tests-
Sonnet-passes-tests can go green on a shared misreading. Different families, the
registry as the meeting point, conformance as the referee.

## 4. Feedback protocol (the only coupling between rings)

Every Ring-1 failure is triaged into exactly one of three bins:

1. **Package bug** → the owning agent fixes it; nobody else notices.
2. **Toolchain bug** → a minimal repro *as a `brix.test` case* attached to its
   `diag` code, filed to the Ring 0 queue; Opus triages; fixes ride the toolchain
   release train (frequent, versioned, lockfile-pinned — Ring 1 upgrades
   deliberately, never ambiently).
3. **Spec ambiguity** → drafted as an erratum by Opus, ruled by Tony, merged into
   `spec/` — the document stays the single truth, and every ruling makes the next
   agent's context better.

Two permanent background loops: the differential fuzzer, and the nightly spec-drift
agent (Opus) diffing merged behavior against the spec. Akhil's lane: `brix-rt`
performance in Ring 0 and the Part XII policy/RL track in Ring 1 — the human who
owns the machine, in both rings.

## 5. Gates (not dates)

```text
G0  canon golden vectors frozen, cross-family verified          (Day 1 target)
G1  flagship runs on the oracle                                 (Day 2–3 target)
G2  engine = oracle, CONF-green under sustained fuzz            (convergence-bound)
G3  RushWeek deterministic on two machines, backend parity      (convergence-bound)
G4  Developer Day: fresh agent ships a package via public tools (Day 7 target — flips the swarm)
G5  demo slice: TransConnect world on BrixMS + Delta/Lakebase,
    one policy (ONNX Driver), one grounded language task,
    brix why live against Databricks                            (gated, not dated)
```

Week one is the blitz through G0/G1/G4 with G2/G3 convergence started; the honest
uncertainty lives entirely in G2/G3's divergence tail and the erratum queue — which
is to say, in how well the spec survives contact. Everything after G4 scales with
agent count because agents are just developers, and the toolchain — not the plan,
not the prompts — is what keeps them honest.
