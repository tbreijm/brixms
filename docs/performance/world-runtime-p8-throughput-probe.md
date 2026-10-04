# World Runtime P8 — Release-Mode Throughput & Edit-Latency Probe (De-Risking)

Status: **de-risking measurement only — no src/ changes.** Governed by
`docs/planning/persistent-world-runtime-plan.md` §1, P7, P8, §5 matrix. Follows on from
the debug-mode numbers and open risks recorded in `docs/performance/world-runtime-p5-results.md`
("Risks carried into P7/P8").

Harness: `crates/brix-kb/tests/world_throughput_probe.rs` (new file, `#[ignore]`d tests, no
assertions on performance — it measures, it does not gate). Drives the public
`WorldSession` + `WorldNetwork` API in the same shape as `p5_adversarial_probe.rs`'s
p01/p02 (3-relation linked model: `orders` ⋈ `inventory` ⋈ `shipping` → `fulfillment`,
decided by `dispatch`), plus a lower-level micro-benchmark against the `soc_core::store`
primitives (`TrieMap`, `FileNodeStore`) that `WorldSession` is built on — both are existing
public dependencies of `brix-kb`, so no production code was touched to get this breakdown.

## Machine & commands

- Machine: Apple M1 Pro, 16 GB RAM, macOS 15.6.1 (24G90), APFS on internal SSD (Apple Fabric,
  "Solid State: Yes").
- Build profile: **release** (`cargo build --release -p brix-kb --tests`, build time 31.7 s cold).
- Commands (each run individually, `--test-threads=1` implicit since one test per invocation):
  ```
  cargo test --release -p brix-kb --test world_throughput_probe <test_name> -- --ignored --nocapture
  ```
- Branch: `lane-e/p8-throughput`, based on `f1ec1b8` (P5 close).

**Honesty note on environment noise:** this sandbox's filesystem/fsync latency is
noticeably more variable run-to-run than a typical bare-metal benchmark box. Direct
evidence: `probe_ingest_1k` was re-run twice with byte-for-byte identical I/O work
(`objects_written=1594`, `bytes_written=194939` both times) and measured 7.64 s the first
time and 17.71 s the second — a **2.3x swing on identical work**. Every number below is a
real measured number from an actual run (labeled which), but absolute magnitudes should be
read as "this general size," not precise to the percent; growth *trends* across 1k→10k→100k
(same process, same few minutes) are more trustworthy than any single absolute figure.

## 1. Ingestion throughput (single batch, full linked model + network)

`session.apply_batch` = store write + fsync + publication. `network.apply_batch` = pure
in-memory incremental re-deliberation (verified by inspection of `network.rs`: no `fs::`/
`File::` calls anywhere in that file — it is not instrumented further for that reason).

| Rows | session.apply_batch | rows/s | objects_written | bytes_written | network.apply_batch | intermediate_deltas | settlements |
|---|---|---|---|---|---|---|---|
| 1,000 | 7.64 s (rerun: 17.71 s) | 130.9 (rerun: 56.5) | 1,594 | 194,939 B (194.9 B/row) | 77.9 ms (rerun: 62.1 ms) | 5,400 | 800 |
| 10,000 | 10.77 s | 928.6 | 16,062 | 1,979,172 B (197.9 B/row) | 640.9 ms | 54,000 | 8,000 |
| 100,000 | 51.59 s | 1,938.4 | 160,334 | 20,007,550 B (200.1 B/row) | 7.16 s | 540,000 | 80,000 |
| 1,000,000 | **EXTRAPOLATED, not run:** ≈ 516 s (≈ 8.6 min), linear from the 100k rate | ≈ 1,938 (if the 100k rate holds) | — | — | **EXTRAPOLATED:** ≈ 71.6 s, linear from the 10k→100k growth (≈11x per 10x rows) | — | — |

All numbers above are **measured** except the 1,000,000 row, which is explicitly
**EXTRAPOLATED** (see §4 — I did not run it; reasoning below).

Bytes written per row is flat (~195–200 B/row) across two orders of magnitude — no
footprint blow-up with scale, and a useful sanity check that the encoding itself isn't the
problem.

## 2. Batch granularity (does it matter whether you ingest as 1 row/batch or 1 batch?)

Fixed total of 2,000 rows into a **single un-joined relation** (no network — isolates the
`WorldSession` store/fsync path), varying only how many `apply_batch` calls it takes:

