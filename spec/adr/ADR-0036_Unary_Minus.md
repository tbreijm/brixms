# ADR-0036 — Unary minus and negative integer literals

Status: **Proposed implementation**, 2026-09-25. Closes the residual recorded
in [ADR-0035](ADR-0035_Integer_Division.md). The rules below describe the
implementation submitted for review, not separate semantic ratification.

Touches the source grammar only. It adds no node to `L3ExprV2`, no canonical
ordinal, and no evaluator operation, and it changes neither the SOC authority
boundary nor any transport contract.

## Motivation

The grammar had no unary minus, so a negative number could not be written at
all: `let x = -7` was a parse error, and `0 - 7` was the only available
spelling.

ADR-0035 recorded this as a residual rather than fixing it inline, because it
is a grammar question and not a division one. But it lands immediately after,
for a reason ADR-0035 states: that milestone's entire subject is *how negative
quotients round*, and a language that cannot spell a negative number is a poor
surface for it. Every worked example there had to be written `0 - 7`.

## User-visible behavior

```brix
let below = -7
let shifted = -offset
let scaled = -a * b
```

Unary minus is a prefix operator. It binds tighter than every binary operator
and looser than postfix field projection:

- `-a * b` is `(-a) * b`
- `-a + b` is `(-a) + b`
- `-a.field` is `-(a.field)`
- `--a` is `-(-a)`, and nests to any depth the nesting bound allows

Binary subtraction is unaffected: a `-` in infix position is still binary, so
`a - 1` is subtraction and `a - -1` is subtraction of a negative literal.

## Lowering: a desugaring, not a new operation

`-e` desugars in the parser to `0 - e`, reusing the existing `BinOp::Sub`,
`ArithOpV2::Sub`, and its checked evaluation.

This was chosen over introducing an AST and `L3ExprV2` node for negation.
A new node would have required a new canonical ordinal, a new evaluator arm
with its own overflow handling, and arms in every lowering pipeline — all to
express something `Sub` already expresses exactly. The desugaring adds
**nothing** to the canonical encoding, so every existing program keeps its
program id by construction rather than by careful ordinal hygiene.

The consequence worth stating: **negation is checked, because subtraction
is.** `-(Int::MIN)` desugars to `0 - Int::MIN`, which overflows `Int::MAX` by
one and raises `Overflow(Sub)`. It never wraps. A dedicated negation node
would have had to re-derive that guarantee; here it is inherited.

## The literal exception

A `-` immediately followed by a numeral does **not** desugar. It folds into a
negative numeric literal: `-7` parses as the literal `-7`, not as `0 - 7`.

This is not a cosmetic optimization. It is the only way `Int::MIN` is
expressible at all. `Int::MIN` is `-9223372036854775808`, and its positive
magnitude `9223372036854775808` is **not** a representable `Int` — it exceeds
`Int::MAX` by one. So `0 - 9223372036854775808` can never be built: the
positive literal alone fails to parse as an integer before any subtraction is
considered. Folding the sign into the literal text is what lets the negative
string parse directly, and it is why the most negative integer is writable.

Consequently `-9223372036854775808` is a valid literal, while the
parenthesised `-(9223372036854775808)` is not — the parentheses defeat the
fold and expose the unrepresentable positive magnitude. That asymmetry is
inherent to two's complement, not an artifact of this design; it is the same
rule C and Rust apply, and it is stated here so it is not discovered.

## Frontend resource bounds

The desugaring path recurses through the depth-charged prefix operand helper,
exactly as `!`, `prove`, and `audit` do (ADR-0034, ADR-0022 D6). A deep run of
minuses is **refused** by `max_nesting_depth` rather than overflowing the
stack.

The literal-folding shortcut needs no charge: it peeks one token and consumes
it without recursing, so it consumes no stack per level. The depth test is
written to terminate in a non-numeral operand precisely so the shortcut cannot
mask a missing charge.

## Identity and replay

No canonical encoding changes. No ordinal is added or renumbered. A program
that does not use unary minus produces **byte-identical** canonical bytes and
the same program id as before this ADR, and a program that does use it encodes
as the `Sub` expression it desugars to.

This is pinned by a test carrying a program id captured before the change,
verified against the pre-change commit rather than against the implementation
that would confirm itself.

## Acceptance

- Precedence and associativity asserted at the AST: `-a * b`, `-a + b`,
  `-a.field`, and nesting.
- `Int::MIN` is writable as a literal and evaluates to `Int::MIN`.
- `-(Int::MIN)` — parenthesised, so it cannot fold — faults with
  `Overflow(Sub)` rather than wrapping, both as a literal and via a bound
  fact.
- A deep run of unary minuses is refused by the nesting bound rather than
  aborting the process.
- A program not using unary minus keeps a program id captured before the
  change.
- Workspace tests, formatting, Clippy, canonical cross-checks, TCB dependency
  checks and semantic-law traceability pass before the milestone PR.
