# ADR-0005 — Type Inference as Realization: Native SOC Type-Realization Regime

Status: **Proposed** (2026-07-29) (governs BrixMS Stage-2 Legacy Retirement; extends [ADR-0002](./ADR-0002_SOC_Constitution.md), [ADR-0003](./ADR-0003_Proof_Kernel_Profile.md), [ADR-0004](./ADR-0004_Kernel_Profile_1_1.md)).

Date: 2026-07-29.

Foundation documents: [ADR-0002: SOC Constitution](./ADR-0002_SOC_Constitution.md) (§5 commitment, §7 regimes, §12 legacy retirement), [ADR-0003: Proof Kernel Profile](./ADR-0003_Proof_Kernel_Profile.md), [ADR-0004: Kernel Profile 1.1](./ADR-0004_Kernel_Profile_1_1.md) (`RealizesComp`), and the BrixMS Legacy Retirement Plan (Stage 2).

---

## 1. Thesis

Type inference is not a separate subsystem — it is an **instance of the SOC settlement, realization, and proof machinery**.

In BrixMS Stage 2, type inference is reconceived natively within the SOC substrate:

1. **Types are Configurations:** A canonical type description (e.g. `Ty::Int`, `Ty::Fn`, `Ty::Record`) is a canonical configuration (`ConfigId` in `brix-semantic`).
2. **Typing Rules are Generator Sets:** Typing rules are the generator set $\mathcal{G}_{\text{type}}$ of a native *type-realization regime* (primitive typing steps: `g_var`, `g_app`, `g_abs`, `g_let`, `g_record`, `g_row`, `g_match`, `g_subsume`, etc.).
3. **Typing Derivations are Composed Witnesses:** A typing derivation is a composed witness $k = g_n \circ \dots \circ g_1$ over rule-generators, forming an audit-generated commitment ([ADR-0002](./ADR-0002_SOC_Constitution.md) §5).
4. **`HasType(e, T)` IS `Realizes(w, e, T)`:** The assertion that expression term $e$ has type $T$ is identically the proposition $\mathsf{Realizes}(w, e, T)$, asserting that term configuration $e$ realizes typed configuration $T$ under derivation witness $w$.
5. **Type Inference IS Settlement:** Type inference is the execution of the settlement loop (`soc-core` `commit_tick`), searching admissible typing-witness candidates under constraint admissibility $\mathsf{Adm}_{\text{type}}$ and committing a derivation to yield a `Derived` `HasType` judgement.
6. **Audit & Kernel Elaboration (Curry–Howard Realized):** Replaying the committed derivation through the reference replayer upgrades the judgement from `Derived` to `Audited`. Kernel elaboration of the tight decomposition via $\mathsf{RealizesComp}$ ([ADR-0004](./ADR-0004_Kernel_Profile_1_1.md)) upgrades it to `Proven`. **This is Curry–Howard realized through the substrate: types-as-configurations/propositions, typing-derivations-as-proof-witnesses.**
7. **Conflict Classification as Settlement Outcomes:** The 14 `ConflictKind` classifications from `brix-ir` become explicit SOC settlement outcomes: missing admissible witness $\to$ `Outcome::Unknown` with a refutation/conflict witness; overload ambiguity $\to$ calendar/governance escalation; type mismatch $\to$ refutation witness.

---

## 2. SOC <-> Type-System Correspondence

