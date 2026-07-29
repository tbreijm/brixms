# ADR-0007 — Tree-Structured Typing Derivations & Multi-Step Elaboration to Proven

Status: **Accepted** (2026-07-29, ratified by user) (realizes [ADR-0005](./ADR-0005_Type_Inference_as_Realization.md) using [ADR-0004](./ADR-0004_Kernel_Profile_1_1.md) `∘` + [ADR-0006](./ADR-0006_Kernel_Profile_1_2.md) `⊗`; governs `brix-elaborate` and `soc-regimes`).

Date: 2026-07-29.

Foundation documents: [ADR-0005: Type Inference as Realization](./ADR-0005_Type_Inference_as_Realization.md), [ADR-0006: Kernel Profile 1.2](./ADR-0006_Kernel_Profile_1_2.md), [ADR-0004: Kernel Profile 1.1](./ADR-0004_Kernel_Profile_1_1.md), [ADR-0003: Proof Kernel Profile](./ADR-0003_Proof_Kernel_Profile.md) §7. This ADR pins the mechanism by which a **branching (tree-shaped) typing derivation** elaborates end-to-end to a `Proven` `HasType` judgement — Curry-Howard past the base (literal) case.

---

## 1. Motivation

The ADR-0005 depth slice reached `Proven` for a **literal** (single-generator) typing derivation, and surfaced the blocker for anything larger: typing derivations are **trees**, but the slice-2 encoding is flat and unsound in the audit direction.

Two concrete debts in `crates/soc-regimes/src/type_realization.rs`:

1. **Flat derivation.** `infer` returns `Vec<GeneratorId>` — the App case emits `[g_app, ...df, ...dx, ...unify]`, pretending the independent `f`- and `x`-branches are one sequential chain.
2. **Endpoints-only config padding** (`configs = [src, dst, dst, …, dst]`, lines ~332–337 / ~371–372). This satisfies `Decomposition`'s `configs.len() == generators.len()+1` invariant and even passes *syntactic* `RealizesComp` (because the padded middle `dst ≡ dst` matches), but **fails semantic audit** under sound generator semantics — demonstrated by `test_multi_step_elaboration_tree_vs_linear_tension`. Faking intermediate configs is unsound.

With Profile 1.1 (`∘`) and Profile 1.2 (`⊗`) both on `main`, the kernel can now certify any derivation tree. This ADR builds the bridge that feeds a real tree — **real intermediate configs, no padding** — through that certification to `Proven`.

---

## 2. Decision

Add three artifacts, and scope this slice to the expression fragment **{`Lit`, `Var`, `App`}** (Lam deferred — §7):

1. **`RealizesTree`** (in `brix-elaborate`): a derivation tree of realizations — `Leaf { generator, src, dst } | Seq { left, right } | Tensor { left, right }`, with product-structured endpoints (`TreeObj = Atom(ConfigId) | Prod(Box, Box)`).
2. **`elaborate_tree`** (in `brix-elaborate`): mirrors `elaborate_decomposition`, but folds the tree into a kernel proof term via **both** `RealizesComp` (for `Seq`) and `RealizesTensor` (for `Tensor`), admitting each leaf's `Realizes` as a hypothesis over its **real** configs, and delegates to `elaborate_and_publish`.
3. **`infer_tree`** (in `soc-regimes`): builds the `RealizesTree` for `{Lit, Var, App}` using the generators pinned in §4, alongside a tree-based `audited_type_check` path.

Demonstrator (end-to-end test target): `App(Var "f", Lit 1)` with `ctx = { f : Fn(Int, Bool) }` ⟹ `Bool`, elaborating to `Proven HasType`.

---

## 3. `RealizesTree` — Structure & Semantics

```text
TreeObj      = Atom(ConfigId) | Prod(Box<TreeObj>, Box<TreeObj>)
RealizesTree = Leaf   { generator: GeneratorId, src: TreeObj, dst: TreeObj }
             | Seq    { left: Box<RealizesTree>, right: Box<RealizesTree> }
             | Tensor { left: Box<RealizesTree>, right: Box<RealizesTree> }
```

