# BrixMS

[![CI](https://github.com/tbreijm/brixms/actions/workflows/ci.yml/badge.svg)](https://github.com/tbreijm/brixms/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/tbreijm/brixms?include_prereleases&sort=semver&label=release)](https://github.com/tbreijm/brixms/releases)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](./LICENSE)

BrixMS is an experimental language and runtime for making decisions from
incomplete or changing information while keeping a record of what was
considered, what was chosen, and what evidence supports the result. It runs
small policies once from the command line, and it keeps large, long-lived
decision worlds up to date as facts change, doing work proportional to the
change rather than to the size of the world.

It is pre-release software (`0.1.0-alpha.5`): useful for bounded policy and
settlement workflows, with broader proof and production guarantees still in
progress. See [Status and limits](#status-and-limits).

## The idea

Most programs return an answer. A Brix program returns a *settlement*: the
answer together with the candidates that competed for it, the rules and inputs
that made each candidate eligible or not, and the reason the winner won. That
makes a result explainable (`why`, `whynot`), replayable, and revisable when
the facts behind it change.

The runtime is explicit about how much each result is worth:

- A decision made during execution is `@Derived`.
- Independent replay of an audit bundle by a separate verifier can produce an
  `@Audited` receipt alongside it.
- Unsupported work, incomplete search, resource exhaustion, and failed replay
  are `Unknown`, never a guess.

Evidence is never silently promoted, and caches never become authority. The
model behind this is Settlement-Oriented Computing (SOC). BrixMS continues Tony
Reijm's 2019 Brix formalism and its earlier open-source implementations; this
repository is a ground-up Rust rebuild and does not claim source compatibility
with the legacy engines. The
[scientific article outline](./docs/BrixMS_Scientific_Article_Outline.md)
covers lineage and prior art.

## A first decision

This is the complete [`shipping.brix`](./examples/shipping.brix) program:

```brix
config Decision = Expedite | Ship | Hold

rule stock() = 12
rule threshold() = 10

propose expedite(stock) priority 5 when stock >= 50 = Expedite
propose ship(stock, threshold) priority 10 when stock >= threshold = Ship
propose hold() priority 100 when true = Hold

commit shipping from (expedite, ship, hold)
show shipping
```

With stock at 12, `expedite` is ineligible, and `ship` wins over `hold`
because it has the lower priority number. The result keeps every candidate's
disposition, so you can ask why a choice won or lost.

Install Rust with [rustup](https://rustup.rs/); the toolchain is pinned in
[`rust-toolchain.toml`](./rust-toolchain.toml). Then build and run from source:

```bash
git clone https://github.com/tbreijm/brixms.git
cd brixms
cargo build -p brix-cli
cargo run -p brix-cli -- check examples/shipping.brix
cargo run -p brix-cli -- run examples/shipping.brix
```

`check` validates the program and its plan. `run` deliberates and prints the
facts, the candidate dispositions, and the committed decision
(`shipping = Ship @Derived`).

To have the result checked independently, write an audit bundle and verify it
against a program digest you obtained separately:

```bash
cargo run -p brix-cli -- audit examples/shipping.brix \
  --bundle /tmp/shipping.brixaudit --force
cargo run -p brix-cli -- verify \
  --expect-program 3a815590c807a8af7e7756d8f0edef99a4938e15830de282b24949fe88ba0d5e \
  examples/shipping.brix /tmp/shipping.brixaudit
```

The runtime decision stays `@Derived`; `verify` replays the bundle and checks
the separate audit evidence. `cargo run -p brix-cli -- --help` lists the full
command syntax, including `why`, `whynot`, and regression `test` suites.

## Writing real policies

Real policies take their facts from outside the program. Declare the values in
Brix and supply them as strict, tagged JSON
([`shipping-input.brix`](./examples/shipping-input.brix) with
[`shipping-input.json`](./examples/shipping-input.json)):

```bash
cargo run -p brix-cli -- run examples/shipping-input.brix \
  --input examples/shipping-input.json
```

The input schema is versioned explicitly: `brix.input@1` covers scalars, `@2`
records and variants, `@3` bounded top-level lists, and `@4` adds `F64` and
`Decimal`. Numeric payloads are strings so JSON transport cannot round them.
Repeated `--input` flags combine shards with disjoint keys. Inputs enter as
`@Derived`; they do not establish audited or proven facts by themselves.

On top of that, the language provides:

- **Explicit numerics.** `F64` for approximate measurements and `Decimal` for
  exact base-10 arithmetic, with no implicit mixing, checked operations, and
  explicit rounding for inexact decimal division
  ([`numeric-policy.brix`](./examples/numeric-policy.brix),
  [ADR-0045](./spec/adr/ADR-0045_Explicit_Numeric_Arithmetic.md)).
- **Bounded lists and finite relations.** Joins, comprehensions, filtering,
  mapping, membership, and bounded folds over input lists.
- **Several decisions per program.** Multiple commit pools, and per-entity
  `decide` blocks that make one decision per list element
  ([`fulfillment.brix`](./examples/fulfillment.brix),
  [`order-book.brix`](./examples/order-book.brix), both with regression
  suites).

Limits on input size, derived list size, and evaluator work are enforced;
resource exhaustion fails closed. The [language guide](./docs/brix-language.md)
documents the syntax, types, and executable profiles.

## From one run to a living world

`brix run` evaluates a program from scratch every time. That is fine for one
policy over one input file, but not for a service whose facts change
continuously: re-deciding a million orders because one stock level moved is
wasted work. BrixMS has two persistent modes:

- `brix kb`, the earlier mode, stores a revisable history of named inputs and
  program versions with replay-checked `log`, `show`, `diff`, `audit`, and
  `verify`. It rebuilds the result on each revision.
- `brix world` (alpha.4 onward) is the persistent, incremental world runtime
  ([ADR-0046](./spec/adr/ADR-0046_Persistent_World_Runtime.md)). It is the
  direction of the project and the focus of the current releases. An existing
  knowledge base can be converted with `brix world import-kb`.

A world is a directory bound to one program. The program declares **keyed
relations** for incoming facts, **derived relations** built from them with
`select ... from ... where` joins, and per-entity **decisions** over them;
programs can be split into linked modules with `use`. The runtime compiles this
into an **operator network** (joins, distinct sets, grouped counts, a
candidate frontier) that it maintains incrementally.

Facts arrive in **batches**: a `brix.world.batch@1` file of upserts and removes
with an expected base revision and an idempotency key. A batch commits
atomically: new state is written and synced, then HEAD is switched with a
rename, so a crash leaves either the old revision or the complete new one.
Commit work is proportional to the facts changed and the results they affect
(ADR-0046's O(Δ) contract), not to the number of resident facts. Every revision
is content-addressed, and `brix world verify` re-derives the whole world from
the batch journal and the program, without trusting any runtime cache.

A complete round trip with the linked example in
[`examples/world-linked`](./examples/world-linked/), from the source checkout
after `cargo build -p brix-cli`:

```bash
cat > /tmp/orders-batch.json <<'EOF'
{
  "schema": "brix.world.batch@1",
  "expected_base_revision": 0,
  "idempotency_key": "seed-1",
  "operations": [
    {"op": "upsert", "relation": "policy::orders", "key": "order-1",
     "tuple": {"id": "order-1", "customer": "Ada", "sku": "sku-1", "qty": 2, "express": "yes"}},
    {"op": "upsert", "relation": "policy::inventory", "key": "inv-1",
     "tuple": {"id": "inv-1", "sku": "sku-1", "available": 9}},
    {"op": "upsert", "relation": "policy::shipping", "key": "ship-1",
     "tuple": {"id": "ship-1", "sku": "sku-1", "carrier": "ParcelCo"}}
  ]
}
EOF
BRIX=./target/debug/brix
$BRIX world init /tmp/orders-world examples/world-linked/main.brix
$BRIX world batch /tmp/orders-world /tmp/orders-batch.json
$BRIX world query /tmp/orders-world policy::orders
$BRIX world decide /tmp/orders-world     # order-1 -> ship_express (air_express)
$BRIX world explain /tmp/orders-world order-1 --decide main::dispatch
```

`explain` lists the candidates for the entity and the base facts that support
the winner. To verify independently, export an audit bundle and check it
against head and program digests obtained from a source you trust. Revisions
are deterministic, so this walkthrough always yields the digests below;
`world audit` prints `head_digest` and `world show --json` reports
`program_digest`.

```bash
$BRIX world audit /tmp/orders-world --out /tmp/orders.brixworld
$BRIX world verify \
  --expect-head 7845335831fec64e7f7c41825105826dfa93f87911fdd07d8076f9cd4f170a97 \
  --expect-program 4f0394088b26f86a7bdad62077632d115398149a04a5639052c23daf864cb3ce \
  /tmp/orders.brixworld
```

A successful verification reports `status: audited` together with the work it
did to re-derive the result.

## alpha.5: opening a large world without replay

In alpha.4, opening a world rebuilt the operator network by replaying every
resident fact, so a process restart cost about as much as loading the world:
about 1 s at 10k facts and 11 s at 100k. alpha.5 removes that cost.

- **Restore instead of replay.** The operator network's state is persisted as
  content-addressed tries and restored on open. Restored state is identical to
  a full replay (checked by differential fuzzing, negative controls, and
  crash-point tests).
- **State is a cache, never authority.** Each revision records the root of its
  network state. If a revision has no restorable state (a world written by
  alpha.4, or a change of state schema), open falls back to replay once and
  caches the result. State that should be there but is damaged or missing is a
  hard error (`corrupt-state`, `missing-state`, `state-mismatch`), never a
  different answer. `--replay` forces the fallback, and `world verify` still replays
  from the journal.
- **Fast decision reads.** A settlement checkpoint lets the first read of all
  decisions in a fresh process skip the frontier walk. It is validated against
  the revision's decision root and brought forward from per-revision deltas;
  it is never written at commit time, so commits stay O(Δ).
- **Garbage collection.** `brix world gc <dir> [--dry-run] [--keep-revisions N]`
  removes nodes no retained revision can reach, crash-safely and under a
  single-writer lock. By default every revision stays readable.
- **Health checks.** `brix world open-check <dir>` reports how a world opens;
  `--verify-state` compares restored state against a replay, `--verify-store`
  checks every stored node's checksum, and `--refresh-state` rebuilds caches
  ahead of time.

Measured on a synthetic benchmark, release build, one machine:

| Resident facts | alpha.4 cold open | alpha.5 cold open |
|---|---|---|
| 10k | ~1 s | ~3 ms |
| 100k | ~10.9 s | ~14 ms |
| 1M | not measured | ~140 ms |

At 1M facts (800k decisions), the first full decisions read in a fresh
process takes about 0.6 s with the checkpoint. On the 100k benchmark world, GC
reclaimed about 40% of stored nodes.

Known limits in this release:

- Cold open is fast but not constant; it still grows slowly with world size.
- Bulk loading still slows as the world grows: late batches cost several
  times more per row than early ones at 300k to 1M facts. The remaining cost
  is node-cache misses during incremental maintenance, not replay.
- The first decisions read on a world that has no checkpoint yet walks the
  frontier once (several seconds at 1M) and writes the checkpoint;
  `open-check --refresh-state` moves that off the request path.
- A world stores its operator state as well as its facts, so it is several
  times larger on disk than in alpha.4 (the 100k benchmark world: about 86 MB
  in alpha.4, about 490 MB after loading, about 300 MB after `brix world gc`).
  Roughly half of what remains is per-node framing in the pack format. The
  checkpoint is a second copy of the decisions and is not GC-managed.
- The replay fallback is still memory-hungry on large worlds (about 4 GB at
  300k facts); opt-in chunking (`apply_hydrated_bytes` in the manifest) bounds
  memory at the cost of some time and disk; see the release notes.
- A session open across a GC keeps committing safely (it reloads its pack set
  on the next write), but may serve pre-GC cached nodes for reads until
  reopened. Concurrent readers in other processes are not supported while a GC
  runs, and `--keep-revisions` cannot see reader pins held by other processes.

Embedders should read the
[alpha.5 release notes](./docs/planning/alpha5-release-notes.md) for API and
schema changes (revision schema `brix.world.revision@3`, `Result`-returning
query and storage APIs, new open fields).

## Embedding

To use BrixMS from another process, run `brix serve --stdio`. It exchanges one
JSON request and one JSON response per line (`brix.serve@1`) and exposes the
CLI operations, including `world.*` (`world.batch`, `world.decisions`,
`world.open-check`, `world.gc`, and the rest), without parsing human output.
See [ADR-0044](./spec/adr/ADR-0044_Serve_Protocol.md).

The [Python client](./bindings/python/README.md) needs Python 3.9 or newer and
has no third-party runtime dependency. It wraps the one-shot and `kb`
operations; other methods go through `call`. The `brix` executable is built or
installed separately:

```bash
python3 -m pip install -e ./bindings/python
```

```python
from brix import BrixClient, path

with BrixClient(brix_bin="./target/debug/brix") as client:
    result = client.run(program=path("examples/shipping.brix"))
    print(result["status"], result["decision"])

    # The world created in the walkthrough above.
    world = client.call("world.decisions", {"dir": "/tmp/orders-world"})
    print(world["result"]["settlements"])
```

Rust services can link the `brix-kb` crate and keep a `WorldSession` open
across batches (`WorldSession::open`, `apply_batch`, `all_settlements`), which
avoids reopening the world per request.

## Status and limits

- The [beta contract](./spec/Beta_Contract.md) and the
  [semantic status ledger](./spec/SOC_Semantic_Laws.md) state precisely what is
  claimed and with what evidence.
- ADR-0046, which defines the world runtime, is still a draft; its schemas
  are versioned, but the world program syntax and runtime surface may change
  between alpha releases. The language guide does not yet document the world
  syntax; [`examples/world-linked`](./examples/world-linked/) is the reference
  example.
- The parser recognizes some syntax whose execution profile is not
  implemented; such programs are rejected before execution.
- Arithmetic and comparison are not all kernel-proven, certified refutation is
  not available, and the system does not claim general termination or
  production readiness.

## Install a release

Prerelease archives target macOS Apple Silicon (`aarch64-apple-darwin`) and
Linux x86_64 (`x86_64-unknown-linux-gnu`), each with a SHA-256 sidecar. Pick a
tag from the [GitHub Releases page](https://github.com/tbreijm/brixms/releases)
(the current one is `v0.1.0-alpha.5`):

```bash
VERSION=v0.1.0-alpha.5
TARGET=x86_64-unknown-linux-gnu # or aarch64-apple-darwin
curl -LO "https://github.com/tbreijm/brixms/releases/download/${VERSION}/brix-${VERSION}-${TARGET}.tar.gz"
curl -LO "https://github.com/tbreijm/brixms/releases/download/${VERSION}/brix-${VERSION}-${TARGET}.tar.gz.sha256"
sha256sum -c "brix-${VERSION}-${TARGET}.tar.gz.sha256" # macOS: shasum -a 256 -c "brix-${VERSION}-${TARGET}.tar.gz.sha256"
tar -xzf "brix-${VERSION}-${TARGET}.tar.gz"
cd "brix-${VERSION}-${TARGET}"
./brix run examples/shipping.brix
./brix run examples/numeric-policy.brix --input examples/numeric-policy.json
```

An archive contains the CLI, this README, the license, and the `shipping`,
`shipping-input`, and `numeric-policy` examples. The other examples, including
`world-linked`, are in the source checkout. The README bundled with an archive
describes what that archive supports.

## Documentation

- [Language guide](./docs/brix-language.md): syntax, executable profiles,
  types, inputs, arithmetic, lists, and decisions.
- [ADR-0046](./spec/adr/ADR-0046_Persistent_World_Runtime.md): the persistent
  world runtime; [R0 cold-open spec](./docs/planning/r0-cold-open-spec.md) and
  [design and results](./docs/planning/r0-phase2-design.md) for alpha.5.
- [Beta contract](./spec/Beta_Contract.md),
  [semantic laws](./spec/SOC_Semantic_Laws.md), and
  [type realization contract](./spec/Type_Realization_Contract.md): claims,
  evidence status, and conformance scope.
- [Performance](./docs/performance.md): measured benchmarks and their
  interpretation.
- [Beta plan](./docs/planning/beta-plan.md) and
  [roadmap](./docs/planning/beta-roadmap.md): planned work and open questions.

## Development

Build and run the CLI with Cargo. The contribution gates are documented in
[`CONTRIBUTING.md`](./CONTRIBUTING.md); CI runs formatting, lint, tests,
determinism, conformance, and artifact checks.

## License

BrixMS is licensed under [Apache-2.0](./LICENSE).
