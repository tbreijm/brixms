# BrixMS Build Plan v3 — SOC Constitution

**The master plan under [ADR-0002](./adr/ADR-0002_SOC_Constitution.md). Gates, not dates. One dependency-ordered sequence.**

This plan interleaves SOC's **semantic sequence** (stages 0–8) with the
**engineering order** (E1–E8, ADR-0002 §9.3) into a single dependency-ordered
list of steps. Each step states **what it produces**, its **gate**, and **what
it unblocks**. No dates — structure and sequence only.

Two rails run in parallel and meet at gated joins:

- **Semantic rail (S):** ontology → configurations/witnesses → calendar/commit →
  regimes → saturation → presentation → proof kernel → universality.
- **Engineering rail (E):** interning/store/digest → naive oracle → delta/regime
  trait → indexed candidates/calendar → **O(Δ) gate** → compact support → parallel
  → compilation.

The rails are **not** independent: the naive oracle (E2) *is* SOC's reference
settlement discipline; the calendar heap (E4) *is* SOC's `select_K`. The joins
are marked ⋈.

---

## Legend

- **S**_n_ = semantic stage _n_ (SOC). **E**_n_ = engineering step _n_ (ADR §9.3).
- **Gate** = the executable acceptance property that must be green to proceed.
- ⋈ = a join where a semantic step and an engineering step land together.

---

## Step 0 — S0 · Constitution freeze

- **Produces:** [ADR-0002](./adr/ADR-0002_SOC_Constitution.md) ratified;
  ADR-0001 §§4–5–7 carried forward unchanged; the two SOC obligations (PD-1,
  PD-2) and conjectures (CJ-1, CJ-2) registered as tracked debt; the behavior
  signature decision point ⟨D-FO⟩ resolved (§8) — default
  $F_O=D_O=1+O\times X$, $O=O_{\min}$.
- **Gate:** ADR-0002 status → Accepted; $(O,F_O)$ ratified and version-tagged;
  outcome ordinals and authority table re-confirmed unchanged.
- **Unblocks:** every step below. **Nothing that freezes an encoder may land
  before ⟨D-FO⟩ is resolved.**

## Step 1 — S1 · Extend `brix-semantic` (the substrate)

- **Produces:** the SOC artifacts added to the existing crate (ADR §6):
  `Witness`, `RegimeId`, generator registry `𝒢`, `Decomposition` evidence,
  `Realizes(w,x,y)` proposition kind — each content-addressed with versioned
  encoders. Preserves all ADR-0001 §5 identities.
- **Gate:** golden vectors for every new artifact; malformed-artifact rejection;
  independent digest reproduction; **the `ContextId` root-digest invariant golden
  vector (ADR §6.1) green** (`ScopeId::root()` parity); retraction-closure
  fixtures still green with the new evidence kinds; durability axis places
  `Decomposition` correctly (durable when closed over context).
- **Unblocks:** `soc-core` (S2), the audit-factorization checker (S3), the
  structural regime (S4). Depends only on `brix-canon`.

## Step 2 — E1 ⋈ · Interning + persistent store + chained history digest

- **Produces:** in `soc-core`: canonical-digest → dense `u32` interner; HAMT-style
  persistent configuration/witness stores with structural sharing; the history
  digest chain $h'=H(h_{\text{digest}}, \text{step})$, O(1)/step.
- **Gate:** interning round-trips (handle→digest→handle) under proptest; store
  persistence/sharing property (old snapshot unchanged after update); history
  digest reproducible and O(1) (no rescan of history on append).
- **Unblocks:** the oracle loop (Step 3) and every later engine step. This is the
  first `soc-core` commit.

## Step 3 — S2 ⋈ E2 · `soc-core` skeleton: configuration/witness stores + realization interface + `Adm` + candidate enumeration, as the naive oracle

