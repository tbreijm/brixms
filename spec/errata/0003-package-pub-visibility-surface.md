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
