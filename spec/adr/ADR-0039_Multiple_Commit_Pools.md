# ADR-0039 — Multiple commit pools

Status: **Proposed implementation**, 2026-09-28. Extends the finite-decision
profile from ADR-0030 through ADR-0038. Does not change the SOC authority
boundary, the external-input transport, or the single-tick settlement
discipline within one commit pool.

## Motivation

A finite-decision module could declare exactly one `commit` block: one
deliberation, one decision, one committed step. Real policy modules often
need to make more than one independent decision from the same facts — a
shipping method *and* a billing treatment for the same order, say — and
before this ADR that meant either two separate `.brix` programs (duplicating
every `config`/`input`/`rule` they share) or cramming unrelated candidates
into one pool and hoping their priorities never collide.

This ADR lifts the "exactly one commit" rule: a module may declare **one or
more** `commit` blocks, each an independent deliberation pool over its own
disjoint set of candidates.

## What changes

```brix
input order: Order
rule bulk = order.units >= 50

propose expedite priority 5 when order.priority_customer = Expedited
propose standard priority 10 when order.units >= 10 = Standard
propose hold_shipping otherwise = HoldShipping

propose discount priority 5 when bulk = Discounted
propose full_price otherwise = FullPrice

commit shipping from (expedite, standard, hold_shipping)
commit billing from (discount, full_price)
```

- **Independent pools.** Each `commit` deliberates only among its own
  candidates. `standard` never competes against `discount` — they are in
  different pools — even though both are evaluated in the same run.
- **Disjoint membership.** A proposal is a candidate in at most one pool.
  Naming the same proposal in two `commit` blocks is
  `ProposalInMultipleCommits` (which pool would settle it is otherwise
  ambiguous).
- **Unique names.** Two `commit` blocks may not share a name
  (`DuplicateCommitName`) — a pool's name is itself a readable fact (see
  below) and a CLI/`brix test` selector, so it must be unique.
- **Shared everything else.** All pools see the same `config`/`input`/`rule`/
  `let` declarations and the same once-computed rule facts; only the
  candidate pools themselves are separated.
- **`otherwise` is per pool** (ADR-0038 unchanged): each pool may separately
  declare its own fallback; two pools' fallbacks never conflict with each
  other.
- **A bound, like functions.** At most `MAX_COMMIT_COUNT` (64) commit blocks
  per module; more is `TooManyCommits`.

A module that still declares exactly one `commit` — every module written
before this ADR — keeps exactly its previous meaning: one pool, one
decision, unaffected by anything above.

## Deliberation and settlement

Each pool runs the same guard/value evaluation, frontier admission, and
selection-or-quiescence settlement ADR-0030 already specifies, but scoped to
its own candidates: its own `NamedCandidate`s, its own admission policy
closure, its own calendar comparison. A pool's fault (an evaluation fault, a
type mismatch, a settlement error) is a fault of *that pool only* — a
healthy pool still settles even when a sibling pool's deliberation faults.
The only stage every pool shares is the one before any pool runs at all:
evaluating `let`s and `rule`s. A fault there (an upstream fact none of the
pools could evaluate without) is necessarily a fault for every declared pool
alike, since none of them ever got past it.

Each pool that selects a candidate commits its own settlement tick; a
quiescent pool commits nothing. Every pool's tick, when it has one, is
appended to one shared [`Journal`](../../crates/soc-core/src/journal.rs), in
`commit` declaration order — `Journal` was already an append-only sequence of
independent steps (see its own module docs), so this is not a new capability,
just a new source of more than one step per run.

### `show` and each pool's own name

ADR-0030 already lets `show <commit name>` read a pool's own committed value
like an ordinary rule fact — `show shipping` reads whatever `commit shipping`
decided. With more than one pool, each one's name binds independently: a
quiescent pool leaves only its own name unbound (a fault if shown), while a
selecting sibling's name resolves normally.

## Program identity

The canonical preimage's commit section changes from "one name, then its
candidate list" (implicitly exactly one pool) to a self-delimited list: a
pool count, then each pool's name and candidate list, in declaration order —
exactly the convention the Rules and Proposals sections above it already
use. This is the one deliberate, one-time break in this ADR: **every**
finite-decision program's id changes, including every module that still
declares exactly one `commit` block, because the preimage now says "1 pool"
where it previously said nothing at all. `crates/brix-lower/tests/finite_decision_functions.rs`'s
two frozen-identity regression vectors are pinned to their post-ADR-0039
values with a comment explaining why, the same way ADR-0036 (unary minus)
documented its own frozen vector.

No other identity-bearing behavior changes: a single-commit module's
rules/proposals/functions/schemas encode exactly as before, and its
settlement tick, journal, and decision are byte-for-byte what ADR-0030
already specified.

## CLI and `brix test`

`brix run`/`brix check`'s human output prints one `commit <name>:` section
per pool, in declaration order, when a module declares more than one; a
single-commit module's output is textually unchanged (no pool header) from
before this ADR. Their JSON adds an additive `commits` array — one entry per
pool, each with that pool's own `status`/`candidates`/`decision` — populated
only when there is more than one pool; the existing top-level `status`/
`candidates`/`decision` fields keep meaning exactly the first pool's own
outcome, so a single-commit module's JSON is unchanged.

`brix why`/`brix whynot --candidate NAME` resolve `NAME` to its own pool and
explain it against that pool's own admitted set and winner — never against a
sibling pool's candidates, and a sibling pool's fault never blocks explaining
a healthy one.

`brix test`'s `expect` block gains `decisions`, a per-pool companion to
`decision`: `"decisions": {"shipping": "expedite", "billing": "discount"}`
asserts each named pool's own winning candidate (or `"(none)"` for that
pool's own quiescence). `expect.candidates` already searched by candidate
name; it now searches every pool, since a candidate may be in any one of
them.

See `examples/order-desk.brix` and `examples/order-desk.test.json` for a
worked two-pool module (shipping and billing over one order, each with its
own `otherwise`), including inferred dependencies (ADR-0038) reading the
shared `bulk` rule from both pools' guards.

## What did not change

- The SOC authority boundary and settlement discipline (ADR-0002, ADR-0030):
  each pool still fails closed to Unknown on any fault, never fabricates
  Proven/Refuted, and every published grade stays Derived.
- External input handling (ADR-0031): inputs are bound once, before any
  pool's evaluation, exactly as before.
- `rule`/`propose` dependency inference and `otherwise` (ADR-0038): unchanged
  per pool.