| SOC Substrate Concept | Type-System Concept | Description in `brix-semantic` / `soc-core` |
|---|---|---|
| **Configuration** ($c \in \mathcal{C}$) | Canonical Type / Term State | `ConfigId` representing a term $e$ or typed term $(e : T)$. |
| **Generator** ($g \in \mathcal{G}_{\text{type}}$) | Primitive Typing Rule | Atomic step in $\mathcal{G}_{\text{type}}$ (e.g., $\text{T-Var}$, $\text{T-App}$, $\text{T-Let}$, $\text{T-Row}$). |
| **Composed Witness** ($w = g_n \circ \dots \circ g_1$) | Typing Derivation Tree | `WitnessId::compose_chain(&[g1, ..., gn])` over tight rule generators. |
| **Realization** ($\mathsf{Realizes}(w, x, y)$) | $\text{HasType}(e, T)$ Judgement | Proposition that configuration $e$ realizes typed target $(e : T)$ under $w$. |
| **Settlement Loop** (`commit_tick`) | Type Inference / Constraint Solver | Coalgebraic transition finding candidate witness $w$ satisfying $\mathsf{Adm}$. |
| **Admissibility** ($\mathsf{Adm}_{\text{type}}$) | Typing & Subtyping Constraints | Filter ruling out ill-typed transitions and constraint violations. |
| **Settlement Outcome** | Diagnostic / Inconsistency | `Outcome::Derived` for successful derivation; `Outcome::Unknown` for conflict. |
| **Proof Kernel Elaboration** | Curry–Howard Proof Certification | $\mathsf{RealizesComp}$ ([ADR-0004](./ADR-0004_Kernel_Profile_1_1.md)) transforming $g_n \circ \dots \circ g_1$ into `Proven`. |

---

## 3. Architecture & Component Allocation

```
+-------------------------------------------------------------------------+
|                              soc-core                                   |
|   +-----------------------------------------------------------------+   |
|   |                        commit_tick                              |   |
|   |   (Settlement Loop searching admissible witness candidates)     |   |
|   +-----------------------------------------------------------------+   |
+-------------------------------------------------------------------------+
                                     |
                                     v
+-------------------------------------------------------------------------+
|                        soc-regimes / brix-type                          |
|   +-----------------------------------------------------------------+   |
|   |               TypeRealizationRegime (RegimeId)                  |   |
|   |   - Generator Set G_type (g_var, g_app, g_abs, g_let, g_row...) |   |
|   |   - Admissibility Filter Adm_type (unification & subtyping)     |   |
|   +-----------------------------------------------------------------+   |
+-------------------------------------------------------------------------+
                                     |
                                     v
+-------------------------------------------------------------------------+
|                             brix-semantic                               |
|   +-----------------------------------------------------------------+   |
|   | ConfigId (Term & Ty) | Witness | Realizes | Outcome Lattice    |   |
|   |   Derived -> Audited -> Proven                                  |   |
|   +-----------------------------------------------------------------+   |
+-------------------------------------------------------------------------+
                                     |
                                     v
+-------------------------------------------------------------------------+
|                     brix-kernel + brix-elaborate                        |
|   +-----------------------------------------------------------------+   |
|   | RealizesComp (ADR-0004) certifying tight decomposition         |   |
|   +-----------------------------------------------------------------+   |
+-------------------------------------------------------------------------+
```

### What Lands Where

1. **`brix-semantic` (Substrate):**
   - Reuses `ConfigId` for term and type representations.
   - Reuses `Witness`, `WitnessId::compose`, and `WitnessId::compose_chain` for derivation identity.
   - Reuses `Realizes` proposition and the 6-member epistemic outcome lattice (`Proven`, `Refuted`, `Derived`, `Measured`, `Unknown`, `Audited`).

2. **`soc-regimes` (or dedicated `brix-type` crate):**
   - Implements `TypeRealizationRegime` under `RegimeId::named("brix.type.native@0.2")`.
   - Defines the primitive rule generators $\mathcal{G}_{\text{type}}$:
     - `g_var`: Variable lookup in typing context.
     - `g_app`: Function application ($\text{Fn}(A \to B) \times A \implies B$).
     - `g_abs`: Lambda abstraction.
     - `g_let`: Polymorphic let-binding & generalization.
     - `g_lit`: Literal constant typing.
     - `g_record_proj`: Record field projection.
     - `g_record_cons`: Record field construction.
     - `g_row_extend` / `g_row_restrict`: Row polymorphism operations.
     - `g_subsume` / `g_epistemic_lift`: Epistemic type conversions.
   - Defines $\mathsf{Adm}_{\text{type}}$ for constraint admissibility.