**`TreeObj::to_object_term`** → `ObjectTerm`: `Atom(c)` ↦ `Const(PropositionId(c.digest()))`; `Prod(a,b)` ↦ `Tensor(a.to_object_term(), b.to_object_term())`. (Matches ADR-0006: a product config is an `ObjectTerm::Tensor`.)

**Derived accessors** (recursive, total):
- `src()` / `dst()` → `TreeObj`:
  - `Leaf` ↦ its `src` / `dst`.
  - `Seq{l,r}` ↦ `(l.src(), r.dst())`.
  - `Tensor{l,r}` ↦ `(Prod(l.src(), r.src()), Prod(l.dst(), r.dst()))`.
- `witness_object()` → `ObjectTerm`:
  - `Leaf{g,..}` ↦ `Const(PropositionId(g.digest()))`.
  - `Seq{l,r}` ↦ `Compose(r.witness_object(), l.witness_object())` (outer = `right`, inner = `left`; **same order convention as the kernel `RealizesComp` rule and `compose(g2,g1)`**).
  - `Tensor{l,r}` ↦ `Tensor(l.witness_object(), r.witness_object())`.
- `leaves()` → `Vec<&Leaf>`: **left-to-right depth-first** (`Leaf` ↦ `[self]`; `Seq{l,r}`/`Tensor{l,r}` ↦ `l.leaves() ++ r.leaves()`). This canonical order is the sole hypothesis ordering (§5).

**Well-formedness (`Seq` middle-match).** `elaborate_tree` MUST verify, for every `Seq{l,r}`, that `l.dst() == r.src()` (canonical structural equality). A mismatch is a malformed derivation and MUST yield `NotElaborated(Rejected)` — never a silently padded proof. (`Tensor` has no middle-match, per ADR-0006.)

---

## 4. Generator Semantics (the pinned soundness obligation)

Each leaf generator is a logged primitive of 𝒢. Its realization relation ρ_g is what the regime asserts sound; the kernel certifies only the **lax** composite (ADR-0006 §3.2). Pinned relations:

- **`g_lit`** — ρ = `{ (cfg(Lit n), cfg(Int)) : n ∈ ℤ }`. Every integer literal has type `Int`. (Existing.)
- **`g_var`** — ρ = `{ (cfg(Var x), cfg(T)) : (x:T) ∈ ctx }`. Context lookup. Endpoint pair is fixed by the enclosing context; the regime supplies the resolved `T`. (Existing.)
- **`g_split`** (NEW) — ρ = `{ (cfg(App(f,x)), cfg(f) ⊗ cfg(x)) }`. **Structural projection** of an application node into its two immediate sub-expression configs. **Tight**: an `App` node has exactly those two children. Identity: `GeneratorId::named("type.rule.app.split@1")`.
- **`g_app`** (NEW; supersedes the flat `type.rule.app@1` in tree mode) — ρ = `{ ((cfg(Fn(A,B)) ⊗ cfg(A)), cfg(B)) }`. **Type-level modus ponens**: from a function type `Fn(A,B)` and a matching argument type `A`, the result is `B`. Unification is discharged *before* the leaf is built — the regime unifies `T_f ~ Fn(T_x, β)`, zonks, and emits `g_app` over the **resolved** product endpoint `cfg(Fn(T_x, B)) ⊗ cfg(T_x) → cfg(B)`. So no unification variable survives into the leaf's configs. Identity: `GeneratorId::named("type.rule.app@2")` (append-only; the flat `@1` stays for the legacy `infer` path).

**Epistemic scope (honest, unchanged from B3).** `Proven` here asserts the **compositional-validity implication** — "given the leaf realizations, the composite `Realizes(k, cfg(App), cfg(B))` holds" — a revision-invariant theorem about the derivation. It does **not** claim the settlement outcome is `Proven`; the typing judgement's settlement outcome stays `Audited`. Lax direction only; tightness of ρ_{g_split}/ρ_{g_app} is a PD-1 obligation, not claimed here.

