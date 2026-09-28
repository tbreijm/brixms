# ADR-0040 — Finite relations: filter, map, comprehensions, and membership

Status: **Implemented**, 2026-09-28.

Extends the bounded list values ADR-0037 admits at the finite-decision input
boundary into a small relational layer over those same values: lists become
ordinary values usable in `let` bindings, rule facts ("derived relations"),
helper arguments and results, and proposal guards and values — not only the
top-level input form ADR-0037 fixes. It does not add rule-schema
quantification, witness claims, a rule-level search policy, or nested list
input schemas; ADR-0037's scope restriction there is unchanged.

## Motivation

ADR-0037 gives a program a bounded sequence and four ways to reduce it to a
scalar. That is enough to *summarize* a batch of rows, but not to *relate*
two of them: an order against the stock row for its SKU, a bid against the
lot it names, a shipment against the vehicle assigned to it. Real settlement
policy is full of exactly this shape — "for each order whose SKU has enough
stock, ship it" is a join and a filter, not a fold. Without them, a program
either declares dozens of near-duplicate top-level inputs to fake a join in
the host language, or gives up and does the relational work outside Brix
entirely, which is precisely the pressure this milestone exists to relieve.

## User-visible behavior

```brix
config Order = { id: Int, sku: Int, units: Int }
config Stock = { sku: Int, on_hand: Int }
config Line = { order: Int, on_hand: Int }

input orders: List<Order> max 64
input stock: List<Stock> max 64

let short_orders = filter(orders, o => !(stock in map(stock, s => s.sku)))
let total_units = sum(orders, o => o.units)
let big_order = max(orders, o => o.units)

let coverable = for o in orders, s in stock where s.sku == o.sku yield Line { order: o.id, on_hand: s.on_hand }
let fully_covered = count(coverable, l => l.on_hand > 0)
```

(`short_orders` above is illustrative of `filter`/`map`/`in` composing; see
`docs/brix-language.md` and `examples/fulfillment.brix` for a complete,
verified program including the join written the natural way.)

## Surface syntax

Six new expression forms, plus a reserved-name convention carried over from
ADR-0035/ADR-0037 rather than a pile of new keywords:

- `filter(xs, x => cond)` — keep elements of `xs`, in order, for which `cond`
  evaluates to `true`.
- `map(xs, x => e)` — the list of `e` evaluated for each element of `xs`, in
  order.
- `for x in xs, y in ys where cond yield e` — a comprehension: nested
  generator loops, left to right, in generator order. A later generator and
  the `where` clause may reference an earlier binder — this is how a join is
  written, as the example above and `examples/fulfillment.brix` show. `where`
  is optional; omitting it keeps every combination.
- `[e1, e2, ...]` — a list literal.
- `e in xs` — membership by structural equality. Same precedence as `<`/`==`/
  etc. and equally **non-associative**: `a in xs in ys` is refused by name,
  for the same reason `a < b < c` is (ADR-0010 ⟨D-OPARROW⟩, reaffirmed by
  ADR-0034) — chaining reads as composition, not as the conjunction a reader
  would guess.
- `len(xs)` and `distinct(xs)` — length, and first-occurrence-kept
  deduplication (order otherwise preserved).
- `min(xs, x => Int)` / `max(xs, x => Int)` — two more folds alongside
  ADR-0037's `sum`/`count`/`all`/`any`. Unlike those four, an empty list is
  **not** given a default: `min`/`max` of an empty list is a typed evaluation
  fault (`EvalFault::EmptyAggregate`), never `0` or any other placeholder,
  because there genuinely is no minimum or maximum to report.

