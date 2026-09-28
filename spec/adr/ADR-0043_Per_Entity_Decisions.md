# ADR-0043 — Per-entity decisions

Status: **Proposed implementation**, 2026-09-28. Extends the finite-decision
profile from ADR-0030 through ADR-0041. Does not change the SOC authority
boundary, the external-input transport, or the meaning of an existing
`commit` pool.

## Motivation

ADR-0037/ADR-0040 give a program bounded lists and a small relational layer
over them; ADR-0038 lets a `commit` pool's fallback and dependencies go
unwritten; ADR-0039 lets a module declare more than one independent `commit`
pool. Put together, a policy can *summarize* a batch of rows (`sum`, `count`,
a join) and can make *several* independent decisions about the batch as a
whole (a shipping decision and a billing decision for one order). What it
still cannot do is make **one decision per row** — ship-or-hold for *this*
order, independently, for every order in a list. Before this ADR that meant
either flattening the list into a fixed number of named inputs (`order_1`,
`order_2`, …, capped and unnatural) or deciding only in the aggregate and
losing the per-row answer a real fulfillment or eligibility policy actually
needs. This is precisely the shape a knowledge engine over a batch of facts
is for, so this ADR adds it directly.

## User-visible behavior

```brix
config Order = { units: Int }
config Decision = Ship | Hold

input orders: List<Order> max 64

decide status for o in orders {
  propose ship priority 10 when o.units <= 10 = Ship
  propose hold otherwise = Hold
}
```

A `decide NAME for BINDER in LIST_EXPR { propose ... }` block declares a
**commit pool template** that is instantiated once per element of
`LIST_EXPR`, in list order — not one pool, but one independent pool *per
element*. Each instance:

- deliberates with the exact same frontier/calendar/admission machinery an
  ordinary top-level `commit` pool already uses (ADR-0030), scoped to its own
  candidates only;