---

## 5. `elaborate_tree` Algorithm

Input: `source: &Judgement`, `tree: &RealizesTree`, `budget`. Output: `ElaborationResult`.

1. **Well-formedness.** Recursively check every `Seq` middle-match (§3). On failure ⟹ `NotElaborated(Rejected)`.
2. **Leaves & hypotheses.** `let leaves = tree.leaves();` (left-to-right DFS), `m = leaves.len()`. For each `L_i` (`i = 1..m`) build `H_i = Realizes(Const(g_i), L_i.src.to_object_term(), L_i.dst.to_object_term())`.
3. **Goal.** `G = Realizes(tree.witness_object(), tree.src().to_object_term(), tree.dst().to_object_term())`.
4. **Implication.** `H_1 → H_2 → … → H_m → G` (right-associated `Prop::Impl`).
5. **Proof term.** `m` nested `Lam`s binding `h_1..h_m` (outer = `h_1`). Body = `to_term(tree)` where, threading a **left-to-right leaf counter identical to step 2's ordering**, the `k`-th leaf encountered ↦ `Hyp(Var::Index(m - k))`; `Seq{l,r}` ↦ `RealizesComp { left: to_term(l), right: to_term(r) }`; `Tensor{l,r}` ↦ `RealizesTensor { left: to_term(l), right: to_term(r) }`. (De Bruijn convention identical to `elaborate_decomposition`: `h_i` has index `m - i`.)
6. **Delegate** to `elaborate_and_publish(source, &implication, &term, budget)`.

The counter discipline in step 5 MUST match the enumeration in step 2 so that leaf *k* (a `Leaf` with generator `g_k`) is proved by hypothesis `H_k` of type `Realizes(Const(g_k), …)`. This is the single correctness linchpin of the fold; it is covered by the differential test (§8.4).

`elaborate_decomposition` (linear) is retained unchanged; a linear chain is the degenerate all-`Seq` tree, but the existing entry point stays for the settlement decomposition path.

---

## 6. `soc-regimes` Integration

- Add `g_split()` / `g_app@2` generator constructors and an `infer_tree(expr, ctx, st) -> Result<(Ty, RealizesTree, Infer), TypeError>` for `{Lit, Var, App}`:
  - `Lit(n)` ↦ `Leaf{ g_lit, Atom(cfg(Lit n)), Atom(cfg(Int)) }`.
  - `Var(x)` ↦ `Leaf{ g_var, Atom(cfg(Var x)), Atom(cfg(T)) }`, `T = ctx(x)`.
  - `App(f,x)` ↦ infer `f` (⟹ `T_f`, tree `D_f`), infer `x` (⟹ `T_x`, tree `D_x`); fresh `β`; unify `resolve(T_f) ~ Fn(T_x, β)`; zonk; `A = zonk(T_x)`, `B = zonk(β)`; build
    `Seq{ Leaf{g_split, Atom(cfg(App(f,x))), Prod(Atom(cfg(f)), Atom(cfg(x)))},
        Seq{ Tensor{D_f, D_x},
             Leaf{g_app@2, Prod(Atom(cfg(Fn(A,B))), Atom(cfg(A))), Atom(cfg(B))} } }`.
    - **Middle-match obligation (must hold by construction):** `D_f.dst() == Atom(cfg(Fn(A,B)))` and `D_x.dst() == Atom(cfg(A))` — i.e. the tensored branch outputs must equal `g_app`'s product source. The regime builds `D_f`/`D_x` from the **zonked** types so this holds structurally. `Lam` ⟹ `Err(TypeError::Unsupported)` in tree mode (this slice).
