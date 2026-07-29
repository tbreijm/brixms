# ADR-0009 — Native Type Checker (Parity-First) & the Path to Zero Legacy

Status: **Accepted** (2026-07-30, ratified by user) (governs `soc-regimes`; drives the eventual deletion of `brix-ir`/`brix-ast`/`brix-diag`).

Date: 2026-07-30.

Foundation documents: [ADR-0005: Type Inference as Realization](./ADR-0005_Type_Inference_as_Realization.md), [ADR-0007](./ADR-0007_Tree_Structured_Typing_Elaboration.md), [ADR-0008](./ADR-0008_Lambda_In_Tree_Elaboration.md). This ADR pins the **parity-first** strategy — build a *native* type checker in `soc-regimes` that detects the same type conflicts as `brix_ir::reflect::analyze`, differentially, so `brix-ir`/`brix-ast`/`brix-diag` can be **deleted entirely** (Stage 3, ADR-0005). It is the roadmap ADR for a multi-slice arc; each slice gets pinned as it is built.

---

## 1. Context & Decision

The tree-elaboration work (ADR-0005/0007/0008) built the **positive path**: well-typed programs in a tiny lambda calculus (`Lit/Var/App/Lam`) elaborate to `Proven`. Retiring `brix-ir` needs the **negative path**: detecting type *conflicts* on the rich real language, matching `brix-ir`.

Two facts set the scope:
- The parity corpus (`brix_conformance::typecorpus`) fixtures are `brix_ir::frontend::FrontendSource` — a relational/query language (`Query`, `Rule`, `Call`, `Field`, `Record`, `If`, `Try`, `Comprehension`, `Let`; `Ty` with ~40 constructs incl. dimensions, `Result`/`Option`, records/rows, epistemic `Estimate`/`Probability`).
- `structural.rs` today does **not check** — it calls `brix_ir::reflect::analyze` and *projects its output* into SOC artifacts. So `brix-ir` is a live runtime dependency of the type path.

**Decision (user, 2026-07-30): NATIVE MIRROR → true zero-legacy.** `soc-regimes` grows its **own** input syntax (`Expr`/`Ty`/`Query`/`Rule`/…) mirroring the constructs, a one-time `translate(FrontendSource) -> Option<native>` used only by the differential harness, and a native `analyze` producing SOC conflict outcomes. When the native checker reaches **14/14 `ConflictKind` parity** over the full corpus, Stage 3 deletes `brix-ir`/`brix-ast`/`brix-diag`. This is chosen over "replace `reflect` only" (which would keep `brix_ir::{frontend,core,types}` as a permanent input ABI — only partial retirement).

---

## 2. The Parity Target

`brix_ir::reflect::ConflictKind` has 14 variants on two axes (`brix-conformance/tests/type_parity.rs`):

**Type-inference axis (8)** — have `infer.rs` counterparts, mirrored via `Category`:
`Mismatch`, `Arity`, `UnknownField`, `NonBool`, `Occurs`, `Dimension`, `TryNonResult`, `EpistemicErasure`.

**Rule-side-condition axis (6)** — Appendix-E, from `brix_ir::check::check_rule` (not type inference), mirrored via `RuleCategory`:
`ImpureRule`, `NondeterministicRule`, `DivergentRule`, `UnboundHeadKey`, `MaskRefNotEdgeBound`, `OrdinaryFnOnDerivedRel`.

The frozen parity contract to reproduce natively (per `type_parity.rs`): **verdict equivalence** (consistent ⟺ no conflicts) + **category-set equivalence** (conflicts map to the same `Category` *set*) + **type mirror** (each zonked `Expr.ty` has a matching `HasType`).

---

## 3. Differential-During-Transition Discipline (INVARIANT for every slice)

- `brix-ir` stays the **differential oracle** during the whole arc (per ADR-0005 / constitution). Each slice adds native coverage and asserts, per corpus fixture the native checker covers, that its conflict-`Category` set equals `brix_ir::reflect::analyze`'s.
- A **native differential harness** iterates the corpus: `translate` each fixture; if translatable **and** in the covered fragment, assert native-vs-`brix-ir` category-set parity; else record as *not-yet-covered*. A monotonic **coverage counter** must **strictly increase** each slice and never regress. The harness is GREEN throughout (uncovered fixtures are skipped, not failed).
- No fixture the native checker claims to cover may diverge from `brix-ir`. Divergence = red.
- Deletion (Stage 3) is gated on **coverage == full corpus AND 14/14 category parity**, at which point the harness stops skipping anything and `brix_ir::reflect` has no remaining caller.

---

## 4. Slice Roadmap (category-driven; each: Fable pins semantics → agy implements → Fable reviews → differential green, coverage up)

