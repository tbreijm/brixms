# BrixMS

[![CI](https://github.com/tbreijm/brixms/actions/workflows/ci.yml/badge.svg)](https://github.com/tbreijm/brixms/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/tbreijm/brixms?include_prereleases&sort=semver&label=release)](https://github.com/tbreijm/brixms/releases)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](./LICENSE)

**BrixMS is a language and runtime for systems that need to decide what happens
next—and explain why.**

Most execution engines blur three different questions:

1. What *could* happen from the current state?
2. Which possibility did the system actually choose?
3. How strong is the evidence behind the result?

BrixMS keeps those questions separate. Regimes propose possibilities, policy
filters them, a deterministic calendar commits one next step, and independent
audit or proof can strengthen the evidence afterward. The result is an
execution model designed to be reproducible, inspectable, and honest about what
it does not know.

**Brix** is the language. **SOC (Settlement-Oriented Computing)** is the
paradigm: settlement is its organizing idea in the same way that objects
organize object-oriented systems.

> BrixMS is experimental and pre-release. The core runtime, proof kernel,
> incremental engine, and first end-to-end language workflows exist today. The
> language, verification transport, and broader execution profiles are still
> being completed.

## What is BrixMS for?

BrixMS is aimed at executable worlds where choosing an answer is not enough;
the system must also preserve the context, policy, history, and evidence that
made the answer legitimate.

That makes the architecture relevant to rule engines, simulations, planning
systems, policy-driven automation, and other stateful systems where:

- several next actions may be valid, but commitment must be deterministic;
- results must be replayable or independently checked;
- “not established” must remain different from “false”;
- changing a small part of a large world should trigger local work, not a full
  recomputation;
- users need to inspect not only a result, but its derivation and proof status.

It is not presented as a production-ready solution for those domains yet. They
describe the class of problems the design is being built to handle.

## The four ideas underneath it

### 1. Configurations and witnesses

A configuration is a state, value, model, program fragment, or world fragment.
A witness records a meaningful relationship or transition between two
configurations. Typing is therefore not a privileged built-in relation; it is
one realization regime among others.

Mathematically, configurations are the objects of one category and witnesses
are its arrows. In everyday use, that means every important transition has a
nameable, content-addressed explanation.

### 2. Deliberation is plural; commitment is singular

More than one regime may propose a candidate next step. An admissibility policy
filters those candidates, and a keyed calendar selects exactly one in a stable
order. The committed journal therefore does not depend on thread timing or map
iteration order.

```text
world + policy + history
          |
          v
  regimes propose candidates
          |
          v
   admissibility filter
          |
          v
 deterministic calendar -----> committed step (@Derived)
                                      |
                                      +---- replay audit (@Audited)
                                      |
                                      +---- proof kernel (@Proven)
```

### 3. Evidence has grades

BrixMS does not collapse every outcome into `true` or `false`:

```text
       Proven       Refuted      kernel-certified, incomparable poles
           \         /
             Audited             replay verified
                |
             Derived             committed within a revision
                |
             Measured            certified external result
                |
             Unknown             no truth commitment
```

Each grade has one authority. The settlement kernel may publish `Derived`; the
audit checker may publish `Audited`; only a proof kernel may publish `Proven` or
`Refuted`. Resource exhaustion, unsupported input, incomplete search, and
failed replay remain `Unknown`.

Strengthening a result creates a new judgement linked to the earlier evidence.
It never edits the old claim or silently rounds it upward.

### 4. Cost follows change

The central performance rule is O(Δ): work per committed step must scale with
the changed configurations and their index fanout, not with all inert state in
the world.

```text
cost(step) ∝ |Δ| × fanout
doubling inert |world| ⇒ no per-step cost increase
```

The repository keeps both implementations needed to enforce this honestly: a
simple recompute-the-world oracle and the real incremental engine. The active
[`o_delta_gate`](./crates/soc-core/tests/o_delta_gate.rs) proves that the naïve
path grows with world size while the incremental path stays flat.

## What works now

The project has moved beyond a paper design. These paths are implemented and
covered by executable gates:

| Layer | Current implementation |
| --- | --- |
| Brix language | Hand-written lexer/parser; functions and bindings; records and algebraic sums; directly recursive and parameterized configurations; matching; arithmetic and comparison; grade annotations; sequential and parallel witness composition |
| Type realization | Tree-shaped derivations, conflict reporting, declared function contracts, certified match coverage, and honest per-result grade caps |
| Command line | `check`, `run`, `audit`, `prove`, `why`, and `whynot` |
| Settlement runtime | Admission policies, deterministic keyed selection, transactional candidate deltas, persistent state, append-only journals, and deterministic replay |
| Incremental engine | Materialized candidate views, footprint indexing, differential agreement with the naïve oracle, and the green O(Δ) gate |
| Audit | Replay-verified decompositions, authority-checked `Audited` publication, oracle-bound receipts, and source-re-derived L3 manifests |
| Saturation | Administrative versus realizing steps, certified quiescence, bounded divergence evidence, weak bisimulation/refinement, and closure checking |
| Proof | A small dependent kernel with explicit proof terms, composition and tensor rules, primitive relations, canonical certificate envelopes, and adversarial vectors |
| Reproducibility | Pinned Rust toolchain, canonical encodings, frozen vectors, independent cross-checks, deterministic-order lints, and artifact-drift CI |

There are two language profiles today:

- `brix check`, `prove`, `why`, and `whynot` exercise the growing type-
  realization frontend;
- `brix run` and `audit` use the deliberately smaller
  `brix.l3.rule-agenda-saturated@1` execution profile.

The parser recognizes some designed syntax that a downstream profile does not
yet implement. Those constructs are refused before execution; they do not
become guessed results.

## Try it

Install Rust through [rustup](https://rustup.rs/). The repository pins Rust
**1.96.1** in [`rust-toolchain.toml`](./rust-toolchain.toml), so the matching
toolchain is selected automatically.

```bash
git clone https://github.com/tbreijm/brixms.git
cd brixms
cargo test --workspace

# Type-check the checked-in identity example.
cargo run -p brix-cli -- check crates/brix-lower/tests/fixtures/id.brix
```

The final command prints:

```text
r : Int @Proven
```

### A small Brix program

```brix
config List<T> = Nil | Cons(T, List<T>)

fn head_or(xs: List<Int>, fallback: Int): Int = match xs {
  Nil => fallback
  Cons(head, _) => head
}

let answer: Int @Proven = head_or(Cons(42, Nil), 0)
```

The annotation is a contract, not documentation: the checker must establish
both the declared type and the requested evidence grade.

### CLI guide

```text
brix check   <file.brix>  infer types and report their evidence grades
brix run     <file.brix>  run L3 and report quiescence, divergence, or Unknown
brix audit   <file.brix>  run L3, then independently replay the journal
brix prove   <file.brix>  show kernel proposition and certificate details
brix why     <file.brix>  show the derivation and any grade-limiting leaves
brix whynot  <file.brix>  explain conflicts, unsupported syntax, or proof gaps
```

From the workspace, prefix a command with `cargo run -p brix-cli --`, or build
the executable once with `cargo build -p brix-cli`.

## What is coming

The next work is about completing the trust story and widening the useful
language surface, not replacing the architecture above.

### Near-term engineering

- discharge the remaining primitive typing relations so arithmetic,
  comparisons, and more matches can move from `Audited` to genuine `Proven`;
- implement the proposed offline audit bundle and
  [`brix verify`](./spec/adr/ADR-0026_Audit_Input_Transport_Bundle.md), so a
  separate process can reconstruct and verify every audit input;
- add dependency tracking and incremental invalidation for type-realization
  results;
- extend the executable L3 subset beyond its current static rule-agenda
  profile;
- finish versioned context transport and confinement checks.

### Longer-term design and research

- parallelize deliberation while preserving the exact serial commit sequence;
- build broader native Brix packages and more realization regimes;
- complete the universal-world/faithfulness obligations without overstating
  the still-open mathematical claims;
- grow the language toward self-hosting while keeping the Rust kernels small
  and independently checkable.

The precise status is intentionally explicit:

- arithmetic and comparison currently top out at `Audited` where primitive
  leaves remain undischarged;
- catch-all matching is also deliberately capped;
- recursive functions are refused because functions are currently inlined;
- certified refutation does not exist yet, so negative results are conflicts or
  `Unknown`, never `Refuted`;
- context confinement and several durable artifact obligations remain partial;
- `brix verify` is designed but not implemented.

The authoritative status ledger is
[`SOC_Semantic_Laws.md`](./spec/SOC_Semantic_Laws.md). The exact distinction
between implemented, test-pinned, and specified-only typing clauses lives in
[`Type_Realization_Contract.md`](./spec/Type_Realization_Contract.md).

## How the implementation is organized

BrixMS is a Rust workspace with nine focused crates:

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

| Crate | Role |
| --- | --- |
| [`brix-canon`](./crates/brix-canon) | Canonical bytes, ordering, and digest identity |
| [`brix-semantic`](./crates/brix-semantic) | Shared artifacts, evidence grades, and legal publication routes |
| [`soc-core`](./crates/soc-core) | Settlement, incremental execution, journals, audit, and saturation |
| [`soc-regimes`](./crates/soc-regimes) | Literal and native Brix type-realization regimes |
| [`brix-kernel`](./crates/brix-kernel) | Independent proof-term acceptance and certificates |
| [`brix-elaborate`](./crates/brix-elaborate) | Checked bridge from audited evidence into the proof kernel |
| [`brix-syntax`](./crates/brix-syntax) | Surface AST, lexer, parser, and hostile-input bounds |
| [`brix-lower`](./crates/brix-lower) | Type-realization lowering and the executable L3 adapter |
| [`brix-cli`](./crates/brix-cli) | User-facing commands |

`brix-semantic` depends only on `brix-canon`, and `brix-kernel` depends only on
those two crates. This keeps the trusted proof boundary independent of the
parser, runtime, and regimes that construct proof candidates.

The former `brix-ast`/`brix-ir`/`brixc`/`brix-rt` engine served as a
differential oracle during the SOC transition and has been deleted. The current
workspace is the SOC-native implementation, not two competing engines.

## Repository guide

```text
crates/     the Rust implementation
spec/       constitution, decisions, semantic laws, contracts, and plans
docs/       the SOC foundation, language overview, and article material
vectors/    frozen canonical and certificate artifacts
packages/   experimental Brix package sources
scripts/    independent canonical, dependency, and traceability checks
```

For a conceptual introduction, read
[`docs/brix-language.md`](./docs/brix-language.md). For the governing design and
the exact boundary between claims and conjectures, continue with:

1. [`spec/README.md`](./spec/README.md) — document map and authority.
2. [`ADR-0002`](./spec/adr/ADR-0002_SOC_Constitution.md) — the accepted SOC
   constitution.
3. [`SOC_Semantic_Laws.md`](./spec/SOC_Semantic_Laws.md) — laws, executable
   anchors, and open obligations.
4. [`Type_Realization_Contract.md`](./spec/Type_Realization_Contract.md) — the
   native typing contract and its current limits.
5. [`Build_Plan_v3_SOC.md`](./spec/Build_Plan_v3_SOC.md) — dependency-ordered
   design plan; use the law registry and code for current landed status.

The mathematical source is
[`SOC_core_foundations_revised.tex`](./docs/SOC_core_foundations_revised.tex).
It labels established results, conditional claims, targets, and open questions
separately. The accepted engineering constitution governs where the documents
differ.

## Development and verification

The local merge bar is:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
python3 scripts/canon_crosscheck.py
python3 scripts/check_tcb_dependencies.py --check
python3 scripts/check_soc_law_map.py
```

CI also repeats the suite to detect artifact drift, exposes dedicated
conformance, acceptance, and reproducibility jobs, checks dependency policy
with `cargo-deny`, and reports coverage. `unsafe` is denied workspace-wide, and
unordered standard hash maps are denied in semantic paths.

See [`CONTRIBUTING.md`](./CONTRIBUTING.md) for the determinism discipline,
dependency policy, and specification-erratum workflow.

## License

BrixMS is licensed under [Apache-2.0](./LICENSE).