- Add a tree-based `audited_type_check_tree(expr, ctx, context) -> Result<(Judgement, RealizesTree), TypeError>`: committed witness = `tree.witness_object().witness_digest()` (the tree composite — generalizes Option-1's `compose_chain` to the compose/tensor mix; a tensored+composed witness is a lawful witness per ADR-0006 §3.3), proposition = `Realizes(witness, cfg(expr), cfg(final_ty)).proposition_id()`, `Outcome::Audited`. The "audit" for this slice is **tree well-formedness over real configs** (Seq middles match; endpoints are real inference configs, not padded) — the honest analogue of `replay_verified`; deep ρ-membership audit is the deferred tight direction.

---

## 7. Scope & Non-goals

- **In:** `{Lit, Var, App}` tree inference; `elaborate_tree`; the `App(Var,Lit)` end-to-end `Proven`.
- **Deferred — `Lam` in tree mode.** Lam types its body under an extended assumption (`p:α`); a sound tree encoding needs a scope-closing generator and interacts with `ContextId::extend`/`ScopedWorldNonLeak`. Pinned separately in a follow-up. The flat `infer`/`type_check`/`audited_type_check` path (which handles Lam to `Derived`/`Audited`) is **retained unchanged** — no existing test regresses.
- **Deferred — settlement integration.** Routing tree derivations through `soc-core`'s *linear* `commit_tick`/`audit_step` is out of scope; a settlement `Decomposition` is linear, a typing derivation is a tree (ADR-0006 §8 rejected tree-`Decomposition` at the kernel level; whether the settlement layer grows one is a separate question). This slice audits the tree within the regime.
- **Deferred — tight ρ-audit** of `g_split`/`g_app` (PD-1); breadth (`let`, records/rows, patterns → 14/14 parity); Stage-3 deletion of `brix-ir`/`brix-ast`/`brix-diag`.
- **No kernel change.** Profiles 1.1 + 1.2 already suffice; this ADR touches only `brix-elaborate` and `soc-regimes`.

---

## 8. Test Obligations

1. **End-to-end Proven.** `App(Var "f", Lit 1)`, `f:Fn(Int,Bool)` ⟹ `audited_type_check_tree` ⟹ `elaborate_tree` ⟹ `ElaborationResult::Proven` with `Authority::ProofKernel`, `KernelCertificate`, `ElaborationBoundary` edge to the Audited source; goal proposition = the compositional-validity implication.
2. **Real configs / no padding.** Assert the built tree's intermediate configs are the real inference configs (e.g. `g_app@2` leaf src == `Prod(Atom(cfg(Fn(Int,Bool))), Atom(cfg(Int)))`), and that **no** endpoint equals its neighbor by padding.
3. **Malformed Seq rejected.** A hand-built tree with a broken `Seq` middle ⟹ `NotElaborated(Rejected)` (never a padded pass).
4. **Leaf/hypothesis alignment.** A tree whose leaves carry distinct generators elaborates `Accepted` **iff** the hypothesis-to-leaf index mapping is correct; a deliberately mis-ordered variant is `Rejected` (guards the §5 linchpin).
5. **Witness identity.** `tree.witness_object().witness_digest()` for the App demonstrator equals the hand-computed `compose(g_app, compose(tensor(w_f, w_x), g_split))` digest.
6. **No regression.** All existing `soc-regimes` / `brix-elaborate` tests stay green (flat path untouched).

---

## 9. Implementation Order

1. `brix-elaborate`: `TreeObj`, `RealizesTree` (+ accessors `src`/`dst`/`witness_object`/`leaves`/well-formedness), `elaborate_tree`; unit tests §8.3–8.5.
2. `soc-regimes`: `g_split`/`g_app@2`, `infer_tree`, `audited_type_check_tree`; end-to-end test §8.1–8.2 (brix-elaborate is already a dep of soc-regimes' test scope).
3. Full workspace `cargo fmt --all --check` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo test --workspace` green.

Soundness re-reviewed by Fable before merge (real configs, Seq well-formedness, leaf-index linchpin, honest `Proven`-as-implication scope).
