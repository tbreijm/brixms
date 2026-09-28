# Beta roadmap

*The prioritized plan, with the review behind it, is in
[`beta-plan.md`](./beta-plan.md), and what a beta promises is in
[`spec/Beta_Contract.md`](../../spec/Beta_Contract.md). This file keeps the
design questions for each milestone.*

This is a review of the project as it stands, and a plan for what a beta
needs beyond it. It describes **future work only** — nothing here is a status
report on what has already shipped. For what currently works, read the
[README](../../README.md) and [`docs/brix-language.md`](../brix-language.md);
for the normative law and contract state, read
[`spec/SOC_Semantic_Laws.md`](../../spec/SOC_Semantic_Laws.md) and
[`spec/Type_Realization_Contract.md`](../../spec/Type_Realization_Contract.md).

## What is already strong

Worth naming, because the gap below is a gap *in a good foundation*, not a
sign the foundation is weak:

- **Canonical identity.** Every semantic artifact — program, context, input
  snapshot, decision — is content-addressed through one encoder
  (`brix-canon`), independently cross-checked in a second language
  (`scripts/canon_crosscheck.py`).
- **Graded evidence with separate authorities.** `@Derived`, `@Audited`, and
  `@Proven` are not the same claim rounded to different confidence levels;
  each has exactly one authority allowed to publish it (the settlement
  kernel, the audit-factorization checker, and the proof kernel,
  respectively), and strengthening one into another without doing the work
  is rejected as epistemic erasure.
- **Replayable audit bundles and `brix verify`.** A decision can be
  independently re-derived from source and the inputs that produced it, and
  checked offline against a pinned program identity.
- **A small proof kernel.** `brix-kernel` depends on nothing but
  `brix-semantic` and `brix-canon`, keeping the trusted proof boundary
  independent of the parser, runtime, and regimes that construct proof
  candidates.
- **Determinism CI.** Repeated test execution asserts zero drift on frozen
  artifacts; the `o_delta_gate` proves the incremental engine's cost tracks
  the change, not the world.

## The core gap

When this roadmap was written, the product was **one program, one decision,
over at most 256 scalar inputs**, with nothing persisted between runs. Most
of that gap has since closed:

- **Relations.** Bounded list inputs (`brix.input@3`), folds, `filter`/`map`,
  and multi-generator joins (ADR-0037, ADR-0040).
- **More than one decision.** Several independent `commit` blocks (ADR-0039)
  and per-entity `decide … for o in orders` blocks (ADR-0043).
- **Persistence.** `brix kb` keeps revisions as a hash-chained, replayable
  history with assert, retract, program change, diff, audit and verify
  (ADR-0041).
- **Integration.** `brix serve --stdio` and a Python client (ADR-0044).

What remains is below each milestone. The largest open items are quantified
rule schemas with per-tuple witnesses, incremental re-derivation (`brix kb`
replays each revision in full; `IncrementalEngine` is still wired only into
the L3 v1 path), CSV loading, and the explanation features at the end.

---

## Milestone: One language — largely landed (ADR-0042)

