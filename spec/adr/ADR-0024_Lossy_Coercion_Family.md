# ADR-0024 — The Lossy-Conversion Family, and Why Per-Edge Coercion Discharge Is Moot

Status: **Proposed** (2026-08-15). Implements [ADR-0015](./ADR-0015_Judgment_Scoped_Tightness.md) §5 Stage E ⟨D-PROMOTE⟩, and rules that the stage's *discharge* half no longer has a subject.

Date: 2026-08-15.

Foundation documents: [ADR-0002: SOC Constitution](./ADR-0002_SOC_Constitution.md) (§5.3 fail closed), [ADR-0013: Canonical Certificate Envelope](./ADR-0013_Canonical_Certificate_Envelope.md) (§7 additive versioning, §8 independent vectors), [ADR-0015: Judgment-Scoped Tightness](./ADR-0015_Judgment_Scoped_Tightness.md) (⟨D-PROMOTE⟩, ⟨D-JUDGE⟩, §5 Stages B0/B/E, §7), [ADR-0023: Primitive-Relation Identity](./ADR-0023_Primitive_Relation_Identity.md) (⟨D-RELID⟩, ⟨D-LOSSYROW⟩).

Changes no outcome, no grade, and no inferred type. Governs issue #53.

---

## 1. The finding: ⟨D-PROMOTE⟩'s discharge half has no subject

⟨D-PROMOTE⟩ reads:

> The exact edges `Nat→Int`, `Int→Rat`, `Rat→Real`, `Real→Complex` SHALL be individually dischargeable. **`Int→Float` SHALL NOT be discharged as an embedding or promotion**, now or later.

and gives its reason:

> if `g_promote_edge` carries one shared tightness bit across all edges, it cannot be discharged at all while the lossy edge is in the family.

**That reasoning presumes per-edge leaves, and there are none.** Stage B0 replaced promotion *splicing* — one embedding leaf per edge in the operand's derivation — with promotion *data* carried inside `ArithTypingInputV1`. After it, `promote_generator` has exactly two call sites: populating the minted-generator enumeration, and building `CoercionEdgeV1` values. No coercion generator reaches a tree leaf anywhere in the workspace.

Consequences, all verified against the code rather than inferred:

- `generator_is_tight` is never consulted for a coercion edge, because `all_generators_tight` walks tree leaves.
- A per-edge discharge would therefore move no grade and cap nothing. Minting tightness with no consumer is precisely the mechanism ADR-0015 §5 Stage D rejects ("a boolean whitelist flip is the wrong mechanism"), and §8.7 forbids a generator's registry membership from implying anything about occurrences.
- The shared-tightness-bit hazard ⟨D-PROMOTE⟩ warns about cannot arise: there is no bit to share.

### ⟨D-EXACTCOVERED⟩ The exact edges are already kernel-checked, as part of the arithmetic relation

> The four exact promotion edges SHALL NOT receive a separate per-edge primitive relation. Their admissibility in a typing judgement is already decided by exact membership in the arithmetic relation, which keys on the **whole** `(operator, operand types, promotion paths) → result` tuple.

A row of `TypingArithV2` authorizes a specific ordered path, edge by edge, with each edge's generator id and exactness bound into the source object's canonical bytes. An exact edge appearing in an accepted row *is* a kernel-checked fact about that edge's use — which is what "at the typing level an exact edge's finite relation may be discharged as soon as the kernel owns it" asked for. A second relation covering the same edges in isolation would add trusted TCB data with nothing consulting it.

This is a narrowing of Stage E, not a widening. If a future feature emits standalone coercion leaves, that feature brings the per-edge relation with it, and ⟨D-PROMOTE⟩'s original shape applies unchanged.

## 2. Decision — ⟨D-LOSSYFAMILY⟩ the lossy edge leaves the promotion family

> `Int ↪ Float` SHALL be named under a generator family distinct from the promotion family. An edge's generator family SHALL be derived from its declared exactness, so an id and its `CoercionKind` tag cannot disagree.

Concretely: `type.rule.num.promote.Int_Float@1` becomes `type.rule.num.convert.lossy.Int_Float@1`. The four exact edges keep `type.rule.num.promote.*` unchanged.

Stage B0 labelled this edge `CoercionKind::Lossy` but left it named as a promotion, and said so at the time: *"Labelling it keeps the record honest here; relocating it out of a lattice called `NUMERIC` is Stage E's ⟨D-PROMOTE⟩ work."* Until now the tag said "lossy" while the id said "promote" — two facts that could drift, one of which was wrong. Exactness is now a third component of each lattice edge, and the family follows from it, so they are one fact.

### What is deliberately *not* done: the edge stays in the coercion graph

