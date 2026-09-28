# Performance: wall-clock benchmarks and honest interpretation

This document reports **measured wall-clock numbers**, not aspirational
targets. It exists alongside the deterministic, self-reported-cost gates that
already live in the test suite (notably `crates/soc-core/tests/o_delta_gate.rs`,
which counts work units, never wall-clock, deliberately — see that file's
module docs for why). The benchmarks here ask a narrower, complementary
question: **does wall-clock time agree with the deterministic cost model**,
and what does either tell us about the actual limits BrixMS declares today.

No marketing language below is intended; where a number is small, that is
stated as "small on this machine, for this workload", not "fast".

## Methodology

- All benchmarks are `[[bench]]` targets with `harness = false`, using only
  `std::time::Instant` — no `criterion` (not on the Ring-0 dependency
  whitelist; see `DEPS.md`). Each prints a small table directly to stdout.
- Every benchmark warms up (a handful of untimed iterations, so the first
  timed sample is not paying one-time allocator/cache warmup cost that no
  later call would) and then times a fixed number of iterations, reporting
  the **median** and the **90th percentile (p90)** of those durations.
  Reporting both, rather than a mean, keeps one slow outlier (a GC-less
  runtime still has allocator/scheduler jitter) from being read as the
  typical cost.
- `cargo bench` (not `cargo test`) is what builds and runs these targets:
  `[[bench]]` sections were added to `crates/soc-core/Cargo.toml` and
  `crates/brix-lower/Cargo.toml` and confirmed **not** to appear when running
  `cargo test --workspace` (checked directly against this change's own test
  run — none of the four bench binaries below are listed in that output).
- `cargo bench` builds in Cargo's `bench` profile, which is optimized
  (`opt-level` matching `release`); running the same binaries under `cargo
  test`'s debug profile would show substantially larger absolute numbers for
  the same relative shape.

## How to reproduce

```sh
cargo bench -p soc-core   --bench o_delta_wall_clock
cargo bench -p soc-core   --bench arc_map_insert
cargo bench -p brix-lower --bench finite_decision_end_to_end
cargo bench -p brix-lower --bench audit_verify
```

Each is self-contained (no external fixtures beyond `examples/*.brix` /
`examples/*.json`, read via `include_str!`) and prints its own table.

## The machine

All numbers below were captured on the container this work was done in:

- 4 vCPUs, Intel(R) Xeon(R) (cloud VM; `model name` reports a 2.80GHz part —
  virtualized, so absolute clock behavior is not guaranteed constant)
- 16 GiB RAM
- Linux 6.18 (x86_64), rustc/cargo 1.96.1, `rust-toolchain.toml`-pinned
- A shared cloud container, not an isolated benchmarking rig: absolute
  microsecond figures carry real noise (a repeat run of the same binary
  moved individual numbers by up to ~15% between runs in preparing this
  document). The **growth ratios across scale** (the "ratio" columns/rows
  below) are the load-bearing signal; the absolute numbers are one honest
  sample, not a guarantee reproducible to the microsecond on other hardware.

---

## 1. `IncrementalEngine` vs the naive oracle — does wall-clock agree with O(Δ)?

`crates/soc-core/tests/o_delta_gate.rs` measures the same fixture shape via
self-reported `CostRecord::Steps` work units and is the actual CI gate (ADR-0002
§9.1: "cost per committed step MUST be ∝ |Δ| × fanout, and MUST NOT be ∝
|world|"). `o_delta_wall_clock` mirrors that fixture (one active regime, N
*inert* configurations registered to no regime and never reachable) and times
it with `Instant` instead.

**`IncrementalEngine::step` — one committed step (adding the active config),
after N inert configs are already present:**

| N_inert | iters | median | p90 | bulk ingest of N (unmeasured setup) |
|---:|---:|---:|---:|---:|
| 1,000 | 500 | 145 ns | 163 ns | 454 µs |
| 10,000 | 500 | 145 ns | 162 ns | 5.47 ms |
| 100,000 | 500 | 145 ns | 160 ns | 76.5 ms |

**`naive_view_over_instrumented` — full recompute over N+1 present configs:**

| N_inert | iters | median | p90 |
|---:|---:|---:|---:|
| 1,000 | 200 | 4.40 µs | 8.16 µs |
| 10,000 | 200 | 42.96 µs | 47.30 µs |
| 100,000 | 30 | 458.67 µs | 527.83 µs |

**Growth ratio vs the 1k baseline (median):**

