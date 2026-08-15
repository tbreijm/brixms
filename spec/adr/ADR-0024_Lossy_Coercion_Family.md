# ADR-0024 — The Lossy-Conversion Family, and Why Per-Edge Coercion Discharge Is Moot

Status: **Accepted** (2026-08-15; Proposed the same day — ⟨D-LOSSYFAMILY⟩ and ⟨D-EXACTCOVERED⟩ accepted, the narrow reading of ⟨D-PROMOTE⟩ confirmed, and §3's retention of `TypingArithV1` **overruled in favour of retirement**, per the maintainer ruling on #53). Implements [ADR-0015](./ADR-0015_Judgment_Scoped_Tightness.md) §5 Stage E ⟨D-PROMOTE⟩, and rules that the stage's *discharge* half no longer has a subject.

Date: 2026-08-15; §3 revised the same day to record the retirement ruling.

Note on §7 immutability: retiring a relation is **not** an edit to one. `TypingArithV2`'s rows, identity, and vector are untouched by that ruling; `TypingArithV1`'s id simply stops resolving, which §7 already specifies as failing closed.

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

- `TypingArithV2` is the current relation; the regime's emission and every Stage B gate target it. It keeps the name `V2` now that `V1` is gone — reusing `V1`'s name for a different row set would be exactly the reinterpretation §7 forbids, so version names here are historical, not slots.
- `TypingArithV1` was **retired** (see the ruling below). Its id resolves to `None`, which §7 specifies as failing closed.
- 100 rows were byte-identical across the two. The 20 that moved are exactly those whose path crosses `Int ↪ Float`: the 4 `Div` rows with both operands in `{Nat, Int}` (crossing on both sides), the 4 mixed `Float` pairs under `Div`, and the same 4 under each of `Add`/`Sub`/`Mul`. `Float op Float` never crosses — both paths are empty.
- `vectors/primitive_relation_typing_arith_v1.json` is untouched; `..._v2.json` is new.

This is ⟨D-RELID⟩'s first real exercise, and it behaved as designed: no separate step allocated the new id, and no reviewer had to remember to.

### V1 is retired — the conservative call was overruled, on this ADR's own argument

The original text kept V1, on §7's plain reading that identities persist and old certificates keep verifying, and flagged the alternative for a ruling rather than settling it unilaterally. The ruling went the other way, and the reasoning is better than the reasoning it replaced:

> ⟨D-EXACTCOVERED⟩ rejected a per-edge coercion relation because it "would add trusted TCB data with nothing consulting it." `TypingArithV1` is now exactly that. Retaining it is the same mechanism this ADR just refused, and the two rulings cannot both stand.

The supporting facts were already in this section and point the same way. Nothing emits V1, **no certificate naming it exists**, and none can yet — Stage D has not landed and `elaborate_tree` still emits every leaf as a `Hyp`. §7's promise that "identities persist and old certificates keep verifying" is a promise to certificates that *exist*; it is not an instruction to carry dead rows into the TCB against the possibility that one might have.

And retention was not neutral. V1's rows spell `type.rule.num.promote.Int_Float@1`, a generator family the lattice no longer declares — so what retention preserved was a trusted table asserting rows over an edge id that is gone. That is a legibility hazard pointed the wrong way, and it is the opposite of what ⟨D-LOSSYFAMILY⟩ set out to fix.

Retiring is safe for the reason the original text already gave: §7 makes an unknown relation id fail closed rather than be reinterpreted, so an absent id means *nothing* rather than something different.

**What the retirement costs, and how it is paid.** V1's row set was evidence — it bounded Stage E's blast radius at 100 shared rows and 20 moved. That evidence is kept without keeping the data: the legacy matrix is now reconstructed inside tests, which is where history belongs when nothing may resolve it. Three properties are pinned:

- the legacy matrix is the current one with exactly one edge renamed, 100 rows shared and 20 moved (`prim_registry`'s `the_current_relation_is_the_legacy_matrix_with_one_edge_renamed`, and the independent reconstruction in `primitive_relation_vectors`);
- V1's **literal** historical id no longer resolves, and a term naming it is rejected rather than quietly checked against the surviving relation — asserted on a row the two versions *shared*, so a fallback would show up as an acceptance (`the_retired_relation_does_not_resolve`);
- a legacy-named path is not admitted by the current relation, checked over the exhaustive matrix through the real generator and the real `acceptance` entry point (`arithmetic_rule_is_a_kernel_primitive`).

`vectors/primitive_relation_typing_arith_v1.json` is deleted with it. That is a deletion inside `vectors/`, said loudly here rather than left to a diff: the file froze a relation that no longer exists, and keeping it would freeze data no consumer can reach.

## 4. What this does not establish

- Nothing about **values**. A typing-level exact-edge fact establishes the coercion term's admissibility, not a successful executable conversion. A value-level exact-edge discharge additionally requires totality, denotation preservation, injectivity, canonicality, path coherence, and kernel ownership — deferred until those value domains exist canonically (ADR-0015 §5 Stage E).
- Nothing about `Int→Float` beyond its name. Renaming the family does not make the map exact, and does not discharge it as anything. A future discharge would be of a *different* proposition — a specified width, rounding mode, and overflow behaviour — which `brix-canon` cannot express, because it excludes floats from `Canonical`.
- No grade movement. `7 / 2` is still `Float @Audited`; `1 + 2` is still `Int @Audited`. The arithmetic cap is unchanged and is blocked on ADR-0023 §4's endpoint-vocabulary question, not on coercion.

## 5. Compatibility

- No new `TermKind` ordinal, no envelope change, no schema change. `ArithTypingInputV1`'s marker, version, and all three ordinal spaces are untouched — only the *value* of a generator id inside an edge changes.
- `vectors/arith_typing_input_v1.json` is unchanged and stays valid: it freezes the schema's *encoding*, and that encoding is unaffected. Its `div_int_int_lossy` case now spells a legacy edge id, which `TypingArithV1` still recognises.
- `brix why` now renders the edge as `convert_lossy(Int->Float)` rather than `promote(Int->Float)`.

## 6. Decisions closed

Both of this ADR's open decisions are settled by the 2026-08-15 ruling.

- **Retire `TypingArithV1`** (§3). Overruled in favour of retirement, and done.
- **The narrow reading of ⟨D-PROMOTE⟩ is what was intended** (§2). Family relocation only. The broad reading removes a currently-legal program class — `1 + 1.0` stops typing and integer division has no result type — and §5 Stage E scopes no language change. ⟨D-PROMOTE⟩'s stated concern, that no generator id assert an embedding for a non-injective map, is fully discharged by the narrow reading. If anyone ever wants the broad one it is a language-design ADR of its own, and the burden is on that ADR to say what integer division becomes.
