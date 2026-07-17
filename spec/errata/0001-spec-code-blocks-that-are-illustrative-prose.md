# 0001 — Several spec ` ```brix ` blocks are illustrative prose/templates, not Appendix D syntax

**Lane:** ast + fmt (brix-ast)
**Status:** ruled 2026-07-17
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

## Ruling (adopted 2026-07-17)

Adopt **(a), fence annotation.** Mark illustrative blocks in the spec
  source with a distinguishing fence info-string (e.g. ` ```brix-example ` or
  ` ```text `) so `extract_fixtures.sh` does not carve them as parse fixtures at
  all. This makes the corpus exactly "every block that claims to be real BrixMS,"
  removes the need for an `is_known_errata` allow-list, and documents intent at
  the point of authorship.
The five blocks are non-normative examples, not grammar the parser is expected to
accept. The exclusion is therefore an authored specification decision, not a parser
convenience.

## Implementation alignment

`brix-example` fences are excluded by `scripts/extract_fixtures.sh`; the parser corpus
therefore contains only blocks claiming to be executable BrixMS. The former
`is_known_errata` allow-list is deleted.
