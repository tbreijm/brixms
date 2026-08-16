# ADR-0025 — Kernel-Pinned Foreign Endpoint Identities, and the Operator Binding

Status: **Proposed** (2026-08-15). Rules ADR-0023 §4.3's open question — who owns the canonical
encoding of a realization endpoint that both the TCB and a regime must name — and settles the
operator-binding gap that ADR-0023 §4.1 records. Unblocks ADR-0015 §5 Stage D.

Date: 2026-08-15.

Foundation documents: [ADR-0002: SOC Constitution](./ADR-0002_SOC_Constitution.md) (§5.3 fail
closed), [ADR-0015: Judgment-Scoped Tightness](./ADR-0015_Judgment_Scoped_Tightness.md) (⟨D-PRIM⟩,
⟨D-SPLIT⟩, §5 Stages B/C/D, §8.3, §8.5), [ADR-0022: Source Re-Derived Manifests](./ADR-0022_Source_Re_Derived_Manifests.md)
(the re-derivation doctrine), [ADR-0023: Primitive-Relation Identity](./ADR-0023_Primitive_Relation_Identity.md)
(⟨D-RELID⟩, §4). Governs issue #53.

This ADR moves no grade by itself. It removes the obstruction that made ADR-0015 Stage D's
headline gate unreachable, and names what still has to land before a grade moves.

---

## 1. The premise that was wrong

ADR-0023 §4.1 states the obstruction as:

> A registry row is matched by canonical bytes, so the kernel must be able to author **both**
> endpoints of every row. It can only do that for schemas it owns: reproducing `soc_regimes::Ty`'s
> encoding inside `brix-kernel` would be a second semantic encoder for a type the TCB does not own.

The obstruction is real. **The stated cause is not.** The registry never sees an encoding.

A row is a pair of `PropositionId`s (`crates/brix-kernel/src/prim_registry.rs:148`), and
`PrimRealizes` decides membership by exact id comparison — `resolved.admits(src_id, dst_id)`
(`crates/brix-kernel/src/check.rs:707`) — without decoding either endpoint. The kernel does not
need `Ty`'s *encoder*. It needs `Ty`'s *digest*. The first would be a second semantic encoder in
the TCB and is correctly forbidden by ADR-0015 §8.5; the second is a 32-byte constant.

That distinction is the whole of this ADR. Everything below follows from it.

### Two clarifications recorded while establishing this

**The schema fields are identity components, not checked preconditions.** ⟨D-PRIM⟩'s synthesis rule
reads as though the kernel verifies "`src` canonical under `S`, `dst` canonical under `D`". It does
not, and cannot, from a digest. This is **not a soundness gap**: membership in a row set authored
from values canonical under `S` implies canonicality under `S` by collision resistance, so the
property holds — it is established by construction at authoring time rather than checked at
verification time. It is recorded because the stronger reading would license designs that do not
work, and an erratum on ADR-0015 ⟨D-PRIM⟩ should say so.

**`PrimRealizes` admits only `Const` endpoints today.** A composition, tensor, or bound variable in
an endpoint position is rejected outright (`check.rs:706-712`). §4 extends this, and the extension
is load-bearing rather than incidental.

## 2. Decision — ⟨D-PINNED⟩ the kernel may pin foreign endpoint identities

> A primitive relation MAY contain rows whose endpoints are the `PropositionId`s of values encoded
> by a crate outside the TCB. Such identities SHALL be compiled-in literal constants of a kernel
> release. The kernel SHALL NOT decode them, SHALL NOT reproduce the foreign encoder, and SHALL NOT
> accept them from a caller.

This is not the caller-authorized-facts route ADR-0015 §8.3 forbids. The distinction is exactly
where authority sits: a caller **proposes** `src` and `dst` and the kernel decides membership
against rows a kernel release authored. Nothing a caller sends can add, amend, or select the
contents of a relation. Pinning changes what a row's endpoints may *denote*; it does not change
who authorizes them.

Fail-closed behaviour is inherited rather than added. If the foreign crate ever changes its
encoding, the digests stop matching, membership fails, and the leaf goes unclosed — the grade caps
and nothing is silently reinterpreted. That is ADR-0015 §7's discipline working as designed, with
no new mechanism.

### ⟨D-REDERIVE⟩ A pinned identity SHALL be re-derivable, and the re-derivation is normative

> Every pinned foreign endpoint identity SHALL be accompanied by both:
>
> 1. a **frozen manifest entry** naming, in readable form, the value the identity digests; and
> 2. a **re-derivation test in the crate that owns the encoder**, asserting the pinned constant
>    equals the digest recomputed from the source value.
>
> A pinned identity shipped without (2) is a defect, not a shortcut.

