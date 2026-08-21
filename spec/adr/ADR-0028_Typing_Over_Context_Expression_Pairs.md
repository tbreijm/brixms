# ADR-0028 — Typing Judgements Are Over Context-Expression Pairs

Status: **Accepted** (2026-08-19). Fixes two findings from the ⟨D-OPARROW⟩ constitutional audit:
typing derivations publish under an empty assumption scope, and two discharged generators are
non-functional in their source. Both have the same cause and the same fix.

Date: 2026-08-18. Implemented 2026-08-19.

Foundation documents: [ADR-0002: SOC Constitution](./ADR-0002_SOC_Constitution.md) (§6 the judgement
tuple and its search-invariance, §6.1 the frozen root context),
[ADR-0005: Type Inference as Realization](./ADR-0005_Type_Inference_as_Realization.md),
[ADR-0010: SOC Language Design](./ADR-0010_SOC_Language_Design.md) (§7a ⟨D-OPARROW⟩),
[ADR-0015: Judgment-Scoped Tightness](./ADR-0015_Judgment_Scoped_Tightness.md) (§8.5, §8.7,
⟨D-SPLIT⟩).

---

## 1. The two findings, and why they are one

**The typing regime's objects are the wrong objects.** A typing judgement is `Γ ⊢ e : T`. The
regime's configuration vocabulary carries only `e`: `CfgAtom::Expr(Expr)`. The context reached the
rule but never reached the *object language*, so it appears in no endpoint of any derivation.

Two consequences, both surfaced by the audit and both verified in source:

**(a) Every typing judgement is published under `ContextId::root()`.** ADR-0002 §6.1 freezes the
root as "root snapshot + program + **empty assumptions** + default profile + default limits". So
`let b = a + 1` publishes a judgement claiming *empty assumptions* while its derivation depends on
`a`'s binding. `JudgementId = (ContextId, PropositionId, outcome, EvidenceId)` is documented as
**search-invariant**; with every context equal, it cannot distinguish assumption sets, and two
derivations that differ only in what they assumed are indistinguishable at the judgement level.

`ContextId::extend` — described in its own doc as "the first real assumption-scope machinery" —
has exactly one caller in the workspace, and it is a test.

**(b) `g_var` and `g_lam_close` are discharged, and their relations are not functional.**

```text
g_var       : Atom(Expr(Var "x"))  →  Atom(Type(t))
g_lam_close : Atom(Type(tb))       →  Atom(Type(Fn(param_ty, tb)))
```

`g_var`'s `t` differs per context; `g_lam_close`'s `param_ty` is host-synthesized and appears in
**no** source endpoint. One canonical `src`, many `dst`s.

This is precisely the obstruction on which `g_arith_input` is explicitly *held out* — its own doc
says a relation keyed on such a source "would be **non-functional** … one canonical `src`, four
distinct `dst`s". The same defect is tolerated in two generators that are discharged. Their
discharge rationale appeals to the kernel's `Hyp` and `Lam` rules, but those are *context* rules,
and the context is not an endpoint — so the correspondence is asserted rather than exhibited, which
is the class of claim ADR-0015 §8.5 declines to trust.

**Same cause.** The context is real, load-bearing, and absent from the vocabulary.

## 2. Decision — ⟨D-CTXENDPOINT⟩ an expression endpoint names its context

> A configuration atom standing for an expression under typing SHALL carry the identity of the
> context it is typed under. The typing regime's objects are **(Γ, e) pairs**, never bare
> expressions.

With the context in the endpoint:

- `g_var : Atom((Γ, Var "x")) → Atom(Type(t))` is **functional** — `Γ` determines `t`, so one
  source has one destination.
- `g_lam_close`'s `param_ty` is determined by the binder that `Γ` records, rather than conjured
  beside the derivation.

Both discharges then rest on a correspondence the derivation *exhibits*, which is what ADR-0015
§8.5 requires and what they have been missing.

**This does not by itself make either kernel-checked.** It makes them checkable — a relation over
functional endpoints is one a future ⟨D-PRIM⟩ primitive could own. The discharges remain what they
are today until then.

## 3. Decision — ⟨D-CTXPUBLISH⟩ a judgement is published under the context it was derived in

> A typing judgement SHALL carry the `ContextId` its derivation actually ran under. `ContextId::root()`
> SHALL be published only for a derivation that genuinely assumed nothing.

The context extends at each binder — a lambda parameter, an annotated parameter, a fixpoint's own
name, a match arm's bindings — through `ContextId::extend`, which hashes the parent's digest with
the assumption's canonical bytes. That machinery already exists and is ratified; it has simply
never been used outside a test.

## 4. What this costs — every typing-derivation identity moves

