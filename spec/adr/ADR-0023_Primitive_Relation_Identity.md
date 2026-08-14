# ADR-0023 — Content-Derived Primitive-Relation Identity, and the Endpoint-Vocabulary Boundary

Status: **Proposed** (2026-08-14). Proposes the canonical encoding of `PrimitiveRelationId`, which [ADR-0015](./ADR-0015_Judgment_Scoped_Tightness.md) ⟨D-PRIM⟩ ratified as a concept but left unspecified; and records a finding from implementing ADR-0015 Stage B that blocks its Stage D as written.

Date: 2026-08-14.

Foundation documents: [ADR-0002: SOC Constitution](./ADR-0002_SOC_Constitution.md) (§5.3 fail closed), [ADR-0013: Canonical Certificate Envelope](./ADR-0013_Canonical_Certificate_Envelope.md) (§7 additive versioning, §8 independent vectors), [ADR-0015: Judgment-Scoped Tightness](./ADR-0015_Judgment_Scoped_Tightness.md) (⟨D-PRIM⟩, ⟨D-JUDGE⟩, §5 Stages B/D, §7, §8), [ADR-0019: Verification Tags Are Earned](./ADR-0019_Verification_Tags_Are_Earned.md) (§6, the settlement-lane twin).

This ADR changes no outcome, no grade, no existing canonical id, and no frozen vector. It proposes an encoding for a *new* identity introduced by ADR-0015 Stage B, and it reports a structural obstruction. Governs issue #53.

---

## 1. Why this is a separate document

ADR-0015's assignment on #53 asked for one thing explicitly:

> `PrimitiveRelationId`'s canonical encoding is yours to propose, and I will align the settlement-side receipt to it rather than the other way round. Please raise it explicitly in the PR (or a short ADR note) rather than settling it silently inside the kernel, so we can converge before either side freezes vectors.

Stage B freezes `vectors/primitive_relation_typing_arith_v1.json`, so the encoding is being committed now whether or not it is written down. An ADR is the durable place to say what was chosen and why, and to let the settlement lane disagree with it before ADR-0019 §6's receipt identity is fixed.

§4 is a separate matter: a finding, not a proposal.

---

## 2. Decision — ⟨D-RELID⟩ the relation identity is derived from the relation's contents

> A `PrimitiveRelationId` SHALL be the `Domain::Value` digest of a canonical preimage over the relation's **entire** contents: its judgment kind, its generator, both schema identities, and every row. It SHALL NOT be a name, an index, an allocated constant, or any identifier assigned independently of what the relation contains.

```text
PrimitiveRelationId = Digest(Domain::Value,
  write_bytes  b"brix.kernel.primitive-relation"
  write_uint   1                                  // format version
  write_enum   judgment_kind ordinal              // Typing = 0
  write_bytes  generator digest (32 bytes)
  write_bytes  source_schema      : SchemaId (32 bytes)
  write_bytes  destination_schema : SchemaId (32 bytes)
  write_set    { write_bytes(src digest) ++ write_bytes(dst digest) }
)
```

Rows are written as a **set** (`write_set`: sorted by canonical element bytes, deduplicated, count-prefixed), not a list, because a relation *is* a set of pairs — authoring order must not be part of its identity.

### ⟨D-SCHEMAID⟩ A schema identity is its own frozen header

> A `SchemaId` SHALL be the `Domain::Value` digest of the schema's already-frozen marker and version, and nothing else:
>
> ```text
> SchemaId = Digest(Domain::Value, write_bytes(marker) ++ write_uint(version))
> ```

No new frozen string is minted: `ArithTypingInputV1`'s id is derived from `ARITH_TYPING_INPUT_MARKER_V1` and `ARITH_TYPING_INPUT_VERSION_V1`, which #280 already froze. A schema's id therefore cannot drift from the bytes the schema actually writes, and a v2 of any schema necessarily changes the id of every relation that uses it.

### Rationale

**It makes ADR-0015 §7's immutability structural rather than a discipline.** §7 requires that "adding, removing or changing a row does not update `TypingArithV1` — it allocates `TypingArithV2`", because "otherwise identical certificate bytes would mean different things under different kernel releases." Under a *named* id that is a rule a reviewer must remember and can forget. Under a content-derived id, editing a row changes the id by construction; a stale id cannot survive a semantic edit, and a certificate that names `TypingArithV1` provably refers to the exact row set that existed when it was issued.

