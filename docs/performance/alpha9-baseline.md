# Alpha.9 benchmark baseline capture (A9-1/A9-2 foundation)

Status: **bounded, non-qualifying baseline**. Captured from reviewed release-foundation commit
`b1f65e18268fd6b7c527b371a126d8ce261c70e3` (`work/alpha9-base`) on 2026-11-01 using the existing
`crates/brix-kb/examples/r0_open_bench.rs`. This is a synthetic R0 workload,
not Atlas data and not the A9-7 public fixture. Do not compare it directly to
Atlas's reported 91.5 MB / 2.3 s / ~45 ms numbers.

Build used Rust/Cargo 1.96.1, `--release`, on macOS Darwin 24.6.0, arm64
(Apple M-series host; exact model and memory size were unavailable through the
restricted sysctl interface). Benchmark executable SHA-256:
`2ed07973d662e59a7e39ae4b5c4df4efbcf305d3b8f6c33aaf1a878b8c77fcac`.
OS cache was uncontrolled; all five repetitions were new benchmark/world
processes on the same machine.

## What this workload actually measures

The harness creates 8,000 source facts: 800 stock rows, 800 route rows, and
6,400 order rows. It commits ten equal batches (800 facts each), retaining the
initial revision and all ten commits. It computes a three-way equality join,
grouped count by SKU, and one scalar-key decision per fulfillment row. The
existing harness also probes decisions and a one-row edit, and opens the
persisted world in fresh child processes. Input values are generated
deterministically by row index, but there is no explicit seeded random fixture.

It does **not** implement or measure the A9-7 workload requirements for a
5,000-child skewed group, union/union-distinct, nested-field join, scoped named
query, first-page/complete query latency, or a correctness oracle outside the
timed intervals. Its old relation has a single-level `sku` join. It reports
recursive logical file lengths, not allocated disk blocks or a source/history/
index/scratch breakdown. It emits no peak RSS/CPU values in this run because
the harness's `ps` probes returned null in this environment. The process-open
measurement is a fresh process but the OS cache is not cleared. These gaps
make the result a measurement of this exact synthetic alpha.8 fixture only.

## Captured result

Five raw JSON records are in
[`alpha9-baseline-8k.jsonl`](alpha9-baseline-8k.jsonl). Each reports 8,000
resident source facts and 6,400 decisions. Median load was 1,388 ms (range
1,352–1,418 ms); logical directory bytes were 43,541,968 in every run; median
fresh-process restore/open was 2.16 ms (range 2.12–2.26 ms). The first cold
process decision read was 4.78 ms median. In the separate in-process
frontier-only probe, decision read was about 52 ms. The first one-row batch in
the cold-open child process was about 52 ms. Process RSS is null in every raw
record. These are five repetitions with no cleared-cache claim. The contract's
initial disk budget at 8,000 facts is 115,081,216 bytes (16 MiB + 12 KiB/fact),
but that budget is defined for the not-yet-fixed A9-7 public workload and
cannot be scored against this different history/fixture.

The measured load includes 44 ms initialization and ten sequential batch
commits. `disk_bytes` is measured after closing the session and includes the
retained history in the world directory. Cold-open output comes from a child
process. The example also creates a legacy-head fallback probe; use the
`cold_open` field for the current-head restore time and do not mix its fallback
timings into the main result.

## Reproduce

Use the pinned Rust toolchain and a private target directory. The example
requires `facts >= 100` and a multiple of 100 with its default ten batches.

```sh
git checkout b1f65e18268fd6b7c527b371a126d8ce261c70e3
CARGO_TARGET_DIR="$PWD/target" cargo build --release -p brix-kb --example r0_open_bench
python3 scripts/alpha9_baseline_capture.py \
  "$PWD/target/release/examples/r0_open_bench" 8000 5 \
  docs/performance/alpha9-baseline-8k.jsonl
```

This wrapper retains the complete JSON from each existing harness run, one
record per line. It does not alter the fixture or claim to satisfy A9-7.
`target` is local build output and should not be committed.

## A9-7 fixture and bounded run plan

The release contract requires a fixed-seed Atlas-shaped public fixture and
actual Atlas model data when available. It requires exactly 8k / 80k / 800k
source facts, ten equal insertion batches for the disk budget while retaining
all ten revisions, plus a separate single-batch load profile. Include balanced
and skewed groups (including one group with at least 5,000 children), unions,
nested-field joins, decisions, and scoped named queries. The fixture generator
must freeze payload lengths/distribution and history semantics before the
budget is ratified or scored. Record source and derived row counts separately.

To port this baseline size sweep to the eventual qualifying harness, bound
resources with per-run timeouts and scratch-space checks, start with 8k, then
80k, then 800k; capture all five fresh-process repetitions at every profile.
The contract's disk target evaluates only the specified ten-batch retained
history profile. Do not report a missing timeout/OOM point as a pass. Correctness
checks (`open-check --verify-store`, typed/named query, independent
`WorldSession::verify_oracle`) must run outside timing. New-process runs label
OS-cache status as unknown/warm unless the cache was actually controlled.

For later **exploratory R0 size probes only**, run sequentially and write to
distinct JSONL paths:

```sh
python3 scripts/alpha9_baseline_capture.py \
  "$PWD/target/release/examples/r0_open_bench" 80000 5 \
  --timeout-seconds 7200 docs/performance/alpha9-baseline-80k.jsonl
python3 scripts/alpha9_baseline_capture.py \
  "$PWD/target/release/examples/r0_open_bench" 800000 5 \
  --timeout-seconds 7200 docs/performance/alpha9-baseline-800k.jsonl
```

These may be expensive; check free space first, run sequentially, and preserve
timeout records. The 800k R0 probe is not A9-7 qualification because this
fixture omits required operators. Qualification commands should target the
eventual fixed-seed A9-7 fixture and use its ten-batch retained-history and
single-batch profiles.

Baseline harness facts with higher implementation cost—unions and scoped
queries—are not available in the alpha.8 harness. Reuse existing world
semantics, and do not replace them with unrelated microbenchmarks merely to
fill the table.
