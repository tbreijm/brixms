# ADR-0006 — Proof Kernel Profile 1.2: Realization Tensor (Parallel Composition) Rule

Status: **Proposed** (2026-07-29) (extends [ADR-0004](./ADR-0004_Kernel_Profile_1_1.md) and [ADR-0003](./ADR-0003_Proof_Kernel_Profile.md) §5; governs `brix-kernel`).

Date: 2026-07-29.

Foundation documents: [ADR-0003: Proof Kernel Profile](./ADR-0003_Proof_Kernel_Profile.md), [ADR-0004: Kernel Profile 1.1](./ADR-0004_Kernel_Profile_1_1.md), [ADR-0002: SOC Constitution](./ADR-0002_SOC_Constitution.md) (§5, §7, §10 PD-1), [ADR-0005: Type Inference as Realization](./ADR-0005_Type_Inference_as_Realization.md). This ADR defines Profile 1.2 of `brix-kernel`, adding the realization-**tensor** (parallel-composition) inference rule needed to elaborate *branching* (tree-shaped) realization derivations — the concrete blocker surfaced by ADR-0005's depth slice.

---

## 1. Motivation

ADR-0004 gave the kernel sequential composition, `RealizesComp`:

$$\mathsf{Realizes}(g_1,x,y),\ \mathsf{Realizes}(g_2,y,z)\ \vdash\ \mathsf{Realizes}(\mathsf{compose}(g_2,g_1),x,z)$$

Its linchpin side condition is the **middle-endpoint match** $y \equiv y_s$: sequential composition only connects derivation steps that are *adjacent* (one step's target is the next step's source). This is exactly right for a linear settlement decomposition $k = g_n \circ \dots \circ g_1$.

**Typing derivations are not linear — they are trees.** (Finding: ADR-0005 depth slice; `soc-regimes` test `test_multi_step_elaboration_tree_vs_linear_tension`.) The application rule is the minimal witness:

$$\frac{\Gamma \vdash f : A \to B \qquad \Gamma \vdash x : A}{\Gamma \vdash f\,x : B}\quad(\text{App})$$

Reified as realizations, the two premises are the two sub-derivations

$$D_f : \mathsf{Realizes}(w_f,\ \mathrm{cfg}(f),\ \mathrm{cfg}(A\to B)) \qquad D_x : \mathsf{Realizes}(w_x,\ \mathrm{cfg}(x),\ \mathrm{cfg}(A)).$$

These branches are **independent**: the target of $D_f$ is $A\to B$, the source of $D_x$ is $x$ — they do not meet, so `RealizesComp` cannot join them. Padding the intermediate-config chain endpoints-only passes `RealizesComp` *syntactically* but **fails semantic audit** (the padded generator does not actually realize the faked `(dst, dst)` transition). Faking configs is unsound and was correctly refused.

The missing structure is **parallel** composition of independent branches — the monoidal tensor $\otimes$. Sequential $\circ$ (Profile 1.1) together with parallel $\otimes$ (Profile 1.2) form the monoidal-category structure under which **any derivation tree factors into an alternating composite of $\circ$ and $\otimes$** (see §5).

---

## 2. Decision & Rule Definition

Profile 1.2 adds exactly **ONE** inference rule, `RealizesTensor`, to `brix-kernel`.

### 2.1 Judgement Rule

$$\frac{\Gamma \vdash p : \mathsf{Realizes}(w_1, x_1, y_1) \qquad \Gamma \vdash q : \mathsf{Realizes}(w_2, x_2, y_2)}{\Gamma \vdash \mathsf{realizes\_tensor}(p, q) : \mathsf{Realizes}(w_1 \otimes w_2,\ x_1 \otimes x_2,\ y_1 \otimes y_2)}\quad(\text{RealizesTensor})$$

where $\otimes$ on witnesses and on objects is the **same monoidal bifunctor**, realized in syntax by the single object-term constructor `ObjectTerm::Tensor` (§6).

**There is deliberately no middle-endpoint side condition.** Tensor is defined on *any* two morphisms; the branches are not required to be adjacent. The absence of the adjacency check is precisely what lets this rule join independent branches that `RealizesComp` cannot.

---

## 3. Soundness & Epistemic Scope

### 3.1 Theoretical Justification

