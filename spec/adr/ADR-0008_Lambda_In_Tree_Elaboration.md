# ADR-0008 — Lambda in Tree Elaboration + Zonk-at-Boundary

Status: **Accepted** (2026-07-30, ratified by user) (extends [ADR-0007](./ADR-0007_Tree_Structured_Typing_Elaboration.md); governs `soc-regimes`).

Date: 2026-07-30.

Foundation documents: [ADR-0007: Tree-Structured Typing Elaboration](./ADR-0007_Tree_Structured_Typing_Elaboration.md), [ADR-0005: Type Inference as Realization](./ADR-0005_Type_Inference_as_Realization.md), [ADR-0006: Kernel Profile 1.2](./ADR-0006_Kernel_Profile_1_2.md). This ADR extends the tree-elaboration path to `Lam`, reaching `Proven` for the canonical identity-application `App(Lam "x" (Var x), Lit 42) ⟹ Int`, and pays down ADR-0007 §7's un-zonked-endpoints limitation.

---

## 1. Motivation

ADR-0007 elaborated `{Lit, Var, App}` to `Proven` but deferred `Lam` and left a limitation: **leaf endpoint configs are materialized at sub-inference time (un-zonked)**. Lambda makes this limitation *load-bearing*, not incidental:

Worked example — `App(Lam "x" (Var x), Lit 42)`:
- The lambda sub-tree is built with a fresh param type `α`, so its result endpoint is `cfg(Fn(α, α))`.
- The outer application unifies `α = Int`, so `g_app`'s expected product source is `cfg(Fn(Int, Int)) ⊗ cfg(Int)`.
- With un-zonked endpoints, the lambda's `dst` stays `cfg(Fn(α,α)) ≠ cfg(Fn(Int,Int))`, the `Seq` middle mismatches, and `elaborate_tree` (correctly, safely) **refuses** — never a false `Proven`, but never the true one either.

