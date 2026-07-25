# ADR-0001 — The Shared Proof Substrate and the Two-Kernel Constitution

Status: **Proposed** (constitution freeze — governs #52, #53, #55, #56, #57,
#58, #59, #63 and supersedes the single-kernel language in
`spec/Ring0_Build_Plan.md`).

Date: 2026-07-24.

---

## 1. Thesis

BrixMS is a **universal formalism for modelling structures, relationships, and
actions in a hypergraph** — from the trivial to the wildly difficult — under one
canonical identity discipline. Making that claim precise requires a
**Curry-Howard correspondence type system**: propositions are types, proofs are
explicit terms, and validity is decided by checking a term against a type in a
context.

But a proof calculus alone is not universal here. "Structures and
relationships" are static and want **propositions-as-types**; "actions" are
operational, stateful, and temporal (worlds, revisions, time, effects) and want
a **settlement semantics**. You cannot collapse world-closure into a proof term,
and you cannot collapse proof-checking into settlement. The universality lives in
the **marriage of the two**, joined by one shared artifact substrate.

The distinctive contribution — what makes this *a different perspective on
reality* rather than another proof assistant — is that BrixMS treats
**epistemic status, cost, and provenance as one fabric**. Standard provers are
binary (proved / not-proved) and cost-blind. BrixMS makes `Derived`,
`Proven`, `Refuted`, `Measured`, and `Unknown` first-class distinct outcomes,
each carrying **mandatory cost** and **content-addressed provenance**, with
`Unknown` never collapsing to `false`. That triad — proof-carrying *and*
evidence-grading *and* cost-accounting — is the substrate's reason to exist.

## 2. The contradiction this resolves

`spec/Ring0_Build_Plan.md` (Logic, Ring 0) states: *"proof objects = provenance
subtrees, verified by oracle replay of the subtree (no second checker exists)."*
Issue #63 mandates the opposite target: **two distinct small trusted kernels**
that share canonical artifacts and **must not be fused**. This ADR resolves the
contradiction in favour of the two-kernel model and rewrites the Ring-0 block
accordingly.

A settlement rule-match proves that BrixMS settlement derived an edge at a
revision. It is **not** a theorem in the dependent proof calculus unless
elaborated to an explicit term and accepted by the proof kernel. The two facts
are related but distinct, and the distinction is enforced *structurally* (§5.5),
not by convention.

---

## 3. Decision D1 — Two kernels, one substrate

| Kernel | Question it answers | Trust | Depends on |
|---|---|---|---|
| **Settlement kernel** (`brix-rt` engine + `brix-oracle` reference) | *Which consequences hold at a settled revision?* — world closure, replayable operational evidence | trusted | canonical artifacts |
| **Dependent proof kernel** (`brix-kernel`, new) | *Does this canonical explicit term prove this proposition in this exact context?* | trusted, **tiny**, **independent** | **only** canonical artifacts |

Both share the canonical semantic artifacts (§5), which live in a new narrow
crate **`brix-semantic`** depending **only on `brix-canon`**. Neither kernel
depends on the other. The proof kernel depends on **neither** `brix-ir`,
`brix-rt`, `brix.type`, nor `brix.proof`.

