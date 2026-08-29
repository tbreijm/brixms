# The Brix Type-Realization Contract

Status: **Normative** (2026-08-23). The contract [SOC-LAW-02](./SOC_Semantic_Laws.md#soc-law-02--realization-compositionality) names as outstanding under issue #53. Defines the canonical inputs, contexts, primitive generators, derivation artifacts, conflicts, grades, discharge obligations, and public results of the native typing regime.

Governing decisions: [ADR-0002](./adr/ADR-0002_SOC_Constitution.md) (§4.1 authority, §5.3 fail closed, §10 PD-1), [ADR-0007](./adr/ADR-0007_Tree_Structured_Typing_Elaboration.md) (tree derivations, no padding), [ADR-0008](./adr/ADR-0008_Lambda_In_Tree_Elaboration.md), [ADR-0010](./adr/ADR-0010_SOC_Language_Design.md) (the L2 fragment), [ADR-0013](./adr/ADR-0013_Canonical_Certificate_Envelope.md), [ADR-0015](./adr/ADR-0015_Judgment_Scoped_Tightness.md) (⟨D-JUDGE⟩, ⟨D-PRIM⟩, ⟨D-SPLIT⟩), [ADR-0023](./adr/ADR-0023_Primitive_Relation_Identity.md), [ADR-0025](./adr/ADR-0025_Pinned_Endpoint_Identities.md), [ADR-0028](./adr/ADR-0028_Typing_Over_Context_Expression_Pairs.md) (⟨D-CTXENDPOINT⟩, ⟨D-CTXPUBLISH⟩).

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
`TyCtx`, both canonical under `brix-canon`. The pair is also the regime's
**object**, not merely its input: a derivation endpoint standing for an
expression is `CfgAtom::Judged { context, expr }`, carrying the `ContextId` the
expression is typed under (ADR-0028 ⟨D-CTXENDPOINT⟩). `CfgAtom::Expr`, the bare
expression atom, is retired from emission and kept only so a decoder meeting an
old artifact fails closed on an ordinal it knows rather than silently
reinterpreting one it does not. **[Partial: `Expr::config_id`/`Ty::config_id` exist and every derivation endpoint is built from them, and two manifests now freeze encodings — `vectors/pinned_endpoint_identities_v1.json` (ADR-0025 Stage A: the six numeric `Ty::Con` atoms and the four `ArithOp` atoms) and `vectors/constructor_ordinals_v1.json` (§1.3: one exemplar per constructor, all twenty). Between them every constructor's *shape* is frozen. What is still unpinned is the encoding of value material no exemplar reaches: the `Expr::Record` exemplar carries a single field, so a change to how a multi-field record orders or delimits its fields moves no frozen digest. `TyCtx` itself has no vector.]**

**1.2** `Expr` is the lowered L2 fragment, not a surface AST and not a revived
Core IR. Lowering from surface syntax is `brix-lower`'s; this contract begins
at `Expr`. **[Pinned: the `brix-lower` L2 suite]**

**1.3** Every `Expr` and `Ty` constructor SHALL carry an append-only canonical
ordinal. A new constructor takes the next unused ordinal; no existing ordinal is
renumbered or reused. **[Pinned: `constructor_vectors_are_frozen` and
`every_digest_is_reproduced_by_primitive_canon_writes`, over all twenty-six
constructors — 17 `Expr` and 9 `Ty` — with
`vectors/constructor_ordinals_v1.json`]**

> This clause was `[Specified]` when this document was written — the `ordinal()`
> methods and their doc comments asserted it and nothing tested it, so
> renumbering `Expr::Match` would have passed CI clean. ADR-0025 Stage A then
> narrowed it, pinning ten specific *values* with re-derivation tests; that
> caught any renumbering those ten happened to exercise, and left `Expr::Match`,
> `Expr::Lam`, `Ty::Fn` and `Ty::Rec` unguarded. Closing it needed a vector over
> the **constructor set** rather than over values that happen to use some of it.
> Verified to fail now: renumbering `Expr::Match` 10 → 13 fails both consumers,
> the second naming the constructor.
>
> **The residual gap, stated rather than glossed.** Adding a constructor breaks
> the exhaustive `match` in the vector test and so fails to *compile* — the
> strongest guard available here. But a developer who writes the arm and omits
> the exemplar gets a passing suite. §12.3 carries that obligation; nothing
> mechanically enforces it.

**1.4** A configuration identity is the `ConfigId` of the canonical encoding.
Two inputs with equal `ConfigId` are the same input for every purpose in this
contract, including cache and invalidation identity. **[Partial:
`arith_leaf_round_trips_every_material_field` pins it for the arithmetic source
object, and the derivation gates compare endpoint `ConfigId`s throughout. There
is no general round-trip test over `Expr`/`Ty`. 1.3 is now pinned and 1.1 is
pinned for every constructor's shape, so what 1.4 still rests on is narrower
than it was: distinct inputs are known to receive distinct identities only where
a vector or a gate exercises them, not by a general injectivity argument.
`tests/derivation_differential.rs` narrows it over fourteen expressions,
pinning each one's **witness** id and **proposition** id separately — a change
to *which generators fire* moves the first, a change to *what they conclude*
moves the second, and keeping them apart is what made ADR-0028's migration
legible as an endpoint change rather than a behaviour change. It is a corpus,
not an injectivity proof.]**

**1.5** Type variables (`Ty::Var`) are inference-internal. A *published*
judgement SHALL NOT contain an unresolved `Ty::Var`: the inferred type is zonked
before the result is formed. **[Partial: zonking is applied at
`audited_type_check_tree`; no test asserts the absence of `Ty::Var` in a
published `Judgement` for every path.]**

---

## 2. Contexts

**2.1 Identity.** `TyCtx` is a `BTreeMap<String, Ty>` **plus** the `ContextId`
those bindings constitute. The map makes context identity order-independent and
duplicate-free by construction: two contexts with the same bindings are the same
context. The `ContextId` is carried inside `TyCtx` rather than passed alongside
it, so it cannot drift from the bindings it names — every `extend` moves both or
neither. **[Pinned: `TyCtx` is a `BTreeMap`, so ordering cannot vary; the
`context` field is private and only `extend` writes it]**

**2.2 Assumptions.** `Γ.extend(x, T)` produces a new context; it does not mutate
the receiver. Shadowing replaces the binding for `x` and leaves every other
binding unchanged. The new context's identity is the parent's digest hashed with
the assumption's canonical bytes, so `x : Int` and `x : Str` are different
scopes rather than one scope under a different name. **[Partial: `extend` takes
`&self` and returns a new `TyCtx`, so non-mutation is structural. No test
exercises *shadowing* — rebinding a name already in the context — on any path,
and the identity consequence of shadowing is therefore also unexercised.]**

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

> The first sentence is now true of the implementation as well — see 2.7 — but
> transport itself is untouched. 2.7 gives a judgement the identity of its
> scope; it does not say what a scope *contains*, how one moves between scopes,
> or how a scope is confined. ADR-0028 §5 leaves all three to #59 by name, and
> this clause stays binding on whoever lands them.

**2.7 Publication.** A judgement SHALL carry the `ContextId` its derivation
actually ran under. `ContextId::root()` — ADR-0002 §6.1's "empty assumptions" —
SHALL be published only by a derivation that genuinely assumed nothing
(ADR-0028 ⟨D-CTXPUBLISH⟩). The context flows *down* from the binder that created
it and is recorded; it is never reconstructed from a derivation's endpoints.
**[Pinned: `TyCtx` is the sole carrier — `audited_type_check_tree`'s separate
`context` parameter was retired in #321 precisely because a caller cannot know
what the binders below it assumed, so every call site passed the root]**

> Stated as its own clause because until #321 it was false, and 2.6's wording
> ("no transport step exists today") understated why that mattered. There was
> not only no transport: every typing judgement published the root while its
> derivation depended on real bindings, and `JudgementId` is documented as
> search-invariant — so two derivations differing only in what they assumed
> were indistinguishable at the judgement level. `ContextId::extend` existed
> and had exactly one caller in the workspace, and it was a test.

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
relation. `Atom(Γ ⊢ e)` is a **judged-expression** atom — `CfgAtom::Judged`,
the expression together with the identity of the context it is typed under
(§1.1) — `Atom(T …)` a type atom, `Atom(ArithInput)`/`Atom(ArithResult)` the
two kernel-owned objects, and `Prod(…)` a right-nested product. `d(a)` is the
derivation of `a`: `g_then_split` lands on wherever the left derivation
actually *starts*, not on the left operand's own atom, because a regime whose
derivations do not start at their own expression — an ordering witness starts
at a value — would otherwise get a `Seq` that is ill-formed for reasons having
nothing to do with the composition. `T` is the tightness status for
`ClaimKind::Typing`.

| Generator | `src` → `dst` | T | Ground |
|---|---|:-:|---|
| `g_lit` | `Atom(Γ ⊢ Lit(n))` → `Atom(T Int)` | ✓ | literal introduction |
| `g_str_lit` | `Atom(Γ ⊢ StrLit(s))` → `Atom(T Str)` | ✓ | literal introduction |
| `g_float_lit` | `Atom(Γ ⊢ FloatLit(s))` → `Atom(T Float)` | ✓ | literal introduction |
| `g_bool_lit` | `Atom(Γ ⊢ BoolLit(b))` → `Atom(T Bool)` | ✓ | literal introduction |
| `g_var` | `Atom(Γ ⊢ Var(x))` → `Atom(T Γ(x))` | ✓ | kernel `Hyp` |
| `g_lam_intro` | `Atom(Γ ⊢ Lam(p,b))` → `Atom(Γ,p:tₚ ⊢ b)` | ✓ | kernel `Lam` (→I) |
| `g_lam_close` | `Atom(T t_b)` → `Atom(T Fn(tₚ, t_b))` | ✓ | kernel `Lam` (→I) |
| `g_split` | `Atom(Γ ⊢ App(f,x))` → `Prod(Atom(Γ ⊢ f), Atom(Γ ⊢ x))` | ✓ | product elimination |
| `g_app2` | `Prod(Atom(T fn), Atom(T arg))` → `Atom(T b)` | ✓ | kernel `App` (→E, modus ponens) |
| `g_record_split` | `Atom(Γ ⊢ rec)` → `Prod(Atom(Γ ⊢ fᵢ)…)` | ✓ | structural packaging |
| `g_record` | `Prod(Atom(T tᵢ)…)` → `Atom(T Record)` | ✓ | product introduction |
| `g_record_empty` | `Atom(Γ ⊢ {})` → `Atom(T Record([]))` | ✓ | zero-premise introduction |
| `g_field_split` | `Atom(Γ ⊢ Field(b,f))` → `Atom(Γ ⊢ b)` | ✓ | structural packaging |
| `g_field` | `Atom(T base)` → `Atom(T t_f)` | ✓ | product projection |
| `g_ctor_split` | `Atom(Γ ⊢ Ctor)` → `Prod(Atom(Γ ⊢ aᵢ)…)` | ✓ | structural packaging |
| `g_ctor` | `Prod(Atom(T tᵢ)…)` → `Atom(T sum)` | ✓ | coproduct introduction |
| `g_ctor_nullary` | `Atom(Γ ⊢ Ctor(S,v,[]))` → `Atom(T S)` | ✓ | zero-premise introduction |
| `g_match_split` | `Atom(Γ ⊢ Match)` → `Prod(Atom(Γ ⊢ partᵢ)…)` | ✓ | structural packaging |
| `g_match` | `Prod(Atom(T armᵢ)…)` → `Atom(T result)` | ✓ | coproduct elimination |
| `g_cmp_split` | `Atom(Γ ⊢ Cmp(op,a,b))` → `Prod(Atom(Γ ⊢ a), Atom(Γ ⊢ b))` | ✓ | structural packaging |
| `g_cmp` | `Prod(Atom(T t), Atom(T t))` → `Atom(T Bool)` | ✗ | — |
| `g_then_split` | `Atom(Γ ⊢ Then(a,b))` → `d(a).src()` | ✓ | ⟨D-SPLIT⟩ — see 3.4.2 |
| `g_tensor_split` | `Atom(Γ ⊢ And(a,b))` → `Prod(Atom(Γ ⊢ a), Atom(Γ ⊢ b))` | ✓ | ⟨D-SPLIT⟩ — see 3.4.2 |
| `g_tensor` | `Prod(Atom(T tₐ), Atom(T t_b))` → `Atom(T Prod(tₐ,t_b))` | ✓ | product introduction — see 3.4.2 |
| `g_fix` | `Atom(Γ ⊢ Fix(f,T,b))` → `Atom(Γ,f:T ⊢ b)` | ✗ | assumes what it establishes |
| `g_arith_split` | `Atom(Γ ⊢ Arith(op,a,b))` → `Prod(Atom(Γ ⊢ a), Atom(Γ ⊢ b))` | ✓ | ⟨D-SPLIT⟩, conditional |
| `g_arith_input` | `Prod(Atom(T a), Atom(T b))` → `Atom(ArithInput)` | ✗ | regime→kernel bridge |
| `g_arith` | `Atom(ArithInput)` → `Atom(ArithResult)` | ✗ | kernel-checked, not yet closed |
| `g_arith_result` | `Atom(ArithResult)` → `Atom(T result)` | ✗ | kernel→regime bridge |
| `g_match_catchall` | `Prod(Atom(T armᵢ)…)` → `Atom(T result)` | ✗ | repeated branch premises unrepresented |
| `NUMERIC` / `GRADE` edges | coercion-path data, **not leaves** | n/a | ⟨D-EXACTCOVERED⟩ |

**[Pinned: `the_contract_table_matches_the_minted_generators` (3.4.1);
`zero_arity_intro_generators_are_faithful`,
`literal_intro_generators_are_faithful`,
`structural_generators_are_faithful_kernel_rules`,
`application_rule_is_a_kernel_theorem`,
`arithmetic_split_rule_is_a_kernel_primitive`,
`claim_kind_typing_discharge_is_not_portable` (which derives the tight set from
`minted_generators()` and so cannot silently omit a row). The faithfulness
gates do **not** cover every ✓ row — see 3.4.2.]**

**3.4.1 The table is the generator set.** §3.4's rows and `minted_generators()`
SHALL name the same generators, and each row's `T` column SHALL equal
`generator_is_tight(ClaimKind::Typing, ·)`. Neither side is a literal written
beside the other: the test reads this document and derives the other side from
the enumeration §3.1 makes single-source. **[Pinned:
`the_contract_table_matches_the_minted_generators`]**

> This clause exists because §12.2 — "a new generator SHALL arrive with its row
> in §3.4" — was prose, and four generators arrived without one. `g_fix` (#317)
> and `g_then_split`, `g_tensor_split`, `g_tensor` (#308) were minted, three of
> them discharged, and this table did not move. Nothing failed, because nothing
> was watching. Verified to fail now: removing `g_tensor`'s row, or flipping
> `g_fix`'s `T` cell to ✓, fails the test naming the generator.

**3.4.2 Three ✓ rows do not yet meet §9.2.** `g_then_split`, `g_tensor_split`
and `g_tensor` are discharged in `generator_is_tight`, and their grounds are
stated — ⟨D-SPLIT⟩ for the two splits, product introduction for `g_tensor`.
What they do not have is §9.2(2)'s **faithfulness pin** or §9.2(3)'s **negative
pin**. `structural_generators_are_faithful_kernel_rules` covers `g_record`,
`g_field`, `g_ctor` and `g_match`; `g_tensor` claims `g_record`'s ground and is
absent from it. **[Partial: the discharge is declared and the ground is stated;
the artifacts §9.2 requires do not exist]**

> Recorded rather than repaired here, because repairing it either adds pins or
> withdraws the discharges, and withdrawing them moves the grade of every `then`
> and `and` composition. §9.2 is unambiguous that all four obligations are
> required, so this is a gap and not a variant reading — but which way it closes
> is a decision, not a documentation change.

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

**4.7 Final proposition.** `HasType(Γ ⊢ e, T)` is represented as
`Realizes(w, cfg(Γ ⊢ e), cfg(T))` where `w` is the tree's composite witness.
That proposition's identity is the published judgement's identity. The subject
is the `(Γ, e)` pair, not the bare expression: a proposition naming the bare
expression would claim the derivation established `e : T` under **no**
assumptions (§2.7). **[Pinned: `audited_type_check_tree`;
`tests/derivation_differential.rs` pins fourteen proposition ids]**

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
| Non-composing `then` | `TypeError::CompositionEndpointMismatch` | **[Pinned: `test_composition_operators_lower`]** |
| Unbound variable | `TypeError::Unbound` | **[Pinned: `unbound_var_is_a_type_error`]** |
| Infinite type / occurs | `TypeError::InfiniteType` | **[Pinned: `self_application_is_rejected_by_the_occurs_check`]** |
| Missing field | `TypeError::NoField` | **[Pinned: `test_field_access_missing_field_type_error`]** |
| Non-exhaustive match | `TypeError::NonExhaustive` | **[Pinned: `test_sum_match_non_exhaustive`]** |
| Unsupported syntax | `TypeError::Unsupported` | **[Pinned: `brix-lower` `Unsupported` tests]** |
| Ill-formed derivation | `TypeError::IllFormedDerivation` | **[Partial: constructed, but no fixture drives a real ill-formed tree through the public entry point]** |
| Context mismatch | `TreeAuditError::EndpointMismatch` | **[Partial: pinned at `audit_tree`; not surfaced as a distinct public negative outcome]** |
| Ambiguity | — | **[Specified]** — no carrier exists. Monomorphic L2 has no ambiguous inference today; a carrier is required before any feature that can produce one. |
| Certified refutation | — | **[Specified]** — requires a complete fragment declaration *and* a kernel-accepted refutation certificate. Neither exists. Until both do, untypability is `Unknown`, never `Refuted`. |

> `CompositionEndpointMismatch` is a *reporting* carrier, not a new authority.
> Whether `a then b` composes is still decided downstream by
> `verify_structure` and the kernel's `RealizesComp`, on the actual
> configuration endpoints. The variant exists so the failure can be named
> instead of surfacing as a bare `IllFormedDerivation`, which 5.2 already lists
> as the case with no fixture driving it through the public entry point.

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

**5.5 Nesting depth.** A program the parser accepts SHALL be checked or
rejected, never abort the process. A stack overflow is neither an outcome in
5.2 nor an exhaustion under 5.4: it produces no verdict at all, so a caller
cannot distinguish "too deep" from "the compiler died", and no budget bounds
it.

**[Partial: the reachable abort is gone; the recursion is not.**

**It was reachable, and by ordinary input.** `brix check` on
`let x = 1 + 1 + …` with **50 terms** died with
`thread 'main' has overflowed its stack` — no diagnostic, no exit code, no
verdict. `max_nesting_depth` does not protect against this: it caps *parser
recursion*, and a binary-operator chain is parsed by a loop, so the expression
tree is as deep as the chain is long however short the parse is. Nothing
between the lexer and the kernel bounded it.

Two causes, both since addressed. Inference recursed once per expression level
and aborted between depth 24 and 32 on a 2 MiB stack; it is now iterative and
reaches ~1500, bounded by derived `Clone`/`Drop` glue on the `Box`-based `Expr`
(`soc-regimes/tests/deep_nesting.rs`). `brix_kernel::acceptance` then recursed
once per binder of the `λh₁…λhₘ` spine `brix-elaborate` emits — a spine as long
as the derivation has leaves. (→I) is now peeled by a loop, and the rules that
carry work have their own frames.

Measured through `check_module` on a 2 MiB thread, as chain length:

| | survives |
|---|---|
| before | ~12 |
| after | ~57 |

**What remains, stated as a limit rather than a fix.** The kernel still
recurses once per `RealizesComp` node, so a long enough chain still aborts on a
small enough stack. `brix check` is safe for *any* input only because its 8 MiB
main thread outlasts the 2000-step budget that ends the search around 140
terms — two numbers that happen to be ordered correctly. That is not the
guarantee this clause asks for, and it silently depends on a budget chosen for
a different purpose (§5.4). Closing it properly means an iterative
`infer_type`, which is a `brix-kernel` change; pinned meanwhile by
`brix-lower/tests/deep_expression.rs`.**]**

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

> The previous revision of this section was verified against `main` on
> 2026-08-16 and was stale by #308 two days later. §3.4.1 now fails CI when the
> table drifts; nothing yet does that for this section, so it remains a
> standing obligation under §12.3.

- The λ-calculus core, the structural product/coproduct fragment, literals, and
  the two zero-arity introductions reach genuine `Proven`.
- **Arithmetic is capped.** `1 + 2` is `Int @Audited` and `7 / 2` is
  `Float @Audited`, capped by `g_arith_input`, `g_arith`, and `g_arith_result`.
  `g_arith`'s realization *is* decided by a kernel relation (`TypingArithV2`),
  but no certificate closes the leaf with it yet, and the two bridges are not
  dischargeable by that mechanism.
- **Comparison is capped** by `g_cmp`.
- **Recursion is capped** by `g_fix`, deliberately: it assumes the definition's
  own type in order to check the body, which is the standard and sound rule for
  typing but not a correspondence the kernel owns. Under ⟨D-JUDGE⟩ that scoping
  is the point — `fn loop(x: Int): Int = loop(x)` types correctly, because a
  typing judgement never claimed the function halts.
- **Three discharges are declared without their §9.2 artifacts** —
  `g_then_split`, `g_tensor_split`, `g_tensor` (§3.4.2). They are *not* capping
  anything, which is precisely why this is worth stating: an undischarged
  generator announces itself by capping the result, and these do not.
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
it caps. **[Partial: §3.4.1 pins the row and its `T` cell. Nothing mechanically
checks that a ✓ carries §9.2's four artifacts — §3.4.2 is three instances of
exactly that, found by reading.]**

**12.3** A new **type former** obliges the corpora that quantify over types to
gain the new case. `Ty::Rec` arrived in #298 and the zero-arity gates did not
follow until #301 — the assertions were right, the set they quantified over had
stopped matching the set that exists. This is the same failure as §0's, and it
recurs because nothing forces the corpus to track the language.

**12.4** A clause SHALL NOT be promoted from **[Specified]** to **[Pinned]**
without naming the test that fails when it is violated, and confirming that it
does fail — not assuming it would.