A realization regime is interpreted as a normal **lax monoidal** functor $\rho$ into $\mathbf{Rel}$ (`docs/SOC_core_foundations_revised.tex`; ADR-0002 §5). $\mathbf{Rel}$ is symmetric monoidal under cartesian product: for relations $R \subseteq A\times B$ and $S \subseteq C\times D$,

$$R \otimes S \;=\; \{\,((a,c),(b,d)) \mid (a,b)\in R \wedge (c,d)\in S\,\} \;\subseteq\; (A\times C)\times(B\times D).$$

The lax monoidal structure of $\rho$ supplies the **lax tensor axiom**

$$\rho_{w_1 \otimes w_2}\ \supseteq\ \rho_{w_1} \otimes \rho_{w_2}$$

— the exact monoidal analogue of Profile 1.1's lax-composition axiom $\rho_{g\circ f} \supseteq \rho_g \circ \rho_f$. Hence, from $(x_1,y_1)\in\rho_{w_1}$ and $(x_2,y_2)\in\rho_{w_2}$ we get $((x_1,x_2),(y_1,y_2))\in\rho_{w_1}\otimes\rho_{w_2}\subseteq\rho_{w_1\otimes w_2}$. The rule is sound by the definition of the monoidal product in $\mathbf{Rel}$ plus lax monoidality.

### 3.2 Lax vs. Tight Realization Scope

- **Lax Direction (Proven Here):** `RealizesTensor` proves that $w_1\otimes w_2$ realizes **at least** the outcome pair $((x_1,x_2),(y_1,y_2))$.
- **Tight Direction (Deferred):** That $w_1\otimes w_2$ realizes *nothing beyond* the tensor of the two branches depends on $\mathcal{G}$-tightness (ADR-0002 §10 PD-1) and is **NOT** claimed by this rule. Scope is identical to ADR-0004 §2.2 — this ADR does not change the tight/lax boundary.

### 3.3 Legitimacy of the Tensored Witness

Per the Option-1 lesson (a composed witness is still a witness; ADR-0005 / `soc-constitution` log), a **tensored witness is likewise a lawful witness.** Composition required an adjacency precondition (endpoints meet); **tensor requires none** — $\otimes$ is a total bifunctor on morphisms, so $w_1\otimes w_2 : x_1\otimes x_2 \to y_1\otimes y_2$ is always well-defined. This makes tensor structurally *simpler* than compose at the identity level, not more permissive in the unsound direction: the resulting object is a genuine morphism of the monoidal category.

### 3.4 No New Binders

`RealizesTensor` operates on closed `Realizes` propositions and introduces no object binders and no eigenvariables. Therefore, unlike `∃E`/`+E`/`Comp`, it carries **no freshness / eigenvariable side conditions.**

---

## 4. Mandatory Side Conditions

Every evaluation of $\mathsf{realizes\_tensor}(p, q)$ against an expected proposition $\mathsf{Realizes}(w, x, y)$ MUST strictly verify:

1. **Branch Types are Realizations.** Synthesizing the types of $p$ and $q$ MUST yield $\mathsf{Realizes}(w_1,x_1,y_1)$ and $\mathsf{Realizes}(w_2,x_2,y_2)$ respectively. A non-`Realizes` synthesized type MUST result in `Verdict::Rejected`.
2. **Ordered Source Match.** The goal source $x$ MUST be **canonically equal** to $\mathsf{Tensor}(x_1, x_2)$, in that left/right order. Tensor is **non-commutative**; the ordering of branches is significant and MUST be preserved (`left` ↦ first component, `right` ↦ second component).
3. **Ordered Target Match.** The goal target $y$ MUST be canonically equal to $\mathsf{Tensor}(y_1, y_2)$, in that order.
4. **Witness Structure Match (digest-based).** The goal witness $w$ MUST satisfy $w.\mathsf{witness\_digest}() \equiv \mathsf{tensor}(w_1.\mathsf{witness\_digest}(),\ w_2.\mathsf{witness\_digest}())$, comparing content-addressed witness identities (mirroring ADR-0004 §3 condition ii, which is digest-based so a `Const` of the composite identity is accepted alongside the structural `Tensor(w_1,w_2)`).

**There is NO middle-endpoint side condition** (contrast ADR-0004 §3 condition i). This is intentional and is the defining difference between the sequential and parallel rules.

No normalization, β-reduction, or unification is performed during endpoint comparison; canonical structural equality (source/target) and canonical witness-digest equality (witness) are enforced directly, as in Profile 1.1.