Without this, a pinned digest would be the second non-re-derivable element proposed for this
system. The first was ADR-0021's signature, and ADR-0022 declined it precisely because a
source-available verifier that re-derives beats one that trusts a constant. A hardcoded digest
whose meaning rests on the kernel author's say-so is the same failure in smaller packaging, and it
would be inconsistent to reject one and ship the other.

`soc-regimes` already depends on `brix-kernel`, so requirement (2) needs no new dependency edge and
creates no cycle: the test recomputes `PropositionId` from `Ty::Con("Int")` and compares it to the
kernel's constant. The obligation lands in the crate that can actually discharge it — the one that
owns the encoder — which is also the crate whose change would break it.

This is deliberately the two-consumer discipline already used for frozen vectors, pointed at
constants instead of bytes.

### Rejected: moving the endpoint vocabulary into Ring 0

ADR-0023 §4.3 option (1), and its own recommendation. Rejected on three grounds, in order of
weight:

1. **It cannot be done narrowly.** Rust's orphan rules mean the `Canonical` impl cannot move
   without the type: both `Ty` and `Canonical` would be external to `brix-semantic`. So "move only
   the encoding" is not available; `Ty` itself moves or nothing does.
2. **`Ty` is not an endpoint atom.** It declares an open constructor namespace plus function types,
   inference variables, records and sums, and its encoding bakes in regime-specific normalization
   choices — record field sorting and duplicate elimination
   (`crates/soc-regimes/src/type_realization.rs:24-72`). Moving it freezes a regime's type language
   and its normalization policy into substrate ABI, and makes every new language type constructor a
   Ring-0 release. The TCB would grow in exactly the dimension it is supposed to stay narrow in.
3. **It does not buy the result.** `NumericResultTypeV1(Int)` and `Ty::Con("Int")` do not become one
   object because two crates can name a shared enum. Dissolving `g_arith_result` would additionally
   require the numeric `HasType` endpoint to *be* the shared atom — a change to how the regime
   represents numeric types, which is a language change and not what §4.3 was asking about.

A narrower variant — moving only a closed `NumericTypeNameV1` and the arithmetic operator into
`brix-semantic` — is coherent and is **not** rejected on soundness. It is not selected because it
carries (3) unchanged while still widening substrate ABI, and ⟨D-PINNED⟩ obtains the same result
with no ABI movement at all. If these atoms later turn out to be shared across several regimes, the
narrow move becomes the better design and this decision should be revisited; that is a
consolidation, not a correction.

### Rejected: a kernel rule for vocabulary correspondence

ADR-0023 §4.3 option (2), rejected there and rejected here for the reason given there: it requires
the kernel to accept regime-authored bytes as row *data*, which is §8.3. ⟨D-PINNED⟩ is not that
option in disguise — the bytes are authored by a kernel release, not by a regime at runtime, and
§5 makes that boundary enforceable.

## 3. Decision — ⟨D-OPPROJECT⟩ the split projects the operator, and never translates it

ADR-0023 §4.1 records that nothing kernel-binds the operator to the expression being typed. The
fix must not itself be an undischarged claim.

> `g_arith_split` SHALL carry the operator forward by projecting the value the expression already
> stores, unchanged and in the regime's own vocabulary. It SHALL NOT convert it into another
> vocabulary.

**A translation would have lapsed ⟨D-SPLIT⟩.** The obvious fix — emit `ArithOperatorV1`, the
kernel's own operator type — fails. `ArithOp → ArithOperatorV1` is a hand-written host
correspondence table (`type_realization.rs:104-118`). Rust proves that match is *exhaustive*; it
does not prove that `ArithOp::Add ↦ ArithOperatorV1::Add` is the correct correspondence. That is
the same epistemic class as `g_arith_result`, which is undischarged for exactly this reason, and
ADR-0015 ⟨D-SPLIT⟩ states the discharge lapses if the split "filters operations by unchecked host
logic". Emitting a host-chosen translation would be that. Emitting a projection of a field the
expression already contains is not.

The correspondence itself does not disappear — it becomes kernel-checked. Under §4's relation, a
row maps the regime's opaque `ArithOp` identity to the `ArithTypingInputV1` that names
`ArithOperatorV1`, so `kernel_operator`'s faithfulness is decided by row membership instead of
asserted by the host.

### Erratum — ⟨D-OPPROJECT⟩ is necessary, and not implementable in today's tree language

