# 0003 — The surface syntax and granularity of `pub`/visibility is underspecified

**Lane:** compiler + ast (brixc, brix-ast)
**Status:** drafted 2026-07-21 — **ruled** 2026-07-25 (Tony); see "Ruling (adopted)" below
**Affected sections:** Part XXVIII §28.3 (Runtime closure — "`pub read` / `pub
write` / `pub derive` relation visibility"); §"modules, visibility, imports,
exports, and editions" (the Rust-parity feature list); Appendix D (Normative
surface grammar — EBNF)
**Affected conformance:** issue #42 acceptance ("visibility, and cross-package
symbol resolution"; "duplicate exports"). No frozen conformance vector asserts a
`pub` token today, so this is additive.

## The observation

Three places in the spec speak to visibility and they do not agree on a surface
syntax:

1. **§28.3 (normative, condensed)** names a *relation-granular* visibility:
   > **Packages**: 1:1 with generated crates; `pub read` / `pub write` /
   > `pub derive` relation visibility; orphan rule for `derive` mirroring trait
   > coherence …

   This is visibility on **relations**, split three ways (who may read, who may
   assert/write, who may `derive` against it under the orphan rule).

2. **The feature list** (Rust-parity prose) names "modules, **visibility**,
   imports, exports, and editions" as first-class — but as an unqualified
   language capability, not a grammar.

3. **Appendix D — Normative surface grammar (EBNF)** lists **no `pub` token at
   all.** `Decl` is a bare alternation (`EntityDecl | RelDecl | … | FnDecl | …`)
   with no visibility production, and `RelMod` (the relation-modifier list)
   contains only `key`/`unique`/`time`/`index`/`partition` — no `pub`,
   `pub read`, etc.

So the normative grammar cannot express the visibility §28.3 requires, and
neither §28.3 nor the grammar says whether a **non-relation** declaration (a
`type`, `enum`, `fn`, `protocol`, entity) can be marked exported, or whether
everything non-relation is implicitly public.

Issue #42's own working notes take a **fourth** reading — a single generic
`pub` prefix on any declaration ("only exported declarations are importable
cross-package; Slice 1 currently exports everything") — which is neither what
§28.3 words nor what Appendix D admits.

## Why the compiler cannot leave this unresolved

Issue #42's scope includes "visibility … for types, relations, functions,
protocols, and implementations" and "detect … duplicate exports". Slice 1 of the
implementation re-exports **everything** a dependency declares across the package
boundary; there is no way to mark a declaration package-private. Gating exports
(the remaining #42 visibility item) requires a surface marker, and the compiler
must not invent one: which of the four readings above is normative changes the
grammar (`brix-ast`), the parser, the formatter, and the cross-package export
filter (`brixc::lower_graph`). Per CONTRIBUTING ("when the spec is ambiguous you
do **not** guess"), this is filed for a ruling before implementation.

## Proposed ruling (for adoption)

Reconcile all three by making §28.3's relation granularity a **refinement** of a
single generic `pub`, and amend Appendix D accordingly:

- A leading **`pub`** may prefix any *exportable* declaration —
  `EntityDecl`, `RelDecl`, `EnumDecl`, `TypeDecl`, `RecordDecl`, `ProtocolDecl`,
  `FnDecl`. `pub` marks the declaration as importable across package **and**
  module boundaries. The absence of `pub` means **package-private** (visible
  within its own package's flat namespace only).
- For **relations**, `pub` may be further qualified as **`pub read`**,
  **`pub write`**, or **`pub derive`** (§28.3), naming which capability crosses
  the boundary: `read` = queryable/observable, `write` = assertable, `derive` =
  extensible by a downstream package's rules under the `derive` orphan rule. A
  bare `pub` on a relation is shorthand for the package's default export
  capability (proposed: `pub read`). Non-relation declarations take only bare
  `pub`.
- Appendix D amendments:
  ```ebnf
  Decl        := Vis? ( EntityDecl | RelDecl | EnumDecl | TypeDecl
                      | RecordDecl | ProtocolDecl | FnDecl | … ) ;
  Vis         := "pub" RelVis? ;      (* RelVis only before a RelDecl *)
  RelVis      := "read" | "write" | "derive" ;
  ```
  (`pub` lexes as an ordinary identifier today — no reserved-keyword change is
  needed; the parser matches it positionally, as it already does for `module`,
  `use`, `key`, etc.)

Rationale: this is the minimal surface that satisfies §28.3 literally (relations
keep their three-way granularity), honors the feature-list promise of
"visibility" as a general construct, and gives #42 the generic export gate its
scope calls for — without a second, conflicting visibility concept.

## Open questions for the ruling

1. Is the default really package-private, or package-**public** with `pub`
   meaning "also exported across packages" (a weaker gate)? The proposal assumes
   private-by-default (Rust parity), which will require existing multi-package
   fixtures to annotate their exported declarations. If the flagship and the
   `brix.*` standard-library packages should stay export-everything, the default
   must instead be public and `pub` a no-op until a `priv`/sealed marker exists.
2. Does a bare `pub` relation default to `pub read`, or to all three?
3. Field-level visibility (`FieldDecl`) — in scope, or a later erratum?

## Implementation alignment (pending ruling)

On adoption: add a `Visibility` marker to the affected `*Decl` structs in
`crates/brix-ast/src/ast.rs`, parse a leading `pub` (+ optional relation
granularity) in `crates/brix-ast/src/parser.rs` (`decl()`), emit it in
`crates/brix-ast/src/fmt.rs` (to keep the corpus idempotence test green), and
filter the dependency/module export loop in `crates/brixc/src/lower/mod.rs`
(`lower_graph`) to skip non-`pub` symbols — threading the flag out of each
dependency's lowering via the resolver. Until then, Slice 1's export-everything
behavior stands, documented as the pre-visibility surface.

## Ruling (adopted)

Adopt the proposed EBNF (`Vis?` on `Decl`, `RelVis` before `RelDecl`). Rulings on
the three open questions:

### 1. Default visibility: **package-private (Rust parity).**
Private-by-default, and take the migration now. It is the correct long-term
design and the cheapest moment it will ever cost:
- It aligns with the cohesion/weak-coupling thesis (#63) — export-everything is
  maximal coupling.
- It is the only default under which "detect duplicate exports" (#42 acceptance)
  is meaningful — you can only collide on what is explicitly exported.
- Public-by-default locks in the wrong default; reversing it later is a breaking
  change across every package, whereas the private-by-default migration is
  mechanical and the corpus is still small.

The flagship and `brix.*` stdlib packages must annotate their public surfaces —
honest API-declaration work that should happen regardless. Slice-1
export-everything was always a placeholder.

### 2. Bare `pub` on a relation = **`pub read` only.**
Least privilege. `pub write` and `pub derive` are strictly stronger and must be
explicit (each implies `read`, since you cannot assert into or extend a relation
you cannot observe). The load-bearing reason is **`pub derive`**: it is the
*coherence-affecting* capability (downstream extension under the orphan rule —
exactly #111's cross-package coherence surface). It must never be granted
implicitly by a bare `pub`.

### 3. Field-level visibility (`FieldDecl`): **deferred to a later erratum.**
Not needed to unblock #111/#42 (they need declaration-level export gating), and
it has no clean home yet — records currently lower as non-nominal row aliases
(`crates/brixc/src/lower/schema.rs`), so there is no nominal field surface to
attach privacy to. Revisit when nominal records exist.

Tracked to implementation in #151. The parse/AST/fmt/private-by-default surface
already landed (#108); the remaining relation-granular capability enforcement
(bare `pub` relation = `read`; `write`/`derive` strictly stronger; `pub derive`
gates downstream extension under the orphan rule) is the follow-on.

## Implementation notes (#154)

The relation-granular capability enforcement landed in #154:

- **Bare `pub` relation → `read` (Q2)** is enforced in lowering. The AST still
  stores a bare `pub` as `Visibility::Public(None)` (so `fmt` round-trips it
  unchanged); the normalization is a *reader*, `Visibility::rel_cap()`
  (`crates/brix-ast/src/ast.rs`), which maps `Public(None) → Read`. The
  capability now crosses into `brixc` via `ProgramResolver::export_caps`
  (`crates/brixc/src/lower/resolve.rs`), populated for every public dependency
  export in `lower_graph` — previously `RelVis` was parsed and formatted but
  dropped at the lowering boundary (the whole gap #154 named).

- **`pub derive` gate, two surfaces.** `pub derive` is "extensible by a
  downstream package's rules under the derive orphan rule" — which the compiler
  now enforces at both surfaces a downstream package can extend a foreign
  relation/head:
  - **`impl` heads (`BRX-LOW-0019`)** — a downstream `impl Trait for Head` may
    extend a dependency-owned head only if the trait is local, the head is local,
    or the head was exported `pub derive`. `check_impl_orphan` in
    `crates/brixc/src/lower/schema.rs`.
  - **`derive` rule heads (`BRX-LOW-0020`)** — a downstream `derive` rule may
    produce tuples into a dependency-owned relation only if it was exported `pub
    derive`. Gated in `lower_head` (`crates/brixc/src/lower/decl.rs`); a package's
    own relations are absent from `export_caps` and never gated.

  Both run once per package lowering, so every cross-package extension is checked
  exactly once, in the package that declares it.

- **Interpretation (for the spec owner to ratify or correct).** §28.3 words
  `pub derive` as *relation* visibility, but the `impl` heads that the orphan
  rule ranges over are entities/types (`Order`, `Money`). Because entities lower
  to relations in BrixMS, #154 attaches the `derive` capability to whatever
  exported declaration is the impl **head** (entity or relation): extension of a
  foreign head requires that head declared `pub derive`. This is the reading that
  makes `pub derive` the load-bearing, coherence-affecting capability the ruling
  describes; it should be confirmed when nominal records / dispatch land.

- **`pub write` gate (`BRX-LOW-0021`).** A `scenario` transaction that directly
  *asserts into* a dependency-owned relation (`assert`/`set`/`ensure`/`fresh`)
  requires
  that relation to be exported `pub write` (`write` = "assertable", distinct from
  the `derive` capability, which is a downstream *rule* extending the relation).
  Implemented as `check_scenario_writes` (`crates/brixc/src/lower/schema.rs`).

  The earlier "no lowering site" framing was a **misdiagnosis**: a *visibility*
  gate is static name resolution, not execution lowering. `Decl::Scenario` stays
  a v0 defer-line skip for *running* (its tx-bodies are never lowered to runtime
  IR), but the parser already builds the full write-surface AST
  (`TxExpr::AssertTuple`/`Set`/`Ensure`/`Fresh`/`AssertStruct`), so the gate needs
  only the resolver's `export_caps` + import map — the same inputs the `impl`
  gate uses.

  `retract`/`supersede` carry their target inside an expression rather than a
  head path, so they are pinned indirectly (issue #172): the gate carries a
  scenario-wide map from each `let`-bound name to the relation the bound tx-form
  wrote into, and the two expression forms resolve their operands through it. The
  retraction site **suppresses** its diagnostic when the binding site already
  reported, so one root cause is never counted twice. Note what this does and
  does not buy: every target reachable through a binding was produced by an
  `assert`/`set` the gate already checked, so the retraction path adds no
  diagnostic under today's capability model — its value is that the gate no
  longer skips a write form outright, and that the resolution machinery is in
  place. A `ClaimRef` that reaches a `retract` from anywhere *other* than a local
  write stays unpinned; catching that needs resolution of `ClaimRef<R>`'s type
  argument over scenario tx-blocks, which scenario bodies give no basis for (a v0
  defer-line skip — they are never typed or lowered). That is the prerequisite to
  revisit, not the retract surface itself.

  The binding map is deliberately the *only* handle used: `export_caps` carries
  every public dependency symbol (`pub fn`, `pub enum`, entities), and a bare
  `pub fn` normalizes to `read`, so resolving an arbitrary retract operand's head
  through the resolver would report `retract helper(c)` as a sealed relation.

  One surface limitation to note for a future erratum: a relation's `pub`
  qualifier is single-valued (`read` **xor** `write` **xor** `derive`), so a
  relation cannot today grant a downstream package *both* direct assertion
  (`write`) and rule-extension (`derive`). If a relation needs both, the surface
  must grow (e.g. `pub write derive`) — out of scope for this ruling.

  > **Superseded in part by [erratum 0004](./0004-combined-relation-capability-qualifiers.md)**
  > (**PROPOSED**, not ratified), which makes the qualifier set-valued and rules
  > that `read` is implied by any `pub`. Until that erratum is ruled on, the EBNF
  > above (`RelVis?`) remains the adopted grammar of record; 0004 carries the
  > exact amendment to apply on ratification. Q2 (bare `pub` = `read`) and the
  > least-privilege stance on `write`/`derive` are preserved either way.

- **Test coverage.** `BRX-LOW-0019`/`BRX-LOW-0020`/`BRX-LOW-0021` are inherently
  cross-package,
  so they are covered by the `brixc` graph tests
  (`crates/brixc/tests/graph_coherence.rs`), not the acceptance corpus — whose
  driver (`brixc::lower_file`) compiles a single file and cannot express a foreign
  head, the same reason cross-package coherence (`BRX-LOW-0017`) is corpus-tested
  only in its single-package overlap form. Capability normalization is unit-tested
  in `crates/brix-ast/src/parser.rs`.