---

## 5. Adequacy: Trees Factor into $\circ$ and $\otimes$

The `App` derivation elaborates using **only** Profile 1.1 + Profile 1.2, with the domain generators supplied by the type-realization regime (`soc-regimes`), not the kernel:

- `g_split : Realizes(g_split, cfg(f x), cfg(f) ⊗ cfg(x))` — structural decomposition of the application node into its sub-configurations (a logged primitive generator in $\mathcal{G}$).
- `D_f ⊗ D_x : Realizes(w_f ⊗ w_x, cfg(f) ⊗ cfg(x), cfg(A→B) ⊗ cfg(A))` — **`RealizesTensor`** of the two branches.
- `g_app : Realizes(g_app, cfg(A→B) ⊗ cfg(A), cfg(B))` — the application typing rule as a generator.

Composed sequentially (all middle-endpoints meet):

$$\mathsf{compose}\big(g_{app},\ \mathsf{compose}(w_f \otimes w_x,\ g_{split})\big)\ :\ \mathsf{Realizes}(\_,\ \mathrm{cfg}(f\,x),\ \mathrm{cfg}(B)).$$

This is the general pattern: **any tree = sequential composites of tensors of sub-derivations.** The kernel provides the two combinators ($\circ$, $\otimes$); the regime provides the generators ($g_{split}$, $g_{app}$, $g_{lit}$, $g_{var}$, $g_{lam}$, …) and the assembly. Soundness of $g_{split}$/$g_{app}$ as tight generators is the regime's obligation (ADR-0005), **not** the kernel's — the kernel only certifies the lax combinators.

$n$-ary products (records, rows, multi-binding `let`) nest binary tensor left-associatively; see §6.3.

---

## 6. Canonical Encoding & Ordinals

All new constructs use **append-only** canonical ordinals. Existing frozen ordinals (ADR-0003, ADR-0004) are unchanged.

### 6.1 `ObjectTerm` Addition

- `ObjectTerm::Tensor(Box<ObjectTerm>, Box<ObjectTerm>)`: canonical ordinal **3** (append-only after `Compose = 2`).
- `ObjectTerm::witness_digest()` gains the arm: `Tensor(a, b) ↦ brix_semantic::tensor(a.witness_digest(), b.witness_digest())`.
- `shift_object_term` / `subst_obj` gain a `Tensor` arm recursing structurally into both components (mirroring the `Compose` arms), preserving totality of substitution.

**No `Prop` variant is added.** The endpoints of `Prop::Realizes` are already `ObjectTerm`s; a product object is expressed directly as `ObjectTerm::Tensor`. A single `Tensor` bifunctor serves both object endpoints and witnesses — exactly as `Compose` already serves both roles — keeping the TCB surface minimal. (This intentionally supersedes the exploratory "Prop product-endpoint" note in the handover; see §8 Alternatives.)

### 6.2 `TermKind` Addition

- `TermKind::RealizesTensor { left: Box<TermKind>, right: Box<TermKind> }`: canonical ordinal **16** (append-only after `RealizesComp = 15`).

### 6.3 `brix-semantic` Witness Tensor Primitive (Frozen ABI)

A new primitive mirrors `compose` (`crates/brix-semantic/src/witness.rs`):

- `pub const WITNESS_TENSOR_TAG: &str = "brix.semantic.WitnessId.tensor";`
- `pub fn tensor(left: WitnessId, right: WitnessId) -> WitnessId` with **frozen** encoding under `brix_canon::Domain::Value`:
  1. `write_tag(WITNESS_TENSOR_TAG)`
  2. `left` digest bytes (`write_bytes`)
  3. `right` digest bytes (`write_bytes`)
- **Non-commutative** by construction (`tensor(a,b) ≠ tensor(b,a)`), matching the ordered branches.
- A **frozen golden hex vector** over fixed inputs (as `golden_vector_witness_compose` does), and an independent-reproduction test that does not go through `tensor` itself.
- Companion `WitnessId::tensor(left, right)` associated fn, mirroring `WitnessId::compose`.
- **Optional companion** `tensor_chain(&[WitnessId]) -> Option<WitnessId>` (left-nested, mirroring `compose_chain`) for the regime's $n$-ary products — MAY land with this ADR or with the first regime consumer; it introduces no kernel surface.