**Resolution.** [ADR-0042](../../spec/adr/ADR-0042_One_Evaluator.md) gave the
`let` lane and the finite-decision lane one shared evaluator
(`brix_lower::l3_v2::eval`) for the fragment both admit, and answered the
open design questions below: **yes**, the `let` lane now runs
(`brix_lower::evaluate_let_module` computes a value for every binding in the
exact executable fragment, printed by `brix check` as `name : Type @Grade =
value`, or `(not evaluated: …)` with the specific reason otherwise — never a
guess); recursion and generic configs are admitted and evaluated identically
in both lanes (a config's type parameters are erased for evaluation, exactly
like the values they never constrained in the first place); and the
profile-specific restrictions that remain (`Float`, witness composition, and
a generic type in a finite-decision schema/contract) are named, documented
profile choices — see
[`docs/brix-language.md`](../brix-language.md#what-still-differs-between-the-lanes)
— not silent disagreements between two otherwise-identical-looking
expression languages. Canonical identity for every program that did not use
the newly-admitted forms is unchanged (verified against `examples/*.brix`
program/context ids and the frozen program-id regression tests in
`crates/brix-lower/tests/finite_decision_functions.rs`); a generic config
declaration is now additionally bound into a finite-decision program's
identity (a new, additive preimage section, empty and therefore
byte-identical for every program that declares none).

What the original three splits below became:

- `/` still means one thing everywhere (`Int / Int → Float`), and is still
  refused in the finite-decision lane — because `Float` itself is not
  admitted there, not because `/`'s meaning is unclear. Both diagnostics
  (the finite-decision refusal, and the `let` lane's "not evaluated" reason)
  now say so in the same words and name the same replacements
  (`div_floor`/`div_ceil`/`div_half_even`/`mod_euclid`).
- Recursive `fn` and generic configs are admitted and evaluate identically
  in both lanes now (ADR-0042 supersedes ADR-0032's `FunctionCycle`
  refusal and the finite-decision lane's blanket "generic configs are
  unsupported"); a generic config is still refused specifically in an
  `input`/helper-contract schema, with a diagnostic naming why (type
  parameters are erased, so there is no payload shape left to validate).
- `&&`/`\|\|`/`!` still work only in the finite-decision lane (the `let`
  lane's own type-realization grammar has no Boolean-operator rule yet —
  unlike the other two splits, this one was never about the shared
  evaluator); `then`/`and` (witness composition) still work only in the
  `let` lane, and the shared evaluator still has no value for a composed
  witness, so a `let`-lane `then`/`and` binding type-checks but is not
  evaluated.

**Still open** (deliberately out of this milestone's scope): giving the
`let` lane's own type-realization grammar `&&`/`\|\|`/`!` (a checker-side
gap, not an evaluator one); admitting `Float` or witness composition into
the finite-decision lane's expression grammar at all (a profile-scope
decision, not a mechanical follow-on); and reconciling the relations
milestone's built-in `List<T>` name with a user's ability to declare a
same-named config (currently: the built-in wins in every type position,
which is confusing rather than unsound — see
`docs/brix-language.md`'s note on `Stack<T>`/`Tree<T>`).

---

## Milestone: Relations and per-entity decisions

**Problem.** A real domain has more than one of a thing (orders, vehicles,
customers), relationships between them, and rules that range over a
collection rather than naming one scalar. None of that exists today:
`rule`s are 0-ary-parameterized scalars, there is no join, filter, or
aggregate over a set of facts, and a module commits at most once.

**Direction.** Bounded Datalog-shaped rules: multi-tuple facts, joins,
filters, and aggregates over them, stratified negation so a rule set still
has one well-defined least fixed point, and every derived tuple carrying its
own witness (so "why is this true" stays answerable per-tuple, not just
per-program). Multiple, or per-entity, commits — decide once per order in a
batch, not once per module. First-class queries that return graded results,
not just a settled decision.

**Suggested scoping.** Rather than landing bounded lists (ADR-0037), witness
composition in the finite-decision lane (`docs/planning/language-usability-issue-drafts.md`
item 3), and rule schemas (`docs/planning/language-usability-issue-drafts.md`
item 4) as three separate, sequential steps, consider one "finite relations"
ADR that subsumes all three: a list is a bounded relation with one column: a
fold is an aggregate over it; a rule schema is exactly the quantified-rule
mechanism relations need anyway. Landing them separately risks landing three
incompatible partial answers to the same underlying question (how does a
rule range over more than one fact?).

**Progress.** ADR-0037 and ADR-0040 landed the *value-level* slice of this:
`List<T> max N` inputs (`brix.input@3`), `sum`/`count`/`all`/`any`/`min`/`max`
folds, `filter`/`map`, a comprehension (a bounded multi-generator join, so
`for o in orders, s in stock where s.sku == o.sku yield ...` already answers
"how does an expression range over more than one list" for a *list value* an
expression can name), list literals, `in`, `len`, and `distinct` — see
[`docs/brix-language.md`](../brix-language.md) §4 and
[`examples/fulfillment.brix`](../../examples/fulfillment.brix). What remains
open from this milestone: a **rule schema** (quantifying a `rule` itself over
a bounded domain, rather than joining lists an expression already holds),
per-tuple witness composition — neither of which ADR-0040 attempts (its own
scope note says so explicitly). Multiple commits landed in ADR-0039 and
per-entity commits in ADR-0043: each instance of a `decide` block deliberates
with its own calendar phase, all instances append to one journal in a fixed
order, and `brix audit`/`verify` replay them all. The first three open design
questions below remain open; the fourth is answered by ADR-0043.

**Open design questions.**

- What is the grounding discipline for a quantified rule — finite
  pre-grounding over a declared, bounded domain, evaluated once per
  admitted tuple? How is that domain bounded and checked before evaluation
  (mirroring the existing 256-input, 4,096-node-per-shard discipline)?
- What identity does a derived tuple carry, and how does its witness compose
  with the facts and rule instance that produced it?
- How does stratified negation interact with the existing evidence-grade
  lattice — does a negated dependency cap the derived grade the way an
  undischarged arithmetic leaf does today?
- What does "commit per entity" mean for the deterministic keyed calendar
  that today selects exactly one candidate module-wide? Does each entity get
  its own calendar, and if so, what binds them into one replayable program
  identity?

**Acceptance criteria.**

- A program can declare a relation with more than one tuple, join or filter
  it, and derive facts that range over it, each with its own graded witness.
- A single finite-decision module can commit a distinct decision per element
  of a bounded input collection, with the same audit/verify guarantees a
  single commit has today.
- The grounding domain, instance count, and evaluation work are all bounded
  and checked at `brix check` preflight, exactly as scalar inputs are today.
- Ordered first-success selection (the "for each filter in order, pick the
  first exact match" pattern already identified in
  `docs/planning/language-usability-issue-drafts.md`) is specified in the
  deliberation layer, not hidden inside monotonic rule derivation.

---

## Milestone: Persistent, revisable knowledge base

**Problem.** `brix run` is one shot: read source and inputs, decide, exit.
Nothing survives to be corrected. SOC-LAW-09 (correction/retraction
non-erasure) is `Partial` for exactly this reason — there is no store to
retract *from*.

**Direction.** Facts asserted and retracted as **revisions**, not
overwrites — a retraction is itself a recorded, evidenced event, never a
silent deletion. An on-disk, append-only journal of those revisions.
Incremental re-derivation of affected facts and decisions through
`IncrementalEngine`, actually wired into the path `brix run`/a future
long-lived command exposes (today it sits unused behind `l3_run.rs`). A
"what changed, and what does that invalidate" report between two revisions —
this is what would make both the O(Δ) cost invariant and SOC-LAW-09
*user-visible*, not just true of an internal benchmark.

**Progress.** ADR-0041 landed `brix kb`: a directory holding hash-chained
revisions, each a full input snapshot under one program. Every read replays
from scratch, `brix kb diff` reports which inputs, facts and decisions
changed and why, `brix kb audit` emits a standard audit bundle for any
revision, and `brix kb verify` re-checks the whole chain. Still open: CSV
loading and incremental re-derivation. `brix kb` is honest about cost but
does not yet make the O(Δ) story user-visible.

**Open design questions.**

- What is the journal's on-disk format, and how does it relate to the
  existing audit-bundle format — one persistent store that audit bundles are
  extracted from, or two separate artifacts with a defined relationship?
- Does a revision change canonical program/context identity, or does
  identity attach to a (program, revision) pair? Get this wrong and either
  replay breaks across revisions, or two different worlds silently share an
  identity.
- What triggers re-derivation: does the caller name which facts changed, or
  does the engine diff the new revision against the last one it saw?
- How does a retracted fact interact with a decision already committed on
  top of it — is the old decision left standing with a note that its basis
  was retracted, or does retraction force a new deliberation?

**Acceptance criteria.**

- Facts can be loaded from JSON/CSV, asserted into a persistent store, and
  read back in a later invocation without re-supplying them.
- A fact can be retracted, and the retraction is itself an auditable,
  evidenced event — never a deletion that leaves no trace.
- After a small revision, re-derivation touches only the facts and decisions
  in that revision's footprint, and a command reports which ones and why (a
  user-visible O(Δ) story, not just a benchmark's).
- `brix verify` (or its successor) can independently replay a chain of
  revisions, not just one static snapshot.

---

## Surface ergonomics needing ADRs

*Both items below landed in ADR-0038: dependency lists are inferred when
omitted (an explicit list is still checked), and `otherwise` declares the
fallback. Programs that use neither keep their identities.*

Small in surface area, but each can move program identity or change what an
existing declaration means — so each needs a real design decision, not a
quiet patch:

- **Infer a rule or proposal's dependency list from its body**, instead of
  requiring it named twice. Today, `rule evenly_split(leftover) = leftover
  == 0` *looks* like `leftover` is a parameter, but it is actually a
  dependency declaration the checker verifies against the body — get the
  list wrong and the declaration is rejected, not silently ignored, but the
  redundancy is exactly the kind of thing a language usability pass should
  question. The complication: `input`s are ambient (read directly, not
  declared as dependencies) while `rule`s are not — any inference rule has
  to either unify that distinction or explain why it stays.
- **An `otherwise` fallback**, replacing the `priority 100 when true`
  pattern that appears in essentially every example in this repository
  (`examples/shipping.brix`, `examples/allocation.brix`, and this document's
  own snippets all end a proposal list this way). It is a real pattern
  asking for its own syntax, not just a style preference — an explicit
  "if nothing else matched" case is easier to audit than "priority higher
  than everything else, guard always true."

Both change what a declaration *means*, potentially the canonical identity
of every existing program that uses the pattern being replaced — so both go
through an ADR, not a silent parser change.

## Tooling

- **`brix fmt`.** Needs a lossless, comment-preserving concrete syntax tree
  first — the current AST (`brix-syntax`) is built for checking, not for
  printing back out with comments and formatting intact. This is a
  prerequisite piece of infrastructure, not a small addition.
- **A language server.** Hover-for-type-and-grade is the smallest useful
  slice: given a position in a `.brix` file, report the type and evidence
  grade the checker would assign there. Everything else (diagnostics on
  save, go-to-definition across `use` imports) can follow once that slice
  exists.

## Integration

*The server and the Python binding landed in ADR-0044 as `brix serve --stdio`
(JSON lines) and a standard-library client in `bindings/python/`. The stable
Rust facade is still open.*

- **A stable Rust facade.** Something a host application links against
  directly, with a compatibility contract, rather than the current
  CLI-shaped internal crate boundaries.
- **A JSON-over-stdio or JSON-over-HTTP server.** So a non-Rust host can
  drive `check`/`run`/`audit`/`verify` without shelling out to the CLI and
  scraping text or `--json` output.
- **A Python binding.** The most likely first consumer of the facade above;
  worth designing the facade with this binding in mind rather than
  retrofitting it.

## Explanations beyond traces

`why`/`whynot` today report which candidates were admitted, selected, or
rejected, and at what priority. Two things beyond a trace are worth adding:

- **Minimal counterfactuals.** Not just "candidate X was rejected because
  its guard evaluated to false," but "candidate X would be selected if
  `stock >= 50`" — the smallest change to the inputs that flips the
  decision.
- **Grade provenance.** Not just "this result is `@Audited`," but "`@Audited`
  rather than `@Proven` because leaf `X` is undischarged" — turning the
  honest-status ledger this document and `docs/brix-language.md` currently
  maintain by hand into something the CLI reports per result.

## Deprioritize for beta

Real work, but not what blocks someone from building something real with
this language today:

- Further kernel discharge work (issues #53, #56) — moving more of
  arithmetic and matching from `@Audited` to genuine `@Proven`. It is
  valuable and ongoing, but a user who can already build and audit a real
  decision does not need every leaf tight to get value from the system.
- Context confinement (issue #59, SOC-LAW-06, currently `Open`) — a real
  open obligation, but one that mostly matters once contexts start crossing
  trust boundaries in the ways the knowledge-base and integration milestones
  above would introduce. Revisit it once those milestones are closer.
- Studio/scale issues — keep parked until there is a knowledge base worth
  visualizing at scale.

**Process note.** Keep full ADRs for anything touching program/context
identity, ABI, or semantics — the four milestones above all qualify. For
pure surface sugar that provably does not change what an existing program
means or how it is encoded, a lighter change note (in the style of
`docs/planning/language-usability-issue-drafts.md`) is enough; save the ADR
process for decisions that need it.

## The beta bar

`docs/planning/language-usability-issue-drafts.md`'s "Coverage note" cites a
suite-world contract inventory — approximately 52 of 87 contracts covered —
as the forcing function for prioritization, and says plainly that the figure
is user-supplied and unverified. Treat it as a planning hypothesis, not a
verified metric, but use the inventory it points at as the forcing function
for what "beta" means: **a user can model that domain with relations, load
its facts from JSON/CSV, decide per entity, revise facts and see the
incremental impact, get a derivation-tree why/whynot for any decision, embed
the whole thing in a service, and every decision still verifies offline.**
Every milestone above earns its place in this roadmap by being required to
clear that bar.