3. **`soc-core` (Engine):**
   - The settlement loop `commit_tick` drives candidate selection over $\mathcal{G}_{\text{type}}$, committing derivation witness $w$ to emit `Outcome::Derived`.

4. **`brix-elaborate` + `brix-kernel` (Proof Kernel):**
   - Replays tight decomposition $k = g_n \circ \dots \circ g_1$ via `brix-oracle` to upgrade `Derived` $\to$ `Audited`.
   - Uses `RealizesComp` ([ADR-0004](./ADR-0004_Kernel_Profile_1_1.md)) to elaborate tight composition steps into a kernel-certified proof, upgrading `Audited` $\to$ `Proven`.

---

## 4. ConflictKind to Settlement-Outcome Mapping Table

Every `brix_ir::reflect::ConflictKind` variant maps deterministically to an SOC settlement outcome and evidence structure:

| `brix-ir` `ConflictKind` Variant | SOC Settlement Outcome | Refutation / Evidence Witness Representation |
|---|---|---|
| `Mismatch { left, right }` | `Outcome::Unknown` | Refutation witness $w_{\text{refute}}$ asserting empty candidate set $\mathsf{cand}(e) = \emptyset$ for $left \equiv right$. |
| `Arity { expected, found }` | `Outcome::Unknown` | Refutation witness $w_{\text{refute}}$ certifying function application arity mismatch. |
| `UnknownField { field }` | `Outcome::Unknown` | Refutation witness $w_{\text{refute}}$ asserting row missing required label `field`. |
| `NonBool { found }` | `Outcome::Unknown` | Refutation witness $w_{\text{refute}}$ certifying guard expression type $\text{found} \neq \text{Bool}$. |
| `Occurs { var, into }` | `Outcome::Unknown` | Infinite-regress refutation witness certifying cyclic type variable reference $var \in \text{ftv}(into)$. |
| `Dimension { op, left, right }` | `Outcome::Unknown` | Physical dimension incompatibility witness certifying non-zero dimension exponent vector diff under `op`. |
| `TryNonResult { found }` | `Outcome::Unknown` | Monad mismatch witness certifying `?` operator target $\text{found} \neq \text{Result<T, E>}$. |
| `ImpureRule` | `Outcome::Unknown` | Side-condition failure witness certifying violation of pure-rule side condition $\text{pure}(B, H)$. |
| `NondeterministicRule` | `Outcome::Unknown` | Governance report certifying violation of deterministic rule side condition $\text{det}(B, H)$. |
| `DivergentRule` | `Outcome::Unknown` | Quiescence interface failure witness certifying potential divergence $\text{nondiverge}(B, H)$. |
| `UnboundHeadKey { key }` | `Outcome::Unknown` | Scope refutation witness certifying head key $\text{key} \notin \text{Bindings}$. |
| `MaskRefNotEdgeBound { var }` | `Outcome::Unknown` | Mask-head side condition refutation witness certifying $\text{var}$ not edge-bound. |
| `OrdinaryFnOnDerivedRel { relation }` | `Outcome::Unknown` | Relation category refutation witness certifying ordinary function invoked on derived relation. |
| `EpistemicErasure { from, to }` | `Outcome::Unknown` | Epistemic status boundary violation witness certifying illegal coercion from status-bearing type (`Estimate`, `Missing`, `Probability`) to un-graded payload type `to`. |

---

## 5. Parity Strategy & Stage-2 / Stage-3 Sequencing

