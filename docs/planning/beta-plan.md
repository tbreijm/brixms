# Beta review and investment plan

*September 2026. Companion to [`beta-roadmap.md`](./beta-roadmap.md), which
keeps the open design questions for each milestone; this document says what
to invest in, in what order, and why.*

**2026-10-03 execution update:** Following `v0.1.0-alpha.3`, the
[persistent world runtime handoff](./persistent-world-runtime-plan.md) defines
the next build: linked relational models, durable incremental updates, and
measured scaling under uneven submodel sizes. It advances the knowledge-model
and incremental-revision investments together. The review below remains the
September baseline; the handoff describes proposed work, not shipped support.

## Verdict

BrixMS has a rare core: decisions that are deterministic, replayable, and
independently verifiable, with honest evidence grades and a fail-closed
runtime. Almost no rule engine or LLM-based system can prove a decision
stayed honest. What kept it an R&D project was everything a user touches
first: what they can model, how much data it takes, how data gets in and
results get out, and whether anything they build on will still work next
release.

The work on this branch closes most of the *modeling* and *integration*
gaps. The largest remaining investments are, in order: **make it
releasable** (a compatibility contract, CI, releases), **a real knowledge
model** (rules that range over relations, with per-fact witnesses),
**getting data in** (CSV and JSON loading), and **incremental revision** (the
O(Δ) property users can see). Explanations and developer experience follow.
Proof-kernel and self-hosting research should wait.

## Where BrixMS stands

### Strong, and worth protecting

- **Honest outcomes.** A run is `selected`, `quiescent`, or `unknown`; it
  never guesses. Grades (`Proven`, `Audited`, `Derived`) say how much a
  result is backed.
- **Replayable identity.** Programs, inputs, and contexts have canonical ids
  with frozen vectors and an independent cross-check, so an audit bundle
  verifies offline, byte for byte.
- **Explanations from the real derivation.** `why`/`whynot` report the
  derivation tree the runtime used, not a reconstruction.
- **Engineering discipline.** Pinned toolchain, determinism CI, a
  trusted-core dependency gate, a semantic-law traceability map, and 45 ADRs.

### What held it back

Before this branch, the product was one program making one decision over at
most 256 scalar inputs, with nothing persisted between runs and no way to
call it except a one-shot CLI. A domain with many orders, customers, or
vehicles could not be modeled, and nothing could be revised.

### What this branch changes

| Area | Now | ADR |
| --- | --- | --- |
| Collections | Bounded list inputs, folds, `filter`/`map`, joins with `for … where … yield` | 0037, 0040 |
| Ergonomics | Dependencies inferred from rule bodies; `otherwise` fallback | 0038 |
| More decisions | Several independent `commit` blocks; `decide … for o in orders` per entity | 0039, 0043 |
| One evaluator | Recursion in both lanes; `brix check` prints values | 0042 |
| Persistence | `brix kb`: revisions, assert/retract, diff, audit, verify | 0041 |
| Integration | `brix serve --stdio` and a Python client | 0044 |
| Tooling | `brix test` suites, source locations, derivation explanations | — |
| Contract | [`spec/Beta_Contract.md`](../../spec/Beta_Contract.md) | — |

### What the review of this branch found

The branch was written quickly, largely by parallel agents, and then
reviewed. The review found real defects, all now fixed with regression
tests:

- **A fail-closed violation.** `brix run`, `check`, `test`, and `audit`
  judged success from the first commit block only. A fault in a second
  commit block or a `decide` block exited 0 with `"ok": true`. This is the
  one property the project cannot get wrong.
- **Knowledge bases that could wedge or become unreadable**: a snapshot
  merged from several inputs could exceed the reader's limit; a crash
  between writing a revision and updating `HEAD` blocked every later write;
  the reader rejected escapes its own writer produced; revisions recorded
  only the first decision.
- **An evaluator that copied instead of sharing**: every read of a list
  deep-copied it against a 1 MB budget, so inputs over about 1 MB were
  unusable and joins were refused. The same 256 × 256 workload went from
  8.3 s to 0.2 s once values were shared, and a run-wide step bound now caps
  total work.
- Smaller issues: a weakened audit journal check, a Python client that
  desynchronized after a timeout, and duplicated package loading.

**The lesson for how to invest:** each feature was well tested on its own;
the bugs lived where features met (multiple decisions × status reporting,
multiple inputs × stored snapshots). Cross-feature invariants need their own
tests, and changes of this size need review before they merge, not after.

## What "beta" means

A beta is a promise. [`spec/Beta_Contract.md`](../../spec/Beta_Contract.md)
defines it: program ids, input schemas, `--json` output, exit codes, the
serve protocol, audit bundles, and knowledge bases stay compatible across
beta releases; limits only go up; and a run succeeds only if every decision
settled.

The bar a beta has to clear, as user journeys:

1. **Model** a domain with several kinds of entity and relationships
   between them.
