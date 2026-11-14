# Alpha 9 scale fixture

`crates/brix-kb/examples/alpha9_scale_bench.rs` measures the actual world
initializer, batch loader, incremental network, store/open path, and scoped
named-query API. It is not an operator microbenchmark.

The fixture is deterministic Atlas-shaped generated data. No Atlas source
dataset was available, so reports set `actual_atlas_data` to false. The source
program and fixed-width row generator are embedded in the example. The fixture
digest is a chained Brix canonical `Domain::Value` BLAKE3 digest over the schema,
seed, count, and every generated relation/key/tuple in load order.

## Workload

The harness accepts exactly 8,000, 80,000, or 800,000 source facts. Each size
contains 128 catalogue rows, two decision-label rows, 64 inventory rows, one
order and one union row per ten facts, and the remaining facts as events. The
first 5,000 events form the required skewed `hot` group. Other events are
balanced into groups of at most 64 children. `entity_a` and `entity_b` use
overlapping keys and values to exercise union-distinct. Orders join inventory
through nested `Product.sku`; fulfillment decisions return a record with nested
field `b`, which feeds a derived join through `d.value.b`. Activity uses bounded `collect(...).take(8)`. The
scoped named query returns exact typed rows from the 128-row catalogue; its
host-selected tenant is enforced by the query scope, not by a request argument.
The query process also pages the activity, union, fulfillment, and decision-join
derived relations at a pinned revision, reporting first-page and complete-result
rows, bytes, timings, and a canonical content digest for each.
The named-query page size is fixed at 32 rows; derived-relation page size is
fixed at 1,024 rows. These sizes are recorded before the measurement matrix and
do not change between fixture sizes or load profiles.

The 1- and 10-batch profiles use identical generated source facts and differ
only in commit boundaries. The 10-batch profile is the release-contract disk
budget workload. Storage reports all direct world entries, categorizing known
objects, revisions, receipts, and staging separately and retaining an
`other_required_files` category for any additional directory. Logical and
allocated bytes are both reported. Source tuples, retained trie versions and
operator state share CAS packs; their physical bytes are reported together as
`objects_source_state_and_indexes`. Exact input tuple/key payload bytes are
reported separately as non-additive logical content, not an invented partition
of shared pack storage.

## Measurement phases

Each process prints one JSON object with schema `brix.alpha9.scale-bench@1`.
The runner invokes the modes in this order against a fresh world:

```text
alpha9_scale_bench --full --facts 8000 --batches 10 --seed 0 --world-dir DIR
alpha9_scale_bench --child-load DIR
alpha9_scale_bench --child-open DIR
alpha9_scale_bench --child-query DIR
alpha9_scale_bench --child-query-warm DIR
alpha9_scale_bench --child-verify DIR
alpha9_scale_bench --child-edit DIR add
alpha9_scale_bench --child-edit DIR retract
alpha9_scale_bench --child-verify DIR
```

`--full` initializes an empty world and records initialization separately.
`--child-load` loads exactly the requested one or ten batches, records total
load wall time, operation-generation time, commit time/counters, and storage
before any edit revisions. Open time is separate. Effective storage settings
are reported: default `node_cache_bytes` is 512 MiB and default
`apply_hydrated_bytes` is 0 (chunk persistence disabled); manifest overrides
would be reported separately, and the harness does not change them.
`--child-verify` runs store,
oracle, derived-count, and exact typed named-query checks outside the measured
load/open/query processes. `--child-open` measures only a fresh-process open.
`--child-query` measures fresh-process named and derived relation reads;
`--child-query-warm` does untimed passes through the same page APIs, then
reports the measured pass. Both report first-page and complete-result rows,
bytes, and times. The named query also reports work units, matches, and
expressions evaluated. `--child-verify` checks store, oracle, derived counts,
exact relation-page content, and canonically sorted typed named-query rows
outside measured processes. `--child-edit` measures add or retract in separate
processes and emits real commit operator-work, match, expression, and storage
I/O counters. The final verification checks the world after both edits.

Every JSON object includes `main_entry_unix_ns` so the parent runner can derive
launcher-inclusive startup from its own process-start timestamp. The example
reports only the build commit embedded at compile time through
`BRIX_BENCH_BUILD_COMMIT`; the runner also records the executable's SHA-256.
The report includes the source-program Brix digest and Rust toolchain string.

Use five fresh-process repetitions per size and load profile. Retain each raw
record, stdout, stderr, command, peak RSS, free-space snapshots, timeout, and
exit status. Timeouts, OOMs, or correctness failures remain visible results.
Cold means a fresh process opening the world; the executable does not claim to
clear the operating-system cache. The query process opens a session with the
maintained network and invokes read APIs only. The runner reports cache state as
unknown unless it controls and records it. It owns cleanup after all phase
processes complete.
