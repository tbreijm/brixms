# ADR-0038 — Inferred dependencies and `otherwise`

Status: **Proposed implementation**, 2026-09-28. Extends the finite-decision
profile from ADR-0030 through ADR-0037. Does not change the SOC authority
boundary, the external-input transport, or the single-commit-per-program rule
(lifted separately by ADR-0039).

## Motivation

Every `rule` and `propose` carries a parenthesized dependency list today, e.g.
`rule is_eligible(credit_score, fraud_flag) = ...`. That list is required even
though it says nothing a reader could not already tell from the body: a rule
can only read a rule declared above it, so the set of rules it may read is
exactly the set it does read, checked. Naming it twice is ceremony, not
information, and it is ceremony that grows with every rule a policy adds.

`otherwise`, separately: a "catch-all" proposal is common (see `hold` in
`examples/shipping.brix`) and today it is spelled by hand as the highest
`priority` number the author is willing to type. That is not a fallback, it is
a guess at how many other proposals will ever outrank it.

## Inferred dependencies

The parenthesized list becomes **optional** on both `rule` and `propose`:

```brix
rule evenly_split = leftover == 0
propose ship priority 10 when can_ship = Ship
```

- **Omitted entirely** (`rule name = body`, `propose name priority ... when ... = ...`):
  dependencies are inferred. The body/guard/value may read any rule declared
  **above** it; lowering computes which ones it actually reads and records
  exactly those as `depends_on`/`deps`.
- **Written, even empty** (`rule name() = body`, `propose name() priority ...`):
  keeps today's meaning exactly — only the declared rules are readable, and
  reading anything else is `UndeclaredFactRead`. This is the same check that
  already exists; nothing about it changes.

A program that already declares every dependency it reads is unaffected: its
`depends_on`/`deps` and program id are byte-identical before and after this
change, because that is exactly the fixed point of "declared" and "inferred"
agreeing.

### How inference is computed

Lowering already builds each rule's/proposal's body/guard/value through the
shared `lower_expr_v2`, told which rule names are "readable" (turned into a
`RuleFact` node) versus a hard error. For an omitted list, lowering makes
*every* rule declared so far readable, lowers the expression as usual (so a
genuinely unknown name is still rejected exactly as before), and then walks
the **lowered `L3ExprV2` tree** collecting every `RuleFact` name it contains.
The walker (`collect_rule_fact_reads` in `finite_decision/plan.rs`) is an
exhaustive match over `L3ExprV2` with **no wildcard arm** — a new expression
form added by a later change is a compile error here, not a silently-missed
dependency, and match binders (and any lambda/comprehension binders a later
change adds) are already resolved to ordinary local references by
`lower_expr_v2` before this walk ever runs, so they need no special handling.

The canonical dependency list is: the declared entries as written (deduplicated,
as today — this part cannot change since inference does not run in that case),
then any inferred rule read but not already declared, **in rule declaration
order** (not the order it happens to appear in the expression). Declaration
order is available for free: it is the order rules already appear in the
plan's own `rules` vector.

### Forward references

Reading a rule declared **below** the reader (or reading itself) is rejected
with a message that names the problem and suggests the fix:

```
rule 'a' reads rule 'b', which is declared below it; rules can only read rules declared above them
```

This is detected because lowering already tracks every rule name declared
anywhere in the module (used today for other diagnostics); when an inferred
read resolves to nothing but the unresolved name is one of those, it is a
forward (or self) reference rather than a genuinely unknown name, and gets
this dedicated `FiniteDecisionLowerError::ForwardRuleRead` instead of the
generic "unresolved reference".

This diagnostic is message-only in this change, like every other
`FiniteDecisionLowerError` variant today — the codebase has no source-span or
source-map infrastructure yet (no `location_subject()`, no `SourceMap`)
for any lowering error to attach a line/column to. Adding one is a
reasonable follow-up but is a distinct, cross-cutting piece of work (every
`ast::Expr`/`Item` would need to carry a span from the parser through to
every error site), not part of this change.

## `otherwise`

```brix
propose hold otherwise = Hold
```

is exactly sugar for:

```brix
propose hold priority 18446744073709551615 when true = Hold
```

(`18446744073709551615` is `u64::MAX` — the parser desugars `otherwise`
directly to this priority and guard, so the two spellings lower to identical
`L3ExprV2`/`FiniteDecisionProposal` values and therefore an identical program
id.) The dependency list is independently optional, as for any other
`propose`: `propose hold(base) otherwise = base` and `propose hold otherwise = base`
are both valid.

Because lower priority wins ties in this deliberation (ADR-0030), the maximum
priority is the correct encoding of "last resort": an `otherwise` candidate is
selected only when it is the sole admitted candidate.

Two rules keep `otherwise` from being silently redundant or silently
ambiguous within one commit pool:

- **At most one `otherwise` per commit pool.** A second one is
  `MultipleOtherwiseInCommit`.
- **An explicit `priority 18446744073709551615` in the same pool as an
  `otherwise` is `AmbiguousFallbackPriority`.** Both would be that pool's
  fallback of last resort; which one actually is, is not decidable from the
  source without the reader independently discovering the coincidence, so it
  is rejected instead.

Neither check can fire for a program written before this change: no existing
program declares a `priority 18446744073709551615` proposal (the ADR-0032/
ADR-0033/ADR-0037 example corpus tops out at priority `100`), so both checks
are inert until a program actually opts into the sugar or the literal.

`otherwise` becomes a reserved word — it can no longer be used as a `rule`,
`propose`, `let`, `input`, config, variant, field, or parameter name. No
example, fixture, or package in this repository used it as an identifier
before this change (checked by grep across `examples/`, `packages/`,
`spec/conformance/`, and the crate test suites).

## Identity

Both features are encoding-transparent:

- Inferred dependencies produce the same `depends_on`/`deps` a correctly
  hand-written explicit list would have produced, encoded exactly as today
  (`finite_decision_program_preimage` is unchanged by this ADR).
- `otherwise` desugars in the parser, before lowering ever sees it, to the
  same `priority`/`guard` fields an explicit `priority 18446744073709551615
  when true` would produce.

So: every program that lowered before this change lowers to a byte-identical
program id after it, and a hand-written explicit-dependency-list program has
the same program id as its dependency-list-omitted twin, whenever the two are
semantically the same program (the twin's guard/value read exactly the
declared set).

## Grammar summary

```
rule NAME [ '(' IDENT,* ')' ] '=' EXPR
propose NAME [ '(' IDENT,* ')' ] ( 'priority' UINT 'when' EXPR | 'otherwise' ) '=' EXPR
```

`commit` is unchanged by this ADR (see ADR-0039 for multiple commits).
