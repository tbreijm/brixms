# ADR-0044 — `brix serve --stdio`: an Embeddable JSON-Lines Protocol

Status: **Accepted — implemented**, 2026-09-28.

Foundation documents:
[ADR-0026: The Audit-Input Transport Bundle](./ADR-0026_Audit_Input_Transport_Bundle.md),
[ADR-0030: Finite-Decision Alpha](./ADR-0030_Finite_Decision_Alpha.md) (the `brix.cli.result@1`
JSON result schema this protocol reuses verbatim),
[ADR-0031: External Inputs Alpha](./ADR-0031_External_Input_Alpha.md) (`brix.input@1`/`@2`/`@3`,
the strict duplicate-key-rejecting decoder this protocol's inline inputs go through unchanged),
[ADR-0041: Persistent Knowledge Base](./ADR-0041_Persistent_Knowledge_Base.md) (`brix kb <op>`,
whose nine operations this protocol also exposes).

---

## 1. Context and problem statement

Every existing way to drive BrixMS — `brix check`/`run`/`why`/`whynot`/`audit`/`verify`/`test`/`kb`
— is a one-shot process: start, do one thing, print either a human transcript or one pretty-printed
`--json` object, exit. That is the right shape for a human at a terminal or a CI step, but it is a
poor fit for a service that wants to call the engine many times without paying process-spawn cost
on every call, and without scraping stdout for a JSON blob it then has to trust was the *only*
thing printed.

Nothing before this ADR let a caller embed BrixMS as a long-running component. This ADR adds
exactly one such surface — `brix serve --stdio` — without inventing a second implementation of
any command: it is a thin protocol loop around the exact same pipeline
(`crates/brix-cli/src/pipeline.rs`, `crates/brix-cli/src/commands/*.rs`) the CLI itself calls, so
a result obtained through it is provably identical to what `--json` would have printed for the
same invocation.

## 2. Decision

`brix serve --stdio` runs a **JSON-lines protocol**, schema `brix.serve@1`:

- **Framing.** One JSON object per line on stdin (a *request*), one JSON object per line on
  stdout (a *response*). No other framing (no length prefixes, no batching envelope) — this is
  the simplest framing that a pipe, a `Popen`, or a language's child-process API can speak without
  a purpose-built client library, at the cost of requiring every request/response object to be
  representable on one line (never a problem: JSON has no meaningful embedded newlines outside
  string values, and `serde_json::to_string` — not `to_string_pretty` — is used for responses).
- **Requests carry an `id`** (any JSON value — number, string, object, `null`), echoed back
  verbatim on the matching response. This is what makes **pipelining** safe: a caller may write
  several request lines before reading any response line, and match responses back up by `id`
  once they arrive. It also lets a caller running requests concurrently against *several* server
  processes (not several requests against *one*) know which reply is which without needing a
  synchronous request/response round trip in its own I/O loop.
- **Processing is strictly sequential.** One request is read, fully handled (including any file
  I/O, any temporary-file materialization for inline input — §4), and its response written before
  the next request is read. This is a deliberate simplicity/throughput trade-off: it makes
  response order always equal request order (so a pipelining caller needs no correlation logic
  beyond FIFO matching if it doesn't want `id`-based matching), and it lets the implementation use
  a single thread-local result-capture slot (`crates/brix-cli/src/json.rs`) instead of any
  synchronization, with **no `unsafe` code and no new dependency**. A caller that wants
  concurrency runs several `brix serve --stdio` processes.
- **`hello`** is the one method with no command behind it: it reports the toolchain version, the
  protocol schema, and the list of supported methods, so a client library can fail fast on a
  version it does not understand instead of discovering a mismatch on its first real call.

### 2.1 Request and response shape

Request:

```json
{ "id": <any JSON value>, "method": "<name>", "params": { ... } }
```

`id` is optional on the wire (defaults to JSON `null` if omitted) but a caller that wants to
correlate pipelined responses must supply one. `params` is optional and method-specific; omitted
`params` behaves as `{}`.

Response:

```json
{ "schema": "brix.serve@1", "id": <echoed>, "ok": true, "exit_code": <0|1|2>, "result": { ... } }
```

or, for a protocol-level failure:

```json
{ "schema": "brix.serve@1", "id": <echoed, or null if unparseable>, "ok": false,
  "error": { "code": "<stable-code>", "message": "<human text>" } }
```

**`ok` is a protocol-level flag, not the command's own success flag.** `ok: true` means the named
method was dispatched and produced a result — *including* a rejected `check`, an `Unknown` `run`,
or a failed `verify`: those are exactly as much a "successful call" as an accepted one, and their
failure is recorded the same way `--json` records it, inside `result` (`result.ok: false`,
`result.status`, `result.diagnostics`), with `exit_code` mirroring the CLI's own exit code for
that outcome (0/1/2, per the CLI's existing contract — see each command's module doc). `ok: false`
means the *request itself* could not be dispatched at all: malformed JSON, an unknown method,
or params that do not resolve to a valid command invocation (missing required field, a `program`
naming neither `path` nor `source`, and so on). This split matters for a caller: `result.ok` (or
its absence) is the thing to branch application logic on; `ok`/`error` is the thing that means "my
request was wrong or the server rejected it before running anything."

### 2.2 Methods

One method per existing CLI surface, plus `hello`:

| Method | Mirrors | Result schema |
|---|---|---|
| `hello` | — | `brix.serve.hello@1` |
| `check` | `brix check` | `brix.cli.result@1` |
| `run` | `brix run` | `brix.cli.result@1` |
| `why` | `brix why` | `brix.cli.result@1` |
| `whynot` | `brix whynot` | `brix.cli.result@1` |
| `audit` | `brix audit` | `brix.cli.result@1` |
| `verify` | `brix verify` | `brix.cli.result@1` |
| `test` | `brix test` | `brix.test.result@1` |
| `kb.init` / `kb.assert` / `kb.retract` / `kb.program` / `kb.log` / `kb.show` / `kb.diff` / `kb.audit` / `kb.verify` | `brix kb <op>` | `brix.cli.kb-result@1` |

Every one of these result schemas already existed before this ADR (they are exactly what
`--json` prints for the equivalent CLI invocation); this ADR adds no new result shape for them.
`params` fields map directly onto the equivalent CLI flags/operands:

```jsonc
// check / run / why / whynot / audit / verify
{
  "program": { "path": "examples/shipping.brix" },   // or { "source": "<.brix text>" }
  "inputs": [ { "path": "in.json" }, { "source": "<brix.input@N text>" } ],  // optional
  "package_paths": ["pkgs/"],                          // optional
  "candidate": "ship",                                 // why / whynot only, required
  "bundle_out": "out.bundle",                           // audit only, required
  "force": false,                                       // audit only, optional
  "bundle": "in.bundle",                                // verify only, required
  "expect_program": "<64-hex>",                          // verify only, required
  "profile": "finite-decision"                          // verify only, optional (default shown)
}

// test
{ "files": ["examples/shipping.test.json"] }

// kb.<op>
{
  "dir": "kb-dir",
  "program": { "path": "p.brix" },        // init / program
  "inputs": [ { "path": "in.json" } ],    // init / assert
  "names": ["stock"],                     // retract
  "rev": 3,                               // show (optional), audit (required)
  "rev_a": 1, "rev_b": 2,                 // diff
  "bundle_out": "out.bundle",             // audit
  "force": false,                         // audit (optional)
  "package_paths": ["pkgs/"]              // all ops, optional
}
```

## 3. Getting a value instead of stdout text: `emit_result_json`

Before this ADR, every `--json` code path ended by calling
`println!("{}", serde_json::to_string_pretty(&res).unwrap())` directly, at up to a dozen call
sites per command (one per success/failure branch). To give `brix serve --stdio` the exact same
value without re-implementing any of those branches, every one of those call sites now calls
`crate::json::emit_result_json(&res)` instead — a function generic over any `Serialize` result
type (the schema varies by command: `CliResultJson`, `TestSuiteResultJson`/`TestFatalErrorJson`,
or an ad-hoc `serde_json::Value` for `kb`). By default it prints exactly as before (byte-identical
CLI output — every existing CLI integration test passes unchanged). When a thread-local capture
slot is active (`crate::json::with_captured_result`), it stores the serialized value there instead
of printing, and `with_captured_result` hands that value back to its caller alongside the
command's own return value (its exit code).

`brix serve --stdio`'s dispatcher therefore does not parse or reconstruct a result: it calls the
same `commands::check::execute_check(...)`, `commands::run::execute_run(...)`, etc. functions the
CLI calls, always with `json: true`, wrapped in `with_captured_result`, and forwards the captured
value straight into the response's `result` field. This is the one refactor this ADR needed in the
existing command modules — minimal (one call-site substitution per print) and behavior-preserving
by construction, since the non-captured path is unchanged code.

Because processing is strictly sequential (§2), a single `thread_local!` slot — no mutex, no
channel, no `unsafe` — is enough: only one capture can ever be active at a time.

## 4. Programs and inputs: path or inline, byte-for-byte

A program or an input file can be named **by path** (resolved exactly like a CLI operand or
`--input <path>`, relative to the server process's current working directory) or given **inline**,
as a JSON **string** holding the raw text:

```json
{ "path": "examples/shipping.brix" }
{ "source": "config Decision = Ship | Hold\n..." }
```

Carrying inline text as a JSON *string*, not a nested JSON *object*, is the load-bearing choice.
An inline `brix.input@N` envelope is JSON itself; if the protocol accepted it as a nested object
(`{"source": {"schema": "brix.input@1", ...}}`), reaching that text would mean decoding the outer
request with `serde_json::Value` and re-serializing the inner object back to text before handing
it to the strict decoder in `crates/brix-lower/src/input.rs` — and `serde_json::Value` collapses
duplicate object keys during that decode, exactly the failure mode ADR-0031's strict decoder
exists to reject. Requiring the envelope as a string sidesteps this: decoding a JSON *string*
value is a single, lossless operation (unescape the string literal), so the bytes the strict
decoder eventually sees are exactly the bytes the caller wrote between the quotes, with every
duplicate key, exact whitespace, and byte order preserved.

Inline text is materialized into a private, per-request temporary directory
(`std::env::temp_dir()/brix-serve-<pid>-<nanos>-<counter>`, removed when the request finishes,
including on error) and then read back through the *exact same* bounded file-reading code the CLI
already uses (`packages::read_source_bounded` for a program, `InputLimits`-bounded shard decoding
for an input) — nothing new is added to either decoder. Consequently:

- An inline input with a duplicate JSON key is rejected the same way a file with a duplicate key
  is: `status: "rejected"`, diagnostic `input-duplicate-key: ...` (unchanged from ADR-0031).
- An inline program/input and a file holding the identical bytes produce **identical** program and
  input-snapshot identities — proven by the accompanying integration test, not asserted by
  documentation alone.

## 5. Limits and error model

- **Request line size.** A request line over `MAX_REQUEST_LINE_BYTES` (4 MiB) is rejected without
  ever being buffered in full: the bounded line reader tracks a running total against the limit
  and, once exceeded, drops the partial buffer and keeps draining input (bounded chunk by chunk
  through the standard library's own `BufRead` buffer) until the line's terminating `\n`, so
  framing for the *next* line is never corrupted by a hostile one. The response is
  `{"ok": false, "error": {"code": "request-too-large", ...}}`.
- **Program/input resource limits** are exactly the CLI's own and are not loosened, duplicated, or
  re-implemented here: `ParseLimits::strict()` for source parsing, `InputLimits::default()` for
  input decoding (`crates/brix-lower/src/input.rs`) — the same limits a direct `brix check --input`
  invocation is bound by.
- **Malformed request JSON, an unknown method, or invalid params** each produce one `ok: false`
  error response (`error.code` one of `malformed-request` / `unknown-method` / `invalid-params` /
  `workspace-io-error` / `invalid-utf8` / `request-too-large`) and the server **keeps serving**:
  one bad line never ends the session.
- **stdout discipline.** Nothing but response lines is ever written to stdout. Every place a
  command would otherwise print (both the JSON path, via `emit_result_json`, and the human-text
  path, which `brix serve --stdio` never takes since every dispatched call uses `json: true`) is
  covered; diagnostics that would go to human stderr in the CLI stay inside the captured
  `result` object instead, exactly as `--json` already represents them.
- **Shutdown.** EOF on stdin (the caller closes its write side) ends the loop cleanly; the process
  exits `0`. A local I/O failure *writing* a response (the only case that can leave a request
  effectively unanswered) ends the loop with exit code `2`, matching the CLI's usage/IO exit
  category.

## 6. Versioning

The protocol's own schema tag is `brix.serve@1`, carried on every response and reported by
`hello` (`hello`'s own result is `brix.serve.hello@1`, distinct from the response envelope's tag
so a client can distinguish "which protocol version is this response framed in" from "which
protocol version does the server support", though today they always agree). Every method's
*result* schema is versioned independently and already existed (`brix.cli.result@1`,
`brix.test.result@1`, `brix.cli.kb-result@1`) — `brix serve --stdio` does not fork or shadow those
version numbers, so a change to one of them (e.g. a future `brix.cli.result@2`) is exactly as
additive or breaking for a `brix serve --stdio` caller as it is for a `--json` caller, with no
separate protocol migration needed. A future incompatible change to the *envelope* itself (framing,
required top-level fields) would introduce `brix.serve@2` and a caller would detect the mismatch
through `hello`'s `protocol` field before relying on it.

## 7. What this ADR does not do

- It does not add a network transport (TCP/Unix-socket/HTTP). `--stdio` is the only transport
  today; the flag exists to make a future transport additive rather than a breaking rename.
- It does not add authentication or sandboxing beyond what spawning `brix` as a subprocess already
  implies (the caller controls the process's argv, environment, and file-descriptor inheritance).
  A service embedding `brix serve --stdio` is responsible for the same filesystem-access
  boundaries it would need around any other subprocess it spawns.
- It does not change any identity, canonical encoding, or CLI-visible behavior. Every existing CLI
  integration test passes unchanged; `brix serve --stdio` is purely additive.

## 8. Reference implementation

- `crates/brix-cli/src/serve.rs` — the protocol loop, request/response types, bounded line
  reader, inline-source temp-workspace materialization, and per-method dispatch.
- `crates/brix-cli/src/json.rs` — `emit_result_json` / `with_captured_result` (§3).
- `crates/brix-cli/src/cli.rs` — `brix serve --stdio` argument parsing (`Command::Serve`).
- `crates/brix-cli/tests/serve_stdio.rs` — subprocess integration tests: `hello`; `check`/`run`
  by path and by inline `source`; `why`/`whynot`; an `audit` bundle consumed by a subsequent
  `verify` call in the same session; `test`; a `kb.init`/`kb.show`/`kb.log` session; inline input
  with a duplicate key rejected; inline vs. file input producing an identical snapshot id;
  pipelined requests answered in request order; a malformed JSON line, an oversized line, and an
  unknown method each answered with an error response while the server keeps serving; and clean
  EOF exiting `0`.
- `bindings/python/brix/` — a pure-standard-library Python client (`BrixClient`) that spawns
  `brix serve --stdio` and exposes the methods above as typed-ish calls returning parsed `dict`s
  (see `bindings/python/README.md`).
