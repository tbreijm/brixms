# The Brix Type-Realization Contract

Status: **Normative** (2026-08-16). The contract [SOC-LAW-02](./SOC_Semantic_Laws.md#soc-law-02--realization-compositionality) names as outstanding under issue #53. Defines the canonical inputs, contexts, primitive generators, derivation artifacts, conflicts, grades, discharge obligations, and public results of the native typing regime.

Governing decisions: [ADR-0002](./adr/ADR-0002_SOC_Constitution.md) (§4.1 authority, §5.3 fail closed, §10 PD-1), [ADR-0007](./adr/ADR-0007_Tree_Structured_Typing_Elaboration.md) (tree derivations, no padding), [ADR-0008](./adr/ADR-0008_Lambda_In_Tree_Elaboration.md), [ADR-0010](./adr/ADR-0010_SOC_Language_Design.md) (the L2 fragment), [ADR-0013](./adr/ADR-0013_Canonical_Certificate_Envelope.md), [ADR-0015](./adr/ADR-0015_Judgment_Scoped_Tightness.md) (⟨D-JUDGE⟩, ⟨D-PRIM⟩, ⟨D-SPLIT⟩), [ADR-0023](./adr/ADR-0023_Primitive_Relation_Identity.md), [ADR-0025](./adr/ADR-0025_Pinned_Endpoint_Identities.md).

Implementation: `crates/soc-regimes/src/type_realization.rs`, `crates/soc-regimes/src/tree_audit.rs`, `crates/brix-elaborate`, `crates/brix-kernel`.

---

## 0. How to read this document

Every normative clause carries an **evidence status**. The status is part of the
clause, not commentary on it: a clause that is specified but unimplemented is
still binding on whoever implements it, and must not be read as describing
current behaviour.

| Marker | Meaning |
|---|---|
| **[Pinned: `test`]** | Implemented, and a named test fails if the clause is violated. |
| **[Partial: …]** | Implemented, but the pin is weaker than the clause — the gap is stated. |
| **[Specified]** | Normative, **no implementation exists**. Binding on the implementation when it lands. |

This grading exists because the failure mode this project keeps hitting is a
document or a test that reads as coverage it does not have. Two instances were
found while writing the surrounding work: a padded-step test that ran over one
expression while claiming to guard all inference (#287), and an exhaustiveness
assertion that measured a literal against a number written beside it (#299). A
clause with no pin is more honest than a clause with a pin that cannot fail.

**This document defines. It does not discharge.** Nothing here makes a
generator tight, moves a grade, or authorizes a proof. §9 states what a
discharge requires; the discharges themselves live in code and in the ADRs.

---

## 1. Canonical typing inputs

**1.1** A typing input is a pair `(e, Γ)` where `e` is an `Expr` and `Γ` a
`TyCtx`, both canonical under `brix-canon`. **[Partial: `Expr::config_id`/`Ty::config_id` exist and every derivation endpoint is built from them, but no vector freezes an `Expr` or `Ty` encoding — `vectors/` has no expression vector. A re-encoding change would be caught only where it happens to move a frozen arithmetic or certificate digest.]**

**1.2** `Expr` is the lowered L2 fragment, not a surface AST and not a revived
Core IR. Lowering from surface syntax is `brix-lower`'s; this contract begins
at `Expr`. **[Pinned: the `brix-lower` L2 suite]**

**1.3** Every `Expr` and `Ty` constructor SHALL carry an append-only canonical
ordinal. A new constructor takes the next unused ordinal; no existing ordinal is
renumbered or reused. **[Specified]** — the `ordinal()` methods implement it and
their doc comments state it, but **nothing tests it**: no frozen vector covers
`Expr` or `Ty`, so renumbering an ordinal would pass CI. Given #296–#298 added
four constructors in a week, this is the most load-bearing unpinned clause in
the document.

**1.4** A configuration identity is the `ConfigId` of the canonical encoding.
Two inputs with equal `ConfigId` are the same input for every purpose in this
contract, including cache and invalidation identity. **[Partial:
`arith_leaf_round_trips_every_material_field` pins it for the arithmetic source
object, and the derivation gates compare endpoint `ConfigId`s throughout. There
is no general round-trip test over `Expr`/`Ty`, so 1.4 rests on the same
unpinned encoding as 1.1 and 1.3.]**

**1.5** Type variables (`Ty::Var`) are inference-internal. A *published*
judgement SHALL NOT contain an unresolved `Ty::Var`: the inferred type is zonked
before the result is formed. **[Partial: zonking is applied at
`audited_type_check_tree`; no test asserts the absence of `Ty::Var` in a
published `Judgement` for every path.]**

---

## 2. Contexts

**2.1 Identity.** `TyCtx` is a `BTreeMap<String, Ty>`, so context identity is
order-independent and duplicate-free by construction. Two contexts with the same
bindings are the same context. **[Pinned: `TyCtx` is a `BTreeMap`; ordering
cannot vary]**

**2.2 Assumptions.** `Γ.extend(x, T)` produces a new context; it does not mutate
the receiver. Shadowing replaces the binding for `x` and leaves every other
binding unchanged. **[Partial: `extend` takes `&self` and returns a new `TyCtx`,
so non-mutation is structural. No test exercises *shadowing* — rebinding a name
already in the context — on any path.]**

**2.3 Substitution.** Unification is over an explicit immutable
`BTreeMap<u32, Ty>`. `unify` returns the updated substitution and performs no
mutation and no hashing inside its loop. **[Pinned: `unify`'s signature; the
occurs-check tests]**

**2.4 Occurs check.** A substitution SHALL NOT bind a variable to a type
containing it. Violation is `TypeError::InfiniteType`, never a panic and never a
judgement. **[Pinned: `self_application_is_rejected_by_the_occurs_check`]**

**2.5 Generalization.** The L2 fragment is **monomorphic**: there is no
generalization boundary, no type scheme, and no instantiation. A `let` does not
generalize. **[Pinned: by absence — no scheme type exists]**

> This is a real restriction and is stated so that adding polymorphism is
> recognised as a contract extension under §12 rather than an implementation
> detail. It requires new generators for instantiation and generalization, and
> each needs its own discharge story.

**2.6 Transport.** A judgement is indexed by a `ContextId` (#59). A derivation
established under one `ContextId` SHALL NOT be reused under another without an
explicit transport step. No transport step exists today. **[Specified]**

---

## 3. The primitive typing generators

**3.1** The generator set `𝒢` is exactly the enumeration in
`minted_generators()`. That single enumeration feeds `generator_name`,
`typing_registry`, and `is_minted_generator`; there SHALL NOT be a second list.
**[Pinned: all three derive from it]**

**3.2** A generator declared in `𝒢` but emittable by no code path is **drift**
and SHALL be removed. `g_app`, `g_lam`, and `g_unify` were removed on this
ground. **[Pinned: `the_retired_generators_are_not_declared`]**

**3.3** A leaf SHALL cite a generator in `𝒢`. A leaf citing anything else is
`TreeAuditError::UnmintedGenerator`. **[Pinned: `audit_tree`; the settlement
analogue is `registry.contains` in `audit_step`]**

**3.4 The table.** Each generator's exact source and target realization
relation. `Atom(E …)` is an expression atom, `Atom(T …)` a type atom,
`Prod(…)` a right-nested product. `T` is the tightness status for
`ClaimKind::Typing`.

| Generator | `src` → `dst` | T | Ground |
|---|---|:-:|---|
| `g_lit` | `Atom(E Lit(n))` → `Atom(T Int)` | ✓ | literal introduction |
| `g_str_lit` | `Atom(E StrLit(s))` → `Atom(T Str)` | ✓ | literal introduction |
| `g_float_lit` | `Atom(E FloatLit(s))` → `Atom(T Float)` | ✓ | literal introduction |
| `g_bool_lit` | `Atom(E BoolLit(b))` → `Atom(T Bool)` | ✓ | literal introduction |
| `g_var` | `Atom(E Var(x))` → `Atom(T Γ(x))` | ✓ | kernel `Hyp` |
| `g_lam_intro` | `Atom(E Lam(p,b))` → `Atom(E b)` | ✓ | kernel `Lam` (→I) |
| `g_lam_close` | `Atom(T tb)` → `Atom(T fn_ty)` | ✓ | kernel `Lam` (→I) |
| `g_split` | `Atom(E App(f,x))` → `Prod(Atom(E f), Atom(E x))` | ✓ | product elimination |
| `g_app2` | `Prod(Atom(T fn), Atom(T arg))` → `Atom(T b)` | ✓ | kernel `App` (→E, modus ponens) |
| `g_record_split` | `Atom(E rec)` → `Prod(Atom(E fᵢ)…)` | ✓ | structural packaging |
| `g_record` | `Prod(Atom(T tᵢ)…)` → `Atom(T Record)` | ✓ | product introduction |
| `g_record_empty` | `Atom(E {})` → `Atom(T Record([]))` | ✓ | zero-premise introduction |
| `g_field_split` | `Atom(E Field(b,f))` → `Atom(E b)` | ✓ | structural packaging |
| `g_field` | `Atom(T base)` → `Atom(T t_f)` | ✓ | product projection |
| `g_ctor_split` | `Atom(E Ctor)` → `Prod(Atom(E aᵢ)…)` | ✓ | structural packaging |
| `g_ctor` | `Prod(Atom(T tᵢ)…)` → `Atom(T sum)` | ✓ | coproduct introduction |
| `g_ctor_nullary` | `Atom(E Ctor(S,v,[]))` → `Atom(T S)` | ✓ | zero-premise introduction |
| `g_match_split` | `Atom(E Match)` → `Prod(Atom(E partᵢ)…)` | ✓ | structural packaging |
| `g_match` | `Prod(Atom(T armᵢ)…)` → `Atom(T result)` | ✓ | coproduct elimination |
| `g_cmp_split` | `Atom(E Cmp(op,a,b))` → `Prod(Atom(E a), Atom(E b))` | ✓ | structural packaging |
| `g_cmp` | `Prod(Atom(T t), Atom(T t))` → `Atom(T Bool)` | ✗ | — |
| `g_arith_split` | `Atom(E Arith(op,a,b))` → `Prod(Atom(E a), Atom(E b))` | ✓ | ⟨D-SPLIT⟩, conditional |
| `g_arith_input` | `Prod(Atom(T a), Atom(T b))` → `Atom(ArithInput)` | ✗ | regime→kernel bridge |
| `g_arith` | `Atom(ArithInput)` → `Atom(ArithResult)` | ✗ | kernel-checked, not yet closed |
| `g_arith_result` | `Atom(ArithResult)` → `Atom(T result)` | ✗ | kernel→regime bridge |
| `g_match_catchall` | `Prod(Atom(T armᵢ)…)` → `Atom(T result)` | ✗ | repeated branch premises unrepresented |
| `NUMERIC` / `GRADE` edges | coercion-path data, **not leaves** | n/a | ⟨D-EXACTCOVERED⟩ |

**[Pinned: `zero_arity_intro_generators_are_faithful`,
`literal_intro_generators_are_faithful`,
`structural_generators_are_faithful_kernel_rules`,
`application_rule_is_a_kernel_theorem`,
`arithmetic_split_rule_is_a_kernel_primitive`,
`claim_kind_typing_discharge_is_not_portable` (which derives the tight set from
`minted_generators()` and so cannot silently omit a row).]**

**3.5** The coercion families are **data carried inside a source object**, not
tree leaves. No coercion generator reaches a leaf, so `generator_is_tight` is
never consulted for one and a per-edge discharge would cap nothing (ADR-0024
⟨D-EXACTCOVERED⟩). **[Pinned: `promote_generator` has exactly two non-test call
sites, neither producing a leaf]**

**3.6** An edge's generator family SHALL be derived from its declared
exactness, so an id and its `CoercionKind` cannot disagree. **[Pinned:
`the_edge_family_follows_declared_exactness`]**

---

## 4. Derivation trees

**4.1 Shape.** A derivation is a `TyTree` of `Leaf`, `Seq`, and `Tensor`.
Materialization to a `RealizesTree` resolves deferred substitutions (ADR-0008).

**4.2 Middle-match.** For every `Seq{l, r}`, `l.dst() == r.src()` under
canonical structural equality. A mismatch is `TreeAuditError::MalformedTree` and
MUST NOT be silently padded. **[Pinned: `audit_tree`; `elaborate_tree` rejects
with `NotElaborated`]**

**4.3 No padding.** No leaf SHALL have `src == dst`. Faking an intermediate
configuration passes syntactic `RealizesComp` — a padded middle `dst ≡ dst`
always matches — and fails a sound audit, because no generator realizes
`(x, x)` (ADR-0007 §1). **[Pinned: `tree_derivation_carries_no_padded_step`,
over a corpus rather than a single expression. The corpus is the evidence and
its adequacy is a standing obligation, not a fixed number — see §12.3.]**

> This clause is stated separately from 4.2 because 4.2 cannot catch it: a
> padded step *satisfies* the middle-match. Both zero-arity branches violated
> 4.3 while satisfying 4.2 until #287, under generators that were discharged
> tight. The corpus is the evidence, and §12.3 requires extending it.

**4.4 Leaf ordering.** `Tensor` children are in source order; `Prod` endpoints
are right-nested. Ordering is observable and part of the derivation's identity.
**[Pinned: `right_nest_prod` / `right_nest_tensor`; the endpoint assertions in
the faithfulness gates]**

**4.5 Zero-arity shape.** Where an expression has no subexpressions to
decompose, the derivation SHALL be the single leaf carrying the claim, with no
split. **[Pinned: `the_zero_arity_branches_emit_no_split`]**

**4.6 Endpoints.** A derivation's endpoints SHALL be the configurations the
claim is about, supplied by the caller rather than read off the tree. A checker
that took the derivation's word for its own endpoints would check nothing.
Mismatch is `TreeAuditError::EndpointMismatch`. **[Pinned: `audit_tree`]**

**4.7 Final proposition.** `HasType(e, T)` is represented as
`Realizes(w, cfg(e), cfg(T))` where `w` is the tree's composite witness. That
proposition's identity is the published judgement's identity. **[Pinned:
`audited_type_check_tree`]**

---

## 5. Conflicts and negative evidence

**5.1** Failed or incomplete inference SHALL NOT produce `Refuted`. Absence of a
derivation is absence, never refutation (ADR-0015 §8.8). **[Pinned: no code path
constructs `Outcome::Refuted` in this regime]**

**5.2 The taxonomy.** The negative outcomes are distinct and SHALL remain
distinguishable in the public result:

| Case | Carrier | Status |
|---|---|---|
| Type error | `TypeError::Mismatch` | **[Pinned: `test_ctor_arg_type_mismatch`]** |
| Unbound variable | `TypeError::Unbound` | **[Pinned: `unbound_var_is_a_type_error`]** |
| Infinite type / occurs | `TypeError::InfiniteType` | **[Pinned: `self_application_is_rejected_by_the_occurs_check`]** |
| Missing field | `TypeError::NoField` | **[Pinned: `test_field_access_missing_field_type_error`]** |
| Non-exhaustive match | `TypeError::NonExhaustive` | **[Pinned: `test_sum_match_non_exhaustive`]** |
| Unsupported syntax | `TypeError::Unsupported` | **[Pinned: `brix-lower` `Unsupported` tests]** |
| Ill-formed derivation | `TypeError::IllFormedDerivation` | **[Partial: constructed, but no fixture drives a real ill-formed tree through the public entry point]** |
| Context mismatch | `TreeAuditError::EndpointMismatch` | **[Partial: pinned at `audit_tree`; not surfaced as a distinct public negative outcome]** |
| Ambiguity | — | **[Specified]** — no carrier exists. Monomorphic L2 has no ambiguous inference today; a carrier is required before any feature that can produce one. |
| Certified refutation | — | **[Specified]** — requires a complete fragment declaration *and* a kernel-accepted refutation certificate. Neither exists. Until both do, untypability is `Unknown`, never `Refuted`. |

**5.3** The 14/14 conflict corpus lives in `brix-conformance` (#210), not here.
This contract governs the *taxonomy* and its carriers; that corpus governs
structural conflict analysis. Neither duplicates the other. **[Pinned: by
location]**

**5.4** Exhaustion (resource budget) is a statement about the search, not about
the program, and SHALL be distinguishable from every case in 5.2. **[Partial:
`brix-elaborate` returns `ElaborationResult::NotElaborated(verdict)` carrying the
kernel's `ResourceBudget` verdict, so the information survives; but it is not
mapped to a distinct typing-level negative outcome, and no fixture drives a
budget exhaustion through the public entry point.]**

---

## 6. Annotations, grades, and coercion coherence

**6.1** An annotation is *checked*, never trusted. A declared type is a
contract. **[Pinned: the `brix-lower` declared-type suite (#295)]**

**6.2 Weakening is permitted; strengthening is not.** A grade assertion is
satisfied by an actual grade at or below the asserted one. Grades move **down,
never up**. **[Pinned: `grade_assertion_satisfied`,
`grade_assertion_satisfied_and_downgrade_ok`, `grade_assertion_on_let_type_rejects`]**

**6.3 No erasure.** A weakened grade SHALL NOT erase the evidence that produced
it; the judgement and its evidence identity survive weakening. **[Partial: the
`Judgement` carries `EvidenceId` through weakening, but no test asserts
non-erasure across a weakening step.]**

**6.4 Coercion coherence.** A promotion path is an ordered sequence of
coercion-edge ids, each carrying its declared exactness. Exactness is bound into
the source object's canonical bytes, so a relation can never accept a lossy path
where an exact one was claimed. **[Pinned: the `ArithTypingInputV1` vectors;
`no_current_row_names_the_lossy_edge_as_a_promotion`]**

> ADR-0015 §5 Stage B0 originally said "exact promotion-edge ids"; that wording
> carries an inline erratum. Reading it strictly would have made integer
> division undischargeable forever.

**6.5** A lossy edge SHALL NOT be named under a promotion family. **[Pinned:
`the_edge_family_follows_declared_exactness`]**

---

## 7. Determinism

**7.1** The same canonical `(e, Γ)` SHALL produce the same inferred type, the
same derivation tree, the same conflicts, and the same observable ordering.
**[Pinned: the CI `determinism` job; `BTreeMap`/`BTreeSet` throughout, with
`HashMap`/`HashSet` denied by `clippy.toml` in semantic paths]**

**7.2** No wall-clock, RNG, address, or iteration-order dependence in any path
that contributes to a configuration, derivation, or judgement identity.
**[Pinned: the `reproducibility` job]**

---

## 8. Incremental invalidation

**Status: [Specified] throughout.** SOC-LAW-09 records "future invalidation
engine"; none exists. Every clause here is binding on that engine and describes
nothing that runs today.

**8.1** An edit SHALL invalidate exactly the derivations and certificates that
depend on what changed — no more, no fewer. Over-invalidation is a performance
defect; **under-invalidation is a soundness defect**, because a stale `Proven`
survives the fact that earned it.

**8.2 Dependency identity.** A derivation depends on: the `ConfigId` of its
expression, the `ContextId` of its context, the `ConfigId` of every config
declaration it resolved, the `GeneratorId` of every leaf, and — for a closed
leaf — the `PrimitiveRelationId` it was checked against.

**8.3** Because those identities are content-derived, an edit that changes
nothing observable changes no identity and invalidates nothing. This is a
consequence of ⟨D-RELID⟩ rather than a separate mechanism.

**8.4 Generator revision.** Revising a generator's *semantics* SHALL mint a new
`GeneratorId`, not redefine an existing one. Every derivation citing the old id
remains valid for what it claimed; nothing is silently reinterpreted.

**8.5 Relation revision.** Adding, removing, or changing a row allocates a new
`PrimitiveRelationId` by construction (⟨D-RELID⟩). A retired relation's id
resolves to `None`, which fails closed. **[Pinned: `the_retired_relation_does_not_resolve`
— the one clause in §8 with an implementation, because relation identity landed
ahead of the engine]**

**8.6** Invalidation SHALL NOT upgrade a grade. Re-deriving after an edit may
lower a grade or leave it unchanged; it may raise one only by producing new
evidence that independently satisfies §9.

---

## 9. The discharge artifact

**9.1** A Boolean whitelist entry alone is **not** a discharge. **[Pinned:
ADR-0015 §5 Stage D states it; `generator_is_tight`'s doc carries it]**

**9.2** Discharging a generator for `ClaimKind::Typing` requires **all** of:

1. **A stated ground** in one of the recognised families — literal/zero-premise
   introduction, correspondence to a primitive kernel rule, or ⟨D-SPLIT⟩
   structural decomposition — naming which, and why the other families' tests
   do not apply.
2. **A faithfulness pin**: a test that fails if the emission stops matching the
   ground. Exhaustive where the instance set is finite; an explicitly-labelled
   property where it is not.
3. **A negative pin**: a test that the generator is *not* emitted where its
   precondition fails.
4. **Judgment scoping**: tight for `Typing`, not tight for any other
   `ClaimKind`. **[Pinned: `claim_kind_typing_discharge_is_not_portable`]**

**9.3** A discharge is **conditional on its ground continuing to hold**. If a
generator later acquires work its ground does not cover, the discharge lapses
and SHALL be withdrawn. ⟨D-SPLIT⟩ states this for `g_arith_split`; it
generalises. **[Pinned: `arithmetic_split_rule_is_a_kernel_primitive`]**

**9.4** The strongest available ground SHALL be preferred. Where ⟨D-PRIM⟩'s row
mechanism can reach a generator, a prose discharge is an interim, and the
document recording it SHALL say so. `g_record_empty` is such an interim: ADR-0025
⟨D-PINNED⟩ makes a one-row relation available for it. **[Partial: recorded in
`g_record_empty`'s doc; not yet implemented]**

**9.5 Kernel-closed leaves.** After ⟨D-PRIM⟩, a leaf is closed only when the
certificate contains the `PrimRealizes` term for that leaf *and* the kernel
accepted the resulting proof. Registry membership implies nothing about
occurrences (ADR-0015 §8.7). `elaborate_tree` still emits every leaf as a
`Hyp`, so no leaf is closed this way today. **[Specified]**

**9.6** The result grade is the composition outcome capped by the
least-discharged leaf. **[Pinned: `honest_result_outcome`, and the standing
`1 + 2` / `7 / 2` assertions]**

---

## 10. The public result

**10.1** The public result SHALL retain the exact `HasType` judgement and its
evidence identity — not only a displayed grade. **[Pinned:
`audited_type_check_tree` returns `(Judgement, TreeDerivation)`]**

**10.2** A grade SHALL NOT be rendered against an ambiguous proposition.
`1 + 2 : Int @Proven` is honest; a bare `1 + 2 @Proven` is not, because a
reasonable reader takes it to mean the expression evaluates correctly
(⟨D-JUDGE⟩). **[Pinned: the CLI renders the proposition; `brix why` distinguishes
provenance from proof]**

**10.3** A typing result SHALL NOT imply an evaluation result. Discharging a
typing rule discharges `HasType` and nothing else — not evaluation, value
equality, totality, progress, or termination. **[Pinned: ⟨D-JUDGE⟩; no
`EvaluatesTo` judgement exists]**

**10.4** Where a kernel certificate exists, the result SHALL carry it. Where one
does not, the result SHALL NOT imply one. **[Pinned: `ElaborationResult`]**

---

## 11. Current honest position

Stated so the contract cannot be read as claiming more than holds. Verified
against `main` at the date above.

- The λ-calculus core, the structural product/coproduct fragment, literals, and
  the two zero-arity introductions reach genuine `Proven`.
- **Arithmetic is capped.** `1 + 2` is `Int @Audited` and `7 / 2` is
  `Float @Audited`, capped by `g_arith_input`, `g_arith`, and `g_arith_result`.
  `g_arith`'s realization *is* decided by a kernel relation (`TypingArithV2`),
  but no certificate closes the leaf with it yet, and the two bridges are not
  dischargeable by that mechanism.
- **Comparison is capped** by `g_cmp`.
- **Catch-all matching is capped** by `g_match_catchall`, deliberately: ADR-0015
  §4 lists it as a non-goal until repeated branch premises are represented.
- **Incremental invalidation does not exist** (§8).
- **Certified refutation does not exist** (§5.2).

---

## 12. Extension

**12.1** New L2 features extend this contract **append-only**: new generators,
new ordinals, new clauses. No existing generator id, ordinal, or clause number
is reused for something else.

**12.2** A new generator SHALL arrive with its row in §3.4 and either a
discharge meeting §9.2 or an explicit statement that it is undischarged and what
it caps.

**12.3** A new **type former** obliges the corpora that quantify over types to
gain the new case. `Ty::Rec` arrived in #298 and the zero-arity gates did not
follow until #301 — the assertions were right, the set they quantified over had
stopped matching the set that exists. This is the same failure as §0's, and it
recurs because nothing forces the corpus to track the language.

**12.4** A clause SHALL NOT be promoted from **[Specified]** to **[Pinned]**
without naming the test that fails when it is violated, and confirming that it
does fail — not assuming it would.