- **Produces:** the realization interface ($\rho_w$ enumeration per regime), the
  admissibility judgment `Adm`, raw candidate enumeration `cand(e)` and observed
  successors `Succ(e)` — implemented as the **naive recompute-the-world reference
  oracle** (single-threaded, correct-not-fast; SOC settlement discipline made
  concrete; `brix-oracle`'s role reborn).
- **Gate — governance conservation law (executable conformance property):**
  tightening `Adm` shrinks `cand(e)` **pointwise** — $\mathsf{Adm}'\Rightarrow
  \mathsf{Adm}$ implies $\cand'(e)\subseteq\cand(e)$ and
  $\mathsf{Succ}'(e)\subseteq\mathsf{Succ}(e)$ for every reachable `e`
  (SOC "Governance monotonicity at candidate level").
- **Unblocks:** calendar/commit (Step 4); the differential-test baseline for the
  fast engine (Step 6); the first regime (Step 5).

## Step 4 — S3 ⋈ E4 · Calendar + commit: keyed determinization, append-only history, deterministic replay

- **Produces:** the keyed calendar as a priority queue on
  $K=(\text{phase/time}, \text{priority}, \text{digest tie-break})$;
  $\mathsf{select}_K$ committing the least-key candidate into $F_O=D_O$; the
  committed coalgebra $\gamma=\mathsf{select}_K\circ\delta$; append-only history
  storing each committed step's tight `Decomposition`; deterministic replay from
  history.
- **Gate — audit-factorization checker:** replays each committed step's
  `Decomposition`, verifies the **exact** relational composition
  $\rho_k=\rho_{g_n}\circ\cdots\circ\rho_{g_1}$ and the intermediate-configuration
  chain (SOC "Audit factorization"), and publishes the upgraded **`Audited`**
  judgement (ADR §4, ⟨D-AUD⟩ — sole authority row); replay is byte-identical
  (deterministic-replay property); `select_K` picks a unique least key
  (tie-break totality). A replay that does not complete exactly yields
  `Unknown(reason)`, never a pass.
- **Unblocks:** `Derived` and `Audited` outcome publication (ADR §§4–5.1) — and
  thereby the typed `elaboration-boundary` precondition; PD-1's *operational*
  discharge; the first real regime (Step 5). ⟨partial discharge of PD-1⟩

## Step 5 — S4 · First regimes: literal equality, then the structural (`brix.type`) regime

- **Produces:** (a) the **literal-equality** regime (simplest $\rho_w$, exercises
  the whole loop); then (b) the **structural regime** — `brix.type` semantics
  return as a *client regime*: `Fact`→`Proposition`, because-sets→labelled
  evidence steps, content-addressed context extension entering through the
  root-digest anchor (ADR §6.1). `HasType` is one `Realizes` judgment,
  `Outcome=Derived`.
- **Gate:** the structural regime's **14/14 conflict corpus** (the reflect
  `ConflictKind` parity) runs as **differential tests against the retained old
  engine** (`reflect.rs`/`infer.rs` as oracle); `FactId`-for-`FactId` shadow
  parity preserved; `ScopedWorldNonLeak` conformance category green.
- **Unblocks:** retirement path for the old engine internals (kept only as this
  differential oracle thereafter); demonstrates heterogeneous regimes coexisting
  (SOC "one world, several disciplines").

## Step 6 — E3 · Delta protocol + regime trait (incremental) ⋈ E5 · **The O(Δ) benchmark gate**

- **Produces:** the regime trait as a **dataflow operator** —
  `footprint()` / `apply(delta) → candidate delta`; candidates become a
  materialized incremental (semi-naive, delta-driven) view rather than a re-run
  query; the calendar consumes candidate deltas.
- **Gate — THE invariant (ADR §9.1), executable benchmark, blocks all further
  optimization:** cost per committed step $\propto |\Delta|\times$ index fanout,
  **never** $\propto |\text{world}|$. **Doubling inert configurations must not
  change per-step cost** (measured via ADR stage-4a cost records). The fast
  incremental engine is **differentially identical** to the naive oracle (Step 3)
  across the conformance corpus.
- **Unblocks:** everything after — per ADR §9.3, *no further optimization lands
  before this gate is green.* This is the anti-v1 checkpoint.

## Step 7 — E6 · Compact support + lazy audit expansion

- **Produces:** hot-loop `Support(edge, rule, match)` compact records; full tight
  `SettlementStep`/`Decomposition` expanded **lazily off the hot path** (lax
  direction = compiler license; tight direction = auditors).
- **Gate:** lazily-expanded decompositions still pass the audit-factorization
  checker (Step 4) byte-for-byte; hot-loop allocation/step does not grow with
  world size (re-run the O(Δ) gate); support→decomposition expansion is total on
  committed steps.
- **Unblocks:** parallelism (Step 8) without paying tight-decomposition cost in
  the inner loop.

## Step 8 — S5 · Saturation layer + settlement interface (quiescence certificates)

- **Pinned by:** [ADR-0014](./adr/ADR-0014_Divergence_Sensitive_Saturation.md)
  (Accepted) — tracked by #61, staged A–D, all four landed. SOC-LAW-10 moved
  `Open → Partial`; it stays Partial pending #59 (observation-profile taxonomy)
  and #178 (revision identity), per ADR-0014 §10.

- **Produces:** administrative-vs-realizing trace tagging
  ($m\xrightarrow{\tau}m'$ vs $m\xrightarrow{o}m'$), divergence-sensitive
  saturation $\sat\gamma$ (weak transitions + administrative-divergence
  observation), and the **settlement interface**: a total effective one-step
  procedure returning the full encoded $F_O$-structure incl. **explicit
  quiescence certificates** (SOC "Decidable settlement or certified quiescence").
- **Gate:** saturation hides finite $\tau$-prefixes but **never** identifies
  quiescence with $\uparrow_\tau$ (a divergence-sensitivity conformance test — a
  terminal state and an infinitely-searching state are distinguished); quiescence
  certificates are checkable; the invariant rule (one-step closure) holds for a
  sample safety predicate.
