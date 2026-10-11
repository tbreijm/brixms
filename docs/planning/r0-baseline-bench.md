# R0 baseline bench (pre-change cold-open numbers)

Harness: `crates/brix-kb/examples/r0_open_bench.rs`. Benchmark only; no runtime code changed.
Spec: `docs/planning/r0-cold-open-spec.md` section 7 (Acceptance).

## What it measures

- Synthetic world with N facts over `orders::rows`, `stock::rows`, `routes::rows` (N/10 stock,
  N/10 routes, rest orders; every order joins one stock and one route row).
- Program (root module): join on `sku`, filter `o.qty > 0`, projection, grouped count
  (`sku_order_counts`), and a `decide dispatch ... per id` block with two candidates.
- Load in 10 equal batches of N/10 rows; wall time per batch via `WorldSession::apply_batch`.
- Cold open: `WorldSession::open` run in a fresh child process (same binary, re-invoked with an
  env var), so its RSS is not inflated by the loader's high-water mark.
- Decisions: `all_settlements()` immediately after open.
- One-row batch: a single correction (`orders::rows` key `O-0`) immediately after open.
- RSS via `ps -o rss=` (KB) before open, after open, after the one-row batch, and in the
  parent after load. Disk bytes = recursive size of the world directory before the child runs.

Output is one JSON object on stdout (the child's JSON is embedded under `cold_open`).

## Command lines

From the worktree root:

```
cargo run --release -p brix-kb --example r0_open_bench -- 10000
cargo run --release -p brix-kb --example r0_open_bench -- 100000
```

The 100k run was repeated once to check variance (`cargo build --release` once, then run the
built binary `target/release/examples/r0_open_bench 100000`). Test world directories are created
under `std::env::temp_dir()` and removed after each run.

Toolchain: pinned `1.96.1` (`rust-toolchain.toml`). Machine: macOS (Darwin 24.6.0), release
profile. Single run per configuration except 100k (two runs).

## Results

| Metric | 10k (run 1) | 100k (run A) | 100k (run B) |
|---|---|---|---|
| Init (`from_program_with_sources`), ms | 120 | 31 | 26 |
| Load total, ms | 1254 | 10657 | 9978 |
| Load per batch, ms (batches 1..10) | 80, 80, 118, 126, 130, 134, 138, 144, 151, 148 | 213, 322, 852, 1135, 2006, 1247, 1193, 1164, 1218, 1253 | 585, 305, 1254, 973, 1055, 1072, 1132, 1153, 1184, 1213 |
| Cold open (`WorldSession::open`), ms | 757 | 9951 | 11806 |
| World decisions (`all_settlements`), ms | 3.9 | 49.0 | 147.8 |
| Decisions returned | 8000 | 80000 | 80000 |
| One-row batch after cold open, ms | 45.5 | 42.3 | 96.6 |
| One-row batch: changed keys / objects written | 1 / 10 | 1 / 12 | 1 / 12 |
| RSS before open (child), KB | 1,536 | 1,520 | 1,520 |
| RSS after open, KB | 130,464 | 1,193,184 | 1,137,792 |
| RSS after one-row batch, KB | 132,560 | 1,202,864 | 1,145,664 |
| RSS after load (parent, pre-close), KB | 107,712 | 1,126,288 | 1,122,752 |
| World directory on disk, bytes | 8,489,862 | 85,973,070 | 85,973,070 |

## Observations

- Cold open grows superlinearly: 10x facts (10k to 100k) gives about 13x to 16x cold-open
  time (757 ms to 9.9 to 11.8 s). The spec requires flat (+/-20%) across 100k to 10M.
- Load per batch grows with the world size: about 80 ms to 150 ms per 1k-row batch at 10k
  facts, and about 0.3 s to 1.3 s per 10k-row batch at 100k facts (run-to-run variance in the
  middle batches is high, so treat per-batch numbers as indicative).
- RSS at 100k is about 1.1 to 1.2 GB (about 11 KB per fact resident), about 9x the 10k figure.
  The spec target is <= 0.6x today's RSS at 1.09M facts; this baseline is the "today" number.
- Decisions after open are cheap (milliseconds), so the cost is in opening, not in deciding.
- The one-row batch after open is under the 100 ms spec target at both sizes, but run B at
  100k was 97 ms, which is close to the limit.

## Deviations from the spec

- Only the synthetic 10k and 100k worlds were run, as requested. The 1M and 10M synthetic
  points and the real Atlas world (1.09M facts) were not run, so the 1 s cold-open target is
  not evaluated here.
- Cold open is measured in a child process; "load" and "cold open" share a binary but not a
  process, so the parent's RSS after load is reported separately.
- The decide block uses `per id` (required by the world profile); the grouped count output is
  computed but not separately timed or printed.
