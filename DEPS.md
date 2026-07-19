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
| `wasmtime` | The WASM Driver host in brix-rt (issue #27). Component-model instantiation of `sdk/driver-wit/delta-abi.wit`'s `driver` world. Pinned at ≥46.0.1 so `cargo-deny` clears the RUSTSEC advisories that still apply to the originally whitelisted 27.x line. |
| `wasmtime-wasi` | Companion to `wasmtime` for the same lane (same version pin). `rustc --target wasm32-wasip2` links every guest against the standard WASI 0.2 worlds regardless of what the delta-ABI's own `capabilities` interface declares; the host must provide `wasmtime_wasi::p2::add_to_linker_sync` or components fail to instantiate. Not a capability surface the delta ABI defines — purely what makes wasip2 binaries loadable at all. |
| `wit-bindgen` | Guest-side (`sdk/brix-driver-rs`, wasm32 target only): generates the `driver` world's low-level export/import ABI glue from `delta-abi.wit`, mirroring `wasmtime::component::bindgen!` on the host side ("one WIT world" — Ring0_Build_Plan.md §1.7). No `unsafe` beyond what the macro itself emits for the component ABI trampolines (outside this workspace's denied surface, generated code). |
| `unicode-normalization` | Appendix G requires identifiers to be NFC-normalized before canonical encoding. Applied in `brix-canon`'s `write_ident`. Pure, `no_std`-capable, table-driven UAX #15 implementation with a stable API; the de-facto standard crate (a Rust project dependency). ASCII identifiers take an allocation-free fast path, so the cost is paid only on non-ASCII idents. See "Pending justifications" resolution below. |
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
