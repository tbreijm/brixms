# ADR-0045 — Explicit binary64 and decimal arithmetic

Status: **Proposed implementation**, 2026-09-30. Source implementation for
review; not a published release or a claim of kernel-proven arithmetic.

## Purpose and scope

Decision programs need approximate measurements and exact financial values.
Introduce two separate scalar domains, `F64` and `Decimal`, in the shared
evaluator and finite-decision contracts. Neither silently converts to another
numeric domain. Existing `Int` operations retain their meanings.

The historical `Float` typing surface is unchanged. Bare floating literals
remain outside the decision evaluator; explicit constructors make the new
domain visible without changing the meaning of existing source. This is an
additive extension to ADR-0031's scalar policy, not a reinterpretation of its
frozen input schemas.

## Source surface

```brix
input distance: F64
input duration: F64
input price: Decimal

fn speed(d: F64, t: F64): F64 = d / t
let tax = price * decimal("0.21")
let installment = decimal_div(price, decimal("3"), 2, "half_even")

propose accept() priority 1 when speed(distance, duration) < f64("30") = tax
commit decision from (accept)
```

The constructors are `f64(Str)` and `decimal(Str)`. `f64_from_int(Int)` is
an explicitly lossy conversion; `decimal_from_int(Int)` is exact.
`f64_neg(F64)` and `decimal_neg(Decimal)` negate values. Prefix `-e` retains
its existing integer `0 - e` desugaring; use the named negation operations
for the new domains. Negative constructor strings are also admitted.

Arithmetic `+`, `-`, `*`, `/` and comparisons require operands in the same
numeric domain. Integer `/` remains refused; its named rounding operations
are unchanged. `F64` and `Decimal` work in records, sum payloads, lists,
helper contracts, proposal values, and per-entity decisions. Existing
`sum`, `min`, and `max` folds remain integer-only; numeric folds need a
separate empty-result and accumulation contract. Mapping and filtering
numeric collections use the normal expression evaluator.

`F64` and `Decimal` are reserved config names. The constructors, conversion,
negation, and rounded-division names are reserved built-ins and cannot be
shadowed by helpers or constructors. Calls lower to structural operation
tags, not dynamically resolved helper names.

## F64

`F64` is finite IEEE-754 binary64. Its Rust representation is a private-bit
`FiniteF64` wrapper, not an unrestricted host float. All creation and basic
arithmetic pass through `brix-canon`'s `strict_ieee` module.

- Decimal strings parse to the nearest binary64 value, ties to even.
  Optional signs and base-10 exponents are admitted; whitespace and malformed
  strings are rejected.
- Each arithmetic operation rounds once to binary64, ties to even. Evaluation
  follows the expression tree; no fused multiply-add or reassociation.
- Inputs and results must be finite. NaN, infinity, overflow to infinity,
  and division by either signed zero are faults.
- Subnormals and underflow rounded to zero follow binary64 semantics.
- Negative zero normalizes to positive zero at every value boundary. Equality,
  ordering, and identity use that normalized representation. Comparisons are
  numerical, not epsilon comparisons.

These operations use Rust's documented primitive rounding behavior and
correctly rounded parsing, with no transcendental host-library functions.
See the [Rust floating-point contract](https://doc.rust-lang.org/std/primitive.f32.html)
and [binary64 parsing contract](https://doc.rust-lang.org/std/primitive.f64.html#impl-FromStr-for-f64).
Supported builds must retain the standard floating environment, gradual
underflow, and strict compiler semantics: foreign changes to rounding modes,
flush-to-zero, or fast-math builds are outside this profile. Fixed bit-result
vectors are configured to run on Linux and macOS CI; a local run alone does not establish
cross-platform conformance.

## Decimal

`Decimal` reuses the existing normalized `brix_canon::Decimal` value and
encoding: signed `i128` coefficient times `10^-scale`. This executable
profile admits normalized scales 0 through 18. It adds no binary float step.

Constructors accept ordinary signed decimal notation without an exponent.
Trailing fractional zeros normalize away; zero has scale 0. The value does
not retain formatting or a currency's number of minor-unit digits.

Addition, subtraction, and multiplication are exact, subject to checked
coefficient/intermediate bounds and the scale limit. `/` requires an exact
terminating decimal within those bounds. A repeating quotient is a fault,
with a diagnostic pointing to explicit rounded division:

```brix
decimal_div(decimal("10"), decimal("3"), 2, "half_even")
```

The scale argument is an `Int` in 0..=18. Modes are:

| Mode | Rounding |
| --- | --- |
| `trunc` | Toward zero |
| `floor` | Toward negative infinity |
| `ceil` | Toward positive infinity |
| `half_even` | Nearest, ties to an even coefficient at the requested scale |

Zero divisors, an invalid scale or mode, and checked intermediate/result
overflow fault. Bounded intermediate arithmetic may reject a calculation
whose mathematical final result would fit; it must never silently round an
exact operation to make it fit. Result normalization may remove trailing
zeros after rounding. Decimal-to-float, float-to-decimal, and conversions
back to integers are deferred until their precision and rounding contracts
are separately specified.

## Transport, identity, and evidence

`brix.input@4` accepts all existing forms plus these string-valued tags,
including as fields, payloads, and elements of admitted list inputs:

```json
{
  "schema": "brix.input@4",
  "values": {
    "distance": { "type": "f64", "value": "125.5" },
    "duration": { "type": "f64", "value": "5" },
    "price": { "type": "decimal", "value": "19.95" }
  }
}
```

Numeric JSON payloads are deliberately strings so transports do not first
round them through an unrelated JSON-number representation. All existing
duplicate-key, shape, depth, size, and list limits still apply. Schemas
`@1`–`@3` reject the new numeric tags, recursively. Knowledge-base snapshots
and audit transport retain the numeric domains and values.

Canonical additions are append-only:

- Input values (including persisted runtime results): ordinal 6 for F64,
  7 for Decimal. F64 payload is eight bytes of normalized IEEE bits in
  big-endian byte order; Decimal payload is
  its existing `Canonical` encoding. Raw `f64` does not gain `Canonical`.
- Input/helper value types: ordinals 6 and 7. Schema leaves: 4 and 5.
- Expression ordinal 25: numeric built-in tag, argument count, arguments.
  Built-in ordinals are `f64`=0, `decimal`=1, `f64_from_int`=2,
  `decimal_from_int`=3, `decimal_div`=4, `f64_neg`=5, `decimal_neg`=6.
- Arithmetic appends division at operation ordinal 3.

Existing encodings, vector files, and program identities are unchanged.
Source that previously declared a newly reserved name must rename that
declaration; this extension lands before the first beta compatibility promise.
Canonical numeric payloads may participate in snapshot/context digests; this
does not admit approximate numbers as entity keys or settlement priorities.

Numeric computation earns no new evidence grade. Decisions remain `Derived`;
successful replay may produce separate `Audited` receipts. Auditing verifies
the specified finite computation, not equality with an ideal real-number
calculation or physical truth. Any reached numeric fault makes the run
`unknown` under the beta's all-decisions-success rule and prevents an audit
bundle. Lazy evaluation still skips faults on unreached branches.

## Acceptance

Tests cover exact decimal identities and signed rounding, binary64 bit
vectors including ties and subnormals, rejected exceptional values, strict
cross-version transport, structured contracts, unchanged frozen identities,
and successful run → audit → verify and knowledge-base revision workflows.
No dependency or proof-kernel extension is required.
