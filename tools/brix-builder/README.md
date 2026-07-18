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
- a host-owned evidence ledger and final acceptance verdict.

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

## Verification

```sh
. .venv/bin/activate
pytest
cargo test -p brix-cli
```