### Stage 2: Differential Oracle & Dual-Execution Parity (Active Stage)
1. **Retain `brix_ir::reflect::analyze` as Differential Oracle:** `brix-ir` remains intact in the codebase as the reference benchmark.
2. **Build Native `TypeRealizationRegime`:** Implement the generator set $\mathcal{G}_{\text{type}}$ and settlement candidate search.
3. **Validate 14/14 Parity:** Execute `crates/brix-conformance/tests/type_parity.rs` continuously. Assert that for every fixture in the conformance corpus:
   $$\text{SettlementOutcome}(\text{NativeRegime}) \equiv \text{ReflectiveReport}(\text{analyze})$$
   - Equivalent `HasType` judgements (`Derived`).
   - 100% 14/14 `ConflictKind` category matching.
   - Identical resolved canonical types.

### Stage 3: Legacy Retirement & Crate Deletion
1. **Freeze Parity Verification:** Once 14/14 parity holds across the full test corpus with zero regressions.
2. **Decommission `brix-ir` & `brix-ast`:** Completely remove `crates/brix-ir` and `crates/brix-ast`.
3. **Promote Native Regime:** `TypeRealizationRegime` becomes the sole, authoritative type analysis engine of BrixMS.

---

## 6. Machinery Reuse Matrix

Almost all required operational and proof machinery already exists in BrixMS v3:

| Required Operational Step | Pre-existing Mechanism | Location |
|---|---|---|
| **Settlement Loop Execution** | `commit_tick` Coalgebra Engine | `crates/soc-core` |
| **Composed Witness Identity** | `WitnessId::compose_chain` | `crates/brix-semantic/src/witness.rs` |
| **Typing Proposition** | `Realizes(w, src, dst)` | `crates/brix-semantic/src/proposition.rs` |
| **Audit Factorization Replay** | `Decomposition` & Replayer | `crates/brix-semantic/src/decomposition.rs` |
| **Proof Kernel Rule** | `RealizesComp` (Profile 1.1) | [ADR-0004](./ADR-0004_Kernel_Profile_1_1.md) / `crates/brix-kernel` |
| **Epistemic Upgrade** | `Derived` $\to$ `Audited` $\to$ `Proven` | `crates/brix-semantic/src/outcome.rs` + `brix-elaborate` |

---

## 7. Non-goals & Open Questions

### Non-goals
- **No changes to surface syntax:** The user-facing Brix language grammar remains untouched.
- **No kernel modification:** `brix-kernel` requires zero new inference rules beyond Profile 1.1's `RealizesComp` ([ADR-0004](./ADR-0004_Kernel_Profile_1_1.md)).
- **No premature deletion of `brix-ir`:** `brix-ir` will not be removed until Stage 3 differential parity is proven.

### Open Questions & Research Risks

> [!WARNING]
> **Hardest Open Question (Research Risk): Mapping Hindley–Milner Unification & Search onto SOC Candidate Generation**
> 
> Standard HM type inference relies on mutating free unification variables ($\alpha, \beta$) and zonking substitutions. In SOC, candidate generation must be **declarative, side-effect-free, and deterministic**.
> 
> *Key Challenge:* How can `soc-core`'s candidate generator propose typing-witness candidates $g \in \mathcal{G}_{\text{type}}$ without running into a combinatorial explosion of un-instantiated type variables or requiring destructive state mutation?
> 
> *Proposed Path:* Represent unification environments as explicit context configurations ($\text{ConfigId}$) in the assumption context (`ContextId`). Unification steps then become explicit generator applications ($g_{\text{unify}}$) that narrow candidate type configurations deterministically.

Other open questions:
1. **Row-Polymorphism Generator Instantiation:** Do extensible row types require dynamic generator instantiation for each field label, or can row extension be handled by a single parameterized generator $g_{\text{row\_ext}}(label)$?
2. **Overload Candidate Escalation:** When an overloaded function has multiple candidate derivations, should the calendar `select_K` resolve overloads via deterministic scoring, or escalate to governance as a calendar ambiguity?

---

## 8. Summary

**Type inference in BrixMS is not an ad-hoc compiler pass, but a native SOC type-realization regime where typing derivations are composed witness trees, `HasType` judgements are `Realizes` propositions, and Curry–Howard proof certification is achieved by kernel elaboration of tight witness decompositions.**