| N_inert | engine ratio | naive ratio |
|---:|---:|---:|
| 1,000 | 1.00x | 1.00x |
| 10,000 | 1.00x | 9.77x |
| 100,000 | 1.00x | 104.27x |

**Interpretation.** Wall-clock agrees with the deterministic gate, exactly:
the incremental engine's per-step median is *byte-for-byte flat* (145 ns at
every scale, in this run) across a 100x change in the inert population, while
the naive oracle's grows essentially linearly with it (9.77x at 10x inert,
104x at 100x inert). This is not a coincidence of the cost model — the
engine's footprint index never contains an inert handle at all (it is built
once from each provider's *declared* footprint, which in this fixture is
exactly the one active config), so an inert config's arrival is one `BTreeMap`
miss and nothing else; there is no growing structure for wall-clock to
disagree with the work-unit count about. The "bulk ingest" column is the one
place N shows up in wall-clock terms at all — ingesting N inert configs as
one-time setup costs roughly proportional to N (76.5 ms for 100,000 configs,
~0.76 µs/config), which is the expected cost of *presenting* a world of that
size, not of any one committed step over it.

## 2. `ArcMap::insert` — is the documented O(n) real, and how bad is it?

`crates/soc-core/src/store.rs`'s own docs are explicit that `ArcMap::insert`
clones the whole backing `BTreeMap` on every call ("O(n) pointer-sized clones
... not a HAMT's O(log n) per-node sharing") and that this is deliberate v1
scaffolding behind a `PersistentMap` trait seam for a future HAMT. This
benchmark measures it without changing it, per that module's own caveat.

**One `insert` call's wall time at map size N** (read from the last 50 insert
calls of one ascending build from empty to 100,000 entries, rather than
rebuilding per checkpoint — see the bench file's own docs for why rebuilding
per checkpoint would itself cost O(N²) for no benefit):

| N | median | p90 |
|---:|---:|---:|
| 1,000 | 10.93 µs | 11.29 µs |
| 10,000 | 116.64 µs | 142.44 µs |
| 100,000 | 1.673 ms | 2.797 ms |

**`get` at the same sizes, for contrast** (O(log n), unaffected by `insert`'s
clone-on-write):

| N | median | p90 |
|---:|---:|---:|
| 1,000 | 32 ns | 33 ns |
| 10,000 | 32 ns | 33 ns |
| 100,000 | 38 ns | 38 ns |

**Growth ratio vs the 1,000-entry checkpoint:**

| N | N ratio | insert time ratio |
|---:|---:|---:|
| 1,000 | 1.0x | 1.00x |
| 10,000 | 10.0x | 10.67x |
| 100,000 | 100.0x | 153.10x |

**Interpretation.** The O(n) claim is real and wall-clock confirms it plainly:
`get` stays flat (32–38 ns, i.e. noise, across a 100x size change — O(log n)
looks flat at this scale) while `insert`'s cost tracks N. It tracks slightly
*worse* than linear at the largest size measured here (153x time for 100x N,
not ~100x) — plausibly allocator and cache effects as the cloned `BTreeMap`'s
node count grows (more nodes means more individual allocations during the
clone, and a cold walk over a larger tree), not a different asymptotic class;
one run's numbers are not enough evidence to characterize that superlinearity
further, and this document does not attempt to. As `store.rs` already notes
and this change does not dispute: `ArcMap` is not currently used anywhere
outside its own module (`crates/soc-core/src/store.rs`) and its re-export in
`crates/soc-core/src/lib.rs` — grep confirms no other production path
constructs or calls it — so today this cost is paid by nobody. It becomes
relevant the day something is actually built on `PersistentMap` at a size
where O(n) `insert` matters; at that point this table is the argument for
prioritizing the HAMT swap-in `store.rs` already names as the eventual target,
not before.

## 3. End-to-end finite decision: parse → lower → build → run

Each shipped `examples/*.brix` (with its matching `examples/*.json` input
where one exists) run through the same four phases `brix run`/`brix check`
drive, timed individually plus as a total. Then four synthetic programs
scaled toward the finite-decision profile's own declared `MAX_*` bounds
(`crates/brix-lower/src/finite_decision/plan.rs`,
`crates/brix-lower/src/input.rs`) — chosen to be *toward* those limits at a
size still fast enough to benchmark in a few minutes, not necessarily *at*
every limit simultaneously (the "MAX_*" constants aren't jointly saturable in
one small program without changing which axis is under test).