Cross-crate identity alignment MUST be pinned by a differential test in `brix-kernel`: `ObjectTerm::Tensor(a,b).witness_digest() == brix_semantic::tensor(a.witness_digest(), b.witness_digest())` (mirroring the existing `test_witness_digest_alignment_with_brix_semantic_compose`).

---

## 7. Test Obligations (kernel)

Mirror the ADR-0004 adversarial suite:

1. **Accept.** `RealizesTensor` of two well-typed branches against goal `Realizes(Tensor(w1,w2), Tensor(x1,x2), Tensor(y1,y2))` ⇒ `Accepted`.
2. **Accept via `Const` composite witness.** Goal witness given as `Const(id)` where `id == tensor(w1_id, w2_id)` ⇒ `Accepted` (digest-based §4.4).
3. **Reject wrong witness.** Goal witness digest ≠ `tensor(w1,w2)` ⇒ `Rejected`.
4. **Reject swapped order.** Goal source `Tensor(x2, x1)` (branches swapped) ⇒ `Rejected` (non-commutativity §4.2).
5. **Reject wrong source/target object.** Goal source/target not equal to the tensored endpoints ⇒ `Rejected`.
6. **Reject non-`Realizes` branch.** A branch synthesizing a non-`Realizes` type ⇒ `Rejected`.
7. **Adequacy (App shape).** The §5 assembly (`compose(g_app, compose(tensor(D_f,D_x), g_split))`) against `Realizes(_, cfg(f x), cfg(B))` ⇒ `Accepted`.
8. **Determinism / totality.** Budgeted; `ResourceExhausted` on tiny budget maps to `Unknown`, never `Refuted` (unchanged kernel discipline).

---

## 8. Alternatives Considered

- **Tree-structured `Decomposition` (rejected).** Generalize the settlement `Decomposition` evidence to carry a tree instead of a chain, and elaborate the tree directly. Rejected: it duplicates categorical structure the kernel already half-expresses via `Compose`, pushes tree-shape awareness into the audit path, and does not yield a reusable proof combinator. The monoidal tensor is the right *general* primitive — it composes with everything already built and needs no change to `Decomposition`.
- **A dedicated `Prop::Product`-endpoint variant (rejected).** Model product objects as a new `Prop` construct rather than reusing `ObjectTerm::Tensor`. Rejected: objects and morphisms tensor under the *same* bifunctor, so two constructs would be redundant, would enlarge the frozen `Prop` ABI, and would force a second witness-identity notion. One `ObjectTerm::Tensor` is smaller and categorically faithful (§6.1).
- **A symmetry/braiding rule (deferred, not added).** $\mathbf{Rel}$ is *symmetric* monoidal, so a braiding $x\otimes y \cong y\otimes x$ is available. Profile 1.2 keeps tensor **ordered** and adds no braiding rule; the regime fixes a canonical branch order. A braiding rule would be a separate, later profile addition if a consumer needs it.

---

## 9. Non-goals

- No change to existing frozen canonical ordinals (`ObjectTerm` 0–2, `TermKind` 0–15, `Prop` 0–8).
- No introduction of normalization, β-reduction, or unification during comparison.
- No claim of the **tight** direction (§3.2); PD-1 unchanged.
- No braiding/symmetry, associator, or unitor coherence rules (nested binary tensor with a regime-fixed canonical order suffices for current consumers).
- No change to the six-verdict API, the `ResourceExhausted ≠ Refuted` discipline, or the `Authority::ProofKernel` publication boundary.

---

## 10. Implementation Order (for the implementer; kernel soundness re-reviewed before merge)

1. `brix-semantic`: `tensor` + `WITNESS_TENSOR_TAG` + golden vector + independent-reproduction test (+ optional `tensor_chain`).
2. `brix-kernel/term.rs`: `ObjectTerm::Tensor` (ordinal 3) + `witness_digest` arm + `shift`/`subst` arms + `canon_write` arm; `TermKind::RealizesTensor` (ordinal 16) + `canon_write` arm; cross-crate alignment differential test.
3. `brix-kernel/check.rs`: `RealizesTensor` in **both** `check_type` and `infer_type`, enforcing §4 side conditions (note: **no** middle-match).
4. Kernel adversarial tests §7.
5. `Cargo.lock` committed if any dependency graph changes (no new crates expected here).

This ADR is the design pin. The soundness-critical rule logic (§4 side conditions, §3 lax scope) is re-reviewed rule-by-rule before merge.
