# ADR-0015 — Judgment-Scoped Tightness, Kernel-Checked Primitive Realizations, and the Arithmetic Typing Rule

Status: **Proposed** (2026-08-07). Pins what a tight-generator discharge *means*, replaces the informal correspondence argument with a kernel-checked mechanism, and rules on `g_arith`, `g_arith_split`, and the numeric promotion edges.

Date: 2026-08-07.

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

> The kernel SHALL own a frozen, finite **primitive-realization table** and a proof-term constructor that discharges a `Prop::Realizes(g, src, dst)` leaf **iff** that exact triple is in the table. A generator is tight for a judgment when its every emitted leaf is discharged by that constructor.

This replaces the doc-comment correspondence argument with a mechanism a checker executes. It generalises past arithmetic: it is the general discharge route for PD-1's tight generated subcategory, which ADR-0002 §10 currently discharges only *operationally* per committed witness.

Constraints:

- The table is **kernel-owned**. A host-side computation followed by a generic kernel rule that trusts the computed result is **not** sufficient — that would move the trust boundary back out of the TCB.
- Every material field must be bound into what the kernel checks: for arithmetic, the operator, both operand types, the result type, and every promotion edge.
- Adding a `TermKind` constructor uses an **append-only ordinal** (the existing discipline: "append-only ordinals AFTER Sum=3"). Existing frozen certificate vectors are unaffected because their terms do not use the new ordinal; ADR-0013's v1 envelope does not move.
- Expanding the kernel expands the TCB. That is the cost, and it is why this is an ADR rather than a code change.

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

### Stage B — the kernel primitive-realization table ⟨D-PRIM⟩

Add the frozen table and the term constructor. Populate it with the arithmetic typing relation: the finite matrix over `{Add, Sub, Mul, Div}` × supported numeric operand types, with result types and admissible promotion edges.

Gates:

1. `arithmetic_rule_is_a_kernel_primitive` — for the **exhaustive** finite matrix (not a sample): invoke the real generator, elaborate its real proof term, submit to the real kernel, assert `Accepted` for the precise `HasType` conclusion, and compare premises and result type against the kernel relation rather than a snapshot string.
2. `arithmetic_rule_binds_all_material_fields` — mutate one field at a time (`+`→`/`, an operand type, the result type, a promotion edge, operand order, the expression identity); each mutation must **not** be `Accepted`. Assert `Accepted` vs not-`Accepted` — never manufacture `Refuted`.
3. `arithmetic_rule_has_no_unchecked_join` — `Float` mixed with `Rat`, where the lattice has no join, yields no accepted arithmetic typing proof.
4. Frozen certificate vectors unchanged; new `TermKind` ordinal appended.

### Stage C — discharge `g_arith_split` ⟨D-SPLIT⟩

Gate: `arithmetic_split_rule_is_a_kernel_primitive` — splitting yields exactly two ordered child obligations, preserves context and operator, does **not** choose a promotion or synthesise a result type, and rejects malformed arity or a forged child.

### Stage D — discharge `g_arith` for typing, and pin the rendering ⟨D-JUDGE⟩

Add `g_arith` to the typing tightness index. Gates:

1. `let x = 1 + 2` reaches `HasType(x, Int) @Proven`, and the kernel certificate names that exact proposition and context.
2. `arithmetic_typing_proof_does_not_publish_evaluation` — no `EvaluatesTo` judgement is produced or implied. **This is a constitutional test, not a UI preference.**
3. The CLI renders the proposition, never a bare expression-plus-grade. `brix why` continues to distinguish provenance from proof.
4. `brix prove`'s composition-versus-result explanation is updated: with `g_arith` discharged, `1 + 2` is no longer capped, so the existing wording naming `g_arith` as a holdout must go — and must not be replaced by wording implying the *value* is proven.

### Stage E — exact promotion edges ⟨D-PROMOTE⟩

Per-edge typing discharge for the four exact edges; relocate `Int→Float` to an explicitly lossy family. A value-level exact-edge discharge additionally requires totality, denotation preservation, injectivity, canonicality, path coherence, and kernel ownership — deferred until those value domains exist canonically.

## 6. What a future arithmetic *value* layer must supply

Recorded so the boundary is not rediscovered. An honest `1 + 2 ⇓ 3 @Proven` needs a kernel-owned arithmetic value relation — not a general evaluator. For checked integer addition, the kernel must independently: decode canonical operands; verify their numeric constructors and widths; compute or check the mathematical operation without host overflow ambiguity; verify representability; verify the canonical result bytes; bind operation and operands to the exact expression being settled; and return `Accepted` only for the correct relation. A proposed result of `4` for `1 + 2` must not be accepted, and overflow must not be silently wrapped unless wrapping is explicitly adopted as versioned language semantics.

Per ⟨D-OPGRAN⟩ the rollout is operation-specific: checked `Nat`/`Int` addition; subtraction with explicit domain rules for negative results on `Nat`; multiplication; division only after its result and fault semantics are separately settled; then rational/real/complex/float only once those value domains exist canonically. The integer canonical-encoding erratum (`spec/errata/0001-integer-canonical-encoding.md`) must be resolved and version-pinned before those encodings become the identity basis of an arithmetic proof rule.

## 7. Compatibility and evolution

- New `TermKind` ordinals are append-only; ADR-0013's v1 certificate envelope does not move, and existing frozen vectors must not be re-blessed.
- The primitive-realization table is a frozen artifact once populated: an entry may not change in place without a new table version.
- Widening the table beyond typing — to any value or evaluation relation — requires a new ADR, because it changes what a kernel `Accepted` verdict means.

## 8. Open decisions

- **⟨D-PRIM⟩'s exact term-constructor shape** is not pinned here. Whether the kernel gains a dedicated `TermKind::Prim(GeneratorId, src, dst)` or reuses `Prop::Applied` with a frozen predicate id is an encoder-review question for the `brix-kernel` lane, and the choice is ABI-visible. Pin it before Stage B is implemented.
- **Whether `generator_is_tight`'s claim-kind index should be a closed enum or an open identifier** — closed is safer now (an unknown kind cannot silently default to "discharged"), open is cheaper when settlement and coverage judgments join. Recommend closed; revisit if a third kind arrives.
- **Whether the four exact promotion edges warrant one table entry each or a single parameterized entry.** Per-entry is more auditable; parameterized is smaller. Not load-bearing for correctness either way.
