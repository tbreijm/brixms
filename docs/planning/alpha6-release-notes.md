# Alpha.6: typed, chained world decisions

Status: implementation and qualification in progress. This document does not
claim that alpha.6 has been published.

Baseline: `v0.1.0-alpha.5` (`0bad6e4`), with the Atlas R1–R4 work integrated
from `feat/atlas-world-r1-r4`. Alpha.5's persisted cold-open architecture remains
the execution baseline.

## Scope

- R1: nominal record and variant values in world tuples, helper expressions,
  decisions, history and audit.
- R2: settled decision rows as inputs to downstream relations and decisions,
  with deterministic acyclic dependencies and upstream explanation links.
- R3: exact grouped integer/decimal sums and ordered minima/maxima, including
  updates and extremum retraction.
- R4: indexed correlated absence and explicit zero-filled parent counts, so
  missing evidence and removal of the last support have defined outcomes.
- Integration with cold restore, persisted operator state, settlement
  checkpoints, idempotent retries and independent verification.

The combined public-interface example is
[`examples/world-atlas`](../../examples/world-atlas/README.md). It is synthetic;
it is not a measurement of a production Atlas workload.

## Storage and compatibility

Two persisted formats advance:

- The storage value codec moves from `brix.world.state-value@2` to `@3` for
  bounded typed record/variant values. Existing scalar byte encodings remain
  unchanged.
- The network-state header moves from `brix.world.network-state@1` to `@2`. It
  gains operator roots for grouped aggregates (`Group { supports, aggregates }`)
  and for the absence operators (`Anti`, `ParentCount`).

The persisted network validity key includes the codec version, and a header
under a different schema tag is classified as stale, never as corruption.
Previous state is therefore replayed before it can be reused. This is a cache
upgrade, not permission to reinterpret damaged state or weaken independent
journal verification.

Because revision digests bind the network-state root, they change with these
bumps. The three `golden_alpha6_*` digests in `r0_chunked_persist.rs` were
re-pinned for that reason. Over those workloads the decision-delta bytes and
final settlements hash identically on alpha.5 and alpha.6 (checked with a probe
that excludes the revision digest), so the change is confined to the state-root
binding.

Winner rows of chained decisions are published by one shared cascade that every
batch route uses (single chunk, multi-chunk, replay), so results do not depend
on where chunk boundaries fall. On a restored network the previous winner row of
a changed entity is re-seeded lazily from its prior settlement; cold open does
not scale with the number of decisions.

## Final fixes

Release review caught and fixed stale absence output after the last parent was
removed. Absence fanout now preflights match/work budgets before decoding a
whole bucket. Cascade work, match and expression limits apply across the entire
atomic batch. Ordered aggregate state uses persisted tree nodes, so a group with
more than 256 distinct values survives restart without storing its entire
multiset inside one bounded value. Malformed aggregate state is rejected.

The release archive includes both world examples and runs the combined Atlas
CLI walkthrough. Cargo metadata and the lockfile now name `0.1.0-alpha.6`.

## Qualification

Run on this integration branch (`CARGO_TARGET_DIR` outside the tree):

- `cargo fmt --all --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo test --workspace --no-fail-fast` (includes doctests, the numeric
  conformance tests, and the new alpha.6 qualification tests): 161 targets,
  1865 passed, 0 failed, 27 ignored.
- `scripts/smoke_world_atlas.py` against the built CLI and
  `examples/world-atlas`: PASS.
- Gates: `scripts/check_tcb_dependencies.py --check`,
  `scripts/test_tcb_dependency_gate.sh`, `scripts/check_soc_law_map.py`,
  `scripts/test_law_map_provisional_gate.sh`, and
  `scripts/canon_crosscheck.py` (69 vectors): all PASS.
- Python bindings: `pytest bindings/python/tests` against the debug CLI,
  17 passed.
- `cargo deny --all-features check`: advisories, bans, licenses and sources ok
  (exit 0; the wildcard-path warnings are emitted for the workspace's own path
  dependencies).
- Packaged release smoke: `cargo build --release -p brix-cli`, staged as
  `release.yml` does (including `examples/world-linked` and `examples/world-atlas`),
  then `scripts/smoke_release_package.py <dir> 0.1.0-alpha.6` and
  `scripts/smoke_world_atlas.py <dir>/brix <dir>/examples/world-atlas`: all 10 package
  validation stages and the packaged Atlas smoke PASS at `0.1.0-alpha.6`.
- Upgrade from a real alpha.5 world directory. An alpha.5 CLI (`0bad6e4`)
  created a world from `examples/world-linked` and committed two batches. The
  alpha.6 CLI then:
  - opened it with `world open-check --verify-state`: replayed
    (`replay: schema`), not corruption, and verified against a replay of the
    base facts;
  - verified in-place with `world verify` against the alpha.5 head revision
    and program digest: reported `audited`;
  - returned `decide` and `query` results identical to alpha.5's, apart from
    open metadata (`open_mode`, `open_ms`, state source/refresh fields);
  - committed a third batch, after which the world opened `restored` and
    verified, verified in-place with `world verify`, and its `decide` output
    equals that of a world built from the same history entirely on alpha.6.
  Also covered by `r0_restore` with a forged `@1` header.

- Scale qualification in release mode (`cargo test --release -p brix-kb`):
  - Atlas profile (`alpha6_qualification.rs`):
    - `atlas_scale_10k`: 10,000 resident facts; ingest 3,040 ms, cold restore 28 ms, durable retraction 60 ms, 76 objects written.
    - `atlas_scale_100k`: 100,000 resident facts; ingest 35,839 ms, cold restore 310 ms, durable retraction 80 ms, 94 objects written.
  - Acceptance matrix across 102 linked modules, 150 schemas, and 301 helpers (`world_acceptance_workload.rs`):
    - `test_acceptance_10k`: 10,000 facts; ingest 2,137 ms (4,678 rows/s), cold restore 86 ms, single-fact edit 44.9 ms, audit export 43 ms, audit verify 7,548 ms, peak RSS 206 MB.
    - `test_acceptance_100k`: 100,000 facts; ingest 23,098 ms (4,329 rows/s), cold restore 108 ms, single-fact edit 45.9 ms, single-fact retract 49.8 ms, audit export 468 ms, audit verify 79,298 ms, peak RSS 506.3 MB.
    - `test_acceptance_1m_full`: 1,000,000 facts across 100 batches; ingest 659,855 ms (1,515 rows/s), 6.36 GB written, peak RSS 3,362 MB; cold reopen 1,000 ms; single-fact edit 66.98 ms (65 objects written); single-fact retract 62.18 ms; audit export 20,038 ms; independent audit verify 806,221 ms (4,799,998 settlements computed, 188,359,920 work items, 9,399,999 tuples decoded).
  - Measured scale limits: Ingestion throughput moderates to ~1,515 rows/sec at 1M (vs ~4,300–4,700 rows/sec at 10k/100k) with peak RSS reaching 3.36 GB and 6.36 GB written due to upper-trie rewrite amplification across batches. Independent audit verification scales linearly with total fact/settlement count (~806 s at 1M vs ~79 s at 100k).
  - Scale suites not run: `world_integrated_scale` multi-process durability matrices and `world_throughput_probe` multi-lane benchmarks were deferred as out-of-scope for the core relational release candidate; the upgrade probe covers one linked program, not an Atlas-sized world.


## Boundaries

This release does not implement the separate workflow-grounding proposal or
the post-R1–R4 cleanup/performance project. It does not claim to solve alpha.5's
bulk ingestion or storage amplification limits. Runtime settlements remain
`Derived`; a successful independent audit yields separate audit evidence.
