# ADR-0015 — Judgment-Scoped Tightness, Kernel-Checked Primitive Realizations, and the Arithmetic Typing Rule

Status: **Accepted** (2026-08-08; Proposed 2026-08-07 — ⟨D-PRIM⟩'s constructor and registry-location decisions pinned after an external kernel-design consult, and the envelope-version question resolved against ADR-0013). Pins what a tight-generator discharge *means*, replaces the informal correspondence argument with a kernel-checked mechanism, and rules on `g_arith`, `g_arith_split`, and the numeric promotion edges.

Date: 2026-08-07; revised 2026-08-08; §5 errata and Stage D gate 1 re-scope 2026-08-15, per the maintainer ruling on #53 (R2, R3) after Stages B0/B/C/E landed. The errata are inline and additive: no decision is rewritten, and every corrected sentence is left in place beside its correction.

Foundation: [ADR-0002](./ADR-0002_SOC_Constitution.md) §4.1 (authority table), §5.3 (fail closed), §10 PD-1 (the tight generated subcategory); [ADR-0003](./ADR-0003_Proof_Kernel_Profile.md) and [ADR-0013](./ADR-0013_Canonical_Certificate_Envelope.md) (kernel profile and frozen certificate envelope); `crates/soc-regimes/src/type_realization.rs` (`generator_is_tight`, `honest_result_outcome`); `crates/brix-elaborate/src/lib.rs` (`elaborate_tree`). Governs issue #53.

---

## 1. Context: what is actually broken

`generator_is_tight` records which typing generators have had their realization semantics discharged. A derivation's result grade is capped by its least-discharged leaf (`honest_result_outcome`). Sixteen generators are discharged; `g_arith`, `g_arith_split`, and the `g_promote_edge` family are not, so `let x = 1 + 2` is honestly `@Audited`.

Two defects in that arrangement, both surfaced while ruling on `g_arith`.

### 1.1 The stated reason for the holdout does not survive its own consistency check

The doc comment holds `g_arith` out because it "assert[s] *operation/representation* semantics … and [has] no established value semantics yet." But **`g_float_lit` is discharged**, and floats are not merely lacking a value semantics — `brix-canon` **deliberately excludes** floating point from `Canonical`, and ADR-0012 §3.2 rejects float literals from the executable fragment outright.

So the project already discharges a generator whose value semantics are excluded by design. Either tightness is scoped to a judgment, or `g_float_lit` has been over-graded since it landed. It has not been over-graded — which means the scoping was always implicit, and the holdout reasoning for `g_arith` was stated against the wrong proposition.

### 1.2 Discharge is currently an argument in a doc comment, not a checkable fact

`elaborate_tree` turns every leaf into a **hypothesis**:

```rust
h_props.push(Prop::Realizes(g_term, src_term, dst_term));
```

The kernel proves the *composition* — `leaves ⇒ conclusion` — and never checks that any leaf's realization actually holds. That is precisely why the result must be capped by the least-discharged leaf, and it is correct as far as it goes.

But it means "discharged to tight" is currently established by **prose**: `g_var` ↔ `Hyp`, `g_app2` ↔ modus ponens, argued in a doc comment and pinned only by tests that check the *emission shape*. The existing discharges are sound — those correspondences are real — but the mechanism does not scale and cannot be audited. `g_arith` has no kernel primitive to correspond to at all, so under the current mechanism its discharge could only ever be an assertion.

## 2. Decision

Three decisions, in dependency order.

### ⟨D-JUDGE⟩ Tightness is judgment-scoped

> A generator discharge is **relative to a proposition kind**. Discharging generator `g` establishes soundness of `g`'s realization **for the judgment in which it appears**, and for no other judgment.

Concretely: `Γ ⊢ e₁ op e₂ : T` and `e₁ op e₂ ⇓ v` are different propositions. The typing judgment is compatible with mathematical, checked, wrapping, saturating, or arbitrary total interpretations of `op` — so a typing rule **cannot** entail the evaluation equation, and discharging it does not claim to.

**Normative:**

> Kernel acceptance of a typing rule discharges only `HasType`. It never discharges evaluation, value equality, totality, progress, or termination.

This is why `g_float_lit` is honestly tight: it discharges `HasType(3.14, Float)`, not `3.14 ⇓ <bits>`. §1.1's inconsistency is thereby resolved in favour of the existing discharge, not against it.

**Two obligations follow, and both are load-bearing:**

1. **The invariant currently holds by absence, not construction.** There is no evaluation claim kind in the workspace, so nothing today can misuse `generator_is_tight`. That is not a guarantee — it is the same shape as the declared-but-unreachable variants of #254. §5.1 requires that the tightness registry become claim-kind-indexed *before* any evaluation judgment exists, so a future evaluator cannot inherit typing's discharges by default.
2. **A grade must never be rendered against an ambiguous proposition.** `1 + 2 : Int @Proven` is honest. A bare `1 + 2 @Proven` is not, because a reasonable reader takes it to mean the expression evaluates correctly. The current CLI already renders the type (`r : Int @Proven`, `status: Proven — r : Int`); §5.4 pins that with a test rather than leaving it to style.

### ⟨D-PRIM⟩ Primitive realizations become kernel-checked facts, not prose

> The kernel SHALL own a **compiled-in, immutable, judgment-scoped primitive-relation registry** and a dedicated zero-premise introduction term
>
> ```text
> PrimRealizes { relation: PrimitiveRelationId, src: ObjectTerm, dst: ObjectTerm }
> ```
>
> which synthesizes `Prop::Realizes(g, src, dst)` **iff** the registry resolves `relation` and `(src, dst)` is an exact member of its frozen rows.

This replaces the doc-comment correspondence argument with a mechanism a checker executes. It generalises past arithmetic: it is the general discharge route for PD-1's tight generated subcategory, which ADR-0002 §10 currently discharges only *operationally* per committed witness.

**The relation identity fixes the generator; the caller does not supply it.** Each `PrimitiveRelationId` resolves to an immutable descriptor:

```text
PrimitiveRelation { judgment_kind, generator, source_schema, destination_schema, rows }
```

so a typing relation is *structurally incapable* of synthesizing an evaluation generator's `Realizes`. This is the mechanical enforcement of ⟨D-JUDGE⟩ — the scoping lives in the relation identity, never in a comment or naming convention.

The synthesis rule is closed and consults no hypothesis context. Given `K[ρ] = (J, g, S, D, R)`, `src` canonical under `S`, `dst` canonical under `D`, and `(src, dst) ∈ R`, it yields exactly `Realizes(g, src, dst)`. Checking against an expected proposition uses the normal synthesize-then-compare path under existing canonical structural equality; there is no expected-mode shortcut that reconstructs fields from the caller's goal.

**`Prop::Applied` is rejected as the carrier.** It has no checking rule today — one occurrence in the entire checker, inside a free-variable scan — so it would need a new introduction rule *and* a bridge to `Realizes` *and* rules preventing that bridge from firing for the wrong predicate. The bridge would be the real primitive rule; `Applied` would add indirection and a larger generic-axiom surface while reducing nothing. It stays inert until it has independently justified semantics.

**The registry lives in the TCB.** A new primitive relation is a kernel release. That is the correct cost: a generator whose realization is not derivable from existing kernel rules *is* a new trusted axiom, and no honest mechanism adds one without increasing trusted semantic content somewhere. The executable logic need not grow much — one small exact-membership rule — while the trusted *data* grows declaratively.

- **Caller-supplied tables are rejected as circular.** "The caller says this realization is valid; the kernel checks the caller included it in the caller's table" does not move authority into the kernel, it serializes the original assertion. Schema validation and canonical encoding show a table is well-formed, not that its contents are authorized.
- **A digest-pinned hybrid is sound but not selected.** With kernel-allowlisted digests covering relation identity, judgment kind, generator, schema versions and every canonical row, plus kernel-owned parsing and fail-closed behaviour, the digest is a compact trusted commitment and semantic trust stays in the kernel. It is rejected on cost, not soundness: another parser and canonicalization path, a collision-resistance assumption, more loading/DoS surface — and no added flexibility, since each new digest still requires a kernel release.
- **No signing key.** A key that could authorize new tables without a kernel release would be a delegated semantic authority inside the TCB, able to widen what counts as `Proven` without changing the kernel. That would need its own constitutional ADR.

Other constraints:

- Adding a `TermKind` constructor uses the **next unused append-only ordinal**; no existing ordinal is renumbered or reused, including `Unsupported`. Existing certificate bytes and frozen vectors are unchanged; new vectors are added.
- **Relation identities are immutable.** Adding, removing or changing a row does not update `TypingArithV1` — it allocates `TypingArithV2`. Otherwise identical certificate bytes would mean different things under different kernel releases.
- The minimum semantic partition is one relation per **judgment kind × generator × schema version**. One physical registry artifact is fine as packaging; one undifferentiated global set of `Realizes` triples is not.
- **No implicit promotion closure.** If a two-edge promotion path is admissible, either that exact path appears in the row or another explicit kernel rule validates it. The host cannot assert transitivity and have it trusted.

### ⟨D-ARITH⟩ `g_arith` is dischargeable for typing, and only for typing

> `g_arith` SHALL be dischargeable as a **typing** generator once §5.2's kernel rule lands. It SHALL NOT be treated as discharged for any value, evaluation, or totality claim.

Totality does not obstruct the typing discharge. `Expr::Lit` is an `i64`; `i64::MAX + 1` overflows; division adds an independent partiality at zero. None of that bears on `Γ ⊢ e₁ op e₂ : T`. A language may type an operation that later traps: under ADR-0002 §5.3 such a case simply obtains no evaluation certificate and stays `Unknown`. **Typeability implies neither settlement nor progress nor termination.**

One parameterized generator for all four operators is acceptable *for typing*, provided the operator is bound into the checked proof and the kernel's result-type relation is correct per operator. It is **not** acceptable for values — see ⟨D-OPGRAN⟩.

### ⟨D-SPLIT⟩ `g_arith_split` is dischargeable independently, and earlier

> `g_arith_split` SHALL be discharged on the same structural grounds as `g_record_split` / `g_field_split` / `g_ctor_split` / `g_match_split`, independently of `g_arith`.

Its claim is purely structural — an arithmetic node contains this operator and these two ordered subexpressions, and typing it yields these two child obligations in the same context. Its emitted leaf carries no operation or representation claim:

```rust
src: Atom(Expr(e)),  dst: Prod(Atom(Expr(a)), Atom(Expr(b)))
```

**Conditional:** this holds only while the split is purely structural. If `g_arith_split` ever selects a promotion, synthesises a result type, or filters operations by unchecked host logic, those parts inherit `g_arith`'s evidence burden and the discharge lapses. §5.3 pins the condition.

Discharging the split while `g_arith` remains capped is safe: the least-discharged leaf still caps the derivation.

### ⟨D-OPGRAN⟩ Value-level arithmetic is per-operation or nothing

> A future arithmetic **value** relation SHALL be introduced per operation. A single undifferentiated value-level arithmetic generator is forbidden.

Proving checked addition proves nothing about division. Under the least-evidence rule, one generator spanning `+`/`-`/`*`/`/` must remain non-tight until every branch is covered — so it should never be created. §6 states what a value layer must supply.

### ⟨D-PROMOTE⟩ Exact and lossy coercion edges are different claims

> The exact edges `Nat→Int`, `Int→Rat`, `Rat→Real`, `Real→Complex` SHALL be individually dischargeable. **`Int→Float` SHALL NOT be discharged as an embedding or promotion**, now or later.

A lossy map is not injective and does not preserve numeric identity. Declaring `Float` incomparable to the exact branch (`join(Float, Rat) = None`) prevents one mixing ambiguity; it does not make a lossy map exact.

`Int→Float` may eventually be discharged as a **different proposition** — *converting integer `i` under float format `F` and rounding mode `R` yields exactly these bits* — requiring a specified width, rounding mode, overflow/infinity behaviour, canonical zero and exceptional handling, and a durable float value identity. `brix-canon` excludes floats from `Canonical`, so that route is unavailable today.

**Consequence:** if `g_promote_edge` carries one shared tightness bit across all edges, it cannot be discharged at all while the lossy edge is in the family. The `NUMERIC` lattice mints per-edge generator ids (`generator_prefix: "type.rule.num.promote"`), so the exact edges can be handled individually — and `Int→Float` should move to an explicitly-labelled lossy-conversion family rather than sitting in a lattice called `NUMERIC`'s promotion edges.

At the typing level an exact edge's finite relation may be discharged as soon as the kernel owns it. That establishes the coercion term's admissibility, **not** a successful executable conversion.

## 3. What this does NOT establish

Stated explicitly so nothing over-reads it. A typing discharge for `g_arith` does not establish:

- that `1 + 2` evaluates to `3`, or that any arithmetic expression terminates or settles;
- progress or preservation for a future evaluator;
- overflow, division-by-zero, or signed-minimum-over-negative-one behaviour;
- the quotient convention or result type of a future executable division beyond what the typing rule states;
- executable `Rat`/`Real`/`Complex`/`Float` values, or canonical float identity;
- exactness of `Int→Float`;
- correctness of the host-side numeric lattice table, unless the kernel independently checks it;
- `Refuted` for any unsupported, malformed, overflowing, or undefined case — those remain `Unknown`.

## 4. Non-goals

- A general evaluator, an arithmetic value semantics, or an execution fragment containing arithmetic. ADR-0012's L3 profile deliberately excludes `Expr::Bin`; this ADR does not change that.
- Re-opening ⟨D-FO⟩, the outcome lattice, the certificate envelope, or any frozen ABI.
- Discharging `g_match_catchall`, which is held out for an unrelated reason (repeated branch premises are not represented explicitly).

## 5. Staged implementation and acceptance

### Stage A — claim-kind-index the tightness registry

Make `generator_is_tight` take the judgment kind it is being asked about, so a future evaluation judgment cannot inherit typing's discharges. Do this **first**, while there is exactly one claim kind and the change is mechanical.

Gate: `honest_result_outcome` consults the typing index only; a second, empty claim kind exists and returns "not discharged" for every generator; a test asserts a generator tight for typing is **not** tight for the empty kind.

### Stage B0 — re-schema the `g_arith` source object ⟨D-PRIM⟩ **(blocking prerequisite)**

**The registry cannot be populated from the current emission.** `g_arith`'s leaf is:

```rust
src: Prod(Atom(Type(result_ty)), Atom(Type(result_ty))),
dst: Atom(Type(result_ty)),
```

It encodes **neither the operator, nor the original operand types, nor the promotion paths** — both operands appear already coerced to the result type, and the promotions are spliced in as separate coercion leaves.

That is not a cosmetic gap. `Div` has a *different* result-type rule from the other three (`field_of`: `Int/Int → Float`, `Nat/Nat → Float`), so `1.0 + 2.0` and `7 / 2` currently emit the **identical** leaf `Prod(Type(Float), Type(Float)) → Type(Float)`. A table keyed on `(operator, lhs, rhs, promotions) → result` cannot be reached from that source object, and a table keyed on what the source object *does* carry could not distinguish addition from division.

This is harmless today — the leaf is a hypothesis and the grade is capped — and it is precisely why the discharge is unavailable. So: **if the source representation does not encode every field that affects admissibility, the primitive MUST NOT be discharged.** Version the source-object schema first:

```text
ArithTypingInputV1(operator, lhs_type, rhs_type, lhs_promotion_path, rhs_promotion_path)
```

where a promotion path is an ordered sequence of exact promotion-edge ids and the empty path is identity; `dst` is the exact result type.

> **Erratum (2026-08-15).** "an ordered sequence of **exact** promotion-edge ids" is inaccurate and should be read as **"an ordered sequence of coercion-edge ids, each carrying its declared exactness."** The paths this schema carries were never restricted to exact edges — `Div` routes integer division through `field_of(Int) == Float`, so `7 / 2`'s path crosses `Int ↪ Float`, which this same stage tags `CoercionKind::Lossy`. Since Stage E ([ADR-0024](./ADR-0024_Lossy_Coercion_Family.md) ⟨D-LOSSYFAMILY⟩) relocated that edge out of the promotion family, they are not sequences of *promotion*-edge ids either.
>
> The wording had a live consequence: read strictly, it excluded integer division from `TypingArithV1`, and §7 makes a row set immutable, so exclusion would have left a whole operator undischargeable at the typing level without allocating a further version. [ADR-0023](./ADR-0023_Primitive_Relation_Identity.md) ⟨D-LOSSYROW⟩ recorded the ambiguity rather than resolving it silently; the maintainer ruling on #53 confirmed inclusion, on ⟨D-JUDGE⟩'s grounds that a typing relation claims typing and asserts nothing about exactness, value, or evaluation. The rows stay, and this reading is no longer available to re-litigate.

Gate: the emitted `g_arith` leaf round-trips every material field; a fixture proves `1.0 + 2.0` and `7 / 2` now emit **distinguishable** leaves; existing typing results and grades are unchanged (this stage moves no grade).

### Stage B — the kernel primitive-relation registry ⟨D-PRIM⟩

Add the registry and the `PrimRealizes` constructor. Populate `TypingArithV1` — `judgment_kind = Typing`, `generator = g_arith`, schemas `ArithTypingInputV1`/`NumericResultTypeV1` — with the finite matrix over `{Add, Sub, Mul, Div}` × supported operand types, with result types and admissible promotion paths. Rows must satisfy a build-time functionality invariant: one canonical `src` never maps to two result types.

Gates:

1. `arithmetic_rule_is_a_kernel_primitive` — for the **exhaustive** finite matrix (not a sample): invoke the real generator, elaborate its real proof term, submit to the real kernel, assert `Accepted` for the precise `HasType` conclusion, and compare premises and result type against the kernel relation rather than a snapshot string.
2. `arithmetic_rule_binds_all_material_fields` — mutate one field at a time (`+`→`/`, an operand type, the result type, a promotion edge, operand order, the expression identity); each mutation must **not** be `Accepted`. Assert `Accepted` vs not-`Accepted` — never manufacture `Refuted`.
3. `arithmetic_rule_has_no_unchecked_join` — `Float` mixed with `Rat`, where the lattice has no join, yields no accepted arithmetic typing proof.
4. Frozen certificate vectors unchanged; new `TermKind` ordinal appended.

### Stage C — discharge `g_arith_split` ⟨D-SPLIT⟩

Gate: `arithmetic_split_rule_is_a_kernel_primitive` — splitting yields exactly two ordered child obligations, preserves context and operator, does **not** choose a promotion or synthesise a result type, and rejects malformed arity or a forged child.

### Stage D — discharge `g_arith` for typing, and pin the rendering ⟨D-JUDGE⟩

**A boolean whitelist flip is the wrong mechanism, and MUST NOT be used.** After ⟨D-PRIM⟩,

```text
generator_is_tight(g_arith) == true
```

is too coarse to be authoritative: it would regrade certificates whose leaves are still `Hyp`. **Merely shipping the registry must not retroactively upgrade an old proof.**

Normatively:

> The evidence-bearing identity is the exact primitive relation **and the actual kernel-accepted proof term**. A leaf is closed only when the certificate contains the `PrimRealizes` term for that leaf and the kernel Accepted the resulting closed proof. `generator_is_tight` survives at most as a **non-authoritative capability hint** — "this generator *can* be discharged" — never as the evidence itself.

So `honest_result_outcome` must stop asking "is this generator in a set?" and start asking "was this leaf actually closed by an accepted primitive instance?"

> **Amendment (2026-08-15) — gate 1 re-scoped, and why.** Gate 1 previously read: "`let x = 1 + 2` reaches `HasType(x, Int) @Proven`, and the kernel certificate names that exact proposition and context." That gate is **not reachable by discharging `g_arith`**, however completely Stage B succeeds, and it was unmeetable at the moment it was written down.
>
> This ADR predates Stage B0. A registry row is matched by canonical bytes, so the kernel must author *both* endpoints of every row, so both must be schemas it owns — which means the arithmetic sub-derivation now enters kernel vocabulary through `g_arith_input` and leaves it through `g_arith_result`, and each of those has one endpoint that is a `Ty` atom `soc-regimes` encodes. Reproducing that encoding in the TCB is the second semantic encoder §8.5 refuses to trust and `DEPS.md` forbids. So `1 + 2` was capped by two undischarged leaves before Stage B and is capped by two after it; only the *character* of the residue changed, from semantic claims to vocabulary renamings. [ADR-0023](./ADR-0023_Primitive_Relation_Identity.md) §4 reports the finding in full.
>
> Gate 1 is therefore re-scoped to what discharging `g_arith` actually buys, and the `@Proven` goal moves out to its own work item under the endpoint-vocabulary ruling (ADR-0023 §4.3 option 1, which ADR-0025 will pin). **Gates 2–4 stand: none of them depended on the cap moving** — except for one clause in gate 4, corrected inline below. This is a narrowing of what Stage D claims, not a weakening of what it must check: nothing here relaxes ⟨D-JUDGE⟩, and a leaf is still closed only by an accepted `PrimRealizes` term.

Gates:

1. `arithmetic_leaf_is_closed_by_an_accepted_primitive_instance` — for `let x = 1 + 2`, the certificate contains the `PrimRealizes` term closing the `g_arith` leaf, the kernel Accepted the resulting proof, and `honest_result_outcome` reports the leaf as closed. The result stays honestly `@Audited`, because the two vocabulary-bridge leaves either side of it are still `Hyp`, and a test asserts exactly that rather than tolerating it.
2. `arithmetic_typing_proof_does_not_publish_evaluation` — no `EvaluatesTo` judgement is produced or implied. **This is a constitutional test, not a UI preference.**
3. The CLI renders the proposition, never a bare expression-plus-grade. `brix why` continues to distinguish provenance from proof.
4. `brix prove`'s composition-versus-result explanation is updated: with `g_arith` discharged, `1 + 2` is no longer capped, so the existing wording naming `g_arith` as a holdout must go — and must not be replaced by wording implying the *value* is proven.

   > **Erratum (2026-08-15).** "`1 + 2` is no longer capped" is wrong, for the reason the gate-1 amendment above gives: the cap moves to `g_arith_input` and `g_arith_result`. The rest of the gate survives unchanged and is if anything sharper — `g_arith` genuinely stops being a holdout, so wording that names it as one must still go, it must still not be replaced by wording implying the value is proven, **and** the explanation must now name the two bridge leaves as what actually caps the result. A rendering that dropped `g_arith` without naming its replacements would leave the user unable to tell a capped result from an uncapped one.

### Stage E — exact promotion edges ⟨D-PROMOTE⟩

Per-edge typing discharge for the four exact edges; relocate `Int→Float` to an explicitly lossy family. A value-level exact-edge discharge additionally requires totality, denotation preservation, injectivity, canonicality, path coherence, and kernel ownership — deferred until those value domains exist canonically.

> **Erratum (2026-08-15).** The relocation half landed (#283). The **per-edge typing discharge half has no subject**, and requiring it here was an error introduced by this ADR's own Stage B0. Stage B0 replaced promotion *splicing* — one coercion leaf per edge — with promotion *data* carried inside `ArithTypingInputV1`, after which no coercion generator reaches a tree leaf anywhere in the workspace. `generator_is_tight` is therefore never consulted for a coercion edge, and a per-edge discharge would cap nothing while adding trusted TCB data with no consumer — the mechanism Stage D itself rejects. ⟨D-PROMOTE⟩'s shared-tightness-bit hazard cannot arise either: there is no bit to share.
>
> [ADR-0024](./ADR-0024_Lossy_Coercion_Family.md) ⟨D-EXACTCOVERED⟩ carries the ruling — the exact edges are already kernel-checked *as part of* the arithmetic relation, which keys on the whole `(operator, operand types, promotion paths) → result` tuple with each edge's generator id and declared exactness bound into the source bytes. This is a narrowing, not a repeal: if a future feature emits standalone coercion leaves, that feature brings the per-edge relation with it and ⟨D-PROMOTE⟩'s original shape applies unchanged.

## 6. What a future arithmetic *value* layer must supply

Recorded so the boundary is not rediscovered. An honest `1 + 2 ⇓ 3 @Proven` needs a kernel-owned arithmetic value relation — not a general evaluator. For checked integer addition, the kernel must independently: decode canonical operands; verify their numeric constructors and widths; compute or check the mathematical operation without host overflow ambiguity; verify representability; verify the canonical result bytes; bind operation and operands to the exact expression being settled; and return `Accepted` only for the correct relation. A proposed result of `4` for `1 + 2` must not be accepted, and overflow must not be silently wrapped unless wrapping is explicitly adopted as versioned language semantics.

Per ⟨D-OPGRAN⟩ the rollout is operation-specific: checked `Nat`/`Int` addition; subtraction with explicit domain rules for negative results on `Nat`; multiplication; division only after its result and fault semantics are separately settled; then rational/real/complex/float only once those value domains exist canonically. The integer canonical-encoding erratum (`spec/errata/0001-integer-canonical-encoding.md`) must be resolved and version-pinned before those encodings become the identity basis of an arithmetic proof rule.

## 7. Compatibility and evolution

- New `TermKind` ordinals are append-only; no existing ordinal is renumbered or reused, including `Unsupported`. Existing certificate bytes and frozen vectors are unchanged; new vectors are added for the new constructor.
- **Forward/backward behaviour:** new kernels continue to parse and check all old certificates. Old kernels encountering the new ordinal reject it as unknown — they MUST NOT reinterpret an unknown ordinal as another constructor. An unknown relation id under a known `PrimRealizes` constructor also fails closed.
- **Old certificates whose leaves are `Hyp` remain assumption-dependent and remain capped.** Shipping the registry upgrades nothing retroactively.
- Relation identities are immutable; a row change allocates a new id, never an in-place edit.
- Widening the registry beyond typing — to any value or evaluation relation — requires a new ADR, because it changes what a kernel `Accepted` verdict means.
- **Envelope version: stays v1. Resolved 2026-08-08 against ADR-0013.** §7 freezes "the v1 field list, their order, the marker bytes, the version number, and the profile string," and requires a new format version for "any change to *what a certificate is bound to*." A new `TermKind` ordinal changes neither: the envelope's field list is untouched, and a certificate still binds the same context, proposition, and term. What may appear *inside* a term is governed by the ordinal discipline in `term.rs`, which ADR-0013 §2 already describes as "frozen, **append-only** enum ordinals." The v1 vectors continue to reproduce byte-for-byte because their terms do not use the new ordinal.

  Recorded for the next reader: this follows from ADR-0013's append-only characterisation of `TermKind`, not from §7 directly — §7 speaks to the envelope's field list rather than to constructor-set closure. If a future reader reads §7 as freezing a closed constructor set, the correction belongs in an erratum on ADR-0013, not a change here.

## 8. Hard boundaries — what this mechanism must never be widened to do

1. **No generic arbitrary-proposition primitive.** Never `PrimProp(Prop)`, never a registry holding arbitrary `Prop` values. The constructor introduces only `Prop::Realizes`.
2. **No `Applied` laundering.** A primitive `Applied` fact must not be bridgeable into `Realizes`, `Eq`, or `Preserves` without a separately reviewed kernel rule.
3. **No caller-authorized facts.** Request data may select a kernel-owned relation and propose `src`/`dst`; it may never supply or amend the trusted relation.
4. **No mutable relation identities.** Semantic expansion requires a new id and a kernel release.
5. **No host-side semantic normalization.** Joins, coercion paths, result types, wildcard matching, transitive closure, and schema interpretation are not trusted because the host computed them.
6. **No cross-judgment reuse.** A typing relation cannot license evaluation, equality, preservation, totality, canonical value identity, or numeric correctness.
7. **No generator-only grade upgrade.** A generator's presence in a registry does not make every occurrence `Proven`; only an exact accepted primitive instance closes a leaf.
8. **No `Refuted` from absence.** "Not in the registry" means the kernel has not introduced the fact — never that its negation holds.
9. **No wildcard or pattern rows.** This authorizes finite exact relations only. An algorithmic predicate, range rule, or decision procedure is another kernel primitive needing separate review.
10. **No claim of implementation verification.** The rule proves the submitted instance belongs to the kernel's normative realization relation. It does not prove the Rust generator is complete, deterministic, total, or incapable of producing rejected outputs.

## 9. Open decisions

- **Whether the claim-kind index should be a closed enum or an open identifier** — closed is safer now (an unknown kind cannot silently default to "discharged"), open is cheaper when settlement and coverage judgments join. Recommend closed; revisit if a third kind arrives.
- **Whether the four exact promotion edges warrant one relation each or one parameterized relation.** Per-relation is more auditable; parameterized is smaller. Not load-bearing for correctness.
Both previously-blocking items are now closed: ⟨D-PRIM⟩'s constructor shape is pinned in §2, and the envelope-version question is resolved in §7. Neither remaining item gates implementation.