| fixture | parse | lower | build | run | total (median) | total (p90) |
|---|---:|---:|---:|---:|---:|---:|
| shipping.brix | 7.6 µs | 5.4 µs | 11.1 µs | 10.3 µs | 36.2 µs | 46.2 µs |
| shipping-input.brix | 13.4 µs | 10.2 µs | 15.7 µs | 14.3 µs | 56.9 µs | 89.1 µs |
| shipping-functions.brix | 22.2 µs | 17.7 µs | 21.7 µs | 17.1 µs | 84.9 µs | 123.2 µs |
| allocation.brix | 18.8 µs | 19.0 µs | 21.7 µs | 16.9 µs | 82.4 µs | 117.3 µs |
| order-policy.brix | 9.4 µs | 8.3 µs | 13.7 µs | 12.6 µs | 47.3 µs | 75.8 µs |
| synthetic: 200-rule dependency chain | 174.4 µs | 159.3 µs | 73.2 µs | 74.4 µs | 529.2 µs | 650.2 µs |
| synthetic: 200 candidate proposals | 297.9 µs | 200.5 µs | **683.1 µs** | **656.0 µs** | 2.10 ms | 4.52 ms |
| synthetic: 200 declared helper `fn`s (3 called) | 252.2 µs | 340.8 µs | 146.5 µs | 20.0 µs | 828.6 µs | 1.14 ms |
| synthetic: 256-field record input | 103.6 µs | 67.5 µs | 180.8 µs | 63.4 µs | 459.2 µs | 585.5 µs |

**Interpretation.**

- The five shipped examples — the actual current usage profile, one program
  producing one decision over a handful of inputs — are all sub-100-µs
  end-to-end, median. There is no phase here that is a practical latency
  concern for that profile.
- **200 rules in a strict dependency chain** scale roughly in line with
  program size for `parse`/`lower` (each rule is one more declaration to
  tokenize and lower) and stay flat-ish for `build`/`run` relative to the
  200-proposal case below — a chain evaluates each rule once, in order, so
  `run` cost is linear in rule count and small in absolute terms even at 200.
- **200 candidate proposals in one commit pool** is the standout: `build` and
  `run` are each roughly 40-60x the cost of the equivalent phases for the
  200-rule chain, despite a comparable "count of declared items" — and the
  p90 (4.5 ms) is markedly worse than the median (2.1 ms) relative to the
  other rows, suggesting this path's cost is noisier, not just larger. This
  is consistent with (not proven by this benchmark alone — that would need a
  profiler, not a wall-clock table) the deliberation frontier's admission and
  canonical-tiebreak bookkeeping scaling worse than linearly with the number
  of *simultaneously admissible* candidates in one commit — `many proposals`
  is the one axis of the four synthetic programs where growing the count
  measurably changes the *shape*, not just the size, of the cost. This is
  worth a follow-up profiling pass; it is not a regression this change
  introduces (no source under `crates/soc-core`, `crates/soc-regimes`, or
  `crates/brix-lower/src/finite_decision` was modified to produce or fix
  this), and it is flagged here because the benchmark exists precisely to
  surface it.
- **200 declared helper `fn`s, only 3 called**: `lower` cost (340.8 µs) is the
  single largest `lower` number in the table, confirming that *declaring* many
  helper functions costs lowering-time bookkeeping (arity/schema tables)
  regardless of whether they are ever called — while `run` (20.0 µs) is the
  *smallest* run cost in the whole table, confirming that cost is not paid
  again at evaluation time for the 197 functions that are declared but never
  invoked.
- **A 256-field record input** (`MAX_INPUT_CONTAINER_WIDTH`) costs 180.8 µs to
  `build` (input snapshot validation walks every declared field) but the
  program itself is otherwise tiny — the record-width axis is comfortably
  cheap even at its stated limit.
- **What this implies for the "one program → one decision over ≤256 inputs"
  profile the limits describe**: every number measured here, including the
  synthetic programs pushed toward `MAX_*` bounds, is sub-5-millisecond even
  at p90. For that profile, this pipeline is not the bottleneck in any
  workflow that also involves a human reading the result, a network hop, or a
  file write. The one caveat worth carrying forward is the proposal-count
  scaling above, if a future program legitimately wants dozens-to-hundreds of
  live candidate proposals rather than a handful.

## 4. Audit bundle produce/verify

Fixture: `examples/shipping-input.brix` + `examples/shipping-input.json` (a
program that reaches a selected decision and audits cleanly, so a bundle can
actually be produced). Same library calls `brix audit`/`brix verify`
(`crates/brix-cli/src/commands/audit.rs`, `.../verify.rs`) use, called
directly rather than through the CLI subprocess.