> Recorded while implementing Stage B (2026-08-16). Both halves are established by test rather
> than argument; the first strengthens this decision, the second blocks it.
>
> **The obligation is real, and sharper than §3 states.** The gap is now pinned by
> `the_operator_is_not_bound_by_the_derivation`: `Add(a, b)` and `Sub(a, b)` produce a
> byte-identical split *destination*, and consequently the `ArithTypingInputV1`'s operator can be
> transplanted between two derivations while every `Seq` middle still matches and `audit_tree`
> issues `StructureVerified` for a derivation whose subject is `1 + 2` and whose arithmetic input
> says `Sub`.
>
> This is **not** a live unsoundness and not a new one. It is an instance of row (d) of
> `tree_audit`'s own table — no leaf's `ρ_g` is checked — which that module documents as open. The
> same forgery works on `g_lit` (`1 : Str` audits clean), and no production path reaches it:
> `audit_tree` has one caller, which builds the tree from `infer_tree` on the line above, and
> `RealizesTree` has no deserialization path.
>
> What makes it a distinct finding is that **closing row (d) per-leaf would not close this.**
> ⟨D-PRIM⟩'s programme rejects the forged `g_lit` leaf outright, because `(cfg(1), cfg(Str))` is
> not a row. It does not reject the forged operator: a Stage D relation keyed on
> `(op, type_a, type_b) → ArithTypingInputV1` validates the input object against the operator
> supplied *at that leaf*, and that operator is still a host choice, because the chain from the
> expression was severed one step earlier at the split. No per-leaf relation repairs a break
> *between* leaves. Stage D is therefore blocked on Stage B, not merely sequenced after it.
>
> **And the decision as written cannot be implemented.** Suppose `g_arith_split`'s destination
> gains an operator component. By `RealizesTree::well_formed`, a `Seq`'s right subtree must have
> `src == left.dst()`, and a `Tensor`'s `src` is `Prod(left.src(), right.src())` — so that
> component must be the source of some subtree, and every subtree bottoms out in a leaf whose
> source is the operator atom. That leaf's destination is either:
>
> - **the same atom** — a degenerate `src == dst` step. ADR-0007 §1 calls faking intermediate
>   configurations unsound, ADR-0018 §4 retired the flat lane over exactly this, and
>   `tree_derivation_carries_no_padded_step` fails on it across its whole corpus; or
> - **a different atom** — a host-chosen re-expression of the operator, which is the translation
>   this decision forbids by name, merely relocated out of `plan.typing_input` into a leaf of its
>   own. It is also strictly worse for §3's purpose: the Stage D row would then key on a
>   *translated* identity, so `kernel_operator`'s faithfulness would remain host-asserted one
>   level down, which is the outcome ⟨D-OPPROJECT⟩ exists to prevent.
>
> There is no third option: `Tensor` is the only way to consume a `Prod`, and both its branches
> are trees.
>
> **Proposed resolution — the tree language is a monoidal category missing its identity.**
> ADR-0004 gives `Seq` for `∘` and Profile 1.2 gives `Tensor` for `⊗`, but there is no `id`. A
> component that must cross a composition step unchanged has nothing to cross it *with*, and the
> only way to simulate one is a padded generator leaf — which is precisely what ADR-0007 §1
> forbids, and correctly, because such a leaf *asserts* that a generator realizes `(x, x)`. An
> identity node carries no generator and asserts nothing: `id` is part of the categorical
> structure, not a claim within it. Adding it would satisfy ⟨D-OPPROJECT⟩ without weakening §4.3
> of the type-realization contract, whose stated ground is that "no generator realizes `(x, x)`".
>
> This touches `brix-semantic/tree.rs`, `elaborate_tree`, and the kernel's `Realizes` rules — all
> outside this issue's lane — so it is recorded as a proposal for the owner of those crates, not
> implemented here. Stage B's second half (restating Stage C's test) is delivered; its first half
> is blocked on this ruling.

### Erratum to ADR-0015 Stage C's record

Stage C's gate is met and `g_arith_split`'s discharge stands. But its supporting test reasons
incorrectly (`type_realization.rs:3515`): it argues the operator is bound because distinct
operators yield distinct split *sources*. The obligation runs through the *destination*, and
`Add(a, b)` and `Sub(a, b)` have distinct sources and the **same** destination. The assertion is
true and does not establish what it is cited for. It should be restated to assert what it actually
shows, and the operator-binding obligation should be carried explicitly by §3's projection.

## 4. Decision — ⟨D-PRODENDPOINT⟩ exact product endpoints

> `PrimRealizes` SHALL admit an endpoint that is an exact product of object constants, matched by
> the single canonical digest of the product term. It SHALL NOT decompose, decode, or interpret the
> product's structure.

Required because `g_arith_input`'s source is structurally a product: it sits below a `Tensor`, whose
`dst` is always a `Prod`, so no relation over it can have an atomic source. Today such an endpoint
is rejected outright (`check.rs:706-712`), and that rejection is currently correct — it keeps a
failure legible rather than reporting a missing row for a term that could never have been one. This
extends the admissible set rather than weakening the check: a product term has one canonical
encoding and therefore one digest, so membership stays exact id comparison with no decode. The
existing rejection remains in force for compositions and bound variables.

