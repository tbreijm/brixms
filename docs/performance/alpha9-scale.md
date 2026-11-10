# Alpha.9 scale qualification

Status: **not yet qualified**. The runner and fixture are implemented, but the alpha.9
qualification matrix has not completed. The release gate needs five valid
repetitions for every size/profile combination and all three source counts.

## Workload and claim boundary

The benchmark example generates a deterministic, public Atlas-shaped fixture
at 8,000, 80,000 and 800,000 source facts. It includes a 5,000-child hot group,
balanced event groups, overlapping union-distinct inputs, a nested-field join,
decisions and a partition-scoped named query. The exact fact distribution and
digest are recorded in [`alpha9-fixture.md`](alpha9-fixture.md) and in every
measurement record. No actual Atlas source dataset was available, so results
must not be presented as Atlas-model measurements or as a direct performance
comparison with Atlas's reported alpha.8 figures.

Both a one-commit load and a ten-commit load are measured. The proposed storage
target applies only to the ten-commit history, immediately after its load and
before incremental edit commits:

```text
maximum logical world bytes = 16 MiB + 12 KiB × source fact count
```

The ten-commit `child-load` JSON record reports logical and allocated bytes by
storage component at that point. Later edit records append history; they do not
change the captured budget measurement and the runner never compacts or removes
revisions inside a measured world. A budget miss is not a pass unless an
explicit revision is accepted.

## Capture phases

`scripts/alpha9_scale_capture.py` runs each repetition sequentially in a unique
scratch directory, with only one benchmark world present at a time:

1. `--full` creates an empty world and records initialization.
2. `--child-load DIR` inserts the fixture in one or ten commits and records
   load time, commit counters, counts and storage before edit history.
3. `--child-open DIR`, `--child-query DIR` and `--child-query-warm DIR` run in
   separate fresh processes against that exact loaded world. Warm-query mode
   performs one untimed pagination pass, then times its measured pass.
4. `--child-verify DIR` checks the loaded store, oracle and derived counts
   before edits.
5. `--child-edit DIR add` and `--child-edit DIR retract` each run in separate
   processes and report their commit timing and available counters.
6. A final `--child-verify DIR` checks the edited world.

The capture emits one JSONL record for each process phase plus cleanup. Each
record retains its command, release binary path and SHA-256, commit/toolchain
metadata from the example, fixture identity, platform, elapsed wall time,
launcher-inclusive Python-spawn-to-Rust-main-entry time, peak RSS, free-space
snapshots, exit state, and the example's JSON. On macOS peak RSS comes from an
isolated Python resource launcher reading the one measured child's
`RUSAGE_CHILDREN.ru_maxrss` value in bytes. This avoids `ps`; in the restricted
benchmark shell `/usr/bin/time -l` exits after a denied `sysctl kern.clockrate`
call. The compatibility parser accepts the platform's `NUMBER maximum resident
set size` output order. If RSS or the Rust main-entry timestamp is absent, the
record is marked incomplete and is not a valid sample.

Child output goes to bounded temporary files rather than undrained pipes, so a
large diagnostic stream cannot block the child. A run is refused if scratch
space is below 3 GiB; if it falls below that floor during a phase, the process
group is stopped and the failure is retained. Timeout, OOM/signal, failed
correctness, missing measurements and cleanup failure are also retained. No
retry replaces a failed sample. The runner only removes its own unique
per-repetition directory after all child processes have ended; cleanup failures
are recorded.

The resource launcher records each phase in a separate process: initialization,
load, open, query, verification and edits are isolated. Open and both query
passes run against the loaded world before the standalone correctness check;
that check runs before edit measurements. Warm query follows the separate fresh
query process. The OS cache is labeled **unknown**; the runner does not claim a
cleared cache. Rust main-entry latency is reported from the parent process
spawn timestamp and includes launcher overhead. The harness does not estimate
startup by subtracting world-open latency.

Query records include work units, matches and expressions evaluated, plus
first-page/full rows and bytes and canonical content digests for all four
derived relations. The report checks these content measurements across fresh
and warm query processes and repetitions. Edit records include commit counters
for operator work and state I/O/objects. These are the counters exposed by the
actual query/commit APIs; no work is inferred from elapsed time.
Machine model and installed memory remain unavailable unless supplied by the
operator. The root benchmark operator has separately obtained host metadata
for this machine (Apple M1 Pro, 10 CPUs, 16 GiB RAM, macOS 15.6.1 build 24G90,
`rustc 1.96.1 (31fca3adb 2026-06-26)`). Capture accepts this as an explicit
JSON file with `--host-metadata`; it records the supplied values and their
digest without invoking restricted system-inspection commands. Omitting the
option leaves model and memory unavailable.

## Reproduction

Build the release example into a private target directory, then capture the
matrix sequentially. The runner creates and removes one world at a time:

```sh
mkdir -p docs/performance/raw
BRIX_BENCH_BUILD_COMMIT="$(git rev-parse HEAD)" \
  CARGO_TARGET_DIR=/private/tmp/brix-a9-7-target \
  cargo build --release -p brix-kb --example alpha9_scale_bench
python3 scripts/alpha9_scale_capture.py \
  /private/tmp/brix-a9-7-target/release/examples/alpha9_scale_bench \
  docs/performance/raw/alpha9-scale-all.jsonl \
  --sizes 8000 80000 800000 --profiles single ten --repetitions 5 \
  --host-metadata docs/performance/alpha9-host.json --allow-large
```

The checked-in [`alpha9-host.json`](alpha9-host.json) records the operator-
provided machine/toolchain metadata and its collection method.

After all runs finish, validate and aggregate the raw capture without
discarding failed records:

```sh
python3 scripts/alpha9_scale_report.py \
  docs/performance/raw/alpha9-scale-all.jsonl \
  --output docs/performance/raw/alpha9-scale-summary.json
```

The summary reports per-phase sample counts, medians and ranges, all retained
failures, the fixture/build identity checks, and the ten-revision logical
storage gate. It exits nonzero unless every one of the six size/profile cells
has five complete repetitions and all consistency and storage checks pass.

`--allow-large` is an explicit command-line guard for 80k/800k runs; the
resource controller remains active and larger sizes are processed sequentially.
Each repetition uses a new
world directory; the harness does not leave multiple fixture worlds resident.

## Prior exploratory runs and recovery

The initial 8k/80k rehearsals and partial 800k run used an alpha.8-version
build and were excluded from qualification. The partial run was stopped when
the package-version audit identified that mismatch. Temporary-file cleanup
after interrupted sessions removed those exploratory JSONL files; their raw
measurements cannot be reproduced from the remaining logs and are not
presented as qualification evidence.

The recovery restored all 619 source, test, fixture and documentation files
from the saved candidate, checking every file against its retained Git blob
identity. Only the three unqualified exploratory JSONL files were unavailable.
The capture and report reject any package version other than alpha.9. The
qualification matrix below must therefore be newly captured, with all failures
retained, on the recovered alpha.9 candidate.

## Results

No alpha.9 qualification measurements are available yet. Publish the raw JSONL path, exact release
binary hash and a summary of all 30 size/profile/repetition cells. Qualification
requires at least five valid repetitions per cell; timeout, OOM, resource,
correctness, cleanup or missing-measurement records remain visible and prevent
that cell from having five valid samples.