| Batch size | Batches | Wall time | rows/s | objects_written (node files) | files_synced | directories_synced | fsyncs/batch |
|---|---|---|---|---|---|---|---|
| 2,000 (1 batch) | 1 | 9.59 s | 208.7 | 2,633 | 2,633 | 257 | 2,890 |
| 100 | 20 | 30.04 s (rerun: 56.74 s) | 66.6 (rerun: 35.2) | 4,216 | 4,216 | 2,874 | 354.5 |
| 1 | 2,000 | 151.20 s | 13.2 | 8,143 | 8,143 | 10,079 | 9.1 |

This is unambiguous, not noise-sensitive (the *io_stats counts* — not just wall time — are
reported, and they move monotonically with batch count on identical total rows):

- **More, smaller batches write strictly more node files for the same logical data**
  (2,633 → 4,216 → 8,143 objects for the same 2,000 rows): because each batch persists and
  finalizes its own root-to-leaf path independently, there is less opportunity to collapse
  repeated structural changes into one final tree before paying the write+fsync cost.
- **directories_synced balloons** (257 → 2,874 → 10,079): the same ~256 two-hex-char
  object-shard directories get re-`fsync`'d on *every* batch's `flush()`, because
  `FileNodeStore::flush()` has no memory of which directories it already synced in a
  previous `apply_batch` call.
- Net effect: ingesting the identical 2,000 rows as one batch (9.6 s) vs. one row at a time
  (151.2 s) is a **15.8x slowdown** from batching alone, with no change in total row count,
  schema, or hardware.

