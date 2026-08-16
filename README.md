# BrixMS

[![CI](https://github.com/tbreijm/brixms/actions/workflows/ci.yml/badge.svg)](https://github.com/tbreijm/brixms/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/tbreijm/brixms?include_prereleases&sort=semver&label=release)](https://github.com/tbreijm/brixms/releases)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](./LICENSE)

**A language and runtime for deterministic, inspectable settlement.** BrixMS
models configurations, the witnesses relating them, and the evidence behind
every result. Its execution cost is designed around the change being processed,
not the size of the surrounding world.

Brix is the language. **SOC (Settlement-Oriented Computing)** is the paradigm it
implements: settlement is the organizing primitive in the same sense that
objects organize object-oriented systems.

> BrixMS is an experimental, pre-release system. The core semantic machinery,
> proof kernel, incremental settlement engine, and first command-line workflows
> are implemented; the language and artifact transport formats are still being
> completed.

## What exists today

| Area | Implemented |
| --- | --- |
| Canonical identity | Versioned canonical encoding, content-addressed identities, frozen vectors, and an independent Python cross-check |
| Semantic substrate | Configurations, witnesses, regimes, generator registries, decompositions, derivation trees, judgements, dependencies, and the epistemic outcome lattice |
| Settlement | Deterministic keyed selection, persistent state, append-only journals, replay, admission policies, and both naïve and incremental candidate engines |
| Performance gate | The O(Δ) invariant is an active test: doubling inert configurations leaves incremental per-step cost flat; the naïve oracle demonstrates the expected world-size growth |
| Audit | Replay-verified decompositions, authority-checked publication of `Audited`, oracle-bound settlement audit receipts, and source-re-derived L3 manifests |
| Saturation | Administrative/realizing step labels, certified quiescence, bounded divergence certificates, saturated execution, weak bisimulation/refinement, and closure checks |
| Proof | A small dependent proof kernel with explicit terms, composition/tensor rules, canonical certificate envelopes, adversarial vectors, and kernel-owned primitive relations |
| Brix frontend | A hand-written `.brix` lexer/parser, type-realization lowering, algebraic and parameterized configurations, records, sums, matching, functions, arithmetic, comparison, grades, and witness composition |
| CLI | `brix check`, `run`, `audit`, `prove`, `why`, and `whynot` |

The parser intentionally recognizes more of the designed language than every
downstream profile can execute. In particular, `run` and `audit` use the narrow
`brix.l3.rule-agenda-saturated@1` profile and reject unsupported constructs
before creating engine state. Unsupported input is reported as unsupported or
`Unknown`; it is never promoted into a stronger claim.

## Try it

Prerequisite: Rust **1.96.1**, pinned by
[`rust-toolchain.toml`](./rust-toolchain.toml). `rustup` installs it on the first
build.

```bash
git clone https://github.com/tbreijm/brixms.git
cd brixms
cargo test --workspace

# Type-check the small checked-in example.
cargo run -p brix-cli -- check crates/brix-lower/tests/fixtures/id.brix
```

The example prints:

```text
r : Int @Proven
```

The current CLI surface is:

```text
brix check   <file.brix>  # infer/check bindings and report epistemic grades
brix run     <file.brix>  # run L3 and report quiescence, divergence, or Unknown
brix audit   <file.brix>  # run, then independently audit the committed journal
brix prove   <file.brix>  # show kernel certificate details for each binding
brix why     <file.brix>  # show the realization derivation and grade caps
brix whynot  <file.brix>  # explain conflicts, unsupported fragments, or proof gaps
```

Run a command from the workspace with `cargo run -p brix-cli -- …`, or build
the executable once with `cargo build -p brix-cli`.

## A small Brix example

```brix
config List<T> = Nil | Cons(T, List<T>)

fn head_or(xs: List<Int>, fallback: Int): Int = match xs {
  Nil => fallback
  Cons(head, _) => head
}

let answer: Int @Proven = head_or(Cons(42, Nil), 0)
```

The surface language also reserves SOC concepts directly: `regime` and `gen`
declare realization regimes and logged generators; `rule` declares settlement
work; `witness` binds evidence; `then` and `and` compose witnesses sequentially
and in parallel. `prove`, `why`, `audit`, and grade annotations are optional
power-user syntax—ordinary programs can rely on inference.

Not every parsed form is lowered by every command yet. The executable L3
profile is deliberately smaller than the type-checking profile, and recursive
functions are currently refused rather than evaluated indefinitely.

## The model

BrixMS has one category:

- objects are **configurations**;
- arrows are **witnesses** between configurations;
- a regime supplies the realization relation for a class of witnesses;
- settlement chooses one admissible candidate from plural deliberation using a
  deterministic key;
- audit replays the recorded generator decomposition independently;
- proof elaboration crosses a checked boundary into the proof kernel.

Realization is globally lax—composition preserves what its parts realize. A
specified tight subcategory of logged generators supports exact replay and
audit. Saturation hides finite administrative prefixes without confusing
quiescence with an infinite administrative search. These commitments are
governed by the accepted
[`ADR-0002 SOC Constitution`](./spec/adr/ADR-0002_SOC_Constitution.md).

### Outcomes are evidence grades, not booleans

```text
       Proven       Refuted      kernel-certified, incomparable poles
           \         /
             Audited             replay verified
                |
             Derived             committed within a revision
                |
             Measured            certified external result
                |
             Unknown             bottom; no truth commitment
```

Each publication route has one authority. The proof kernel alone may publish
`Proven` or `Refuted`; the settlement kernel publishes `Derived`; the audit
checker publishes `Audited`; a named external driver publishes `Measured`.
Resource exhaustion, incomplete search, failed replay, and unsupported input
remain `Unknown`. Strengthening a result creates a new evidence-bearing
judgement; it never mutates or launders the old one.

The legal `(authority, outcome, evidence)` combinations are enforced by the
publication fence in
[`brix-semantic`](./crates/brix-semantic/src/publication.rs), not left as a
calling convention.

### O(Δ), not O(world)

For each committed step, semantic work must scale with the changed
configuration set and index fanout:

```text
cost(step) ∝ |Δ| × fanout
doubling inert |world| ⇒ no per-step cost increase
```

`soc-core` maintains a materialized candidate view behind a footprint index and
applies candidate deltas transactionally. The deliberately simple recompute-
the-world engine remains in the repository as a differential oracle. The active
[`o_delta_gate`](./crates/soc-core/tests/o_delta_gate.rs) measures both and makes
world-size-sensitive incremental behavior a red build.

## Architecture

```text
                         brix-canon
                             |
                       brix-semantic
                      /      |       \
              soc-core   brix-kernel  soc-regimes
                  |           ^           ^
                  |      brix-elaborate    |
                  |           ^           |
.brix -> brix-syntax -> brix-lower --------+
                           |
                        brix-cli
```

| Crate | Responsibility |
| --- | --- |
| [`brix-canon`](./crates/brix-canon) | Canonical bytes, ordering, and digest identity |
| [`brix-semantic`](./crates/brix-semantic) | Shared canonical artifacts and publication rules; depends only on `brix-canon` |
| [`soc-core`](./crates/soc-core) | Settlement, incremental execution, audit, journals, and saturation |
| [`soc-regimes`](./crates/soc-regimes) | Literal and native Brix type-realization regimes |
| [`brix-kernel`](./crates/brix-kernel) | Independent proof-term acceptance and canonical certificates |
| [`brix-elaborate`](./crates/brix-elaborate) | Audited-source-to-kernel elaboration boundary |
| [`brix-syntax`](./crates/brix-syntax) | Surface AST, lexer, parser, and bounded hostile-input entry point |
| [`brix-lower`](./crates/brix-lower) | Type-realization lowering and the executable L3 settlement adapter |
| [`brix-cli`](./crates/brix-cli) | User-facing `brix` commands |

The former `brix-ast`/`brix-ir`/`brixc`/`brix-rt` engine was used as a
differential oracle during the transition and has now been deleted. The current
workspace is the SOC-native implementation; it does not carry the old engine as
a second semantic center.

## Honest project status

The current implementation is substantial, but it is not feature-complete:

- the lambda-calculus core, literals, structural products/coproducts, and the
  zero-arity introductions can reach genuine `Proven`;
- arithmetic currently tops out at `Audited` because several primitive typing
  leaves remain undischarged; comparisons and catch-all matching have similar
  explicit caps;
- the type-realization dependency invalidation engine does not exist yet;
- no kernel-certified refutation path exists yet—negative results are conflicts
  or `Unknown`, never `Refuted`;
- context confinement and some durable artifact encodings remain open or
  partial in the semantic-law registry;
- parallel deliberation, universal-world/faithfulness work, and broader
  executable language coverage remain future work;
- the offline audit transport bundle and `brix verify` command are designed in
  proposed [ADR-0026](./spec/adr/ADR-0026_Audit_Input_Transport_Bundle.md), but
  are not implemented.

The precise status of every shared obligation is tracked in
[`SOC_Semantic_Laws.md`](./spec/SOC_Semantic_Laws.md). The native typing regime's
implemented, pinned, and specified-only clauses are separated in
[`Type_Realization_Contract.md`](./spec/Type_Realization_Contract.md).

## Repository map

```text
crates/     the nine Rust crates above
spec/       constitution, ADRs, law registry, type contract, and plans
docs/       SOC foundation, language overview, and article material
vectors/    frozen canonical and certificate artifacts
packages/   experimental Brix package sources
scripts/    independent canon and policy/traceability checks
```

## Verification

The local merge bar is:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
python3 scripts/canon_crosscheck.py
python3 scripts/check_tcb_dependencies.py --check
python3 scripts/check_soc_law_map.py
```

CI additionally runs the suite through `cargo-nextest`, repeats it to detect
tracked artifact drift, exposes dedicated conformance/acceptance/
reproducibility jobs, checks dependency policy with `cargo-deny`, and reports
coverage. `unsafe` is denied workspace-wide, and unordered standard hash maps
are denied in semantic paths.

## Reading order

1. [`spec/README.md`](./spec/README.md) — specification index and document
   authority.
2. [`ADR-0002`](./spec/adr/ADR-0002_SOC_Constitution.md) — the accepted SOC
   constitution.
3. [`SOC_Semantic_Laws.md`](./spec/SOC_Semantic_Laws.md) — normative laws,
   executable anchors, and open obligations.
4. [`Type_Realization_Contract.md`](./spec/Type_Realization_Contract.md) — the
   exact native typing contract and its honest current limits.
5. [`docs/brix-language.md`](./docs/brix-language.md) — approachable language
   overview.
6. [`Build_Plan_v3_SOC.md`](./spec/Build_Plan_v3_SOC.md) — dependency-ordered
   design plan; consult the law registry and current code for landed status.

The mathematical foundation is
[`SOC_core_foundations_revised.tex`](./docs/SOC_core_foundations_revised.tex).
It labels conjectures and targets explicitly; the engineering constitution is
the governing source where the two differ.

## Contributing and license

See [`CONTRIBUTING.md`](./CONTRIBUTING.md) for the determinism discipline,
dependency policy, and spec-erratum workflow. BrixMS is licensed under
[Apache-2.0](./LICENSE).