**Grammar note — why lambdas are recognized structurally, not by keyword.**
`filter`, `map`, `sum`, `count`, `all`, `any`, `min`, `max` are ordinary call
syntax (`name(args)`), not new keywords: the parser recognizes an argument of
the shape `ident => expr` as a binder wherever it appears as a call argument,
independent of the callee's name, and lowering accepts that shape only for
the eight names above — reserved as *operation names* exactly the way
ADR-0035 reserves `div_floor`/`div_ceil`/`div_half_even`/`mod_euclid` (a
module may not declare a helper or constructor with one of these names, but
they cost the lexer nothing: no new keyword, so nothing that used to parse as
an identifier stops parsing). `for`, `in`, `where`, and `yield` **are** new
reserved keywords, because the comprehension's grammar is genuinely different
or from ordinary call syntax and cannot be spelled as one. Checked against
every example, package, fixture, and test in the tree: none used `for`,
`in`, `where`, or `yield` as an identifier before this change.

A lambda is fold/filter/map syntax only, never a first-class value: it cannot
be bound with `let`, returned from a helper, or passed to anything but one of
the eight recognized calls. Reaching one anywhere else is a lowering error
(`LambdaNotAllowed`), the same discipline ADR-0037 already stated for its own
four folds.

## Semantics

**Determinism.** Every form has a single specified evaluation order, and
replay (`brix audit`/`brix verify`) reproduces it exactly, because it runs
the same evaluator over the same encoding ADR-0027 already commits to for
every other expression form:

- `filter`/`map`: elements visited left to right, in the source list's order.
- A comprehension: generators nested left to right — the first generator is
  the outer loop — so `for x in xs, y in ys yield (x, y)` visits every `y`
  for a given `x` before advancing `x`, exactly as the written order suggests.
  `where` and `yield` see every binder introduced so far.
- `distinct`: first occurrence kept, elements visited left to right; a
  `BTreeSet` (never a hash map — `disallowed_types` enforces this workspace-
  wide) gives an O(log n) membership check without disturbing the
  insertion-ordered result the expression actually produces.
- `in`: the haystack's elements are visited left to right and structural
  equality is checked against each in turn; short-circuits on the first
  match.

**Typing.** Runtime checks are strict, matching every existing fault
discipline in this evaluator: `filter`'s condition, a comprehension's
`where`, and `count`/`all`/`any`'s bodies must be `Bool`; `sum`/`min`/`max`'s
bodies must be `Int`. A wrong-shaped operand is `EvalFault::OperandShape`,
exactly like every other typed operator in this profile. The **static**
shape-checking pass ADR-0034 introduced for `&&`/`||`/`!` operands
(`finite_decision::boolean_types`) is extended to these forms too, so a
statically-known-wrong body type is rejected at `check` rather than waiting
for a runtime fault — `false && filter(xs, x => 1)`-shaped mistakes are
caught before a program ever runs, the same guarantee ADR-0034 gives
Boolean operands. This pass tracks a list's element shape (not just its flat
runtime type category) precisely so a `map` or `filter`'s result can be
checked again downstream, and a fold/filter/map/comprehension binder is
checked against the shape its source expression actually produced wherever
that is statically known.