Stated plainly rather than discovered: **this is a deliberate one-time identity migration, not a
behaviour-preserving refactor.** Adding the context to an expression atom changes that atom's
digest, hence every derivation containing one, hence the propositions and judgement ids built from
them.

Checked before proposing, so the blast radius is known rather than hoped:

- `vectors/tree_derivation_v1.json` encodes the `TreeDerivation` *envelope* over abstract endpoints
  (`x0 → x1`), not typing atoms. **Unaffected.**
- `vectors/pinned_endpoint_identities_v1.json` (ADR-0025 ⟨D-PINNED⟩) pins **type** atoms and
  operator atoms. Its own note warns that a change there "breaks brix-kernel's rows and is a
  kernel-ABI event" — which is exactly why this ADR leaves `CfgAtom::Type` untouched.
  **Unaffected.**
- `vectors/primitive_relation_typing_arith_v2.json` keys on `ArithTypingInputV1` /
  `NumericResultTypeV1`, which are kernel-owned schemas, not expression atoms. **Unaffected.**
- The typing lane computes its identities fresh on every check and persists none.

What does move is the differential corpus added for the traversal work — and moving it is the
point. That corpus exists to make an *unintended* identity change loud; this one is intended, so
it is re-captured with this ADR named as the reason.

**What the re-capture actually showed** (2026-08-19). All fourteen **proposition** ids moved, as
predicted. All fourteen **witness** ids survived byte-for-byte. That asymmetry is the evidence
that this was an endpoint migration rather than a behaviour change: the same generators fired in
the same composition, and only the subject the composition is stated to be *about* gained its
assumption scope. Had a witness id moved, the correct reading would have been that this ADR
changed the derivation, and the change would have needed re-justifying rather than re-capturing.
The corpus pins the two ids separately precisely so this distinction is visible; a single digest
over the pair would have collapsed it.

**No new `CfgAtom` ordinal is reused or renumbered.** The context-carrying atom is appended, and
the bare-expression atom is retired from emission rather than deleted, so a decoder meeting an old
artifact still fails closed on an ordinal it knows rather than silently reinterpreting one it does
not.

## 5. What this does not do

- It does not discharge anything new. `g_var` and `g_lam_close` keep the grades they have; they
  simply stop resting on a relation that could not be checked even in principle.
- It does not make the context *contents* canonical. `ContextId` is an identity, and #59 still owns
  the question of what a context *contains*, its transport, and its confinement. This ADR uses the
  identity that already exists.
- It does not change `CfgAtom::Type`, and therefore touches no kernel row and no pinned endpoint.
- It says nothing about evaluation. A typing judgement scoped to a real context is still a typing
  judgement (ADR-0015 ⟨D-JUDGE⟩).
- **It does not weaken ⟨D-SPLIT⟩,** though it does change how ⟨D-SPLIT⟩ (iii) is demonstrated.
  That clause claims the arithmetic split encodes no chosen promotion and no synthesised result
  type, and its demonstration was: one expression, two contexts forcing different promotions, a
  *byte-identical* split leaf. Two such contexts are now necessarily two different scopes, so the
  leaves differ and cannot not differ. The claim survives because the scope is part of *which
  judgement is being made*, not part of *what the split concluded* — the split still carries no
  promotion and no result type. The demonstration is therefore modulo the scope, and to keep that
  from being a loophole it *asserts* on the way past that every context appearing in the split's
  endpoints is the ambient judgement's own. A split that varied its endpoints' scope with the
  result would fail there rather than be normalised away, which is strictly more than the old
  byte-equality checked.

## 6. Hard boundaries

1. A judgement SHALL NOT claim `ContextId::root()` unless its derivation assumed nothing.
2. A context SHALL NOT be reconstructed from a derivation's endpoints — it flows *down* from the
   binder that created it and is recorded, never inferred back.
3. The context's presence in an endpoint SHALL NOT be read as making the generator kernel-checked.
   Functional is a precondition for checkable, not a substitute for checked.
4. `CfgAtom::Type` SHALL remain unchanged; the pinned endpoint identities are a kernel-ABI surface.

## 7. Open decisions

- Whether the retired bare-expression atom is eventually deleted. It costs an unused ordinal; the
  alternative is a decoder that can no longer name what it met. Not blocking.
- ~~Whether `TyCtx` should become the sole carrier of the context, retiring the separate `context`
  parameter that `audited_type_check_tree` takes today.~~ **Resolved in implementation: retired.**
  It turned out not to be cosmetic. Leaving the parameter would have left finding (a) half-open —
  the derivation would run under `ctx`'s scope while the *judgement* published whatever the caller
  passed, and a caller cannot know what the binders below it assumed, so it would always pass the
  root. §3 is only enforceable with `TyCtx` as the sole carrier. Every call site passed
  `ContextId::root()`, which is the same defect stated as a fact about the code.