- **N1 — Foundation + `Mismatch`** (this ADR §5). Native `syntax` (core `Expr`/`Ty`), signature table, `translate` (partial), native `analyze` with unification, the differential harness. Differentially covers scalar `Mismatch` fixtures. **`Occurs` detection is built into the native unifier (a correct unifier needs occurs-check) and unit-tested, but its *differential* coverage defers to the container slice** — every real corpus `Occurs` fixture forces occurs *into a container* (`Option`/`Rel`), untranslatable by the scalar fragment; there is no scalar-only occurs case in the language. **Deliberately NOT patching the brix-ir oracle to manufacture one** (an early N1 draft did; reverted — the oracle's independence is the differential's whole value).
- **N2 — `Arity`.** Call/op arity checking.
- **N3 — `UnknownField`.** Records/rows (`Ty::Record(Row)`, `Field`, `Record`).
- **N4 — `NonBool`.** `If`/guards.
- **N5 — `Dimension`.** `Quantity`/`Money`/`Dimensioned` + dimensional algebra.
- **N6 — `TryNonResult`.** `Result`/`Option` + `Try`.
- **N7 — `EpistemicErasure`.** `Estimate`/`Probability` + the §19.1 erasure table.
- **N8 — Relational + rule side-conditions.** `Comprehension`/`Query`/`Rel`/`Rule`; the 6 `RuleCategory` conditions (`check_rule` mirror).
- **N9 — STAGE 3 deletion.** On full-corpus 14/14: delete `brix-ir`/`brix-ast`/`brix-diag`; drop the `translate` bridge and the `structural.rs` delegation; repoint any `.github/workflows/` nextest targets **first** (constitution lesson).

Ordering rationale: N1–N2 reuse the native unifier already built (ADR-0005/0008); N3–N7 each add one type-language feature cluster; N8 is the relational/rule axis; N9 is pure deletion. `Proven`-elaboration of well-typed cases (ADR-0007/0008) is **orthogonal and additive** — parity is a `Derived`/`Audited`-level *detection* contract, not a `Proven` obligation.

---

## 5. Slice N1 — Foundation + Mismatch + Occurs (pinned)

**Native syntax** (`crates/soc-regimes/src/native/syntax.rs`, new module): a `soc-regimes`-owned mirror covering the minimal fragment for `Mismatch`/`Occurs`:
- `NExpr = Lit(NLit) | Var(Sym) | Call { func: Sym, args: Vec<NExpr> }` (plus an `origin` id for the type-mirror).
- `NTy = Unit | Bool | Str | Int | F64 | Fn { params: Vec<NTy>, ret: Box<NTy> } | Var(u32) | Error` (the starter set; extended per later slice). `Error` unifies only with itself (mirrors `brix_ir::types::Ty::Error`'s isolation rule — do **not** make it a bindable `Var`).
- `NLit` carries enough to type literals (int/bool/str/unit).
- A **signature table** `SigTable: Sym -> NSig { params: Vec<NTy>, ret: NTy }` for `Call` resolution (mirrors `FnSignature`/`RelationSchema` name→signature lookup).

**`translate(&FrontendSource) -> Option<NativeSource>`** (`crates/soc-regimes/src/native/translate.rs`): map the supported `FrontendSource`/`Expr`/`Ty` fragment to native; **return `None`** for any construct not yet supported (records, dimensions, `If`, `Try`, comprehensions, rules, epistemic/collection types, …). Translation is total on the supported fragment and lossless for identity/category purposes. Build a `SigTable` from the source's `TableResolver`/`FnSignature`s.

**`analyze(&NativeSource) -> NativeReport { has_types: Vec<(Origin, NTy)>, conflicts: Vec<NConflict> }`** (`crates/soc-regimes/src/native/analyze.rs`): Hindley–Milner-style inference over `NExpr` using the existing declarative `unify` (ADR-0005/0008 algebra — reuse it, do not fork). Emit:
- `NConflict::Mismatch { left: NTy, right: NTy }` on a `Con`-vs-`Con` / structural unify failure (mirrors `ConflictKind::Mismatch`).
- `NConflict::Occurs { var, into }` on occurs-check failure (mirrors `ConflictKind::Occurs`).
On failure, the offending expression's type becomes `NTy::Error` (isolation) and inference continues (error-recovery, so multiple independent conflicts surface as a set — matching `reflect`'s set semantics).
- `NConflict -> Category` map (reuse `brix_conformance::typecorpus::Category`).

**Native differential harness** (`crates/brix-conformance/tests/native_type_parity.rs`, new): for every `typecorpus` `TypeFixture`:
1. `translate(&fixture.source)`. If `None`, `covered += 0` (skip, count as not-yet-covered).
2. Else run native `analyze` **and** `brix_ir::reflect::analyze` on the same source; assert **native conflict-`Category` set == the `Category` set the parity harness already derives from `brix_ir`** (restricted, for N1, to fixtures whose full brix-ir category set ⊆ {`Mismatch`} — the ones N1 fully covers; a fixture with any other category is *not-yet-covered* even if translatable). The native oracle (`brix-ir`) is **never modified** to make a fixture cover.
3. Assert the **coverage count ≥ a pinned floor** (so future slices cannot silently regress it) and print it.

**Scope note.** N1 does **not** touch `structural.rs`, the flat/tree paths, or `Proven`. It adds a parallel native `analyze` + harness. `brix-ir` remains the oracle. No `.github/workflows/` change (new test binary `native_type_parity` is additive — but confirm CI's nextest filters don't need it named to run; if the acceptance/parity job filters by binary, add it).

**Test obligations N1:** ≥1 fixture each of `Mismatch` and `Occurs` covered and parity-green; a translatable well-typed fixture yields **no** conflicts and correct `HasType`s; `translate` returns `None` (not a panic) on an unsupported construct; coverage floor asserted.

---

## 6. Non-goals

- No `Proven` obligation for the negative path (parity is detection-level).
- No deletion until N9's full-corpus gate.
- No fork of the unification algebra — reuse the ADR-0005/0008 `unify`.
- N1 adds only the `Mismatch`/`Occurs` fragment; the other 12 `ConflictKind`s are later slices.

Each slice is soundness/parity-reviewed by Fable before merge; the differential harness is the objective gate.