The fix is to **defer config materialization to a single boundary pass that zonks against the final substitution**. This also better honors the ADR-0005 "materialize digests only at the commit boundary" discipline (today's `infer_tree` materializes mid-inference). Lambda and the zonk fix are therefore delivered together.

---

## 2. Decision

Two coupled changes, both confined to `crates/soc-regimes/src/type_realization.rs` (**no kernel change; no `brix-elaborate` change**):

1. **Zonk-at-boundary refactor.** `infer_tree` threads a `soc-regimes`-local derivation tree carrying **type/expression endpoints** (`TyTree`), materializing **no** `ConfigId`. A single `materialize(&TyTree, &subst) -> brix_elaborate::RealizesTree` pass, run at the top of `audited_type_check_tree`, zonks every `Ty` endpoint against the **final** substitution and only then takes `config_id()`s. This structurally eliminates the ADR-0007 §7 limitation.
2. **`Lam` in tree mode** via two structural generators, `g_lam_intro` and `g_lam_close` (§4), so `infer_tree` covers `{Lit, Var, App, Lam}`.

Demonstrator (end-to-end test target): `App(Lam "x" (Var x), Lit 42) ⟹ Proven Int`.

---

## 3. `TyTree` — Deferred-Materialization Derivation Tree

A `soc-regimes`-local mirror of `RealizesTree` whose leaf endpoints hold **un-materialized** type/expression values:

```text
CfgAtom = Expr(Expr) | Type(Ty)                       // the thing whose config_id we take, later
TyObj   = Atom(CfgAtom) | Prod(Box<TyObj>, Box<TyObj>)
TyTree  = Leaf { generator: GeneratorId, src: TyObj, dst: TyObj }
        | Seq    { left: Box<TyTree>, right: Box<TyTree> }
        | Tensor { left: Box<TyTree>, right: Box<TyTree> }
```

**`materialize(&TyTree, subst: &BTreeMap<u32,Ty>) -> RealizesTree`** (and `materialize_obj(&TyObj, subst) -> TreeObj`):
- `TyObj::Atom(Expr(e))` ↦ `TreeObj::Atom(e.config_id())` (expressions contain no type variables — no zonk needed).
- `TyObj::Atom(Type(t))` ↦ `TreeObj::Atom(zonk(&t, subst).config_id())` (**zonk against the final subst**).
- `TyObj::Prod(a,b)` ↦ `TreeObj::Prod(box materialize_obj(a), box materialize_obj(b))`.
- `Leaf/Seq/Tensor` map structurally to the `RealizesTree` variants.

Because `materialize` runs once with the final substitution, **every** leaf endpoint (however deep) is zonked consistently. Well-formedness (`RealizesTree::well_formed`, ADR-0007 §3) is then checked on the materialized tree by `audited_type_check_tree`'s existing guard.

---

## 4. Lambda Generators & Tree Shape (Design 1: structural)

`Lam "p" body`, inferred by extending the type context with `p : α` (fresh) and inferring `body : T_b`, yields result type `Fn(param_ty, T_b)` where `param_ty = resolve(α)`. Its `TyTree`:

```text
Seq{ Leaf{ g_lam_intro, Atom(Expr(Lam p body)), Atom(Expr(body)) },
     Seq{ D_body,
          Leaf{ g_lam_close, Atom(Type(T_b)), Atom(Type(Fn(param_ty, T_b))) } } }
```

Generators (logged 𝒢 primitives):
- **`g_lam_intro`** = `GeneratorId::named("type.rule.lam.intro@1")` — ρ = `{ (cfg(Lam p body), cfg(body)) }`. **Structural**: strip the binder to expose the body sub-expression. Tight (a lambda node has exactly that body).
- **`g_lam_close`** = `GeneratorId::named("type.rule.lam.close@1")` — ρ = `{ (cfg(B), cfg(Fn(A, B))) }` for the derivation's parameter type `A = param_ty`. **→-introduction at the type level**: from a body of type `B` (under parameter `A`), form `Fn(A, B)`. A relation, not a function (`B` alone does not fix `A`) — which is fine: realizations are relations (lax functor into **Rel**), and the specific leaf pins one `(A,B)`.

**Assumption handling (Design 1).** The parameter assumption `p : α` is *not* a separately introduced scoped fact; it flows through inference as the type-context binding `p:α`, and the body's `g_var(p)` occurrences become ordinary `Realizes(g_var, cfg(Var p), cfg(α))` leaves. At elaboration, every leaf — the `g_var(p)` lookups included — becomes an **antecedent** of the compositional-validity implication (ADR-0007 §4). So the `Proven` statement is the honest conditional "given these realization steps (parameter lookups included), the composite realizes `(Lam, Fn(A,B))`". For a **closed** program (the demonstrator) unification resolves `α`, so no free type-variable antecedent survives. No `ContextId::extend`/scoped-child context is required for this slice.

**Middle-match (holds after zonk).** `g_lam_intro.dst = Atom(Expr(body)) = D_body.src`; `D_body.dst = Atom(Type(T_b)) = g_lam_close.src`; the lambda's `dst = Atom(Type(Fn(param_ty, T_b)))`, which — **after `materialize` zonks against the final subst** — equals the product component `g_app` expects in the enclosing application. This is exactly the case that failed pre-zonk (§1).

---

## 5. Epistemic Scope (unchanged, honest)

`Proven` asserts the **compositional-validity implication**; the typing/settlement outcome stays `Audited`. Lax direction only; tightness of `g_lam_intro`/`g_lam_close`/`g_split`/`g_app` ρ is a PD-1 obligation, not claimed. The parameter assumption is discharged only as an implication antecedent (Design 1), not by kernel `→I` (see §7).

---

## 6. Test Obligations

1. **Lambda end-to-end Proven.** `App(Lam "x" (Var x), Lit 42)` ⟹ `audited_type_check_tree` ⟹ `elaborate_tree` ⟹ `Proven` with `Authority::ProofKernel`, `KernelCertificate`, `ElaborationBoundary` edge to the Audited source; result type `Int`.
2. **Zonk correctness.** Assert the materialized lambda sub-tree's `dst` is `Atom(cfg(Fn(Int,Int)))` (not `Fn(α,α)`), i.e. the boundary zonk fired.
3. **Bare lambda.** `Lam "x" (Var x)` alone ⟹ `Audited` with result `Fn(α,α)` (α unresolved) — materialized tree is well-formed; documents that an open lambda's endpoints are the un-unified param type (elaboration to Proven of an open lambda carries the `g_var(p)` antecedent, still sound).
4. **No regression.** All ADR-0007 tests (`test_tree_elaboration_end_to_end` for `App(Var,Lit)`, the `elaborate_tree` unit tests) and the flat-path tests stay green. The flat `infer`/`type_check`/`audited_type_check` path is untouched.

---

## 7. Non-goals / Deferred

- **Design 2 — genuine `→I` discharge.** Mapping `Lam` to the kernel's `TermKind::Lam` to *discharge* the parameter hypothesis (fuller Curry-Howard) rather than leaving it an antecedent. Larger change to `elaborate_tree` (binder-aware); deferred. Design 1 is chosen for consistency with `g_split`/`g_app` and minimal surface.
- **Let / records / rows / patterns** (breadth → 14/14 ConflictKind parity vs `brix-ir`); tight ρ-audit (PD-1); Stage-3 deletion of `brix-ir`/`brix-ast`/`brix-diag`. Unchanged trajectory.

---

## 8. Implementation Order

1. `soc-regimes`: `CfgAtom`, `TyObj`, `TyTree` + `materialize`/`materialize_obj`.
2. Refactor `infer_tree` (`{Lit, Var, App}`) to build `TyTree` (defer configs); add the `Lam` case + `g_lam_intro`/`g_lam_close`.
3. `audited_type_check_tree`: `infer_tree` → `materialize(final_subst)` → existing `well_formed` guard → `Audited` judgement (witness = `tree.witness_object().witness_digest()` over the materialized tree).
4. Tests §6. Full workspace `cargo fmt --all --check` + `clippy -D warnings` + `cargo test --workspace` green.

Soundness re-reviewed by Fable before merge: single-final-subst zonk consistency, lambda middle-match, honest lax `Proven`-as-implication, no regression.