**Bounds and fail-closed behavior.** Every list a running program can
*derive* — the result of `filter`, `map`, a comprehension, a list literal, or
`distinct` — is capped at `MAX_DERIVED_LIST_LEN` = 4,096 elements,
independent of any input's own declared `max` (which cannot exceed 256).
Exceeding it is `EvalFault::ResourceExhausted`, checked incrementally as
elements are produced rather than after the fact. Every element a
comprehension or fold visits — including every element of every nested
generator's list, so a two-generator comprehension over two lists of size
`m` and `n` visits on the order of `m * n` elements — is charged to the
*existing* ADR-0032 evaluator work budget (`MAX_CALL_STEPS` = 10,000; see
ADR-0037's note on when that budget is active at all). A large cartesian
product therefore fails closed with a typed fault well before it could
exhaust memory, at the cost of a genuinely tight ceiling on how large a join
this first slice can usefully evaluate — a known tightness this ADR accepts
rather than hides, and a candidate for a later, separately reviewed budget
tuned specifically for relational work.

**Hygiene.** Every binder — a fold's, `filter`'s, `map`'s, or a
comprehension generator's — is a hygienic lexical variable scoped to its
body/condition/yield, lowered through the same `LetRef`/`locals` scoping
`match` arm binders already use. A binder may shadow an outer `let`, input,
or rule name; inside its scope, references resolve to the binder, never to
the outer binding, and the outer binding is never mutated or captured by
reference. A binder does not leak past the closing form.

**Identity.** A program that uses none of these forms keeps a byte-identical
program id: every new ordinal is reachable only from source that uses the
corresponding form, so the canonical preimage of a program that never
constructs `L3ExprV2::Filter`/`Map`/`Comprehension`/`ListLit`/`In`/`Len`/
`Distinct`/a `min`/`max` `Fold` is untouched. This was checked directly by
recording `brix run` program and context ids for every `examples/*.brix`
(paired with its `--input` file where one exists) before this milestone's
changes and confirming they are unchanged after.

## Identity and canonical encoding

This ADR's canonical-ordinal allocation is `L3ExprV2` ordinals **18–29** and
fold ops **4–5**; ordinals outside that range belong to other concurrent
work and are not touched here.

| Form | Ordinal | Payload |
|---|---|---|
| `Fold` (ADR-0037, extended here) | 17 | op (`0`=sum,`1`=count,`2`=all,`3`=any,`4`=min,`5`=max), list expr, binder ident, body expr |
| `filter(xs, x => cond)` | 18 | list expr, binder ident, cond expr |
| `map(xs, x => e)` | 19 | list expr, binder ident, body expr |
| comprehension | 20 | generator count, then (binder ident, source expr) pairs, then an optional-tagged `where` expr, then the yield expr |
| list literal `[e1, ...]` | 21 | element count, then each element expr |
| `e in xs` | 22 | needle expr, haystack expr |
| `len(xs)` | 23 | list expr |
| `distinct(xs)` | 24 | list expr |

Ordinals 25–29 are reserved for this ADR's future extension and are not
assigned to any form yet.

`min`/`max` reuse `Fold`'s existing ordinal 17 with op values `4`/`5`,
appended after ADR-0037's `0`–`3` and never renumbering them — the same
append-only discipline every prior ADR in this line has followed.

## Acceptance checklist

- Preserve every ADR-0037 behavior and encoding byte-for-byte; append the
  ordinals above without touching ordinals `0`–`17`'s existing meanings.
- Implement `filter`/`map`/comprehension/list-literal/`in`/`len`/`distinct`
  and the `min`/`max` folds with the evaluation order, typing, hygiene, and
  bounds stated above.
- Reserve `for`/`in`/`where`/`yield` as keywords and confirm no existing
  example, package, fixture, or test used them as identifiers; reserve
  `filter`/`map`/`min`/`max`/`len`/`distinct` as operation names (not
  keywords) under the same discipline ADR-0035/ADR-0037 established.
- Extend the ADR-0034 static shape-checking pass to reject a statically
  known-wrong fold/filter/map/comprehension body type at `check`.
- Charge every derived list's construction and every element visited to the
  shared ADR-0032 evaluator work budget; verify a large cartesian product
  fails closed with a typed fault rather than exhausting memory.
- Verify `min`/`max` of an empty list is a typed fault, never a default
  value.
- Verify programs that do not use these forms keep byte-identical program
  ids, checked directly against `examples/*.brix` before and after.
- `brix why`/`brix whynot` and `brix audit`/`brix verify` work end to end
  through a program using these forms with `brix.input@3` inputs.

## Related decisions

- [ADR-0037](ADR-0037_Bounded_Lists_And_Folds.md) admits bounded list values,
  the `brix.input@3` transport, and the first four folds this ADR extends.
- [ADR-0034](ADR-0034_Boolean_Operators.md) establishes the static
  operand-type checking pass this ADR extends, and the precedence-climbing
  parser `in` reuses at comparison precedence.
- [ADR-0035](ADR-0035_Integer_Division.md) establishes the reserved-operation-
  name discipline (not a keyword) this ADR follows for `filter`/`map`/`min`/
  `max`/`len`/`distinct`.
- [ADR-0032](ADR-0032_Finite_Decision_Functions.md) defines the shared
  bounded-work evaluator model this ADR's list/relational forms are charged
  against.
