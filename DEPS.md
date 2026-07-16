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
| `quote` + `syn` + `prettyplease` | Rust codegen + formatting in brixc. |
| `indexmap` | Insertion-/sort-ordered maps where a deterministic non-BTree map is wanted. |
| `camino` | UTF-8 paths (`Utf8Path`) across the CLI and package manager. |
| `serde` (+ `serde_json`) | **Diagnostics/manifests only.** Never a semantic serializer — canon is the only serializer for semantic data. |
| `wasmtime` | The WASM Driver host in brix-rt. Pulled in only by that lane. |
| `unicode-normalization` | Appendix G requires identifiers to be NFC-normalized before canonical encoding. Applied in `brix-canon`'s `write_ident`. Pure, `no_std`-capable, table-driven UAX #15 implementation with a stable API; the de-facto standard crate (a Rust project dependency). ASCII identifiers take an allocation-free fast path, so the cost is paid only on non-ASCII idents. See "Pending justifications" resolution below. |

## Determinism rules that override convenience

- `std::collections::HashMap` / `HashSet` are clippy-denied (`clippy.toml`).
  Semantic paths use `BTreeMap`/`BTreeSet` or a sorted `IndexMap`/`IndexSet`;
  observable iteration order is always canon byte order. A non-semantic use
  whose order is never observed may `#[allow(clippy::disallowed_types)]` with a
  one-line justification.
- No floats in any Ring 0 semantic path except behind the strict-IEEE ops module
  (spec Part V §8).
- `unsafe` is denied workspace-wide except an allowlisted arena module.

## Resolved freeze blockers

- **`unicode-normalization` — RESOLVED (added), 2026-07-16.** Appendix G requires
  identifiers to be NFC-normalized before canonical encoding. `brix-canon`'s
  `write_ident` previously encoded raw UTF-8 with an `APP-G:` TODO. Decision:
  **add the dependency and apply NFC**, rather than defer via erratum. Rationale:
  (1) NFC folding is *normative* App. G text ("strings: NFC for identifiers"), so
  deferring would freeze knowingly-wrong identifier bytes at G0 and force a
  `CANON_VERSION` bump later; (2) the crate is the de-facto standard, pure, and
  table-driven (no ambient authority, no nondeterminism); (3) identifiers are a
  narrow surface and ASCII takes an allocation-free fast path. `write_ident` and
  record field-name encoding now fold to NFC; string *values* (`write_str`)
  deliberately do **not** fold, per App. G ("values as raw Unicode scalar
  sequences"). The golden vectors include a decomposed-vs-precomposed identifier
  case so the cross-check pins this behavior. Whitelist entry above.
