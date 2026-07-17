# 0001 — Several spec ` ```brix ` blocks are illustrative prose/templates, not Appendix D syntax

**Lane:** ast + fmt (brix-ast)
**Status:** proposed, awaiting ruling
**Affected sections:** Part I §3 (Pattern language), §6 (Queries and watches),
§7 (Constraints); Part VI §2 (Scenarios and adapters); Appendix D (surface
grammar)
**Affected conformance:** none directly — this is a corpus-classification
question, not a semantic one. It governs which spec blocks the parser corpus
(`crates/brix-ast/tests/fixtures/spec/`) counts toward clean-parse.

## The observation

`scripts/extract_fixtures.sh` mechanically carves every ` ```brix ` fenced
block from `spec/BrixMS_v9_0.md` into a parse fixture. Most are real, compilable
BrixMS. A minority are **illustrative prose or grammar templates** that were
never meant to parse as literal source. They contain constructs that appear
nowhere in Appendix D's EBNF:

1. **`0005` — Pattern language.** A *catalog* of clause forms shown one per line
   at file top level (outside any `derive … from { … }` block), each annotated
   with an explanatory `//` comment, e.g.:

   ```
   R(role: x, other)                    // edge clause; punning
   x: Entity { field, f2: v }           // entity attribute clause
   any { case {...} case {...} }        // disjunction
   exists { ... }                       // existence test
   ```

   Clauses are only grammatical *inside* a rule block; presented bare at
   top level they are not a `Decl`. The `...` are placeholders.

2. **`0009` — Queries and watches.** A single grammar template, not a program:

   ```
   query Name(args) -> Rel<Row> = from { ... } yield { ... }
   ```

   `args`, `Row`, and both `...` are metavariables standing for "fill this in".

3. **`0010` — Constraints.** A template whose parenthesised
   `(advisory | strict | audit)` is BNF alternation prose, not a constraint
   modifier list, plus `...pattern...` / `when ...` placeholders:

   ```
   constraint Name (advisory | strict | audit) { ...pattern... when ... }
   ```

   (Appendix D's `ConstraintDecl` picks exactly one of the three, unparenthesised.)

4. **`0011` — Scenarios and adapters.** A scenario skeleton with an
   angle-bracket metavariable `<adapter>` and `...transactions...` placeholders:

   ```
   bind P to <adapter>
   setup { ...transactions... }
   step every D for T { ...transactions per tick... }
   ```

`0003` (Declaration forms) is already treated this way by the corpus
(`is_known_errata`); this erratum simply records the same status for the four
above, and the general principle behind it.

## Why brix-ast cannot leave this unresolved

The corpus test `corpus_parses` asserts a floor of ≥50 of 57 fixtures parsing
with zero error diagnostics. If these prose blocks are counted, that floor is
unreachable no matter how complete the parser is, because the blocks are not
grammatical by construction. They are therefore excluded from the clean-parse
count (`is_known_errata` returns `true` for the `0003-`, `0005-`, `0009-`,
`0010-`, `0011-` prefixes) while still being required to (a) never panic and
(b) format idempotently — which they do, because `...` is parsed as a
first-class placeholder (`ExprKind::Ellipsis`, formatted back as `...`) and any
other unparseable prose is captured verbatim and re-emitted unchanged (see the
`fmt` idempotence design; the formatter never encodes anything on the comment
channel).

## Proposed ruling

Adopt one of:

- **(a) Fence-annotation (recommended).** Mark illustrative blocks in the spec
  source with a distinguishing fence info-string (e.g. ` ```brix-example ` or
  ` ```text `) so `extract_fixtures.sh` does not carve them as parse fixtures at
  all. This makes the corpus exactly "every block that claims to be real BrixMS,"
  removes the need for an `is_known_errata` allow-list, and documents intent at
  the point of authorship.
- **(b) Keep the allow-list.** Accept `is_known_errata` as the normative
  mechanism and freeze the five prefixes above as the known illustrative set,
  revisiting only if new prose blocks are added to the spec.

Either way, the substantive request to the spec author (Tony) is to **confirm
these five blocks are non-normative examples** and not grammar the parser is
expected to accept — so the exclusion is an authored decision, not a parser
convenience.

## What brix-ast does until ruled

Implements (b): `crates/brix-ast/tests/corpus.rs::is_known_errata` excludes the
five prefixes from the clean-parse count; all five still parse without panic and
format idempotently. If the ruling is (a), the `is_known_errata` list is deleted
and `extract_fixtures.sh`'s fence filter changes — a two-file change confined to
this lane, no AST or parser impact.
