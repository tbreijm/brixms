# BrixMS

[![CI](https://github.com/tbreijm/brixms/actions/workflows/ci.yml/badge.svg)](https://github.com/tbreijm/brixms/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/tbreijm/brixms?include_prereleases&sort=semver&label=release)](https://github.com/tbreijm/brixms/releases)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](./LICENSE)

**A typed language in which a program is an executable model of a world.**

Durable state is a versioned, typed hypergraph; dynamics are pure derivation
rules settled deterministically to a fixpoint at each revision; the model couples
to reality only through explicit boundaries — with identical settlement semantics
under scenario-bound and production-bound boundaries.

> Everything durable, observable, explainable, and interactively meaningful is a
> relation. Computation is settlement. Time is versioning. Intelligence is typed
> by its epistemic status. Components are context-independent semantic bricks.
> The system always maintains a model of itself.

This repository is the **Ring 0 toolchain** — the small, trusted core (canonical
encoding, parser, Core IR + typechecker, reference oracle, runtime, `brixc`
codegen, the `brix` CLI, the package manager, and the Driver SDK). Everything
else in the language is, by the spec's own layer rules, ordinary BrixMS packages
built on these tools (Ring 1).

## Status

**Pre-G0 alpha.** The current pre-release is
**[v0.1.0-alpha.1](https://github.com/tbreijm/brixms/releases/tag/v0.1.0-alpha.1)**
— the Ring 0 foundation. The canonical encoding is frozen and the core kernels
(parser + idempotent `brix fmt`, Core IR + checks, the reference oracle, the
runtime substrate, and the package manager) are in place and tested. **APIs are
unstable and the `brix` CLI is still a scaffold** — this is not yet a usable
toolchain. Progress is measured by gate, not date (see
[Gates, not dates](#gates-not-dates)); open spec questions awaiting a ruling
live in [`spec/errata/`](./spec/errata).

## Origin & license

BrixMS grows out of the original open-source Brix work published at
**[tbreijm/tbreijm.github.io](https://github.com/tbreijm/tbreijm.github.io)** —
the "detect–execute" collision-loop formalism whose production form is the
incidence index at the heart of this runtime. This project continues that line
in the open: it is licensed **[Apache-2.0](./LICENSE)** and developed entirely
through the public toolchain and published formats, so the containment the spec
promises production is the containment the build itself runs under.

## Layout

```
crates/
  brix-canon        canonical encoding + identity (App. G) — frozen first (G0)
  brix-diag         diagnostic types, BRX codes, JSON/SARIF
  brix-ast          lexer, hand-written parser, CST/AST, spans, fmt
  brix-ir           types, effects, traits, Core IR, checking
  brix-phase        dependency graph, SCC, phase assignment (App. F)
  brix-oracle       naive reference evaluator — the semantic authority (G1)
  brix-rt           runtime: revisions, deltas, provenance, sim clock, WASM host
  brixc             pipeline + Rust codegen
  brixpkg           manifest, lockfile, resolve, local registry
  brix-cli          new/build/run/repl/test/sim/fmt/why/whynot/explain
  brix-conformance  fixture format, CONF runner, differential harness, fuzzer
sdk/
  driver-wit        WIT worlds: delta ABI + host capabilities
  brix-driver-rs    Rust guest SDK for WASM Drivers
vectors/            frozen canon golden vectors (Day-1 artifact, G0)
spec/               the v9.0 normative specification + build plans + errata/
```

## Building

```
cargo build --workspace      # green from commit 1
cargo test  --workspace
cargo clippy --workspace -- -D warnings
```

Determinism is enforced mechanically: `HashMap`/`HashSet` are clippy-denied in
semantic paths (see `clippy.toml`), `unsafe` is denied workspace-wide, and the
toolchain is pinned in `rust-toolchain.toml` — two-machine reproducibility is a
release gate (G3).

## The plan

- `spec/BrixMS_v9_0.md` — the normative language specification (v9.0).
- `spec/Ring0_Build_Plan.md` — crates, decisions, order, gates for this repo.
- `spec/Build_Plan_v2.md` — the two-ring "toolchain first" strategy.
- `CONTRIBUTING.md` — the feedback protocol (package bug / toolchain bug / spec
  erratum) and the determinism discipline every change is held to.

## Gates, not dates

```
G0  canon golden vectors frozen, independently cross-checked
G1  flagship runs end-to-end on the oracle; oracle freezes
G2  engine = oracle, conformance-green under sustained fuzz
G3  flagship deterministic on two machines; backend parity
G4  Developer Day: a fresh agent ships a package with public tools only
```