**Answer to "is ingestion per-row batches or one batch?":** the `p01`/`p5` 10k-row probe (and
this probe's §1 table) already uses one big batch — that is the right pattern and this repo's
test fixtures already do it. The risk is only if a *caller* streams one-row batches into a
1M-row load; §2 quantifies exactly how bad that would be (extrapolating the per-batch-size=1
rate of 13.2 rows/s to 1M rows is on the order of **21 hours** — this is also an
EXTRAPOLATED, not measured, number, flagged as such because it is far outside anything run
here; it is reported only to make the shape of the risk vivid, not as a target number).

## 3. Store-phase breakdown (isolating hashing / write / fsync)

Micro-benchmark directly against `soc_core::store::{TrieMap, FileNodeStore}` (the exact
types `WorldSession` uses internally), bypassing `WorldSession` entirely, decomposed into:
- **Phase A** — pure in-memory `insert_with_store` loop (no I/O: freshly-built nodes are
  never `Node::Lazy`, so `resolve_node` never touches the store — confirmed by the io_stats
  delta after phase A being all-zero in every run).
- **Phase B** — `persist_to_store` (creates + atomically renames one file per new node;
  **no fsync** — `FileNodeStore::put_node` only records the path as "pending").
- **Phase C** — `flush()` (the actual `sync_all()` calls: one per pending file, then one per
  distinct parent directory, then one for the top-level `objects/` dir).
- **Hashing-cost proxy** — an independent loop computing `Digest::of` over similarly-sized
  payloads (not the exact same internal calls — those aren't separately instrumentable
  without editing `src/` — but representative of per-node content-hashing cost).

| n | Phase A (in-mem) | Phase B (write, no fsync) | Phase C (fsync) | objects | Hashing proxy | A+B+C total |
|---|---|---|---|---|---|---|
| 1,000 | 3.75 ms | 224.3 ms (166 µs/node) | 445.3 ms | 1,350 | 0.16 ms (160 ns/digest) | 673.3 ms |
| 10,000 | 52.2 ms | 2.25 s (164 µs/node) | 17.49 s | 13,717 | 1.58 ms (157 ns/digest) | 19.80 s |
| 100,000 | 651.3 ms | 31.42 s (229 µs/node) | 15.33 s | 137,451 | 15.69 ms (157 ns/digest) | 47.40 s |

Observations (all measured, each a single run — see the noise caveat above before reading
precise ratios):

- **Hashing is never the bottleneck**: 157–160 ns/digest throughout; at 100k rows that is
  15.7 ms out of a 47.4 s total (0.03%).
- **Phase A (pure CPU) is cheap and scales linearly and cleanly**: 3.7–6.5 µs/op, no growth
  signal.
- **Phases B and C (the two I/O phases) dominate completely** — together 99.4%+ of total
  time at every size — and between them is where all optimization leverage lives.
- Per-call costs for B and C both show real but noisy variance across runs (B: 164–229
  µs/node; C: 111 µs–1.25 ms per sync call) — consistent with the §0 noise caveat, but the
  *aggregate* conclusion (I/O syscalls, not hashing or allocation, own this budget) is
  robust across every run.

## 4. One-fact edit latency

Same 3-relation linked model, seeded once, then a single-field upsert on one existing row,
timing `session.apply_batch` (store write + fsync + publication) separately from
`network.apply_batch` (pure in-memory re-deliberation — confirmed zero `fs`/`File` calls):

| World size | session.apply_batch | objects_written | network.apply_batch | intermediate_deltas | settlements | reads (trie lookups) |
|---|---|---|---|---|---|---|
| 1,000 | 56.9 ms | 4 | 0.37 ms | 12 | 1 | 39 |
| 10,000 | 72.5 ms | 5 | 0.44 ms | 12 | 1 | 55 |
| 100,000 | 93.1 ms | 6 | 0.67 ms | 12 | 1 | 68 |

- `objects_written` grows **4 → 5 → 6** across 1k → 10k → 100k, i.e. **O(log₁₆ N)**, exactly
  as the trie's branching factor predicts — not a scan. `intermediate_deltas` stays pinned
  at the structural bound of 12 regardless of world size (matches P5's `p02` gate of ≤12).
  This directly confirms the P5 doc's "strictly local" claim still holds at 10x and 100x
  the P5 scale.
- Network propagation is always sub-millisecond and essentially flat (0.37 → 0.44 → 0.67 ms)
  — not the cost driver.
- **Nearly all edit latency (>99%) is the handful of `fsync`-class syscalls in
  `WorldSession::apply_batch`**: the node-store `flush()` (files_synced + directories_synced,
  which also grows with `objects_written`: 4+5=9, 5+6=11, 6+7=13 total sync calls) plus the
  four explicit `sync_all()` call-sites in `session.rs` for the revision temp file, the
  revisions directory, the HEAD temp file, and the root directory (lines ~1052, 1055, 1069,
  1081 in the version at `f1ec1b8`). At ~9–13 total fsync-class syscalls per edit and
  57–93 ms total, that is **~5–10 ms per syscall**, consistent with macOS's `F_FULLFSYNC`
  (which Rust's `File::sync_all()` uses on this platform) being a *device-wide* write
  barrier rather than a per-file operation — notoriously expensive per call, and the reason
  this number barely moves with world size: it is dominated by a near-fixed count of
  expensive syscalls, not by anything that scales with N.
- **Release vs. debug**: edit latency in release (57–93 ms) is in the same range as — and in
  this run, slightly *slower* than — the debug numbers in `world-runtime-p5-results.md`
  (59.9 ms / 56.6 ms). This is expected once you know the cost is fsync-dominated:
  **recompiling in release buys ~2x on ingestion throughput (CPU-bound phase A benefits) but
  essentially nothing on one-fact edit latency**, because edit latency is almost entirely
  kernel/device-syscall time that a faster CPU cannot shrink.

## Ranked recommendations (not implemented here — src/ is other lanes' territory right now)

1. **[HIGH impact] Stop doing one `fsync`/`sync_all()` per content-addressed object file.**
   `FileNodeStore::put_node` creates one file per node and `flush()` calls `sync_all()` on
   every one of them individually, plus once per touched shard directory. This is the
   dominant cost everywhere in §1–§4 (Phase C in §3 is 32–88% of total store time by itself,
   and is the near-entire explanation for §4's edit-latency floor). Concretely: either (a)
   batch new objects for one `apply_batch` into a single append-only segment/pack file and
   `fsync` that one file + its one parent directory once per batch (this is the standard
   log-structured-store fix and would plausibly cut both ingestion time and one-fact-edit
   latency by an order of magnitude or more — e.g. 100k ingestion's §1 51.6 s "run" and §3's
   47.4 s "pure store" overlap substantially; collapsing ~160k individual fsyncs into O(1)
   per batch should bring this well under 5 s), or (b) if per-object files must stay for
   content-addressing/dedup reasons, at minimum coalesce the final `sync_all()` calls so each
   distinct directory and the top-level `objects/` dir is synced **once per batch** (already
   true) but files within a directory are **not each individually `sync_all`'d** — a single
   `fsync` on the directory *after* all its files are written is sufficient for durability of
   directory entries, and the per-file `fsync` is only needed to guarantee the file's own
   *contents* reached disk, which could instead be done with one `fsync` per batch on a
   single always-open log file that the per-node writes are appended to.

2. **[HIGH impact for the "many small batches" scenario, same root cause as #1] Don't let
   callers (or default CLI/bulk-load tooling) stream 1M facts as 1M one-row batches.** §2
   shows a clean, reproducible (in io_stats, not just wall-clock) 15.8x slowdown from
   batch-size=1 vs. one big batch on identical data, caused by redundant directory
   `fsync`s and redundant node-path rewrites. Any P8 bulk-load path should explicitly chunk
   into large batches (the existing `p01`-style single-batch pattern is already correct);
   worth adding an assertion/lint or documented minimum recommended batch size to the
   `brix world` ingestion CLI/docs so this doesn't regress by accident.

3. **[MEDIUM impact, low risk, mechanical] `FileNodeStore`'s `contains()` uses the trait
   default (`get_node(..).is_some()`), which does a full `fs::read` of the file to answer an
   existence question.** Evidence: in every ingestion run, `node_store.io_stats().reads`
   equals `writes` 1:1 (e.g. 100k: reads=160,334, writes=160,334) — every persisted node pays
   for one existence-check read attempt in addition to its write. A `Path::exists()`/`stat`
   based override removes a full `open()` syscall (and, for any node that *does* already
   exist — e.g. shared secondary-index buckets — a full content read) in favor of a metadata
   check. Smaller than #1/#2 but free and compounds with every node touched.

4. **[LOW impact, confirms a non-problem] Hashing is not worth optimizing.** §3's proxy
   measurement puts content-hashing at 157–160 ns/digest regardless of scale — ~0.03% of
   total time at 100k rows. Do not spend effort here; all the leverage is in syscall count
   (#1–#3), not CPU.

## Growth check — does per-row/edit cost grow with world size?

| Dimension | 1k → 10k → 100k trend | Verdict |
|---|---|---|
| Ingestion rows/s (single batch) | 130.9 → 928.6 → 1,938.4 (but 1k reproduced at 56.5 on rerun — noisy) | No scan signature; if anything, per-row cost *improves* with scale because the fixed ~257-directory `fsync` cost amortizes over more rows. Dominant cost is still fsync-count, which scales ~linearly with objects_written, not super-linearly. |
| One-fact edit latency | 56.9 ms → 72.5 ms → 93.1 ms | Grows slowly, tracking `objects_written`'s O(log₁₆ N) growth (4→5→6 objects) — **not** a scan. Confirms P5's "strictly local" claim holds 10x and 100x past the P5 scale. |
| Network (join+decide) propagation, ingestion | 77.9 ms → 640.9 ms → 7.16 s | Roughly linear in `intermediate_deltas` (5,400 → 54,000 → 540,000, exactly 10x each step); in-memory, never the bottleneck next to store I/O. |
| Network propagation, one-fact edit | 0.37 → 0.44 → 0.67 ms | Flat; confirms strictly local, matches `intermediate_deltas` pinned at 12. |
| bytes_written/row | 194.9 → 197.9 → 200.1 | Flat — no encoding blow-up. |

## 1M feasibility verdict

**Borderline, and not comfortably inside budget as currently built.** I did not run the real
1,000,000-row ingestion probe (`probe_ingest_1m`, present in the harness file but not
invoked in this session). Reasoning for stopping at extrapolation, per the lane brief's own
rule ("if it completes in < ~15 min; otherwise report the extrapolation and stop"):

- Linear extrapolation from the 100k single-big-batch rate (1,938 rows/s) puts
  `session.apply_batch` alone at **≈ 516 s (≈ 8.6 min)**, plus **≈ 72 s** extrapolated for
  `network.apply_batch`, for a sequential total of **≈ 588 s (≈ 9.8 min)**.
- That estimate sits within ~2 minutes of this tool's own hard 600 s (10 min) per-command
  timeout cap, and this session already measured a **2.3x run-to-run variance on identical
  work** at the 1k scale (§0). A 2.3x unlucky swing on the 9.8-minute estimate is ~22.5
  minutes — past both the hard tool cap and the task's 15-minute guidance. Attempting it
  blind risked burning a non-restartable ~10-minute command on a result I could not report
  either way (killed mid-run = no numbers, success = marginal value next to the extrapolation
  already in hand).
- **Conclusion: 1M ingestion as a single bulk batch is plausible in the 9–20+ minute range
  today, which is too slow and too variance-prone to be a comfortable P8 acceptance result.**
  The fsync-per-object-file design (recommendation #1 above) is very likely to be the fix:
  collapsing ~1M+ individual `fsync` calls into O(1) per batch should move 1M-row ingestion
  from "tens of minutes, maybe" to "tens of seconds, reliably," at which point a real,
  confidently-measured 1M run becomes cheap to obtain.
- One-fact edit latency at 1M is **not** the risk: extrapolating `objects_written`'s
  O(log₁₆ N) trend (4, 5, 6 → ~7 at 1M) suggests edit latency would land around
  **100–115 ms**, comfortably bounded and consistent with the P5 structural guarantee.

**Recommendation before attempting a real P8 1M-row run:** land fix #1 (batch the object-file
fsyncs) first, then re-run this harness's `probe_ingest_1m` (already written, `#[ignore]`d,
ready to invoke) to get a real, fast, low-variance 1M number instead of an extrapolation.
