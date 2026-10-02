# BrixMS

[![CI](https://github.com/tbreijm/brixms/actions/workflows/ci.yml/badge.svg)](https://github.com/tbreijm/brixms/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/tbreijm/brixms?include_prereleases&sort=semver&label=release)](https://github.com/tbreijm/brixms/releases)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](./LICENSE)

BrixMS is an experimental language and runtime for making decisions from
incomplete or changing information while preserving what was considered, what
was chosen, and what evidence supports the result. It is in beta development;
the current executable language surface is useful for bounded policy and
settlement workflows, while broader proof and production guarantees remain
in progress.

## Why BrixMS

Most programs return an answer. BrixMS also represents the candidates behind
that answer, the rules and inputs that shaped it, and the boundary between a
runtime decision and later independent verification. This helps when a result
may need to be explained, replayed, or revised as facts change.

The runtime deliberates over admissible candidates and commits deterministically.
A decision made during execution is `@Derived`; successful independent replay
can produce a separate `@Audited` receipt. Unsupported work, incomplete search,
resource exhaustion, and failed replay remain `Unknown`. Evidence is never
silently promoted.

BrixMS continues Tony Reijm's 2019 Brix formalism and its earlier open-source
implementations, while this repository is a ground-up Rust rebuild around
Settlement-Oriented Computing (SOC). The
[scientific article outline](./docs/BrixMS_Scientific_Article_Outline.md)
documents the lineage and prior art; this implementation does not claim source
compatibility with the legacy engines.

## Current capabilities

The source checkout currently supports:

- finite decision programs with typed rules, proposals, explicit commit pools,
  multiple independent decisions, and per-entity `decide` blocks;
- bounded list inputs and finite relations, including joins, comprehensions,
  filtering, mapping, membership, and bounded folds;
- explicit `F64` arithmetic for finite approximate measurements and `Decimal`
  arithmetic for exact base-10 values, including explicit rounded decimal
  division;
- strict, tagged JSON inputs, including structured values and bounded lists;
- `check`, `run`, `audit`, `verify`, `why`, `whynot`, and regression `test`
  commands;
- a persistent, revisable knowledge base (`brix kb`) with replay-checked
  history;
- a JSON-lines server (`brix serve --stdio`) and a small standard-library
  Python client;
- a Rust runtime with deterministic settlement, an incremental engine, a
  recomputation oracle, and a small proof kernel.

The parser recognizes some syntax whose downstream execution profile is not
implemented. Such programs are rejected before execution. Arithmetic and
comparison are not all kernel-proven, certified refutation is not available,
and the system does not claim general termination or production readiness.
See the [beta contract](./spec/Beta_Contract.md) and
[semantic status ledger](./spec/SOC_Semantic_Laws.md) for precise evidence and
scope.

## Try it from source

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

With stock at 12, `expedite` is ineligible and `ship` wins over `hold` because
it has the lower priority number. The result retains the candidate dispositions
so you can ask why a choice won or lost.

Install Rust using [rustup](https://rustup.rs/). The repository pins its Rust
toolchain in [`rust-toolchain.toml`](./rust-toolchain.toml).

```bash
git clone https://github.com/tbreijm/brixms.git
cd brixms
cargo build -p brix-cli
cargo run -p brix-cli -- check examples/shipping.brix
cargo run -p brix-cli -- run examples/shipping.brix
```

`check` validates the program and its plan. `run` deliberates and prints the
facts, candidate dispositions, and committed decision. Both commands work with
the finite-decision profile demonstrated in
[`examples/shipping.brix`](./examples/shipping.brix).

For independent replay, create and verify an audit bundle:

```bash
cargo run -p brix-cli -- audit examples/shipping.brix \
  --bundle /tmp/shipping.brixaudit --force
cargo run -p brix-cli -- verify \
  --expect-program 3a815590c807a8af7e7756d8f0edef99a4938e15830de282b24949fe88ba0d5e \
  examples/shipping.brix /tmp/shipping.brixaudit
```

The runtime decision remains `@Derived`; `verify` independently replays the
bundle and checks the separate audit evidence. Use `cargo run -p brix-cli -- --help`
for the complete command syntax.

### External inputs

Declare values in Brix and provide them in a strict tagged JSON file. For
example, [`examples/shipping-input.brix`](./examples/shipping-input.brix)
accepts the values in [`examples/shipping-input.json`](./examples/shipping-input.json):

```bash
cargo run -p brix-cli -- check examples/shipping-input.brix \
  --input examples/shipping-input.json
cargo run -p brix-cli -- run examples/shipping-input.brix \
  --input examples/shipping-input.json
```

Input shards can be supplied with repeated `--input` flags when their keys are
disjoint. The schema version is explicit: `brix.input@1` covers scalar values,
`@2` structured records and variants, `@3` bounded top-level lists, and `@4`
adds `F64` and `Decimal`. Numeric payloads are strings so JSON transport does
not round them. Inputs enter as `@Derived`; they do not establish audited or
proven facts by themselves.

### Numeric policies

Use `F64` for approximate measurements and `Decimal` for exact base-10
arithmetic. The domains do not mix implicitly. `F64` rejects non-finite values
and results; `Decimal` uses checked arithmetic and explicit rounding when a
quotient is not exact. Decimal values normalize trailing zeros, so display
padding is not preserved.

The complete example calculates speed, tax, and an installment:

```bash
cargo run -p brix-cli -- run examples/numeric-policy.brix \
  --input examples/numeric-policy.json
```

See [explicit numeric arithmetic in the language guide](./docs/brix-language.md#explicit-floating-point-and-decimal-arithmetic-adr-0045)
and [ADR-0045](./spec/adr/ADR-0045_Explicit_Numeric_Arithmetic.md) for the
supported operations, rounding modes, and limits.

### Lists, relations, and per-entity decisions

Inputs may contain bounded lists of scalars, records, or variants. Programs can
join lists using finite comprehensions, derive bounded lists, and make one
decision per item with `decide`. The examples
[`fulfillment.brix`](./examples/fulfillment.brix) and
[`order-book.brix`](./examples/order-book.brix) show these patterns with
regression cases. Limits on input size, derived list size, and evaluator work
are enforced; resource exhaustion fails closed.

## Knowledge base and Python embedding

`brix kb` stores a revisable sequence of assertions and program versions. Its
`log`, `show`, and `diff` operations inspect revisions; `audit` and `verify`
support replay-checked history. Run `brix --help` for the operation forms.

To embed Brix in another process, start `brix serve --stdio`. The protocol
exchanges one JSON request and response per line and exposes the CLI operations
without human-output parsing. The
[Python client](./bindings/python/README.md) requires Python 3.9 or newer and
has no third-party runtime dependency:

```bash
python3 -m pip install -e ./bindings/python
```

Run from the source checkout after building the CLI:

```python
from brix import BrixClient, path

with BrixClient(brix_bin="./target/debug/brix") as client:
    result = client.run(program=path("examples/shipping.brix"))
    print(result["status"], result["decision"])
```

The `brix` executable must be built or installed separately. See
[ADR-0044](./spec/adr/ADR-0044_Serve_Protocol.md) for the protocol contract.

## Releases

The current workspace version is `0.1.0-alpha.3`. Its prerelease archives
target macOS Apple Silicon (`aarch64-apple-darwin`) and Linux x86_64
(`x86_64-unknown-linux-gnu`); each has a SHA-256 sidecar. Download the exact
tagged asset from the [GitHub Releases page](https://github.com/tbreijm/brixms/releases):

```bash
VERSION=v0.1.0-alpha.3
TARGET=x86_64-unknown-linux-gnu # or aarch64-apple-darwin
curl -LO "https://github.com/tbreijm/brixms/releases/download/${VERSION}/brix-${VERSION}-${TARGET}.tar.gz"
curl -LO "https://github.com/tbreijm/brixms/releases/download/${VERSION}/brix-${VERSION}-${TARGET}.tar.gz.sha256"
sha256sum -c "brix-${VERSION}-${TARGET}.tar.gz.sha256" # macOS: shasum -a 256 -c "brix-${VERSION}-${TARGET}.tar.gz.sha256"
tar -xzf "brix-${VERSION}-${TARGET}.tar.gz"
cd "brix-${VERSION}-${TARGET}"
./brix run examples/shipping.brix
./brix run examples/numeric-policy.brix --input examples/numeric-policy.json
```

Release archives include the CLI, README, and the `shipping`,
`shipping-input`, and `numeric-policy` examples. Other examples linked above
are available in the source checkout. Always consult the README and examples
bundled with a downloaded release when checking what that archive supports.

## Documentation

- [Language guide](./docs/brix-language.md) — syntax, executable profiles,
  types, inputs, arithmetic, lists, and decisions.
- [Beta contract](./spec/Beta_Contract.md) — what the beta claims and does not
  claim.
- [Semantic laws](./spec/SOC_Semantic_Laws.md) and
  [type realization contract](./spec/Type_Realization_Contract.md) — evidence
  status and conformance scope.
- [Beta plan](./docs/planning/beta-plan.md) and
  [roadmap](./docs/planning/beta-roadmap.md) — planned work and open questions.
- [Contributing](./CONTRIBUTING.md) — development discipline and specification
  workflow.

## Development

Build or run the CLI with Cargo. The workspace's contribution gates are
documented in [`CONTRIBUTING.md`](./CONTRIBUTING.md); the CI workflow runs
formatting, lint, tests, determinism, conformance, and artifact checks.

## License

BrixMS is licensed under [Apache-2.0](./LICENSE).
