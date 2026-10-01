# Beta compatibility contract

This is what a BrixMS beta promises the people who build on it. It applies
from the first beta release and to every beta release after it. Anything
not listed under **Stable** can change between beta releases.

The contract covers the **finite-decision profile** — decision programs run
with `brix run`, `check`, `audit`, `verify`, `why`, `whynot`, `test`, `kb`,
and `serve` — because that is the path a user builds a product on. The
`let` lane, the proof kernel, and the L3 v1 rule agenda remain research
surfaces (see **Not covered**).

## The guarantee everything else serves

**A run succeeds only if every decision it declares settled.** If any
commit pool or any `decide` block instance faults — an evaluation error, a
type fault, an exhausted budget — the run is `unknown`: `brix run` and
`check` exit 1, `--json` reports `"ok": false, "status": "unknown"` with
the first fault in `diagnostics`, `audit` produces no bundle, and a `kb`
revision records status `unknown`. BrixMS never reports a guessed or
partial result as a success. A change that weakens this is a bug, not a
compatibility question.

## Stable

### Program identity

A program's id (`program:` in `brix run`) is a hash of its canonical
encoding. Within the beta, **the same source always has the same id**, on
every platform and every beta release. Audit bundles and knowledge bases
are keyed by these ids, so this is what lets a bundle produced today verify
next year.

- Enforced by frozen vectors: `crates/brix-lower/tests/multi_commit_identity.rs`,
  `crates/brix-lower/tests/finite_decision_functions.rs`, and
  `crates/brix-cli/tests/frozen_identities.rs` (program, context, and input
  snapshot ids of the examples), plus `scripts/canon_crosscheck.py` against
  the canonical-encoding spec.
- A language addition gets new, appended encoding (a new expression ordinal
  or a new tagged section written only when used), so programs that do not
  use it keep their ids. This is how ADR-0033 through ADR-0043 were added.
- A change that would alter an existing program's id needs an ADR that says
  so and a new encoding tag. **Never update a frozen vector to make a test
  pass.**

### Source language (decision programs)

Every construct documented in `docs/brix-language.md` for the
finite-decision lane keeps its meaning. New constructs are additive. These
words are reserved and cannot be used as names:

`config regime gen rule fn let show witness true false use match prove why
audit then and propose priority when commit from input for in where yield
otherwise decide`

These names are reserved for built-ins in a decision program: the integer
operations `div_floor div_ceil div_half_even mod_euclid`, the list
operations `sum count all any min max filter map len distinct`, the numeric
operations `f64 decimal f64_from_int decimal_from_int decimal_div f64_neg
decimal_neg`, and the type names `List F64 Decimal`. A declaration that uses one is refused with an error, never
silently shadowed.

### Input files

`brix.input@1` (scalars), `@2` (records and variants), `@3` (bounded
lists), and `@4` (explicit F64/Decimal values, ADR-0045) are accepted by every beta release, with the same strict decoding:
duplicate keys, unknown fields, and out-of-bound values are rejected, never
repaired. A new capability gets a new schema version; existing versions do
not change.

### Command line

- **Commands and flags** in `brix --help` keep their meaning. New flags and
  commands are additive.
- **Exit codes:** `0` success; `1` the program was rejected or the run is
  `unknown`; `2` usage or I/O error. Scripts may rely on these.
- **`--json` output** (`brix.cli.result@1`, `brix.cli.kb-result@1`,
  `brix.test.result@1`): existing fields keep their names, types, and
  meaning. New fields may be added; consumers must ignore fields they do not
  know. Human-readable output is for people and may change.
- **Test suites** (`brix.test@1`) keep working.

### Embedding

`brix serve --stdio` speaks `brix.serve@1` (ADR-0044): one JSON request per
line, one response per line, strictly in order. Its methods, parameters,
and envelope are stable under the same additive rule as `--json`. Each
method's `result` is exactly the object the corresponding `--json` command
prints.

### Stored artifacts

- **Audit bundles** (`brix.soc.audit-input-bundle@1`) produced by any beta
  release verify with `brix verify` in every later beta release.
- **Knowledge bases** (`brix.kb@1`, `brix.kb.revision@1`, `brix.kb.head@1`)
  created by any beta release open, replay, and verify in every later beta
  release. A format change gets a new version and a migration command, never
  an in-place reinterpretation.

### Limits

Limits are part of the contract: a program and inputs accepted within these
bounds are accepted by every later beta release. Bounds may be raised, never
lowered. Exceeding one is a clear rejection (for inputs and programs) or
`unknown` with `ResourceExhausted` (for evaluation), never a crash.

| Limit | Value |
| --- | --- |
| Decimal normalized scale / coefficient | 0..=18 / signed i128 |
| Input file size / files per run / total input bytes | 1 MiB / 16 / 4 MiB |
| Declared inputs | 256 |
| Elements in a list input (`max N`) | 256 |
| Elements in a list built by an expression | 4,096 |
| Commit blocks / `decide` blocks per program | 64 / 64 |
| `decide` instances per run, all blocks together | 4,096 |
| Helper functions / parameters per helper | 256 / 32 |
| Outstanding helper calls (recursion depth) | 1,000 |
| Evaluation steps per expression / per run | 2,000,000 / 50,000,000 |
| Values built per expression (nodes / bytes) | 10,000 / 1,000,000 |

The run-wide step bound caps the worst case of a single run at about two
seconds in a release build (ADR-0042 §Evaluation budgets).

## How the contract changes

- **Additive changes** (new syntax, schema versions, JSON fields, serve
  methods, higher limits) need an ADR and tests, and ship in any beta.
- **Breaking changes** to anything under Stable are not made during the
  beta. If one is unavoidable (a security fix, a soundness bug), it gets an
  ADR that names what breaks, a changelog entry, and, for stored artifacts,
  a migration path, in a release whose notes lead with it.
- Fixing behavior that contradicts this document (for example a run that
  reports success when a decision faulted) is a bug fix, not a break.

## Not covered

These may change in any release:

- **The `let` lane and `brix check`'s binding report** (`name : Type @Grade
  = value`): type realization and evidence grades are still research
  surfaces (`spec/Type_Realization_Contract.md`).
- **Rust crate APIs** (`brix-lower`, `brix-kb`, `soc-core`, …): there is no
  stable Rust facade yet; embed through `brix serve` or the CLI.
- **The Python client's API** in `bindings/python/`, beyond the wire
  protocol it speaks.
- **The proof kernel, certificates, the L3 v1 profile, and saturation
  internals.**
- **Human-readable output, error message wording, and `why`/`whynot`
  explanation text.** Use `--json` for anything a program reads.
- **Performance**, beyond the bounds in the limits table.
