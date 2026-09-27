# ADR-0037 — Bounded list inputs and deterministic folds

Status: **Proposed design**, 2026-09-28. This ADR specifies a candidate first
slice; list values and folds are not implemented or shipped.

Extends the finite-decision input and evaluation profiles described by
ADR-0030 through ADR-0034. It does not add rule-schema quantification, witness
claims, or a rule-level search policy.

## Scope

The first slice admits `List<T>` only as a top-level external input, where `T`
is an admitted scalar (`Int`, `Bool`, or `Str`) or a closed nominal schema
already admitted by ADR-0033. Lists nested in records/sums, lists in helper or
nominal annotations, recursive schemas, and nested lists are deferred. This
restriction keeps list transport and element validation at one explicit input
boundary; support for recursive composition would require its own resource and
schema review.

Every list declaration has a mandatory finite maximum length, for example:

```brix
config Item = { quantity: Int, price_cents: Int }
input items: List<Item> max 64

let total = sum(items, item => item.quantity * item.price_cents)
let count_positive = count(items, item => item.price_cents > 0)
let all_valid = all(items, item => item.quantity > 0)
let any_discounted = any(items, item => item.price_cents < 100)
```

`max` is required, positive or zero, and cannot exceed 256. The bound is part
of the input declaration/program contract and is checked before allocating or
decoding the element sequence. A value longer than the declared bound fails
closed. Existing limits still apply: at most 4,096 tagged value nodes per
shard, depth 32, 1 MiB per input file, and all existing aggregate/file/input
limits. Element schemas remain closed under ADR-0033. An empty list obtains
its element type from the declaration.

## Folds

The only first-slice folds are `sum(list, x => Int)`, `count(list, x => Bool)`,
`all(list, x => Bool)`, and `any(list, x => Bool)`. The binder is a hygienic
lexical variable scoped to the fold body. Lambdas are fold syntax only, not
first-class values; they cannot escape, be stored, or be passed as ordinary
function values.

Elements are visited in sequence order. `sum` adds each body result from left
to right and checks the signed `i64` result after every addition; overflow is a
typed evaluation fault. `count` requires a strict `Bool` result and checks its
integer increment. `all` and `any` require strict `Bool` results and
short-circuit in sequence order. Empty results are respectively `0`, `0`,
`true`, and `false`.

Static checking validates every body path regardless of whether the list is
empty or runtime short-circuiting leaves an element unreached. Runtime
evaluation charges every reached element and every called helper against the
existing ADR-0032 evaluator work budget. Nested folds consume the same budget;
there is no fresh per-fold allowance. A fault or exhaustion fails closed and
cannot produce a committed decision. The list is a finite value computation:
it creates no facts or rule instances and proves no witness, schema grounding,
or quantified rule property.

## Transport and identity

Keep the published scalar contract `brix.input@1` byte-for-byte unchanged and
keep the record/sum contract proposed by ADR-0033 at `brix.input@2`. This
proposal assigns bounded lists to a new `brix.input@3`; `@3` accepts the
existing scalar, record, and sum forms as well as the list form. Equivalent
admitted values retain identity when transported through different supported
versions or shard arrangements. Transport version alone is not part of value
identity.

The list wire shape is `{ "type": "list", "items": [<tagged value>, ...] }`.
Only those two keys are admitted, each once; object key ordering is irrelevant.
The element type and maximum come from the source input declaration, never
from trusted metadata in the shard. Duplicate keys, unknown keys and list
values in an `@1` or `@2` shard are refused. The `@3` decoder receives the
input contract and checks its maximum before appending each element; it also
charges each list container and element to the existing node/depth budgets.
List order is significant, and shard names remain disjoint rather than
allowing overrides.

The proposed canonical layout extends the inspected current encodings in
`input.rs` and `finite_decision/plan.rs`:

- Input value ordinal `5` is `List`: write the element count with `write_uint`,
  followed by each existing tagged canonical input value in sequence order.
  Existing value ordinals `0` through `4` are unchanged.
- Input declaration type ordinal `5` is `List`: write its scalar/nominal
  element type with the existing `encode_input_type` layout, then its maximum
  with `write_uint`. Nested ordinal `5` is refused in this first slice.
  This payload stays inside `brix.l3.finite-decision.inputs@1`; no empty new
  frame or bytes are added to programs without list declarations.
- Nominal element schemas use the existing reachable-schema frame. Changing
  an element schema or a declared maximum changes the program identity.
- Expression ordinal `17` is `Fold`, following ADR-0035's ordinal `16`:
  write operation `0 = sum`, `1 = count`, `2 = all`, or `3 = any` with
  `write_uint`, then the list expression, binder identifier, and body
  expression. Binder references use the existing scoped `LetRef` encoding;
  lowering resolves lexical shadowing without capturing outer bindings.

These are design assignments, not implemented encoders or frozen vectors.
Implementation must add independently reproduced vectors for the new forms
without editing old vectors. Empty-list value bytes do not repeat the element
type: the program's declaration binds that type, and context binds program
and snapshot. The snapshot binds every element value in order; a permutation
that changes the sequence changes snapshot and context identities. Replay
reconstructs the declaration, maximum, and ordered values from source and
caller-supplied inputs.

## Acceptance checklist

- Preserve frozen `brix.input@1` scalar behavior and ADR-0033 `@2` record/sum
  behavior; implement the `@3` wrapper and appended canonical forms above.
- Enforce each mandatory maximum before sequence allocation and preserve
  existing byte, node, depth, file, aggregate, and input limits.
- Reject malformed/unknown elements, duplicate JSON keys at every level,
  overlong sequences, and unsupported nested list/schema shapes.
- Freeze canonical value/type encoding and program-bound length encoding with
  cross-version and ordering vectors; prove old vectors remain unchanged.
- Specify and implement hygienic fold binding, strict result types, empty
  cases, left-to-right checked summation/counting, short-circuit behavior, and
  static checking of unreached bodies.
- Charge reached iterations and helper calls to the shared evaluator budget;
  verify failures and exhaustion never commit a result and replay agrees.
- State that folds do not ground rule schemas or add evidence/witness claims.
- Review the kernel residual tracked by [issue #53](https://github.com/tbreijm/brixms/issues/53) and
  [Type Realization Contract §5.5](../Type_Realization_Contract.md)
  as a separate follow-up: the lambda-spine recursion is already peeled iteratively, while
  recursion through `RealizesComp` remains. This is tracked context, not a
  list implementation acceptance claim and not a reason to alter that
  contract in this ADR.

## Related decisions

- [ADR-0033](ADR-0033_Structured_Input_Contracts.md) proposes `brix.input@2`
  records and sums and supplies current structured limits.
- [ADR-0031](ADR-0031_External_Input_Alpha.md) freezes scalar `brix.input@1`.
- [ADR-0032](ADR-0032_Finite_Decision_Functions.md) defines helper evaluation
  and its shared bounded-work model.
- [ADR-0034](ADR-0034_Boolean_Operators.md) defines short-circuit Boolean
  behavior used by `all` and `any`.
