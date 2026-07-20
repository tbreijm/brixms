# BrixBuilder-4B

BrixBuilder is a small local coder–tester–reviewer team for BrixMS packages.
Qwen supplies intent, synthesis, and repair; the checked-in `brix` compiler is
the authority. The host records an in-memory candidate and runs every tool in a
temporary package tree. It never writes the proposed patch into the live package.

## What works in this baseline

- strict Pydantic JSON actions, one action per model turn;
- one MLX model reused with separate coder, tester, and reviewer role prompts;
- an 8K working-context ceiling with targeted source retrieval;
- package-local `project_context`, `find`, `inspect`, and ranged source reads;
- compiler-backed `check`, canonical-format checking, and package `build` gates;
- public `test` and `quality` gates that compiler-check first and fail closed
  with structured evidence until their dedicated engines are implemented;
- candidate diff and conservative lexical impact reports;
- a Codex-like interactive terminal and a mode for one-shot tasks;
- a mode for a permission-restricted local Unix-socket sidecar;
- a host-owned evidence ledger and final acceptance verdict;
- a durable, resumable ticket queue that runs two explicit Qwen roles
  (coder, critic) per ticket, with per-ticket candidate isolation, bounded
  repair loops, duplicate-action detection, write allowlists, a single-writer
  worker lock, and a fail-closed acceptance verdict -- see "Ticket loop" below.

The current Ring 0 compiler does not yet expose resolved semantic graph facts,
test execution, quality rule-pack evaluation, or semantic diff/impact verbs.
The public `brix test` and `brix quality` commands preserve compiler diagnostics,
then report their missing engines as structured fail-closed diagnostics. The
sidecar labels those results `unavailable` (and lexical diff/impact as partial),
so a real run cannot claim `Validated candidate` until the toolchain implements
the complete oracles. It never substitutes prose or a generic shell command for
a missing oracle.

## Install

From this directory on Apple silicon:

```sh
python3 -m venv .venv
. .venv/bin/activate
python -m pip install --upgrade pip
python -m pip install -e '.[mlx,dev]'
cargo build --manifest-path ../../Cargo.toml -p brix-cli
```

The model is downloaded by MLX LM on first use. To test an already-running local
MLX OpenAI-compatible server instead, select `--backend server`.

## Run

Point `--root` at a BrixMS package, not at the toolchain repository:

```sh
./scripts/run-local.sh \
  --root /path/to/my-brix-package \
  chat
```

One task with machine-readable output:

```sh
./scripts/run-local.sh \
  --root /path/to/my-brix-package \
  run --json "Add a query for open orders without adding capabilities"
```

With a QLoRA adapter:

```sh
./scripts/run-local.sh \
  --root /path/to/my-brix-package \
  --adapter adapters/brix-builder-v0 \
  chat
```

With an MLX server:

```sh
mlx_lm.server --model mlx-community/Qwen3.5-4B-MLX-4bit --port 8080
./scripts/run-local.sh --root /path/to/package --backend server chat
```

Run `brix-builder ... doctor` to see which authoritative gates the current
compiler revision provides. `schema` prints the exact JSON Schema to use for
replay generation and later QLoRA data.

After collecting compiler-validated JSONL data, run `scripts/train.sh`. Its
defaults are batch size 1, four adapted layers, gradient accumulation 8,
gradient checkpointing, prompt masking, and an 8K sequence ceiling. Environment
variables `BRIX_BUILDER_MODEL`, `BRIX_BUILDER_DATA`, `BRIX_BUILDER_ADAPTER`, and
`BRIX_BUILDER_ITERS` override the defaults. `scripts/evaluate-adapter.sh` runs
the held-out MLX test split; package-level acceptance still comes from the
sidecar's compiler evidence, not that loss value.

## Sidecar protocol

```sh
./scripts/run-local.sh --root /path/to/package serve \
  --socket /tmp/brix-builder.sock
```

The socket is mode `0600`. Send one newline-terminated object per connection:

```json
{"brief":"Create a reusable approval workflow package"}
```

The response contains the status, evidence, unresolved gates, and proposed diff.
There is deliberately no apply, publish, production-boundary, or arbitrary-shell
operation in the protocol.

## Ticket loop

