# Beta roadmap

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

The user-reachable product today is **one program, one decision, over at
most 256 inputs.** There is no knowledge *base*:

- `rule threshold() = 10` is a named scalar, not a relation — there is no way
  to say "for every order, its threshold is..." and have that range over a
  set of orders.
- A finite-decision module may contain only **one** `commit`
  (`FiniteDecisionLowerError::MultipleCommits`); there is no way to decide
  once per entity in a batch.
- Nothing persists between runs. Every invocation of `brix run` starts from
  the source and the `--input` files given on that command line; there is no
  on-disk fact store and nothing to revise.
- The incremental engine (`soc_core::engine::IncrementalEngine`) exists and
  is proven to hold the O(Δ) invariant, but it is wired into only the L3 v1
  rule-agenda path (`crates/brix-lower/src/l3_run.rs`) — which `brix run`
  does not call. The path a user actually exercises re-derives everything,
  every time.
- Correction/retraction non-erasure (SOC-LAW-09) is **Partial**: the
  evidence-durability taxonomy exists, but there is no invalidation engine
  that makes "what changed, and what does that invalidate" a real,
  user-visible operation (tracked as #59, #178).

Put together: BrixMS can settle one bounded decision honestly and prove it
stayed honest. It cannot yet model a domain of things, hold facts about them,
or tell you what changed when a fact does. That is the beta gap.

---

## Milestone: One language

**Problem.** The `let` lane (`check_module`) and the finite-decision lane
(`lower_finite_decision_plan`) currently accept different fragments of what
looks like the same expression grammar, for reasons that are historical
(each fragment grew to serve its own profile) rather than semantic. Three
concrete splits, all verified in [`docs/brix-language.md`](../brix-language.md#known-inconsistencies-between-the-lanes):

- `/` is `Int / Int → Float` field-of-fractions division in the `let` lane,
  and refused outright in the finite-decision lane (which instead offers
  `div_floor`/`div_ceil`/`div_half_even`/`mod_euclid`, refused in turn in the
  `let` lane).
- Recursive `fn` and generic/recursive configs (`List<T>`) type-check in the
  `let` lane and are refused in the finite-decision lane
  (`FunctionCycle`, `"generic configs are unsupported"`).
- `&&`/`\|\|`/`!` work only in the finite-decision lane; `then`/`and`
  (witness composition) work only in the `let` lane (`brix-lower/src/l3.rs`
  still carries `WitnessCompositionNotAllowed` for the L3 path, per
  `docs/planning/language-usability-issue-drafts.md` item 3).

**Direction.** One shared checker and evaluator for one expression language,
used by every profile. A profile restricts what is *allowed* — bounds,
termination, which items may appear — never what things *mean*. The `/`
split, "recursion and generics only outside the executing lane," and "the
`let` lane never produces a runtime value" should each become an explicit,
documented profile restriction on one language, not a second language that
happens to share syntax.

**Open design questions.**

- Does unifying the grammar change canonical program identity for existing
  finite-decision programs? (Likely yes for some cases — e.g. if `/`
  becomes meaningful there — so this is ADR territory, not a patch.)
- Where do the profile-specific restrictions live: as a single grammar with
  a profile-parameterized checker, or as a shared core IR that each profile's
  checker restricts before evaluating?
- Does the `let` lane gain the ability to *run* (produce a value), or does
  "checks a type and grade, never a value" become its permanent, documented
  restriction — with values only ever produced by an executing profile?

**Acceptance criteria.**

- A single written decision (ADR) on which lane's behavior is authoritative
  for each of the three splits above, or an explicit statement that both
  behaviors are intentional and permanent, with the profile boundary that
  makes them so named in the grammar/checker, not left implicit in two
  separate code paths that happen to diverge.
- `crates/brix-lower/tests/doc_snippets.rs` (or its successor) still passes
  with fewer "known inconsistency" fragments than it has today — the
  fragments existing because the lanes really disagree, not because the
  documentation is being generous.
- No silent change to any existing program's canonical identity; a
  deliberate change to identity is versioned and stated as such.

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
