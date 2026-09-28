# brix-client (Python)

A small, pure standard-library Python client for `brix serve --stdio`, the embeddable JSON-lines
protocol described in [ADR-0044](../../spec/adr/ADR-0044_Serve_Protocol.md). No compiled
extension, no third-party runtime dependency, Python >= 3.9.

It spawns `brix serve --stdio` as a subprocess, speaks the `brix.serve@1` protocol over its
stdin/stdout, and exposes typed-ish methods that return parsed `dict`s — the same JSON objects
`brix <command> --json` would print, obtained without parsing any human-readable text.

## Install

From this directory:

```bash
pip install -e .
# or, for running the test suite too:
pip install -e ".[test]"
```

The `brix` binary itself is not installed by this package. Point the client at it either by
passing `brix_bin=` explicitly, or by setting the `BRIX_BIN` environment variable, or by having
`brix` on `PATH`.

## Quick start

```python
from brix import BrixClient, path

with BrixClient(cwd="/path/to/brixms") as client:
    hello = client.hello()
    print(hello["toolchain_version"], hello["methods"])

    result = client.run(program=path("examples/shipping.brix"))
    print(result["status"])          # "selected"
    print(result["decision"])        # {"candidate": "ship", ...}
```

An inline program or input (no file on disk) uses `source(...)` instead of `path(...)`:

```python
from brix import BrixClient, source

program_text = "config Decision = Ship | Hold\n..."
with BrixClient() as client:
    result = client.check(program=source(program_text))
```

Inputs work the same way, as a list mixing paths and inline sources:

```python
from brix import path, source

client.run(
    program=path("examples/shipping-input.brix"),
    inputs=[path("examples/shipping-input.json")],
)
```

## API

`BrixClient(brix_bin=None, cwd=None, timeout=30.0, extra_args=None)` — spawns the server. Use as
a context manager (`with BrixClient() as client:`) or call `client.close()` yourself; both close
the child's stdin (clean EOF) and wait for it to exit, killing it if it does not exit promptly.

Typed methods, each returning the command's result `dict` and accepting an optional
per-call `timeout=` override:

- `client.hello()`
- `client.check(program, inputs=None, package_paths=None)`
- `client.run(program, inputs=None, package_paths=None)`
- `client.why(program, candidate, inputs=None, package_paths=None)`
- `client.whynot(program, candidate, inputs=None, package_paths=None)`
- `client.audit(program, bundle_out, inputs=None, package_paths=None, force=False)`
- `client.verify(program, bundle, expect_program, profile="finite-decision", inputs=None, package_paths=None)`
- `client.test(files)`
- `client.kb_init(kb_dir, program, inputs=None, package_paths=None)`
- `client.kb_assert(kb_dir, inputs, package_paths=None)`
- `client.kb_retract(kb_dir, names, package_paths=None)`
- `client.kb_program(kb_dir, program, package_paths=None)`
- `client.kb_log(kb_dir, package_paths=None)`
- `client.kb_show(kb_dir, rev=None, package_paths=None)`
- `client.kb_diff(kb_dir, rev_a, rev_b, package_paths=None)`
- `client.kb_audit(kb_dir, rev, bundle_out, force=False, package_paths=None)`
- `client.kb_verify(kb_dir, package_paths=None)`

Lower-level: `client.call(method, params, request_id=None, timeout=None)` returns the full
response envelope (`{"schema", "id", "ok", "exit_code", "result"}`); `client.result(method,
params, timeout=None)` returns just `response["result"]` (what every typed method above uses
internally).

## Errors

- **`BrixProtocolError`** — the request itself was rejected at the protocol level (malformed
  params, an unknown method, bad JSON). This is *not* raised for a rejected or `Unknown` command
  outcome — that is a normal return value; check `result["ok"]` / `result["status"]`.
- **`BrixTimeoutError`** — no response arrived within the call's timeout.
- **`BrixProcessError`** — the subprocess could not be started, or its stdout/pipes closed
  unexpectedly mid-call.

All three subclass `BrixError`.

## A note on concurrency

One `BrixClient` instance serializes calls against one `brix serve --stdio` process (the server
itself processes requests strictly sequentially — see ADR-0044 §2). For concurrent work, use
multiple `BrixClient` instances (multiple server processes), not multiple threads sharing one
client for overlapping in-flight calls; `call()` is thread-safe in the sense that it will not
corrupt the wire protocol (it holds a lock across write+read), but concurrent callers will simply
queue behind each other, not run in parallel.

## Running the tests

```bash
cd bindings/python
BRIX_BIN=../../target/debug/brix pip install -e ".[test]" && pytest
```

If `BRIX_BIN` is not set, the tests fall back to `target/debug/brix` relative to the repository
root (i.e. `cargo build -p brix-cli` must have been run first).