## 5. Hard boundaries

ADR-0015 §8's ten boundaries stand unchanged. Three additions specific to pinning:

11. **No runtime-generated trusted rows.** A pinned identity is a compiled-in constant of a kernel
    release. The kernel SHALL NOT compute one from data supplied at check time, and SHALL NOT link
    against the crate that owns the foreign encoder in a production path.
12. **No pinning without re-derivation.** ⟨D-REDERIVE⟩'s manifest entry and owner-crate test are
    normative, and a pinned identity that has neither SHALL be treated as a defect.
13. **No inference from a pinned identity.** That the kernel holds a digest for `Ty::Con("Int")`
    says nothing about `Ty`, its other constructors, or its encoding. It authorizes one endpoint in
    one relation.

## 6. Staged implementation

Each stage is separately mergeable and moves no grade until the last.

- **Stage A — pin the endpoint identities.** The six numeric `Ty::Con` atoms and the four regime
  `ArithOp` atoms, with their frozen readable manifest and their re-derivation tests in
  `soc-regimes` per ⟨D-REDERIVE⟩. *Gate:* a deliberate mutation of any pinned constant fails the
  owner-crate test.
- **Stage B — project the operator.** `g_arith_split` emits the regime's own `ArithOp`, unchanged;
  Stage C's test restated per §3. *Gate:* `g_arith_split` remains discharged, and a fixture shows
  `Add` and `Sub` nodes now differ in the split's **destination**.
  **Partially delivered, and blocked.** Stage C's test is restated and the operator-binding gap is
  now pinned by `the_operator_is_not_bound_by_the_derivation`, which asserts the gate's *negation*
  and must be inverted when the projection lands. The projection itself cannot be built in the
  current tree language — see §3's implementability erratum, which proposes the identity morphism
  it needs. Stage D is blocked on this, not merely ordered after it.
- **Stage C — exact product endpoints.** ⟨D-PRODENDPOINT⟩ in the checker, with new vectors.
  *Gate:* a product endpoint whose digest is not a row member is rejected as
  `PrimitiveRowNotFound`, not as a type mismatch; compositions and bound variables still fail as
  before.
- **Stage D — the `g_arith_input` relation.** Functional in `(op, a, b)`, with the exhaustive
  matrix and the material-field mutation tests ADR-0015 §5 Stage B requires — including the
  expression-identity mutation, which the existing suite does not cover.
- **Stage E — retire `g_arith_result`.** Point `g_arith` at the numeric destination directly.
  Allocates a new relation identity per ⟨D-RELID⟩ if the destination schema changes.
- **Stage F — close the leaves.** The grade moves here and nowhere earlier.

## 7. What still caps the grade after all six stages

Stated plainly so no one reads Stage F as automatic.

`elaborate_tree` emits every leaf as a hypothesis (`crates/brix-elaborate/src/lib.rs:259`), so
shipping relations closes nothing on its own. ADR-0015 §5 Stage D already requires the mechanism
change — "`honest_result_outcome` must stop asking 'is this generator in a set?' and start asking
'was this leaf actually closed by an accepted primitive instance?'" — and that is the work that
cashes everything above.

Beyond it, a strict reading of Stage D's gate implicates `g_lit` and `g_arith_split` too: both
leaves remain hypotheses backed by prose discharges, so until they have kernel-checkable closure the
certificate proves a *conditional* composition theorem rather than unconditional
`HasType(1 + 2, Int)`. This ADR does not expand Stage D to cover them. It does require Stage D's
gate to state which reading it means, because the two are not the same claim and the difference is
exactly the kind of thing ⟨D-JUDGE⟩ exists to keep honest.

## 8. Compatibility

- No envelope change, no new `TermKind` ordinal, no schema change, and no existing canonical id
  moves. ⟨D-PRODENDPOINT⟩ widens an accepted endpoint set; it renumbers nothing.
- `vectors/primitive_relation_typing_arith_v2.json` is unchanged by this ADR. New relations
  allocate new vectors.
- Old certificates are unaffected in both directions: their leaves are `Hyp` and stay capped, and
  nothing here upgrades anything retroactively.

## 9. Open decisions

- Whether the numeric and operator atoms are later consolidated into `brix-semantic` as shared
  vocabulary (§2's narrow variant). Revisit if a second regime needs them; not blocking.
- Which reading of ADR-0015 Stage D's gate is intended — `g_arith`'s leaf closed, or the whole
  derivation unconditional (§7). Needs an answer before Stage F, not before Stage A.
