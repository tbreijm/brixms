# Ring 0 dependency whitelist

Nothing enters `[workspace.dependencies]` without a justification recorded here
(Ring0_Build_Plan.md §0). The bar is high on purpose: the toolchain is the
trusted core, and every dependency is attack surface and a determinism risk.

| Crate | Why it is allowed |
|---|---|
| `blake3` | The canonical hash. Fast, keyed/domain-separable, stable output — `Digest` in brix-canon. |
| `logos` | Lexer generator for brix-ast. The *parser* is hand-written; only tokenization is generated. |
| `petgraph` | Dependency-graph + SCC condensation for brix-phase (App. F). |
| `pubgrub` | Version resolution for brixpkg. |
| `proptest` | Property tests (canon roundtrip/ordering laws, phase-order invariance, fuzzing). |
| `insta` | Golden snapshots for canon vectors, IR `Display`, generated code. |
| `miette` | Human diagnostic rendering in brix-diag (JSON/SARIF are hand-emitted). |
| `quote` + `proc-macro2` + `syn` + `prettyplease` | Rust codegen + formatting in brixc. `proc-macro2` is the token-stream substrate of this trio (`quote!` produces a `proc_macro2::TokenStream`; `syn`/`prettyplease` consume one) — named explicitly because brixc's emit stage passes token streams across function boundaries, not new surface. |
| `indexmap` | Insertion-/sort-ordered maps where a deterministic non-BTree map is wanted. |
| `camino` | UTF-8 paths (`Utf8Path`) across the CLI and package manager. |
| `serde` (+ `serde_json`) | **Diagnostics/manifests only.** Never a semantic serializer — canon is the only serializer for semantic data. |
| `wasmtime` | The WASM Driver host in brix-rt. Pulled in only by that lane. |
| `toml` | Parses/serializes `brixpkg` package manifests (`brix.toml`) and the lockfile's on-disk TOML shape. **Manifests and lockfiles only, never semantic data** — same rule as `serde`: package metadata (name, version, dependency table) is not a BrixMS relation/graph value, so this is not a second semantic encoder. Every digest that must be stable (lockfile entry digests, content-addressed registry keys) is computed by hashing canonical bytes through `brix-canon`'s `Digest`/`Canonical`, never by hashing the TOML text or relying on `toml`/`serde` for byte-stability. |

## Determinism rules that override convenience

- `std::collections::HashMap` / `HashSet` are clippy-denied (`clippy.toml`).
  Semantic paths use `BTreeMap`/`BTreeSet` or a sorted `IndexMap`/`IndexSet`;
  observable iteration order is always canon byte order. A non-semantic use
  whose order is never observed may `#[allow(clippy::disallowed_types)]` with a
  one-line justification.
- No floats in any Ring 0 semantic path except behind the strict-IEEE ops module
  (spec Part V §8).
- `unsafe` is denied workspace-wide except an allowlisted arena module.

## Pending justifications (freeze blockers)

- **`unicode-normalization` (or equivalent)** — Appendix G requires identifiers
  to be NFC-normalized before canonical encoding. `brix-canon`'s `write_ident`
  currently encodes raw UTF-8 with an `APP-G:` TODO. Adding NFC needs an entry
  here and must land (or be consciously deferred via erratum) **before the canon
  vectors freeze at G0**, because it changes identifier bytes.
