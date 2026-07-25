# Issue Disposition — 2026-07 (post-ADR-0002)

**A planning document. No issue mutations are performed by this file.** It records
how every open GitHub issue is re-classified under
[ADR-0002](./adr/ADR-0002_SOC_Constitution.md) and
[Build_Plan_v3_SOC.md](./Build_Plan_v3_SOC.md), and lists the new issues to open.
Disposition verbs: **keep** (unchanged), **reframe** (retitle/re-scope),
**park** (valid but blocked until a later stage), **close** (subsumed/obsolete).

Issue titles below are as read from `gh issue list` on 2026-07-25.

---

## Part A — Existing open issues

### Core substrate & kernels

| # | Title (current) | Disposition | Rationale / new one-line scope |
|---|---|---|---|
| 148 | Crate: brix-semantic — canonical proof-substrate artifacts (ADR-0001 stage 1) | **keep + extend** | Extend with SOC artifacts: `Witness`, `RegimeId`, generator registry `𝒢`, `Decomposition` evidence, `Realizes(w,x,y)` (ADR §6). Same crate, same "canon-only" dependency rule. Maps to Build Plan v3 **S1**. |
| 149 | Crate: brix-kernel — dependent proof kernel, first profile (ADR-0001 stage 5, profile of #56) | **keep, resequenced** | Role unchanged; **moves later** — after the core settlement loop and O(Δ) gate (v3 **S7/Step 11**). New proof targets added: decomposition validity, governor monotonicity, counter-machine trace correspondence. |
| 63 | Architecture: semantic cohesion and two-kernel re-engineering audit | **keep, absorbed** | The audit that produced ADR-0001 D1; its conclusion is now ADR-0002 §3. Keep open as the standing "does anything actually need a kernel?" audit against the SOC realization. |
| 56 | Kernel: normative explicit dependent calculus and acceptance judgment | **keep** | Still the calculus `brix-kernel` (#149) implements a profile of. Unchanged by SOC. |

### Resolvers → realization regimes

| # | Title (current) | Disposition | Rationale / new one-line scope |
|---|---|---|---|
| 52 | Architecture: normative incremental semantic resolver contract | **reframe (strengthen)** | Becomes **the realization-regime interface**: a regime is a dataflow operator `footprint()`/`apply(delta)→candidate delta` presenting a class of witnesses under one `ρ_w` (ADR §7, §9.2). The "incremental" in the title is now load-bearing (the O(Δ) invariant). |
| 53 | Spec: normative brix.type fact and judgment contract | **reframe** | Becomes **"the first structural realization regime"** — `brix.type` re-enters as a *client regime* at semantic **stage 4** (v3 Step 5), carrying its 14/14 conflict corpus as differential tests against the retained old engine. Not a resolver, not the ontology. |
| 54 | Spec: normative brix.compat semantic compatibility contract | **park (≥ S4)** | A realization regime like #53; parked until the regime interface (#52) and first regimes land. |
| 55 | Spec: normative brix.proof goals, candidates, and certificate contract | **park (≥ S5)** | The reference regime instance of the regime contract; needs `brix-kernel` (S7). Parked until stage ≥5. |
| 57 | Analysis: brix.complexity proof-carrying classification by certified reduction | **park (≥ S5)** | Cost is folded into propositions (ADR §6); complexity is "more propositions." Needs the kernel + cost-gate. Parked until stage ≥5. |

### Semantics / metatheory (now partly discharged by SOC)

| # | Title (current) | Disposition | Rationale / new one-line scope |
|---|---|---|---|
| 58 | Spec: normative BrixMS semantic laws and metatheory | **reframe** | SOC **is** the metatheory. Reframe as "track SOC-derived normative laws + the two obligations (PD-1, PD-2)"; much of the free-form metatheory is now settled. |
| 59 | Semantics: canonical context-indexed judgments | **keep** | Directly realized by `ContextId` + `JudgementId` (ADR §6); the root-digest invariant is its acceptance test. |
| 61 | Semantics: observational equivalence, refinement, and bisimulation | **reframe** | Now concretely = SOC **divergence-sensitive weak bisimulation** + saturation (v3 **S5**, Step 8). Retitle to name the SOC construction. |
| 62 | Semantics: compositional denotation and domain-theoretic fixpoints | **reframe** | Recast as SOC's **final-coalgebra behavior map** + the lax/tight composition split. The denotational goal is met coalgebraically, not by a separate domain theory. |
| 60 | Analysis: normative abstract-interpretation contract | **park (≥ S5)** | An admissibility/governor refinement; sits above the governance-monotonicity law (ADR §5.5). Parked until the governor is real. |

### Old-engine on-ramp

| # | Title (current) | Disposition | Rationale / new one-line scope |
|---|---|---|---|
| 140 | brixc: reflect-free syntactic fact extractor — role-binding fragment (#15 north-star) | **reframe (narrow)** | Value now is **only** as the read/write **footprint feed** for independence batching + parallel deliberation (ADR §9.2, v3 **E7**). Retitle to "footprint/independence extractor." Not on the critical path. |

### Scale family (#64–67) — orthogonal, park

| # | Title (current) | Disposition | Rationale |
|---|---|---|---|
| 64 | Spec: scale, resolution, and multiscale composition | **park** | Multiscale sits above the settlement loop; revisit after S6. SOC's size-discipline (set-sized reachable space) is the hook it will attach to. |
| 65 | Analysis: brix.scale demand-driven resolution planner | **park** | Ditto — a deliberation-side planner; a regime concern, post-core. |
| 66 | Runtime: multirate synchronization frontiers and local solver steps | **park** | Calendar-key structure (phase/time) is the substrate; multirate is a later discipline `D`. |
| 67 | Conformance: scale bridges, error budgets, and adaptive resolution | **park** | Conformance concern layered on `Measured`/error profiles; post-core. |

### Studio (#69–72) — UI over the substrate, park

| # | Title | Disposition | Rationale |
|---|---|---|---|
| 69 | Studio: context-pinned semantic workbench architecture | **park** | Consumes `ContextId`/`JudgementId`; unblocked once S1 artifacts stabilize. |
| 70 | Studio: type and proof resolution workspace | **park** | Needs regimes (S4) + kernel (S7). |
| 71 | Studio: semantic explorer vertical slice | **park** | Needs the core loop first. |
| 72 | Studio: worlds, time, scenarios, and counterfactuals | **park** | Maps to execution configurations `⟨x,p,h⟩` + branching; post-core. |

### Toolchain / Ring-0 surface — demoted to presentation frontend

| # | Title | Disposition | Rationale |
|---|---|---|---|
| 42 | Compiler: package graph loading and cross-package name resolution | **keep** | Still needed for the presentation frontend (S6) and for loading regime packages. Not on the semantic critical path. |
| 43 | CLI: G4 test, simulation, inspection, and interactive workflows | **park** | Ring-1 developer surface; re-scopes onto the presentation frontend (S6). |
| 46 | Ring 0: G4 Developer Day and release-pinning acceptance | **park** | The "toolchain-first" G4 gate is demoted (ADR §2); re-anchor to a presentation-frontend milestone later. |
| 111 | compiler: real trait impl declarations + cross-package impls | **park** | Compiler-frontend feature; only relevant to S6 presentation salvage (⟨D-PARSE⟩). |
| 151 | spec: rule on errata 0003 — pub read/write/derive visibility surface | **keep** | Localized spec/errata ruling; independent of the reorg. Let it land on its own track. |
| 154 | compiler: relation-granular pub capability enforcement | **keep** | Same — a compiler-surface item, unaffected. |
| 79 | Tooling: resumable two-Qwen ticket loop for core package work | **keep** | Orchestration tooling; unaffected by the semantic reorg. |

### Epics

| # | Title | Disposition | Rationale |
|---|---|---|---|
| 19 | Epic: Ring 0 bootstrap gates (G1–G4) | **reframe** | The G-gates are demoted; the governing gates are now v3's per-step gates (O(Δ), audit-factorization, governance-conservation). Retitle/relink to Build Plan v3, or close in favor of a new SOC epic. |
| 20 | Epic: Ring 0 v9 surface completeness and rolling wave gates | **park** | Surface completeness is a presentation-frontend concern (S6+); not the near-term spine. |

### Already closed (recorded for continuity)

| # | Title | State | Note |
|---|---|---|---|
| 15 | Compiler follow-on: reflective type analysis and brix.type | **CLOSED (history)** | Goal reached; its successor is **PD-2 / CJ-1** (universality + faithful self-image). Do not reopen. |

---

## Part B — New issues to open (drafts; not created here)

> Titles + 2–3 sentence bodies. Open in dependency order. Each links to its ADR /
> Build Plan step.

1. **ADR-0002: ratify the SOC Constitution (supersede ADR-0001 §1)**
   Ratify `spec/adr/ADR-0002_SOC_Constitution.md`: SOC ontology as thesis; ADR-0001
   §§4–5–7 carried forward unchanged; the hypergraph demoted to one configuration
   family. Acceptance flips the ADR status to Accepted and unblocks the substrate
   extension. (Build Plan v3 **S0**.)

2. **Freeze the behavior signature (O, F_O) — decision ⟨D-FO⟩**
   Ratify the observation set `O` and behavior functor `F_O` and version-tag them
   alongside the outcome ordinals. Default proposal: `F_O = D_O = 1 + O×X`
   (partial deterministic committed behavior) with `O = O_min` (settlement-event
   tags + committed `JudgementId` digest); deliberation in `B^uk_{K,O}`. Nothing
   that freezes a `soc-core` encoder may land before this closes. (ADR §8, v3 **S0**.)

3. **Crate: `soc-core` — configuration/witness stores, realization interface, Adm, candidate enumeration**
   New crate implementing SOC's execution substrate: interning + persistent
   stores + chained history digest (E1), then the realization interface, `Adm`,
   and `cand(e)`/`Succ(e)` as the naive reference oracle (E2/S2). Gate: the
   governance-conservation law (tightening `Adm` shrinks `cand(e)` pointwise) as
   an executable conformance property. (v3 Steps 2–3.)

4. **Calendar + commit + audit-factorization checker**
   Keyed determinization (`select_K` least-key commit into `F_O`), append-only
   history storing each step's tight `Decomposition`, deterministic replay, and
   the audit-factorization checker that replays a decomposition and verifies exact
   relational composition (SOC "Audit factorization"). This is the operational
   discharge of **PD-1**. (v3 **S3/Step 4**.)

5. **The O(Δ) benchmark gate (THE invariant)**
   A conformance/benchmark harness enforcing: cost per committed step ∝ |Δ| ×
   index fanout, never ∝ |world|; doubling inert configurations must not change
   per-step cost (instrumented via ADR stage-4a cost records). Blocks all further
   engine optimization until green (ADR §9.1). (v3 **E5/Step 6**.)

6. **Saturation layer + settlement interface (quiescence certificates)**
   Administrative-vs-realizing trace tagging, divergence-sensitive saturation, and
   a total one-step settlement interface returning the full `F_O`-structure incl.
   explicit quiescence certificates. Gate: saturation never identifies quiescence
   with administrative divergence. Prerequisite for **CJ-1**. (v3 **S5/Step 8**.)

7. **Proof debt PD-1: tight, generated settlement subcategory**
   Identify the primitive logged settlement witnesses `𝒢`; prove they generate a
   tight subcategory `𝒦`; require every committed witness to carry a certified
   `𝒢`-decomposition. Operational discharge via the audit checker (#4); theorem
   discharge via `brix-kernel` decomposition-validity certificate. (ADR §10, v3
   Steps 4 & 11.)

8. **Proof debt PD-2: universality via two-counter-machine encoding**
   Complete the two-counter-machine encoding with halting + control policy; prove
   trace correspondence; conclude Turing completeness; identify the observed
   behavior preserved so the governance (Rice-style) reduction is extensional.
   Successor to the closed #15 self-hosting arc. (ADR §10, v3 **S8/Step 12**.)

9. **Conjecture CJ-1: faithful F_O-self-image (universal world)**
   Construct the finitely presented world `𝒰_{F_O}` and prove the saturated typed
   adequacy theorem for presentations with a settlement interface. Depends on the
   settlement interface (#6). Tracked as a conjecture — must not block the core
   loop. (ADR §10, v3 **S8/Step 12**.)

> Optional: a single **Epic: SOC realization (Build Plan v3)** umbrella linking
> #1–#9 above plus the reframed #52/#53/#61/#62/#148/#149 — to replace the demoted
> Ring-0 epics (#19/#20) as the tracking spine.