- **Unblocks:** CJ-1 (faithful self-image needs the settlement interface); the
  `Unknown`-never-collapses guarantee becomes operationally grounded (ADR §5.3).

## Step 9 — E7 · Footprint batching + parallel deliberation

- **Produces:** sharded deliberation; parallel min-reduce for the least key
  (sound by naturality of `select_K`); independence batching via read/write
  footprints (the #140 role-fragment extractor reused as the footprint feed);
  revocable optimistic speculation (governed by SOC's scheduling-stability
  caveat — removing the least-key candidate may reselect).
- **Gate:** parallel commit sequence is **identical** to the serial one
  (determinism preserved); speculation is revocable without observable effect;
  O(Δ) gate still green under sharding; least-key selection unchanged under
  reordering of shards.
- **Unblocks:** scale; does not change semantics (differential vs oracle stays
  green).

## Step 10 — S6 · Presentation frontend (finite $F_O$-presentation)

- **Produces:** a minimal finite-presentation format
  $\Pres=(C_0,W_0,\mathsf{Real},\mathsf{Adm},\mathsf D,e_0)$ — the effectively
  enumerable frontend that feeds the engine. The `Ty`/`brixc` stack is demoted to
  *one such presentation frontend* (ADR §2).
- **Gate:** a presentation round-trips (parse → canonical presentation → replay
  matches); presentations are effectively enumerable with decidable syntactic
  equality; the flagship structural corpus expressible as a presentation.
- **Open decision ⟨D-PARSE⟩:** salvage `brixc`'s parser vs a fresh restricted
  parser for the committed-core language — **decided at this step, not now.**
- **Unblocks:** external authoring; the universality encoding (Step 12) needs a
  presentation format.

## Step 11 — S7 · `brix-kernel` (dependent proof kernel)

- **Produces:** the dependent proof kernel (unchanged role, ADR-0001 stage 5),
  depending **only** on `brix-semantic`. New SOC proof targets:
  **decomposition validity** (PD-1 as a theorem), **governor monotonicity**
  (SOC governance prop as a certificate), **counter-machine trace
  correspondence** (PD-2 input). First profile is the declared subset of #56's
  calculus (explicit contexts, implication, composition, finite products/sums,
  existentials, equality+substitution, transformation-preservation) — no
  metavariables/tactics/search.
- **Gate:** adversarial certificate vectors; acceptance API usable **without**
  loading any regime, the engine, or the presentation frontend; `Proven`/`Refuted`
  published by the kernel alone (authority table); a decomposition-validity
  certificate accepted end-to-end. ⟨PD-1 discharged as a theorem⟩
- **Unblocks:** `Proven` outcomes about behavior (ADR §5.2); PD-2's proof; CJ-2.

## Step 12 — S8 · Universal world / faithfulness (PD-2, then CJ-1)

- **Produces:** **PD-2** — the two-counter-machine encoding with halting +
  control policy, trace correspondence proof, Turing completeness, and
  identification of the preserved observed behavior (so the governance reduction
  is extensional). Then **CJ-1** — the faithful $F_O$-self-image
  $\U_{F_O}$ and the saturated, typed adequacy theorem (needs the settlement
  interface from Step 8).
- **Gate:** the encoding executes on the engine with verified trace
  correspondence (PD-2 discharged); for CJ-1, the faithfulness square holds on
  presentations with a settlement interface, with divergence-sensitive adequacy.
- **Unblocks:** the universality claim; successor to the #15 self-hosting arc;
  CJ-2 (governance incompleteness) follows.

---

## Dependency summary

```text
S0 ─▶ S1 ─▶ E1 ─▶ (S2⋈E2 naive oracle) ─▶ (S3⋈E4 calendar/commit) ─▶ S4 regimes
                                                          │
                        (E3 delta/regime trait ⋈ E5 O(Δ) GATE) ◀──┘
                                     │  [blocks all further optimization]
                                     ▼
                        E6 compact support ─▶ S5 saturation/settlement iface
                                     │                     │
                                     ▼                     ▼
                        E7 parallel        S6 presentation ─▶ S7 brix-kernel ─▶ S8 PD-2/CJ-1
```

## Proof-debt discharge map

| Debt | Operational discharge | Theorem discharge |
|---|---|---|
| **PD-1** tight generated subcategory | Step 4 (audit-factorization checker) | Step 11 (decomposition validity certificate) |
| **PD-2** universality | Step 12 (encoding executes) | Step 12 (trace-correspondence proof) |
| **CJ-1** faithful self-image | — | Step 12 (needs Step 8 settlement interface) |
| **CJ-2** governance incompleteness | — | follows PD-2 (Step 12), via `brix-kernel` |

## Open decision points carried by this plan

- **⟨D-FO⟩** (Step 0): ratify $(O, F_O)$. Default: $F_O=D_O=1+O\times X$,
  $O=O_{\min}$. See ADR-0002 §8.
- **⟨D-PARSE⟩** (Step 10): `brixc` parser salvage vs fresh restricted parser.
  Deferred to the presentation step.