**It answers ADR-0019 §6's gap in the same shape.** That section records that a caller still supplies the `GeneratorSemantics`, so a verified artifact proves the predicate was *executed*, not *authenticated*, and "its id does not name the semantics or registry it was checked against." A content-derived id does name them: the digest covers the semantics, the schemas, and the judgment scope. If the settlement receipt adopts the same preimage shape — marker, version, scope ordinal, subject digest, schema ids, content set — the two lanes share one identity discipline rather than two.

**It costs legibility, and that is paid for elsewhere.** `f285a12c…` says nothing to a reader. The mitigation is that the complete row set is frozen in a vector in **readable** form — operator, operand types, promotion paths, result — beside each row's digests, so a diff is auditable by eye. A hundred-and-twenty opaque hex pairs would be a fence nobody can actually read.

### Rejected alternatives

- **A named constant (`"TypingArithV1"` hashed as a string).** Cheap and legible, but it decouples the id from the contents, which is precisely the coupling §7 exists to create.
- **A monotonic registry index.** Worse: two kernel releases could disagree about what index 3 means with no detectable difference.
- **Hashing only the schemas and generator, not the rows.** Would let a row be added without changing the id — the exact failure §7 names.

---

### ⟨D-LOSSYROW⟩ Lossy promotion paths are admitted as rows of `TypingArithV1`

> A row whose promotion path contains a lossy coercion edge SHALL be admitted to a **typing** relation. Admitting it discharges nothing about the edge itself.

`Div` routes integer division through `field_of(Int) == Float`, so `7 / 2` travels the `Int ↪ Float` edge, which Stage B0 tags `CoercionKind::Lossy`. Those rows are included, for ⟨D-JUDGE⟩'s reason: a typing discharge never claims a value, evaluation, or exactness property, and `7 / 2 : Float` is a correct typing judgement for a language whose declared rule is `Int/Int → Float`. ⟨D-PROMOTE⟩'s prohibition is on discharging `Int→Float` *as an embedding or promotion* — the `g_promote_edge` family, a different generator and a different proposition, which Stage E owns. Exactness is bound into the row's `src` bytes, so the relation can never accept a lossy path where an exact one was claimed.

**The counter-reading is real and is recorded rather than buried.** ADR-0015 §5 Stage B0 defines a promotion path as "an ordered sequence of **exact** promotion-edge ids". Read strictly, that excludes these rows. It matters which reading wins *now* rather than later, because §7 makes the row set immutable: excluding them would mean integer `Div` can never be discharged at the typing level without allocating a `TypingArithV2`. If the strict reading is preferred, say so before this vector is treated as settled.

## 3. What this does not establish

- It does not make any relation trustworthy. The digest identifies a relation; it does not authorize one. Authorization is that the relation is compiled into the kernel (ADR-0015 §8.3: no caller-authorized facts).
- It does not create a signing or delegation route. There is still no key, and adding a relation is still a kernel release (§8.4).
- It says nothing about evaluation. The judgment kind is inside the identity precisely so that it cannot (⟨D-JUDGE⟩, §8.6).

---

## 4. Finding — Stage D's headline gate is not reachable as ADR-0015 states it

**This is reported, not fixed**, per the standing instruction on #53 that a discovery of this kind should be surfaced rather than quietly worked around.

### 4.1 What was found

A registry row is matched by canonical bytes, so the kernel must be able to author **both** endpoints of every row. It can only do that for schemas it owns: reproducing `soc_regimes::Ty`'s encoding inside `brix-kernel` would be a second semantic encoder for a type the TCB does not own, which ADR-0015 §8.5 refuses to trust and `DEPS.md` forbids outright.

Stage B0 solved this for the source endpoint by moving `ArithTypingInputV1` into the kernel. Stage B does the same for the destination (`NumericResultTypeV1`, as §5 Stage B anticipated). But the arithmetic sub-derivation must still begin and end in the regime's own vocabulary, so each conversion needs a leaf:

