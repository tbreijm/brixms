# 0004 — A relation cannot grant both `write` and `derive`

**Lane:** compiler + ast (brixc, brix-ast)
**Status:** **PROPOSED** — drafted 2026-07-26, **not ratified**. Awaiting a
maintainer ruling. The implementation below has landed behind this draft; if the
ruling goes the other way it is a revert, not a migration.
**Affected sections:** [errata 0003](./0003-package-pub-visibility-surface.md)
§"Ruling (adopted)" (the `Vis`/`RelVis` EBNF and the least-privilege reading).
Appendix D of `BrixMS_v9_0.md` carries no `pub` token, so the normative document
is untouched — the surface grammar for visibility lives entirely in errata 0003.
**Affected conformance:** none. No frozen conformance vector asserts a `pub`
token, and the change is additive — every string errata 0003 accepted still
parses to the same grant.

## The observation

Errata 0003 adopted a **single-valued** capability qualifier:

```ebnf
Vis         := "pub" RelVis? ;      (* RelVis only before a RelDecl *)
RelVis      := "read" | "write" | "derive" ;
```

`read` **xor** `write` **xor** `derive`. Errata 0003 noted the consequence itself,
in its own implementation notes:

> a relation cannot today grant a downstream package *both* direct assertion
> (`write`) and rule-extension (`derive`). If a relation needs both, the surface
> must grow (e.g. `pub write derive`) — out of scope for this ruling.

The three capabilities are independent — they gate three different acts, at three
different sites in the compiler (`BRX-LOW-0019`/`BRX-LOW-0020` for extension,
`BRX-LOW-0021` for assertion). Nothing about them is mutually exclusive; the
single-valued surface is the only thing making them so. A relation that a
dependency should be able to both assert into and extend by rule is not
expressible.

**Honest note on need.** No relation in the current corpus requires both, so this
is not a bug report — it is the surface-completeness item errata 0003 explicitly
deferred, picked up on the maintainer's instruction. That is the whole of the
justification; it should be weighed as such when ruling.

## Open questions for the ruling

1. **Set or lattice?** Is `pub write` shorthand for `{read, write}` (capabilities
   accumulate, `read` implied by any `pub`), or exactly `{write}` (a relation
   could be assertable but not queryable)?
2. **Ordering.** Is `pub derive write` legal, and does it mean the same as
   `pub write derive`?
3. **Repeats.** Is `pub write write` an error or absorbed?

## Proposed ruling

**1 — Set, with `read` implied by any `pub`.** The qualifier becomes a set;
`read` is granted by every `pub`, and `write`/`derive` are additive grants on top.
`pub write derive` grants all three.

The alternative (strict least privilege, `pub write` granting write but *not*
read) was rejected as describing a distinction nothing enforces: there is **no
read gate in the compiler**, so every public relation is already queryable
regardless of qualifier. Adopting the strict reading would mean either building a
read gate — a behavior change to every existing `pub write`/`pub derive` relation
— or shipping a representation that lies about what is checked. This ruling keeps
the model and the enforcement in agreement.

This does not weaken errata 0003's least-privilege stance, which is about
`write`/`derive`: those remain strictly stronger than `read` and must still be
granted explicitly. Bare `pub` still means `read` and nothing more (errata 0003
ruling Q2 is preserved verbatim).

**2 — Order-insensitive, canonically formatted.** Any order parses. `fmt` emits
the canonical order `read`, `write`, `derive`, so `pub derive write` normalizes to
`pub write derive`. The AST stores the qualifiers **as written** (empty for a bare
`pub`), so `pub` and `pub read` stay distinguishable for round-tripping even
though both grant exactly `read`.

**3 — A repeated qualifier is a parse error.** `pub write write` reports
`BRX-AST-0001` rather than being silently absorbed.

### Grammar amendment (apply to errata 0003 on ratification)

```diff
  Vis         := "pub" RelVis? ;      (* RelVis only before a RelDecl *)
- RelVis      := "read" | "write" | "derive" ;
+ Vis         := "pub" RelVis* ;      (* RelVis only before a RelDecl *)
+ RelVis      := "read" | "write" | "derive" ;   (* set-valued; each at most once *)
```

Every string errata 0003 accepted still parses, to the same grant — `RelVis?` is a
subset of `RelVis*`. This is why the change is additive and needs no conformance
revision.

## Implementation

Landed behind this draft (issue #172):

- **`crates/brix-ast/src/ast.rs`** — `Visibility::Public(Option<RelVis>)` becomes
  `Visibility::Public(RelCaps)`, a `Copy` bitset. `Visibility::rel_cap()` becomes
  `rel_caps()`, applying both the errata 0003 Q2 normalization and the implied
  `read` of this ruling. The written set is stored unnormalized so `fmt`
  round-trips.
- **`crates/brix-ast/src/parser.rs`** — `parse_vis` consumes capability keywords
  in a loop, reporting a repeat. The loop stops at the first non-capability token,
  exactly where the single-valued form stopped, so `pub derive Foo: ..` is
  unaffected.
- **`crates/brix-ast/src/fmt.rs`** — `vis_prefix` emits the canonical order.
- **`crates/brixc/src/lower/resolve.rs`** — `export_caps` maps to `RelCaps`;
  `export_cap`/`with_export_cap` become `exported_caps`/`with_export_caps`.
- **Gates** — the three capability comparisons become set membership:
  `crates/brixc/src/lower/schema.rs` (`BRX-LOW-0019`, `BRX-LOW-0021`) and
  `crates/brixc/src/lower/decl.rs` (`BRX-LOW-0020`).

Tests in `crates/brixc/tests/graph_coherence.rs` cover the combined grant with the
controls that make additivity falsifiable: `pub write` alone must still seal
rule-extension, `pub derive` alone must still seal assertion, and `pub read write`
must behave exactly as `pub write`. Parser tests in `crates/brix-ast/src/parser.rs`
cover the set normalization, canonical-order formatting, and the duplicate error.

## What this does not change

- Bare `pub` on a relation is still `read` (errata 0003 Q2).
- `write` and `derive` are still never implied — only ever explicit.
- Visibility granularity is still **declaration-level**. Field-level `pub` on a
  `FieldDecl` remains deferred (errata 0003 open-question 3, issue #172): records
  lower as non-nominal row aliases (`crates/brixc/src/lower/schema.rs`), so a
  field still has no privacy anchor. Nominal records remain the prerequisite.