Everything else — type inference, proof search, compatibility, authorization,
complexity — remains an **ordinary sealed BrixMS package** (a *resolver*, §6)
unless an audit (#63) demonstrates a genuine kernel requirement.

## 4. Decision D2 — The single epistemic outcome lattice

There is **one** outcome vocabulary. Every kernel and every resolver projects
into it; it is defined once, in `brix-semantic`, and frozen here.

```
                 Proven            Refuted
                    \               /
                     \             /
                   Derived (revision-local, settlement-authoritative)
                        |
                    Measured / Estimated
                        |
                     Unknown            ← bottom; never collapses to true/false
```

- **`Proven`** — a proof-kernel-accepted certificate exists. Revision-invariant.
- **`Refuted`** — a proof-kernel-accepted refutation exists. Revision-invariant.
- **`Derived`** — the settlement kernel derived it at a revision. Authoritative
  *within that revision*; not a theorem.
- **`Measured` / `Estimated`** — external certified result, simulation, or
  measurement. Carries its own error/approximation profile.
- **`Unknown`** — includes resource-exhausted and incomplete search.
  **Bottom.** Never `false`, never `true`.

Rules on the lattice:

1. **Resource exhaustion is `Unknown`, never `Refuted`/`Rejected`.** A prover
   that runs out of budget has proved nothing, not the negation.
2. **Fail closed to `Unknown`.** Missing cost, unbounded normalization growth,
   unbounded output growth, or unbounded proof size make admission fail *to
   `Unknown`* — never silently to `Proven`.
3. **One authority per outcome route.** Every route capable of publishing
   `Derived`/`Proven`/`Refuted`/`Unknown` has **exactly one named authority**
   (§4.1). This is a typecheck of the substrate, not a review gate.

### 4.1 Verifier-authority table (frozen)

| Outcome | Sole authority | Nobody else may publish it |
|---|---|---|
| `Proven` / `Refuted` | `brix-kernel` acceptance judgement | resolvers may *construct candidates*, never assert acceptance |
| `Derived` | settlement kernel (`brix-rt`; `brix-oracle` reference) | — |
| `Measured` / `Estimated` | the named external driver / simulator, via a certified-result envelope | — |
| `Unknown` | any resolver may *emit* `Unknown(reason)`; no one may *downgrade* a stronger outcome to hide a failure | — |

## 5. Decision D3 — Canonical artifact identities

All content-addressed, all in `brix-semantic`, all with versioned encoders. No
new variants are added to `Ty`; artifacts are normalized values, not types.

### 5.1 `ContextId`
`world/snapshot × program-revision × assumptions × semantic/checker-profile ×
resource-limits`, content-addressed.

**Compatibility invariant (frozen):** the *root* context —
`root snapshot + program + empty assumptions + default profile + default
limits` — MUST canonically encode to a digest **equal to today's
`ScopeId::root()` digest** (`crates/brix-ir/src/reflect.rs`, PR3.5/#68). This
is the hinge that lets `brix.type` gain real scoped contexts without breaking
its `FactId`-for-`FactId` shadow parity. It requires a golden vector.

### 5.2 `PropositionId`
The canonical domain-specific statement. **Named `Proposition`, not `Claim`** —
`ClaimId` already means a *committed transaction claim* in BrixMS; reusing it
would fuse two unrelated identities.

### 5.3 `EvidenceId` — with a durability axis
Evidence is a sum, but the variants split on a **durability** axis that governs
retraction (§7):

- **Durable** (revision-invariant): `KernelCertificate`, `KernelRefutation`.
  A proof stays a proof.
- **Revision-scoped** (invalidates on retraction): `GroundAssertion`,
  `SettlementReplay`, `Measurement` / `Simulation`, `Suggestion`,
  `CertifiedExternalResult`.

The schema encodes this axis explicitly — it is not discovered later.

### 5.4 `JudgementId`
`(ContextId, PropositionId, epistemic-outcome, EvidenceId)`. **Search-invariant**
— it names *what is true and why*, never *how it was found*.

### 5.5 `Dependency` — one type, typed edge *kinds*
One `Dependency` artifact, with typed edge kinds: `premise`, `assumption`,
`revision`, `rule`, `checker`, and — critically — **`elaboration-boundary`**.

**The boundary rule is structural:** a `SettlementReplay` support can become
*proof* evidence for a `Proven` judgement **only** across an
`elaboration-boundary` edge (i.e. after kernel elaboration + acceptance). There
is no direct edge from a settlement support to a proof judgement. This is how
"a rule match is not a theorem" is enforced by the graph, not by discipline.

### 5.6 `DiscoveryRun` — outside `Judgement`
Strategy, search history, and the cost of *finding* the evidence. Deliberately
**not** part of `Judgement`: a different search strategy may find the same
proof; it must never alter validity. (This is proof irrelevance applied to
provenance: validity is invariant under both search *strategy* and search
*history*.)

### 5.7 Cost — folded, not a parallel judgement
A *proven cost bound* ("this runs in `O(n²)`") **is a `Proposition`** whose
`Evidence` is a BCAM measurement or a kernel certificate — not a separate
top-level artifact. What *is* mandatory is a per-operation **emitted cost
record** attached to `Evidence`: cost may be `UnknownCost(reason)` but must
**never be omitted or defaulted to zero**. This collapses the earlier separate
`CostClaim` into the Proposition/Evidence/Judgement structure and makes the
complexity work (§8, stage 7) "just more propositions."

## 6. Resolvers vs kernels

A **resolver** (`brix.type`, `brix.proof`, `brix.complexity`, authorization,
compatibility …) is an ordinary sealed BrixMS package. It may **search, rank,
derive candidates, and construct certificates**. It may **never** publish
`Proven`/`Refuted` — only `brix-kernel` may. `brix.proof` (#55) is the reference
resolver instance of the shared resolver contract (#52).

## 7. Retraction (frozen from stage 1)

Incremental retraction must never leave a judgement supported by retracted
evidence. Because durability is in the schema (§5.3):

- retracting a revision-scoped support invalidates every dependent judgement;
- a durable `KernelCertificate` survives revision changes (it is closed over its
  own explicit context);
- the retraction-closure over `Dependency` edges is a **stage-1 property**, with
  conformance fixtures, not a stage-8 afterthought.

## 8. Consequences — implementation sequence

The staged sequence, with two adjustments over the original sketch: cost is
folded into propositions (§5.7), and the BCAM gate is split so nothing fails
closed before the kernel exists.

0. **Freeze the constitution** — *this ADR* + the Ring-0 rewrite. Ratify the
   outcome lattice (§4) and the authority table (§4.1) **before** encoders.
1. **`brix-semantic`** — canonical artifacts + validation only
   (`ContextId`/`PropositionId`/`JudgementId`/`EvidenceId`/`CertificateId`,
   versioned encoders, dependency targets, outcome vocabulary, resource/cost
   records). Depends only on `brix-canon`. No parser, search, settlement, IR, or
   proof algorithms. **Gate:** golden vectors, malformed-artifact rejection,
   independent digest reproduction, **root-context digest invariant (§5.1)**,
   retraction-closure (§7).
2. **`brix.type` becomes the first client** — `Fact` → `Proposition`;
   because-sets → labelled evidence steps naming the typing rule + premises;
   content-addressed context extension (`TypeScope(parent, assumption)`);
   activate the reserved `ScopedWorldNonLeak` conformance category
   (`crates/brix-conformance/src/typecorpus.rs`). **Preserve all existing
   `FactId`-for-`FactId` shadow parity** (rests on §5.1). First specimen: a
   `HasType` judgement in the root context, `Outcome = Derived` (not
   `KernelProven`), `Cost = UnknownCost`.
3. **Replayable settlement evidence** — a parallel proof projection alongside
   the compact `Support(edge, rule, match)` ABI (`SettlementStep`/`StepPremise`/
   `StepBinding`/`StepGuard`/`StepAggregateEvidence`/`StepPhase` + snapshot +
   revision). Oracle first, runtime matches. Absence/aggregation need
   phase-completeness evidence, not a match digest.
4. **Honest resource semantics — split.**
   - **4a (now):** cost *records* — a restricted canonical algebra (named
     input-size vars, ℕ constants, `+`/`×`, sparse polynomials, output-size
     substitution; categories time/space/value-bits/output-bits/proof-bytes/
     verifier-work) as a **parallel graded row** (not an `Effect` atom).
     Purely **observational**; nothing fails closed; `UnknownCost` everywhere.
   - **4b (after §8.5):** BCAM as a *gate* — `ReferenceCost` /
     `ImplementationCost` / `BoundClaim` distinct; composition computed as
     `T_{g∘f}(n) = T_f(n) + T_g(S_f(n))`; certificate construction *and*
     verification both charged; unknown normalization/output/proof growth make
     admission fail closed to `Unknown`.
5. **`brix-kernel`** — the dependent proof kernel; depends only on
   `brix-semantic`. First profile is a *declared subset* of #56's final
   calculus: explicit contexts + assumptions, implication, composition, finite
   products/sums, existential witnesses, equality + substitution,
   transformation-preservation. **No** metavariables, tactics, search, implicit
   elaboration, or general recursion. Acceptance distinguishes `Accepted` /
   `Rejected` / `Malformed` / `Unsupported` / `ContextMismatch` /
   `ResourceExhausted` (the last is never logical rejection). **Gate:**
   adversarial certificate vectors; acceptance API usable **without** loading
   `brix.type`, `brix.proof`, the runtime, or the compiler.
6. **`brix.proof`** — the resolver package (#55), only after cross-package
   loading and the relevant generic/ADT lowering are usable (records are still
   non-nominal row aliases, `crates/brixc/src/lower/schema.rs`). First package
   ABI uses **normalized relations + IDs**, not pretend rich generic proof
   types. Search/rank/construct only; the kernel alone publishes acceptance.
7. **Transformations & complexity** — `Transformation`/`PreservationProfile`/
   `Equivalence`/`Abstraction`/`Refinement`/`Reduction` as ordinary
   proof-library concepts ("equivalent" always names *which* dimensions are
   preserved). Then #57 `brix.complexity`. **Gate:** hardness cannot imply
   membership; optimization guarantees cannot cross a reduction without their
   own preservation evidence; failed search yields `Unknown`.
8. **Exploration & causal compression** — P/NP experiments: branching
   contexts/interventions, refutation-learned facts, symmetry certificates,
   quotient regions, abstraction/refinement, merge/pruning certificates.
   Depends on #60–#62. New executable rules require verified
   next-program-revision activation.

## 9. Non-goals

- **Not** enlarging `brix.type` into a universal mathematical library. It is the
  *first client* of the substrate.
- The proof kernel's TCB **excludes** parsing, elaboration, proof search,
  tactics, metavariables, implicit holes, general recursion, provenance ranking,
  and optimization.
- Proof search is **not** required to be complete.
- A resolver verdict is **never** trusted without a kernel-accepted certificate.

## 10. Relationship to existing work

- The **authoritative-gate spike** (#119) already proved the settlement kernel
  can *gate* on a native resolver's verdict via `settle`/`strict_ok` — the
  settlement-side of D1, in miniature.
- The **`brix.type` self-hosting / independence arc** (#15) is precisely the
  move from *second observer of `reflect`* to *independent resolver* — it is
  stage 2's on-ramp, not a separate track. `reflect.rs`/`infer.rs` are the
  *reference to be retired*, not the destination.
- The **mint primitives** (`Value::Bytes` + `brix.ty.mint_*`, #125) are the
  in-engine value-construction a resolver needs to *build* candidate terms
  rather than only observe them.

---

### One-line summary

Two trusted kernels — a **settlement kernel** for actions-in-worlds and a
**dependent proof kernel** for propositions-as-types — over **one canonical
artifact substrate** that carries epistemic status, cost, and provenance as a
single fabric. `brix.type` is its first client, not its library.
