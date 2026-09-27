# ADR-0035 — Exact signed integer division, rounding, and modulo

Status: **Proposed implementation**, 2026-09-25. Follows the authorized
usability milestone after ADR-0034. The encoding and evaluation rules below
describe the implementation submitted for review, not separate semantic
ratification.

Extends the finite-decision profile from ADR-0030 and the checked-evaluation
boundary from ADR-0032. It does not widen either rule-agenda profile, does not
touch the external-input transport of ADR-0031/ADR-0033, and does not change
the SOC authority boundary.

## Motivation

`/` is refused today (`DivisionNotAllowed`), so a module that needs a ratio,
a per-unit allocation, or a percentage has to take the already-divided value
as an input and check an inequality over it. That puts the arithmetic that
decides the outcome **outside** the expression that is audited: replay
reproduces the comparison, not the division, and the rounding rule lives in
whatever produced the input.

The reason `/` was refused is good and survives this ADR: integer division has
no single correct rounding, and a language that picks one silently makes half
its users wrong. `-7 / 2` is `-3` in Rust, C, and Java, and `-4` in Python.
Both are defensible; neither is *the* answer.

So we do not answer it. We remove the question instead: every operation here
**names its rounding**, and `/` stays refused.

## User-visible behavior

Four operations, each taking exactly two `Int` arguments and yielding an `Int`:

| Operation | Meaning |
|---|---|
| `div_floor(a, b)` | exact quotient rounded toward negative infinity |
| `div_ceil(a, b)` | exact quotient rounded toward positive infinity |
| `div_half_even(a, b)` | exact quotient rounded to nearest, **ties to even** |
| `mod_euclid(a, b)` | the `r` in `a = b*q + r` with `0 <= r < \|b\|` |

```brix
let lf_units = div_half_even(total_lf, 250)
let per_car_cents = div_floor(price_cents, car_count)
let leftover = mod_euclid(price_cents, car_count)
```

`/` remains refused, and its diagnostic now names these four as the
replacements rather than only saying no.

This milestone does not add unsigned integers, arbitrary-precision integers,
rationals, or any floating-point value. No float enters decision arithmetic at
any point, including as an intermediate.

## The reserved names

`div_floor`, `div_ceil`, `div_half_even`, and `mod_euclid` are **reserved**. A
module that declares a helper or a constructor with one of these names is
refused at the declaration (`ReservedOperationName`).

The alternative — resolving the built-in first and letting a same-named helper
sit unreachable — was rejected. It makes a declaration that is visibly present
in the source have no effect at any call site, which is the kind of silence
this system exists to remove. Refusing at the declaration reports it once, at
the place the author can fix.

They are reserved *names*, not keywords: they are recognised only in call
position, so a rule, let, input, or parameter may still be called `div_floor`.

## Semantics

Let `a` be the dividend and `b` the divisor. All four operations are exact:
there is no approximation step to specify.

**Division by zero.** `b == 0` raises `DivisionByZero` for all four
operations. Its own fault variant rather than an overflow, because the
quotient does not exist at all — a different fact from one that exists and
does not fit.

**Rounding.** Each quotient is characterised by its residual `r = a - b*q`:

- `div_floor` — `r` takes the **sign of `b`**: `0 <= r < b` for `b > 0`.
- `div_ceil` — `r` takes the sign **opposite `b`**: `-b < r <= 0` for `b > 0`.
- `div_half_even` — `2*|r| <= |b|`, and when `2*|r| == |b|` the quotient `q`
  is even. Worked cases: `div_half_even(5, 2) = 2`, `div_half_even(7, 2) = 4`,
  `div_half_even(-5, 2) = -2`, `div_half_even(-7, 2) = -4`. Ties go to the
  even neighbour in **both** directions; this is not "away from zero", which
  would give `3` and `-3`.
- Exact division agrees across all three: no rounding rule perturbs a quotient
  that is already an integer.

