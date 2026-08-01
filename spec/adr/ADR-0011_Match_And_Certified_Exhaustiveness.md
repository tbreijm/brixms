# ADR-0011 — Brix `match`: Layered Coverage & Certified Exhaustiveness

Status: **Accepted** (2026-08-01, ratified by user). The design pin for pattern matching in Brix (SOC paradigm), extending [ADR-0010](./ADR-0010_SOC_Language_Design.md) (§4 sketched `match … proving exhaustive`) and honoring the honest-outcome discipline established in the tight-generator work (a result is `@Proven` only when genuinely certified).

Date: 2026-08-01.

Foundation: [ADR-0002 SOC Constitution](./ADR-0002_SOC_Constitution.md) (epistemic lattice `Derived → Audited → Proven`), [ADR-0007/0008](./ADR-0007_Tree_Structured_Typing_Elaboration.md) (tree elaboration to the kernel), [ADR-0010](./ADR-0010_SOC_Language_Design.md).

---

## 1. Decision (layered)

Pattern matching has **two forms**, and they occupy **different rungs of the epistemic lattice**:

### Ordinary `match` — structural coverage, `@Audited`
```
match value {
  Zero    => a
  Succ(k) => b
}
```
- Compiles via a **structural coverage checker** over the pattern matrix.
- **Closed, finite (nominal) sum types only** — initially the only scrutinee kind.
- A **non-exhaustive** match is **rejected** (a compile error with diagnostics), not silently accepted.
- Coverage checking yields diagnostics and an **`Audited`** coverage result. It **does not claim `Proven`.**

### Explicit `match … proving exhaustive` — kernel-certified, `@Proven`
```
match value {
  Zero    => a
  Succ(k) => b
} proving exhaustive
```
`proving exhaustive` **must** mean, with no weaker interpretation:
- The structural checker constructs a **canonical coverage certificate**.
- The certificate is **tied to the exact** sum declaration, pattern matrix, context, and revision.
- The **proof kernel independently accepts** it (an independent re-check, not the checker's say-so).
- **Failure, unsupported patterns, or exhaustion never falls back** to "structurally good enough" — it is a hard failure (`Unknown`/error), never a silent downgrade to a bare structural pass presented as proof.
- **`@Proven` applies to the coverage proposition** ("these patterns exhaustively cover this sum in this context/revision"), **not** automatically to the match expression's result type nor to arm-body correctness. Those remain at whatever grade they independently earn.

This preserves the core promise: **ordinary code stays ergonomic; spelling `proving` always crosses a genuine proof boundary.**

## 2. Why layered (not one mechanism)

Making *every* match a kernel proof would tax ordinary code with certificate construction it never asked for, and would blur the honest-outcome line the tight-generator work drew: `@Proven` must mean *kernel-certified*, never *structurally plausible*. So ordinary `match` earns `Audited` (real replay-verified coverage, an honest strong-but-not-theorem grade), and only `proving exhaustive` pays for — and earns — `Proven`. This mirrors the numeric/grade coercion story: the strong claim requires the witness, and the witness is independently checked.

## 3. Scope of the first certified slice

Certification (the `proving exhaustive` path) is restricted, initially, to:
- **closed nominal sums** (declared `config T = A | B(…) | …`),
- **constructor patterns** (`Succ(k)`), **variable patterns** (`k`), and **wildcards** (`_`).

Explicitly **out** for now — these may **return `Unknown`** (never a false `Proven`) until their certificate rules exist:
- **guards** (`when`),
- **open sums** / extensible variants,
- **GADT-style refinements** / dependent pattern refinement,
- nested/literal patterns beyond the above.

An `Unknown` here is the honest bottom: "no certificate rule exists for this shape yet," never `false`, never a silent structural pass.

## 4. Implementation sequencing (each a slice/PR)

1. **Canonical closed sum types + constructor typing.** Add nominal closed sums to `type_realization` (canonical identity: name + variants in declaration order), constructor introduction generators, and constructor typing (arg count/types vs the variant's declared fields). Grade of a bare constructor = whatever its generators earn (Audited unless discharged).
2. **`match` typing + structural pattern-matrix coverage.** Type the scrutinee (must be a closed sum), each arm's pattern against the sum's variants, unify arm-body result types; build the structural coverage checker over the pattern matrix (redundancy + exhaustiveness). Non-exhaustive → reject.
3. **Ship ordinary exhaustive `match` as `Audited`.** Lower `ast::Expr::Match` onto the above; a well-formed exhaustive match type-checks with an `Audited` coverage result. `proving exhaustive` not yet enabled.
4. **Define the kernel coverage proposition/certificate profile.** A `Coverage`/`Exhaustive` proposition in `brix-kernel` (or an elaboration of the coverage tree to an existing kernel proposition) + a canonical certificate binding (sum decl id, pattern-matrix digest, context, revision) the kernel independently checks.
5. **Enable `proving exhaustive`.** Elaborate the structural coverage certificate through the kernel; on acceptance, the *coverage proposition* is `Proven`. Anything outside §3's certified fragment → `Unknown`.

Slices 1–3 land ordinary match (Audited) with no proof-path risk; slices 4–5 are the genuine proof boundary and get their own careful review (ORACLE-INTEGRITY: the kernel check must be independent; the checker must never mark its own homework).

## 5. What this does not change

- The honest-outcome propagation (a typing result is only as strong as its weakest generator) is untouched; coverage is an *additional* proposition with its own grade, orthogonal to the arm-body/result-type grades.
- Ordinary records/arithmetic/functions keep their current grades.
- No existing golden vector changes in slices 1–3 beyond appended enum ordinals + new generators.
