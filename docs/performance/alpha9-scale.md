# Alpha.9 scale qualification

**Qualified.** The validated capture has 30 complete size/profile/repetition runs: five repetitions in each of six cells. There are zero failures, and all three ten-commit storage budgets pass. This is a fresh native alpha.9 qualification.

The workload is a public synthetic Atlas-shaped fixture. No actual Atlas source dataset was available, so these results are not Atlas measurements and do not establish an improvement over Atlas performance claims.

## Build and host

| Field | Value |
|---|---|
| Source and embedded build commit | `12e37f8b40f2137516e6c9ede60c7431d942fdd5` |
| Benchmark executable SHA-256 | `fc33c9f1746551bd5aa50d68e8e1afae4f98c4cdbfed99789e5cb79030d9538e` |
| Host | Apple M1 Pro, 10 CPUs, 16 GiB RAM |
| Operating system | macOS 15.6.1 (24G90) |
| Rust | `rustc 1.96.1 (31fca3adb 2026-06-26)` |
| Raw records and validated summary | [`raw/alpha9-scale-all.jsonl`](raw/alpha9-scale-all.jsonl), [`raw/alpha9-scale-summary.json`](raw/alpha9-scale-summary.json) |

## Ten-commit load and storage

All table values summarize 5/5 repetitions as median [minimum–maximum]; the storage values were identical in all five runs. Storage is measured immediately after the ten-commit load, before edit history. “Cold open” is a separate fresh-process open measurement; OS cache state is unknown. Allocated bytes are filesystem allocation; logical bytes are the storage gate metric.

| Source facts | Init ms | Load ms | Load peak RSS bytes | Logical bytes | Allocated bytes | Cold open ms | Logical budget (pass) |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 8,000 | 51.22 [44.63–55.62] | 12,720.32 [12,532.05–13,098.01] | 237,355,008 [223,690,752–245,432,320] | 17,465,242 [17,465,242–17,465,242] | 17,616,896 [17,616,896–17,616,896] | 2.72 [2.70–2.77] | 115,081,216 |
| 80,000 | 51.69 [39.24–80.63] | 18,472.54 [17,343.79–19,088.08] | 881,754,112 [842,596,352–920,141,824] | 132,341,944 [132,341,944–132,341,944] | 132,501,504 [132,501,504–132,501,504] | 7.03 [6.74–8.00] | 999,817,216 |
| 800,000 | 30.21 [26.13–32.81] | 70,423.32 [68,817.04–71,496.56] | 5,147,983,872 [5,031,788,544–5,176,524,800] | 1,314,944,760 [1,314,944,760–1,314,944,760] | 1,360,510,976 [1,354,076,160–1,379,835,904] | 42.55 [41.28–44.38] | 9,847,177,216 |

The budget is `16 MiB + 12 KiB × source fact count`. All six matrix cells passed; storage is shown here for the ten-commit profile because that is the profile governed by this budget.

## Query and edit results

The detailed appendix contains the complete per-phase medians and ranges, disk component accounting, RSS, query results, and edit counters. Representative ten-commit fresh-process derived-query full-result medians were 5.47 ms (8k fulfillment), 451.73 ms (80k fulfillment), and 55,646.07 ms (800k fulfillment); the latter returns 80,000 rows. At 800k, the decision-value join full result was 50,031.12 ms and activity pages were 1,084.43 ms. These query APIs paginate outputs, but the current derived pagination implementation scans the full relation on each page, so large full-result queries are expensive. The [full derived-query and named-query tables](alpha9-scale-details.md#four-derived-relation-page-queries) include first-page/full-result timings and row/byte counts.

Measured one-row add/retract edits remained similar across fixture sizes: at 800k facts, medians were 94.70 ms and 96.46 ms for ten-commit add and retract, respectively. The 5,000-child hot-group workload reported three operator work units and three expressions evaluated for each operation; unrelated world size did not increase those counters. These are exposed logical work counters, not CPU instruction counts. The appendix lists state reads/writes, objects, and all repetitions' ranges.

## Capture audit and limits

Capture timestamps span 2026-11-10 08:56:11–20:18:31 UTC; summed process-monotonic elapsed time is 15,333.49 s. The audit found one 25,605.86 s discontinuity between the last 800k ten-commit repetition 5 verify-after-edits phase and the following 80k setup. This falls around the untimed correctness/transition work; its cause was not recorded. Timings use Rust `Instant` and Python monotonic clocks. No measured load, open, or query interval contains this discontinuity.

Power and system load were uncontrolled, cache state is unknown, and process-wall measurements include the runner's 200 ms polling interval. The prior alpha.8 exploratory raw records were lost during temporary-file cleanup and are not used or reconstructed. The current alpha.9 qualification records are complete and durable.

## Reproduction

From the source commit above, build with the commit embedded in the example, then capture all sizes and profiles with five repetitions using a durable scratch root:

```sh
BRIX_BENCH_BUILD_COMMIT=12e37f8b40f2137516e6c9ede60c7431d942fdd5 \
  CARGO_TARGET_DIR=target/alpha9-bench-build \
  cargo build --release -p brix-kb --example alpha9_scale_bench
python3 scripts/alpha9_scale_capture.py \
  target/alpha9-bench-build/release/examples/alpha9_scale_bench \
  docs/performance/raw/alpha9-scale-reproduction.jsonl \
  --sizes 8000 80000 800000 --profiles single ten --repetitions 5 \
  --host-metadata docs/performance/alpha9-host.json --allow-large \
  --scratch-root target/alpha9-benchmark-worlds
python3 scripts/alpha9_scale_report.py \
  docs/performance/raw/alpha9-scale-reproduction.jsonl \
  --output docs/performance/raw/alpha9-scale-reproduction-summary.json
```

For methodology and all detailed measurements, see [the qualification appendix](alpha9-scale-details.md).