**`mod_euclid` is defined on `r` alone.** `0 <= r < |b|`, with `q` left
existentially quantified. This is deliberate. For negative `b` the Euclidean
quotient is *not* the floored one, so pairing this remainder with `div_floor`
would be wrong: `mod_euclid(7, -3) = 1`, while the floored remainder is `-2`.
Naming a quotient here would pin the wrong pairing, so it names none.

Further cases: `mod_euclid(-7, 3) = 2`, `mod_euclid(-7, -3) = 2`,
`mod_euclid(7, 3) = 1`. The result is non-negative regardless of either sign.

**Overflow.** `DivisionOverflow` is raised when the quotient is mathematically
defined but not representable in `Int`. Exactly one operand pair reaches it:
`a = Int::MIN`, `b = -1`, whose quotient is `2^63`. It applies to the three
division operations.

**`mod_euclid` never overflows**, including at `mod_euclid(Int::MIN, -1)`,
which is `0`. Its result is bounded by `|b|`, so it always fits, and refusing
it merely because the *quotient* does not fit would refuse a well-defined
answer. This is stated separately because inheriting the quotient's fault is
the natural implementation mistake.

**Intermediates cannot overflow.** All four are computed in a wider
representation throughout, so there is one range check, applied to the result.
No absolute value, remainder, or doubled residual can overflow on the way —
which matters because `|Int::MIN|` is itself unrepresentable, and a check
performed after a wrapped intermediate has not checked anything.

**Operand shape.** A non-`Int` operand raises the existing `OperandShape`
fault. Both operands are evaluated, left before right, exactly once each.

Every fault above fails closed: no decision is committed, and it surfaces at
`brix check` preflight rather than only at `run`.

## Identity and replay

`L3ExprV2` appends one ordinal, `IntDivMod = 16`, carrying an operation
ordinal (`div_floor = 0`, `div_ceil = 1`, `div_half_even = 2`,
`mod_euclid = 3`) and its two operands. Ordinals `0`–`15` keep their existing
meanings and bytes, so **every program not using these operations encodes
exactly as before** and keeps its pin.

The operation is pinned **structurally, not by name**. A call to `div_floor`
does not lower to a helper invocation carrying the string `div_floor`; it
lowers to a distinct node whose ordinal *is* the rounding rule. Two programs
differing only in which rounding they name therefore have different program
ids, and no helper table, import, or audit environment can resolve the
operation to different behavior than the one the pin records.

Replay runs the same evaluator over the same encoding, so a fault reproduces
as the same fault rather than as a different outcome.

## Acceptance

- Every specified literal case holds, in all four sign combinations.
- Each operation is additionally checked against its **defining identity** in
  a wider representation, independently of how the evaluator computes it, so
  a sign-handling bug cannot pass by agreeing with itself. The identity tests
  are verified to fail under a broken floor correction, under half-away-from-
  zero ties, and under a truncated `mod_euclid`.
- Division and modulo by zero fault for every operation.
- `Int::MIN / -1` overflows for all three divisions and for **no other operand
  pair**; `mod_euclid(Int::MIN, -1)` is `0`.
- Non-`Int` operands fault on either side of every operation.
- A helper or constructor claiming a reserved name is refused at its
  declaration; a call with other than two arguments is refused.
- `/` stays refused and its diagnostic names the replacements.
- Two programs differing only in the named rounding have different program ids
  and reach different decisions end to end through the runtime.
- Scalar and structured workflow pins and frozen vectors remain unchanged.
- Workspace tests, formatting, Clippy, canonical cross-checks, TCB dependency
  checks and semantic-law traceability pass before the milestone PR.

## Residual — CLOSED by ADR-0036

**The grammar had no unary minus.** A negative literal could not be spelled,
so the tests and examples here originally wrote `0 - 7` — a poor surface for a
milestone whose entire subject is how negative quotients round. It was tracked
separately rather than smuggled in here, and closed immediately after by
[ADR-0036](ADR-0036_Unary_Minus.md), which adds it as a parser-level
desugaring with no change to any canonical encoding.