⟨D-PROMOTE⟩ says `Int→Float` "should move to an explicitly-labelled lossy-conversion family rather than sitting in a lattice called `NUMERIC`'s promotion edges." That admits two readings.

- **Family relocation** (adopted). The edge remains in the coercion graph; only its generator family changes. No type and no grade moves.
- **Removal from `NUMERIC`** (rejected). `join(Int, Float)` and `Div`'s `field_of` both depend on the edge. Removing it would stop `1 + 1.0` typing and leave integer division with no result type. That is a language change, and §5 Stage E does not scope one.

The narrow reading is taken because it is the one that discharges the stated concern — no id asserting an embedding for a non-injective map — without changing what the language accepts. If the maintainer intends the broad reading, it needs its own ADR: it removes a currently-legal program class.

### ⟨D-LOSSYROW⟩ is strengthened, not revisited

ADR-0023 ⟨D-LOSSYROW⟩ admitted rows whose paths cross the lossy edge, against a counter-reading that ADR-0015 §5 Stage B0 defines a promotion path as "an ordered sequence of **exact** promotion-edge ids". After this ADR that counter-reading is weaker still: such a path is no longer a sequence of *promotion*-edge ids at all, so the phrase no longer describes what those rows contain. The rows stay.

## 3. Consequence — `TypingArithV2`

Renaming the edge changes `CoercionEdgeV1`'s canonical bytes, hence the source object's `ConfigId`, hence **20 of the 120 rows**, hence the content-derived relation identity. Per ADR-0015 §7 and ADR-0023 ⟨D-RELID⟩ this allocates a new relation rather than editing one:

- `TypingArithV2` is the current relation; the regime's emission and every Stage B gate target it.
- `TypingArithV1` is retained, unchanged and still resolvable.
- 100 rows are byte-identical across the two. The 20 that moved are exactly those whose path crosses `Int ↪ Float`: the 4 `Div` rows with both operands in `{Nat, Int}` (crossing on both sides), the 4 mixed `Float` pairs under `Div`, and the same 4 under each of `Add`/`Sub`/`Mul`. `Float op Float` never crosses — both paths are empty.
- `vectors/primitive_relation_typing_arith_v1.json` is untouched; `..._v2.json` is new.

This is ⟨D-RELID⟩'s first real exercise, and it behaved as designed: no separate step allocated the new id, and no reviewer had to remember to.

### Retaining V1 is the conservative call, and the alternative is real

Nothing emits `TypingArithV1`, and **no certificate naming it exists** — Stage D has not landed, `elaborate_tree` still emits every leaf as a `Hyp`, and nothing yet closes a leaf with a `PrimRealizes` term. Retiring V1 would also be safe: §7 makes an unknown relation id fail closed rather than be reinterpreted, so an absent id means *nothing* rather than something different.

V1 is nevertheless kept, because §7's plain reading is that identities persist and old certificates keep verifying, and because retention costs one small naming function rather than a second row table. **Flagged for a maintainer ruling** rather than settled unilaterally: if the preference is to retire superseded relations that never had a consumer, that is a one-line change and a vector deletion.

## 4. What this does not establish

- Nothing about **values**. A typing-level exact-edge fact establishes the coercion term's admissibility, not a successful executable conversion. A value-level exact-edge discharge additionally requires totality, denotation preservation, injectivity, canonicality, path coherence, and kernel ownership — deferred until those value domains exist canonically (ADR-0015 §5 Stage E).
- Nothing about `Int→Float` beyond its name. Renaming the family does not make the map exact, and does not discharge it as anything. A future discharge would be of a *different* proposition — a specified width, rounding mode, and overflow behaviour — which `brix-canon` cannot express, because it excludes floats from `Canonical`.
- No grade movement. `7 / 2` is still `Float @Audited`; `1 + 2` is still `Int @Audited`. The arithmetic cap is unchanged and is blocked on ADR-0023 §4's endpoint-vocabulary question, not on coercion.

## 5. Compatibility

- No new `TermKind` ordinal, no envelope change, no schema change. `ArithTypingInputV1`'s marker, version, and all three ordinal spaces are untouched — only the *value* of a generator id inside an edge changes.
- `vectors/arith_typing_input_v1.json` is unchanged and stays valid: it freezes the schema's *encoding*, and that encoding is unaffected. Its `div_int_int_lossy` case now spells a legacy edge id, which `TypingArithV1` still recognises.
- `brix why` now renders the edge as `convert_lossy(Int->Float)` rather than `promote(Int->Float)`.

## 6. Open decisions

- Whether to retire `TypingArithV1` (§3). Not blocking.
- Whether the broad reading of ⟨D-PROMOTE⟩ — removing the edge from `NUMERIC` outright — is intended (§2). It would change what typechecks and needs its own ADR.