```text
g_arith_split    Expr(node)            → Prod(Expr a, Expr b)      tight (Stage C)
Tensor(da, db)                                                      operand derivations
g_arith_input    Prod(Type a, Type b)  → Atom(ArithInput)           regime → kernel bridge
g_arith          Atom(ArithInput)      → Atom(ArithResult)          kernel-checked (Stage B)
g_arith_result   Atom(ArithResult)     → Atom(Type(result))         kernel → regime bridge
```

Neither bridge is dischargeable by ⟨D-PRIM⟩'s mechanism, for the same reason and in mirror image: each has exactly one endpoint that is a `Ty` atom the regime encodes.

So ADR-0015 §5 Stage D gate 1 — "`let x = 1 + 2` reaches `HasType(x, Int) @Proven`" — **cannot be met by discharging `g_arith`**, however completely Stage B succeeds. `1 + 2` was capped by two undischarged leaves before Stage B and is capped by two after it.

This is a gap in the ADR rather than in the implementation: ADR-0015 was written before Stage B0 introduced the first of these bridges. `g_arith_input`'s doc anticipated part of it ("Stage D needs *two* relations rather than one"), but a second relation is necessary and **not sufficient** — the obstruction is not the count of relations, it is who owns the endpoint encoding.

### 4.2 What Stage B is nonetheless worth

The residue changed character, and that is the whole gain. Before, three undischarged arithmetic leaves each carried a *semantic* claim, chief among them that `Div`'s result rule differs from the other three operators'. Now that claim is a membership decision a checker executes over 120 rows of kernel-owned data, and what remains outstanding is two vocabulary renamings that assert nothing about arithmetic. One nameable problem has replaced several unnamed ones.

Grades are unchanged in both directions: `1 + 2` is still `@Audited`, and Stage C's `g_arith_split` discharge does not lapse.

### 4.3 The question that needs a ruling

> Who owns the canonical encoding of a realization endpoint that both the TCB and a regime must name?

Sketched options, none of them decided here:

1. **Move the endpoint vocabulary into Ring 0.** If `brix-semantic` owned the canonical encoding of the type atoms that appear as realization endpoints, both the kernel and the regime would encode them through one encoder, both bridges would disappear, and `g_arith` would connect directly to the surrounding derivation. Largest blast radius; touches shared Ring-0 ABI.
2. **A kernel rule for vocabulary correspondence.** A primitive relation whose rows pair a kernel schema value with a regime configuration id — which requires the kernel to accept regime-authored bytes as row data, and so runs directly into §8.3's caller-authorized-facts prohibition. Probably a non-starter, recorded so the next reader does not have to rediscover why.
3. **Accept the cap and re-scope Stage D.** Rewrite Stage D's gate 1 to target what is actually reachable — a kernel-checked `g_arith` leaf inside an honestly-`@Audited` result — and open the `@Proven` goal as its own work item under option 1.

Recommend (1) as the eventual answer and (3) as the immediate one, so ADR-0015's Stage D is not left with an unmeetable gate.

---

## 5. Compatibility

- `TermKind::PrimRealizes` takes the next unused append-only ordinal, **17**. No existing ordinal is renumbered or reused, including `Unsupported = 9`.
- Envelope version stays **v1**, per ADR-0015 §7's resolution against ADR-0013: the envelope's field list is untouched and a certificate still binds the same context, proposition, and term.
- `vectors/kernel_certificate_v1.json` and `vectors/arith_typing_input_v1.json` are unchanged. `vectors/primitive_relation_typing_arith_v1.json` is new and additive, guarded by three consumers (frozen manifest, primitive-`CanonWriter` reconstruction, and an independently declared matrix).
- Old certificates whose leaves are `Hyp` remain assumption-dependent and remain capped. Shipping the registry upgrades nothing retroactively (ADR-0015 §7).

## 6. Open decisions

- Whether the settlement lane adopts ⟨D-RELID⟩'s preimage shape for ADR-0019 §6's receipt identity, or proposes a different shared scheme. This ADR does not presume the answer; it exists so the two can be compared before either freezes further vectors.
- §4.3's ruling on endpoint-vocabulary ownership. Blocking for ADR-0015 Stage D, not for Stage B.