- may read the current element through `BINDER`, plus everything an ordinary
  top-level `propose` can already read (inputs, `let`s, and any rule declared
  above the `decide` block) — but *not* another instance's binder or
  decision, and not a sibling `decide` block's or `commit` pool's decision
  (which, like a commit pool's own name, is only readable via `show`);
- settles independently: instance 3 selecting `hold` has no bearing on
  whether instance 7 selects `ship`.

`propose` inside a `decide` block is spelled exactly like a top-level
`propose` (including `otherwise`, ADR-0038) and is checked by the same
per-pool `otherwise`/dependency discipline ADR-0038/ADR-0039 already
establish — applied to this block's own candidates. No explicit `commit …
from (…)` is written for a `decide` block: the block's own `propose` items
*are* its pool, implicitly, for every instantiated element.

### Naming and scope

A `decide` block's candidate names are checked for uniqueness **program-wide**
— in the same namespace a top-level `propose` occupies — even though a given
candidate is only ever a member of its own block's per-instance pools, never
a top-level `commit`'s. This keeps one discipline for "what does `--candidate
NAME` (or `why`/`whynot`) mean" everywhere in a program, rather than two
different candidate namespaces a reader has to keep apart. A `commit` pool
may not name a `decide`-scoped candidate (`CandidateOwnedByDecide`): that
candidate is never a free-standing proposal a `commit` could adopt, only ever
instantiated as part of its own block.

A `decide` block's own name (`status` above) lives in the same namespace a
`commit` pool's name occupies — both are readable only through `show`, and a
`decide` cannot share a name with a `commit` or another `decide`
(`DuplicateDecideName`).

### `show`

`show status` evaluates to **the list of decided values, in element order** —
the per-entity analogue of `show <commit name>` reading a pool's own decided
value (ADR-0030/ADR-0039). It resolves once every instance in the block has
settled with a selection; if the block itself is Unknown (see below), or if
even one instance is quiescent (selected nothing), `show status` is unbound —
a fault, exactly like showing a quiescent commit pool's own name is a fault
today. A future ADR could specialize this to admit partial/`Option`-shaped
results per element; this one keeps the discipline that already exists for a
`commit` pool's own name, extended to the plural case, rather than inventing
a second one.

## Deliberation, independence, and faults

Every instance of a `decide` block deliberates with the same guard/value
evaluation, frontier admission, and selection-or-quiescence settlement
ADR-0030 specifies for a `commit` pool, scoped to that instance's own
candidates. The block's list expression is evaluated **once**, from the
shared inputs/`let`s/rule facts — the same "computed once, deliberated many
times independently" split ADR-0039 already uses between the shared
lets/rules stage and each pool's own deliberation, one level deeper.

**Independence between blocks and pools.** A fault in one `decide` block
never affects a sibling `decide` block or any `commit` pool — every
declared pool and every declared block is its own island, exactly as
ADR-0039 established between `commit` pools. A `commit` pool's fault
likewise never blocks a `decide` block from settling.

**All-or-nothing within one block.** Unlike two *different* `commit` pools
(independent of each other), the instances of *one* `decide` block are not
independent of each other for fault purposes: a fault evaluating the block's
own list expression, a guard/value fault in any one instance, a proposal
value type mismatch in any one instance, or a deliberation/settlement fault
in any one instance makes the **whole block** Unknown
(`FiniteDecisionDecideStop::Unknown`) with **no partial per-instance
results** published. This is a deliberate choice, not an oversight: a
per-entity block is one *logical* decision — "decide status for every
order" — and a caller reading `entity_decisions` for `status` should never
have to distinguish "this order was never evaluated" from "this order was
evaluated and quiesced" from "the whole batch failed". Collapsing all three
into "the block published nothing, with a typed reason" keeps the same
fail-closed guarantee ADR-0002/ADR-0030 already give a single pool, applied
uniformly to the plural case. A future ADR could relax this to a per-instance
partial-result mode; nothing here forecloses it, but it needs its own design
for what a caller does with a decision that is only half-published.

## Bounds and fail-closed behavior

- Each block's own list is already bounded by its source: an input's
  declared `max` (≤256, ADR-0037) or the derived-list cap
  (`MAX_DERIVED_LIST_LEN` = 4,096, ADR-0040) if the list expression is
  itself a `filter`/`map`/comprehension/list-literal/`distinct` result.
- A **program-wide** cap, `MAX_TOTAL_DECIDE_INSTANCES` = 4,096, bounds the
  *sum* of every `decide` block's own list length together, checked once
  every block's list has been evaluated and before any block's per-instance
  deliberation runs. Exceeding it is a typed Unknown
  (`FiniteDecisionUnknownReason::DependencyFault`) for **every** `decide`
  block in the module — the cap is a shared resource, not a per-block one,
  so one block cannot "use up" the budget silently at another's expense; a
  reader sees every block report the same cap violation rather than an
  arbitrary subset succeeding depending on declaration order.
- Each instance's own guard/value evaluation is charged to the *existing*
  ADR-0032 evaluator work budget exactly like a top-level proposal's guard/
  value already is — one fresh `MAX_CALL_STEPS` (10,000) budget per
  `eval()` call, i.e. per instance per proposal, not a new shared counter.
  A large instance count is bounded by `MAX_TOTAL_DECIDE_INSTANCES` above,
  not by trying to fit all of it in one evaluator budget.

## Identity

A `decide`-free program keeps a byte-identical preimage and program id to
one written before this ADR: `decide` blocks are appended at the very end of
the canonical preimage, after ADR-0039's `brix.l3.finite-decision.commits@2`
section, under a new tag `brix.l3.finite-decision.decides@1`, written only
when the module declares at least one block — following exactly the
append-only precedent `commits@2` itself set over the single-commit
preimage. The section is:

```
tag "brix.l3.finite-decision.decides@1"
uint  block_count
for each decide block, in declaration order:
  ident block_name
  ident binder_name
  <L3ExprV2>            // the list expression
  uint  proposal_count
  for each proposal, in declaration order:
    uint  ordinal        // 0-based, within this block only
    ident proposal_name
    uint  dep_count
    ident dep, ...
    uint  priority
    <L3ExprV2>            // guard
    <L3ExprV2>            // value
```

No new `L3ExprV2` ordinal is needed: a `decide` block's binder is lowered
exactly like a fold/filter/map/comprehension binder (ADR-0037/ADR-0040) —
a hygienic local resolved to the existing `LetRef` encoding by
`lower_expr_v2`'s `locals` scoping, not a new expression form. The block
itself is plan-level structure (a new field on `FiniteDecisionPlan`,
`FiniteDecisionDecide`), not a new expression, so it needs no `L3ExprV2`
ordinal at all; the 40–49 range this ADR was allotted for expression
ordinals is therefore unused.

Adding, removing, renaming, or reordering a `decide` block or any of its
candidates, or changing its binder name, list expression, or any candidate's
guard/value/priority/dependencies, changes the program id. `decide` becomes a
reserved keyword (checked against every example, package, fixture, and test
in the tree — none used it as an identifier before this change).

### Run-time identity: per-instance namespacing

A `decide` block's candidate name (`ship`, `hold`) is declared **once** in
source but instantiated **many times** at run time — once per element. Two
different instances' `ship` candidates must never collide on the same
destination world, generator id, or witness (a top-level `commit`'s
candidates never have this problem, since each name is declared, and thus
digested, exactly once). Every identity-bearing digest for an instance's
candidate is therefore namespaced by `(decide block name, element index,
candidate name)`, not just `candidate name`:

- `entity_proposal_digest(program, decide, index, name)` — the per-instance
  analogue of `proposal_digest`, under its own tag
  `brix.l3.finite-decision.entity-proposal@1`.
- `entity_generator_id(program, decide, index, name, src, dst)` — the
  per-instance analogue of `generator_id`, under its own tag
  `brix.l3.finite-decision.entity-generator@1`.

This guarantees every instance's destination `ConfigId` (and thus its
`GeneratorId` and `Witness`) is distinct from every other instance's, from
every top-level commit pool's, and from itself under structural equality —
even when the *candidate name itself* is reused verbatim across instances
(as it always is: `ship` names "the ship candidate for whichever order this
instance is about", not one specific order).

### Calendar phase

Every commit pool and every `decide`-block instance is assigned its own
calendar phase (the `phase` argument to the shared settlement-tick/
quiescence machinery), counting up in declaration order: commit pools first
(`0..commits.len()`), then every `decide` block's instances, block
declaration order and then element index, continuing from where the commit
pools left off. **This is not load-bearing for correctness** — each pool's
and each instance's own destination worlds already differ by construction
(see above), so two deliberations never actually contend over the same
calendar key regardless of phase — but assigning distinct phases keeps every
settled step traceable to a specific pool or instance by phase alone, and
costs nothing to provide. A later design that *does* need phase to carry
ordering weight (e.g. a cross-instance settlement discipline) is free to
revisit this; nothing here assumes phase is currently significant beyond
traceability.

### Journal order

`Journal` order is `commit` pool declaration order first (unchanged from
ADR-0039), then `decide` block declaration order, and within one block,
element index — i.e. exactly the order the blocks and their candidates are
written and instantiated. `brix audit`/`brix verify` replay every instance's
step from this same order; nothing about audit/verify's existing discipline
changes beyond having more steps to replay when a program uses this feature.

## CLI

`brix run`'s human output prints a `decide <name>:` section per block (after
every `commit <name>:` section, ADR-0039), one line per settled instance, in
element order:

```
decide status:
  [0] Order { units: 5 }: ship = Ship @Derived
  [1] Order { units: 20 }: hold = Hold @Derived
```

A quiescent instance prints its own line with no winning candidate; an
Unknown block prints one line naming the block's own fault reason instead of
per-instance lines. JSON output adds an additive `entity_decisions` array —
one entry per `decide` block, each with that block's own `status` (settled/
unknown) and a list of per-instance `{index, binder, candidate, value,
grade}` (or the fault reason) — populated only when the module declares at
least one `decide` block; a `decide`-free module's JSON is textually
unchanged from before this ADR (matching the `commits` array's own
already-additive precedent).

### `why` / `whynot`

A `decide`-scoped candidate name is still unique program-wide (see
*Naming and scope*), so `brix why NAME`/`brix whynot NAME` already resolve
`NAME` to its own `decide` block unambiguously; explaining *which instance*
needs one more coordinate. This ADR adds `--entity INDEX` (0-based, matching
`entity_decisions`'s own indexing): `brix why ship --entity 2` explains
instance 2's own admission/selection, including the binder's own value in
the explanation, against that instance's own admitted set and winner —
never a sibling instance's. Omitting `--entity` on a `decide`-scoped
candidate is refused with a message naming which flag is required, rather
than guessing an instance; a `commit`-scoped candidate keeps ignoring the
flag exactly as today; `--entity` on a non-`decide`-scoped candidate name is
likewise refused. This was chosen over threading a full `(decide, index)`
pair through `--candidate` itself (e.g. `--candidate status.ship.2`) because
it keeps `--candidate`'s own value exactly the plain proposal name a reader
already writes in source, with the one genuinely new piece of information —
*which* instance — as its own explicit flag.

### `brix test`

`expect` gains `entities`, a per-`decide`-block companion to `decisions`
(ADR-0039): `"entities": {"status": ["ship", "hold"]}` asserts block
`status`'s own decided candidates, **in element order** — the natural
sibling of `decisions`' per-pool `"(none)"` sentinel would be an element
that quiesces; an Unknown block is asserted by checking the run's overall
stop/fault the same way an Unknown `commit` pool already is. `expect.
candidates` already searches every `commit` pool for a named candidate; it
is extended to search every `decide` block's own proposals too, so a
candidate declared inside a `decide` block can still be asserted admitted/
selected/rejected by name without `--entity` (searching across every
instance) — asserting one *specific* instance's disposition uses `entities`
instead.

### `brix audit` / `brix verify`

Round-trip unchanged in kind: every `decide`-block instance's own committed
step is an ordinary journal step (see *Journal order*), so a module using
this feature audits/verifies exactly like a multi-commit module already
does — more steps to check, not a new checking discipline.

### `brix kb`

`brix-kb`'s diff currently compares two `FiniteDecisionRun`s' `commits`.
Extending it to diff `decides` as well (a block appearing/disappearing, an
instance's decided candidate changing) is straightforward — same shape, one
more list level — and is a direct, low-risk follow-up; it is not included in
this change's implementation to keep the slice reviewable, and is called out
explicitly rather than silently deferred.

## What did not change

- The SOC authority boundary and settlement discipline (ADR-0002, ADR-0030):
  every instance still fails closed to Unknown on any fault, never
  fabricates Proven/Refuted, and every published grade stays Derived.
- A `commit` pool's own meaning, encoding, and CLI output (ADR-0030,
  ADR-0039): a module using no `decide` block is unaffected byte-for-byte.
- `rule`/`propose` dependency inference and `otherwise` (ADR-0038): unchanged,
  and reused verbatim per `decide` block.
- List/relational values and folds (ADR-0037, ADR-0040): a `decide` block's
  list expression is an ordinary list-typed expression: any input, `let`,
  `filter`/`map`/comprehension result, or list literal works as `LIST_EXPR`
  without new grammar.

## Performance note

The recursive evaluator (`eval_internal_body` in `l3_v2.rs`) and the static
Boolean/list-shape analysis (`expr` in `finite_decision/boolean_types.rs`)
gained no new match arms for this feature — a `decide` block's binder reuses
the existing `LetRef`/`locals` machinery, and its list/guard/value bodies are
ordinary `L3ExprV2` trees evaluated and analyzed exactly like any other rule/
proposal body. The new per-entity orchestration
(`FiniteDecisionRuntime::run_decides`/`run_decide_block`) lives entirely in
`finite_decision/runtime.rs`, outside both recursive functions, so neither
function's stack frame grew and the existing deep-recursion stack-size
guarantees are unaffected.

## Acceptance checklist

- Preserve every existing finite-decision behavior and encoding byte-for-byte
  for a `decide`-free module; append `brix.l3.finite-decision.decides@1`
  only when the module declares at least one block.
- Parse `decide NAME for BINDER in LIST_EXPR { propose ... }`; reserve
  `decide` and confirm no existing example, package, fixture, or test used
  it as an identifier.
- Lower each block into a `FiniteDecisionDecide` plan node: its own list
  expression and its own candidate proposals (binder-scoped, program-wide
  unique names, per-block `otherwise`/dependency discipline).
- Instantiate each block once per list element at run time, deliberating
  each instance independently with the existing frontier/calendar/commit-
  tick machinery, namespaced per `(block, index, candidate)` so instances
  never collide with each other or with commit pools.
- Enforce the program-wide instance cap across every block together; exceed
  it and every block reports a typed Unknown.
- A fault in one instance fails its whole block closed with no partial
  results; a fault in one block never affects a sibling block or any commit
  pool.
- `show <block name>` evaluates to the list of decided values in element
  order.
- `brix run` prints a `decide <name>:` section (human) and an additive
  `entity_decisions` array (JSON); `brix test` gains an additive `entities`
  expectation; `brix why`/`brix whynot` gain `--entity INDEX`; `brix audit`/
  `brix verify` round-trip through every instance's journal step; `brix kb`
  keeps working (diffing `decides` noted as a follow-up, not implemented
  here).

## Related decisions

- [ADR-0030](ADR-0030_Finite_Decision_Alpha.md) establishes the finite-
  decision profile, the single-commit deliberation/settlement discipline,
  and program identity this ADR extends.
- [ADR-0037](ADR-0037_Bounded_Lists_And_Folds.md) and
  [ADR-0040](ADR-0040_Finite_Relations.md) admit the bounded list values a
  `decide` block instantiates over.
- [ADR-0038](ADR-0038_Inferred_Dependencies_And_Otherwise.md) supplies the
  inferred-dependency and `otherwise` discipline this ADR reuses per block.
- [ADR-0039](ADR-0039_Multiple_Commit_Pools.md) establishes multiple
  independent commit pools per module and the pool-level independence this
  ADR mirrors one level down, to a block's own instances.