For a durable, resumable worker instead of one interactive session, enqueue
scoped tickets and run them with two separate model roles: a coder and an
independent critic. The host remains the sole check/format/build authority --
a critic's self-reported verdict never overrides a failing or unavailable
gate.

```sh
# Warm the model once (keeps weights resident overnight):
./scripts/serve-model.sh

# Enqueue, then drain against the warm server -- no per-ticket MLX startup:
./scripts/run-local.sh --root /path/to/my-brix-package \
  enqueue --from-file tickets/seeds/math-abs-fn.json

./scripts/run-local.sh --root /path/to/my-brix-package \
  --backend server --endpoint http://127.0.0.1:8080/v1 \
  loop

# Or one-shot overnight helper (starts server if needed, nohup's the loop):
./scripts/overnight-loop.sh
```

Coder and critic share one backend when they use the same model/endpoint, so an
in-process MLX run also loads weights only once. Prefer `--backend server` for
overnight work: `serve-model.sh` pays startup once; every ticket reuses it.

`--root` must point at a BrixMS package (a directory with `brix.toml`), never
at this toolchain or builder checkout -- `enqueue`/`run-ticket`/`loop` reject
a `--root` without one instead of silently snapshotting the wrong tree.

Default acceptance gates are `format`, `check`, `test`, `quality`, and
`package_build` (executable after #78). The host auto-applies `brix fmt` to the
in-memory candidate when it already checks, so format thrash does not burn
model turns. `diff` and `impact` remain informational-only and are rejected as
acceptance gates.

Ticket state (queued tickets, every accepted typed action, host evidence,
critic reports, and the in-memory candidate overlay) is persisted to
`--queue` (defaults outside the source checkout, under
`$XDG_STATE_HOME/brix-builder` or `~/.local/state/brix-builder`) after every
single accepted action -- never only at the end of an iteration. If the
process is killed mid-run, the ticket is left `interrupted` (or, if the kill
happened between actions, `running`, auto-reclaimed back onto the queue after
15 minutes by `reclaim`/the next `loop`/`run-ticket`); `resume` puts an
`interrupted` ticket back on the queue immediately, and the next run picks up
from the exact phase (coder, critic, or host gates) it was in, without
re-issuing an already-accepted action or re-running an already-passed host
gate. `cancel` takes effect even while a worker is mid-iteration on that
ticket -- the worker checks the durable record before every write and stops
without resurrecting the ticket it was told to cancel.

Only one `loop`/`run-ticket` worker may hold a given `--queue` at a time; a
second one refuses to start with a clear "another worker already holds this
queue's lock" error instead of racing the first and corrupting ticket state.

Commands:

```sh
brix-builder --root <pkg> enqueue "<brief>" [--ticket-id ID] [--package PATH] \
  [--allow-file GLOB ...] [--gate GATE ...] [--max-iterations N] \
  [--from-file tickets/seeds/*.json]
brix-builder --root <pkg> tickets                 # list every durable ticket
brix-builder --root <pkg> status                  # queue root, lock, counts
brix-builder --root <pkg> reclaim                  # requeue abandoned 'running' tickets
brix-builder --root <pkg> inspect-ticket ID        # full persisted state
brix-builder --root <pkg> run-ticket ID            # run to a terminal status
brix-builder --root <pkg> run-ticket ID --one-iteration
brix-builder --root <pkg> loop [--once]            # drain the queued tickets
brix-builder --root <pkg> resume ID                # requeue an interrupted ticket
brix-builder --root <pkg> cancel ID "<reason>"      # inert; never touches source
brix-builder --root <pkg> export-proposal ID out.json
```

A ticket carries its own package scope, write allowlist, acceptance gates,
iteration budget, and per-role action budget (see
`brix_builder.tickets.TicketSpec`); authority is fixed and non-configurable
-- a ticket can never apply to the canonical checkout, publish, cross a
production boundary, or invoke arbitrary shell. `tickets/seeds/` has runnable
example tickets for ordinary, narrowly-scoped BrixMS package changes and
doubles as fixtures for the deterministic scripted-backend integration
tests. `export-proposal` writes the proposed patch, the host oracle
evidence, the latest critic verdict, the unresolved obligations, and the
exact base revision the ticket was built against -- never an applied diff.

## Verification

```sh
. .venv/bin/activate
pytest
cargo test -p brix-cli
```