2. **Load** its facts from the files the user already has.
3. **Decide** per entity, and **explain** any decision as a derivation.
4. **Revise** facts over time and see what changed and why, at a cost that
   follows the change.
5. **Embed** it in a service, and **verify** every decision offline.

Today journeys 3 and 5 work, 1 works for list-shaped data, 4 works but
replays everything, and 2 needs hand-written input envelopes.

## Invest now, in order

Sizes are rough: **S** is days, **M** is a couple of weeks, **L** is a month
or more.

### 1. Make it releasable — S/M, do first

*Why:* nothing else matters if users cannot depend on it.

- Land this branch through review in slices (see **Sequence**), with CI
  green on each.
- Adopt the beta contract; add a `CHANGELOG.md`; cut `0.2.0-beta.1`
  through the existing release workflow, adding checksums and signatures
  to its binaries (it publishes neither today).
- Add cross-feature invariant tests of the kind the review found missing:
  every command's status and exit code against every decision shape
  (single, multiple, `decide`, mixed with faults), and every stored artifact
  round-tripped under maximum limits.
- Run fuzzing in CI on a schedule, and a security review of `serve`.

*Done when:* a tagged beta ships, and a contract violation is caught by CI.

### 2. A real knowledge model — L, the differentiator

*Why:* this is what turns a decision calculator into a knowledge engine.
Lists and joins let one expression range over data; a `rule` still names a
single value. Users need "for every order, its threshold is…" as a relation.

- Quantified rule schemas over bounded domains: multi-tuple facts, joins,
  aggregates, and stratified negation with one well-defined fixed point.
- A witness per derived fact, so `why` works per tuple, not per program.
- Decide the open questions in `beta-roadmap.md` (grounding discipline,
  tuple identity, negation and grades) in one ADR before writing code.

*Done when:* the order and inventory examples are written as relations and
rules, not list expressions, and every derived tuple explains itself.

### 3. Data in, results out — M

*Why:* hand-written `brix.input@3` envelopes are a non-starter for real
data.

- Load CSV and plain JSON through a declared schema mapping, with the same
  strict, bounded, fail-closed decoding as today.
- `brix serve` over HTTP as well as stdio, with the same protocol.
- A small, stable Rust facade (build, run, explain, audit) with a
  compatibility promise, since the Python client and future bindings sit on
  it.

*Done when:* a user decides over a CSV export without writing JSON by hand,
from Python or over HTTP.

### 4. Incremental revision — M/L

*Why:* "cost follows change" is a core claim, proven for an internal engine
but invisible to users. `brix kb` replays each revision in full.

- Wire `soc_core::IncrementalEngine` into `kb` revisions, with a differential
  test against full replay on every change.
- Report touched facts and decisions per revision (`kb diff` already shows
  what changed; this makes the cost follow it).

*Done when:* a one-fact revision on a large knowledge base re-derives only
its footprint, and `kb verify` still agrees with a full replay.

### 5. Explanations as the product — M

*Why:* explanations are where BrixMS beats both rule engines and LLM
systems.

- Counterfactuals: "this order would ship if stock ≥ 20".
- Grade provenance: "`Audited`, not `Proven`, because of this leaf".
- Full derivation trees for per-entity decisions (`why --entity` currently
  gives a shorter answer than a commit block's `why`).

### 6. Developer experience — M

*Why:* people judge a language in its first half hour.

- Error messages that point at the problem and suggest the fix, reviewed
  against real mistakes.
- `brix fmt` (needs a comment-preserving syntax tree first) and a minimal
  language server with type and grade on hover.
- One end-to-end tutorial on a realistic domain.

## Deprioritize for now

Valuable, but not why anyone adopts a knowledge engine: expanding the proof
kernel and discharging more primitives to `Proven`, self-hosting, parallel
deliberation, and the universal-world/faithfulness research. Keep their
gates green; do not start new work there until items 1–3 ship.

## Sequence

1. **Beta 1 (weeks):** item 1. Land this branch in reviewed slices, in this
   order so each is small and testable on its own: the fail-closed and
   evaluator fixes; `brix test`, explanations, and source locations; lists
   and relations; inferred dependencies, `otherwise`, and multiple commits;
   per-entity `decide`; `brix kb`; `brix serve` and the Python client. Tag
   `0.2.0-beta.1`.
2. **Beta 2 (a quarter):** items 2 and 3 in parallel (the relations ADR
   first), plus item 5's per-entity trees.
3. **Beta 3:** item 4, the rest of item 5, and item 6.

## Risks

- **Scope.** Relations can grow into a full Datalog. Keep it bounded and
  finite, matching the rest of the language.
- **Contract debt.** Every surface in the contract is a promise; add to it
  slowly.
- **Review capacity.** This branch showed that fast, parallel feature work
  produces bugs at the seams. Budget review time with every feature, and
  keep changes small enough to review.