| phase | median | p90 |
|---|---:|---:|
| produce (run → bundle) | 12.3 µs | 22.0 µs |
| encode (bundle → bytes) | 2.8 µs | 4.5 µs |
| decode (bytes → bundle) | 0.39 µs | 0.88 µs |
| verify (re-lower + check) | 44.6 µs | 74.2 µs |
| **total** | **60.8 µs** | **101.5 µs** |

Bundle size for this fixture: 687 bytes.

**Interpretation.** `verify` is, as expected, the most expensive single phase
(roughly 3.6x `produce`'s median) — it re-lowers the source module from
scratch, exactly what an offline verifier re-deriving trust from source rather
than a signed pin must do (ADR-0022's whole premise). Even so, the entire
produce+encode+decode+verify round trip is ~61 µs median for this fixture —
again, no practical latency concern for a workflow that involves writing the
resulting 687-byte file to disk.

---

## What a knowledge-base-scale workload would need (and why it isn't measured here)

The task framing this document was written against also asks what a
10k–1M-fact knowledge-base-scale workload would need. Section 1 above *is*
effectively a proxy for the SOC settlement engine's per-step scaling behavior
at that population size, and it looks good (flat per-step cost). But that is
not the same claim as "BrixMS handles a 10k–1M-fact knowledge base today", and
this document does not round it up to that claim. Concretely, what is missing:

- **No bulk fact ingestion path.** `brix.input@1`/`@2` caps the number of
  named inputs at `MAX_INPUT_COUNT = 256` (`crates/brix-lower/src/input.rs`)
  and the aggregate shard size at 4 MiB — a deliberate hostile-input bound for
  the "one program, one decision" profile (ADR-0031), not a knowledge-base
  loader. There is no code path in this workspace that accepts, validates, or
  canonicalizes 10k+ discrete facts as one unit.
- **No persistent, shared fact store at that scale.** `soc-core::store` is
  explicit that `ArcMap` is "v1" scaffolding — an O(n)-insert placeholder
  behind a `PersistentMap` trait seam, with a real HAMT (O(log n) per-node
  structural sharing) as the stated eventual target (see Section 2 above for
  why that matters once something actually depends on it at scale). Nothing
  in this workspace currently builds a fact base on top of it.
  `IncrementalEngine`'s footprint index (Section 1) is a different mechanism
  — a static routing table built once from providers' declared footprints —
  and is not a general persistent fact store either. Section 1 addresses the
  "the settlement engine scales" half of a knowledge-base workload, not the
  "you can load one into it" half.
- **No query surface over a fact population.** The finite-decision profile is
  "one program → one decision", evaluated once per `run()` over inputs that
  are already bound before evaluation starts (`FiniteDecisionRuntime::build_with_inputs`).
  There is no indexed lookup, no incremental multi-decision pipeline reusing a
  loaded fact base across many decisions, and no query language — a 10k–1M
  fact knowledge base implies at least one of those, and Brix as a language
  does not have surface syntax to express it (no bulk `input` collections, no
  iteration over declared facts, no `rule` that ranges over an arbitrary-sized
  set rather than referencing named prior rules by identifier).

None of this is a defect being reported — it is a scope statement: the
finite-decision profile this workspace implements was designed and bounded
for "one program, one decision, ≤256 named inputs" (ADR-0030/ADR-0031), and
every limit exercised in Sections 1–4 is consistent with that design holding
up comfortably on this machine. A knowledge-base-scale workload is a
different profile that would need new surface syntax, a new input contract
(or a deliberately widened one), and a real persistent store before there
would be anything honest to benchmark. Measuring today's mechanisms at
1M-fact scale and reporting the number would answer a question nobody is
actually asking yet, since the language cannot express the workload that
number would describe.

## Added property-test runtime (cross-reference)

Not a wall-clock benchmark, but reported here since it is the other
performance-adjacent number this change introduces: the new `proptest`
property tests (`crates/brix-syntax/tests/fuzz_parse.rs`,
`crates/brix-lower/tests/fuzz_input.rs`,
`crates/brix-lower/tests/fuzz_finite_decision.rs`) add **~1.4 seconds** total
to a debug-profile `cargo test --workspace` run on this machine, at
proptest's default case count (256 cases/property, no `PROPTEST_CASES`
override) — measured directly from that run's own per-binary "finished in"
lines: `fuzz_finite_decision` 0.40 s, `fuzz_input` 0.31 s, `fuzz_parse` 0.72 s.
All three also pass at `PROPTEST_CASES=5000` (checked in release profile:
2.69 s / 0.74 s / 1.32 s respectively), with no failures found.
