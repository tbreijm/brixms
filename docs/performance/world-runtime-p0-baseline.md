# World Runtime P0 — Cost Baseline and Negative Controls

Status: **Baseline recorded**, 2026-10-03, commit base `44d46ef` (`v0.1.0-alpha.3`).
Governed by `docs/planning/persistent-world-runtime-plan.md` §4 (P0). Contract
text: [ADR-0046](../../spec/adr/ADR-0046_Persistent_World_Runtime.md) (**proposed**, not ratified).

Every number below was copied from real output of the commands in §2. If a
quantity is not here, it was not measured.

## 1. What this baseline does and does not establish

**Establishes:** three existing paths cost more as the world grows, and the
repository's own O(Δ) routing gate is unaffected:

- `ArcMap::insert` copies every entry it holds.
- `Frontier::apply_delta` copies every candidate it holds.
- `brix kb assert` rewrites the whole input snapshot for a one-fact change.

**Does not establish (deferred to P1+):** structural counters — tree nodes
visited/copied, bytes hashed, tuple probes, operator invocations, bytes
allocated. They need the P1 structures to exist; reporting them now would mean
inventing them. The plan's rule "no counter may report zero for unmeasured
work" is why no such counter is included yet. An earlier draft added a
`PhysicalCost` struct that nothing populated; it was removed for that reason.

## 2. Reproduce

```sh
# Deterministic controls (raw measurements printed with --nocapture):
cargo test -p soc-core --test world_scale_negative_controls -- --nocapture --test-threads=1
cargo test -p brix-kb  --test kb_scale_negative_controls    -- --nocapture
# Existing routing gate, unchanged:
cargo test -p soc-core --test o_delta_gate
# Wall clock (bench profile):
cargo bench -p soc-core --bench arc_map_insert
cargo bench -p soc-core --bench world_frontier_delta
```

**Environment:** Apple M1 Pro, 16 GiB RAM, macOS 15.6.1 (arm64),
rustc/cargo 1.96.1 (2026-06-26). Local developer laptop, not an isolated
benchmark rig; treat absolute times as indicative and the growth ratios as the
signal. Tests ran in the `dev` profile, benches in `bench`.

## 3. Measurements

### 3.1 How the deterministic counts are obtained

The control stores a `Counted` value whose `Clone` increments a per-thread
counter. Cloning a `BTreeMap` clones each value, so the count is the number of
entries the operation actually copied — observed, not derived from `len()`.
A harness self-check confirms a single targeted read registers exactly 1 clone
at both N=1,000 and N=4,000, so the counter can distinguish flat from
proportional. (No custom allocator: the workspace sets `unsafe_code = "deny"`.)
This counts value clones only; key clones are not counted.

### 3.2 `ArcMap::insert` (`crates/soc-core/src/store.rs`)

| Store size N | Entry clones for one insert |
|---:|---:|
| 1,000 | 1,000 |
| 2,000 | 2,000 |
| 4,000 | 4,000 |

Wall clock (`arc_map_insert`, one insert at size N, median / p90):

| N | median | p90 | N ratio | time ratio |
|---:|---:|---:|---:|---:|
| 1,000 | 16.708 µs | 17.208 µs | 1× | 1.00× |
| 10,000 | 76.583 µs | 79.541 µs | 10× | 4.58× |
| 100,000 | 978.625 µs | 1.240 ms | 100× | 58.57× |

Time grows sub-linearly in N here (4.6× for 10×, 58.6× for 100×) — more
consistent with cache effects at small N than with a clean O(N) line, so the
deterministic clone count, not the wall-clock ratio, is the evidence of
O(N) work. `get` read 0–41 ns across the same sizes (the bench's timer
resolution, so effectively flat).

### 3.3 `Frontier::apply_delta` (`crates/soc-core/src/calendar.rs`)

One removal plus one addition:

| Frontier size N | Entry clones |
|---:|---:|
| 500 | 501 |
| 1,000 | 1,001 |
| 2,000 | 2,001 |

The `+1` is the added candidate's own clone; the rest is the full-map staging
copy (`let mut staged = self.entries.clone()`).

Wall clock (`world_frontier_delta`; the per-sample frontier clone is made
**before** the timer starts, so only `apply_delta` is timed):

| N | median | p90 | N ratio | time ratio |
|---:|---:|---:|---:|---:|
| 1,000 | 10.625 µs | 12.375 µs | 1× | 1.00× |
| 10,000 | 209.458 µs | 232.083 µs | 10× | 19.71× |
| 50,000 | 983.167 µs | 1.099 ms | 50× | 92.53× |

Time grows faster than N (19.7× for 10×, 92.5× for 50×); this is consistent
with the working set outgrowing cache, not proof of super-linear algorithmic
cost. An earlier draft of this bench timed an extra `clone()` and reported
23.3 µs / 331.5 µs / 1.658 ms; those figures were inflated and are superseded.

### 3.4 `brix kb assert` (`crates/brix-kb/src/ops.rs`)

Bytes of the new snapshot file written by one single-fact `assert` into a KB
already holding N facts (read from the file on disk):

| Resident facts N | Snapshot bytes | ratio to N=25 |
|---:|---:|---:|
| 25 | 970 | 1.00× |
| 50 | 1,920 | 1.98× |
| 100 | 3,820 | 3.94× |

Only snapshot size is measured. Revision-record size, re-validation and replay
cost were not measured.

### 3.5 SOC routing gate (unchanged invariant)

- `o_delta_gate` (`crates/soc-core/tests/o_delta_gate.rs`): all 8 tests pass,
  including the armed gate and its naive-oracle negative control. Its raw work
  units were not re-captured here.
- This P0 test's own fixture (one regime, 1k/2k/4k inert configurations):
  incremental work units **[3, 3, 3]** — flat.

## 4. Status against the P0 exit criteria

| Criterion | State |
|---|---|
| Existing world-proportional paths visibly fail the scale gate | **Met** for `ArcMap::insert`, `Frontier::apply_delta` (deterministic clone counts) and `kb assert` (bytes on disk). |
| Existing routing gate stays green | **Met** (`o_delta_gate` 8/8; fixture flat at 3). |
| Fixtures/meters for tuple probes, operator invocations, nodes visited/copied, bytes hashed/read/written | **Not met.** Deferred to P1, when those structures exist. |
| Reproducible command, environment, raw data | **Met** (§2, §3). |
| Seeds / dataset digests | N/A so far: fixtures are deterministic index-generated, no randomness. Needs revisiting for P3+ generators. |
| Baseline of the real public KB path separate from the routing-only test | **Partly met**: bytes written only. |

P0 is therefore **partially complete**: contract draft and baseline controls
are in place; the structural meters are not.
