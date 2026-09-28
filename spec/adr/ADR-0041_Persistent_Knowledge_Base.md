# ADR-0041 — Persistent Knowledge Base: Assert, Correct, Retract, Re-Decide

Status: **Accepted — implemented**, 2026-09-27.

Foundation documents: [ADR-0002: SOC Constitution](./ADR-0002_SOC_Constitution.md) (§5.3 fail
closed), [`spec/SOC_Semantic_Laws.md`](../SOC_Semantic_Laws.md) (SOC-LAW-09, "Correction and
retraction non-erasure"), [ADR-0026: The Audit-Input Transport Bundle](./ADR-0026_Audit_Input_Transport_Bundle.md),
[ADR-0030: Finite-Decision Alpha](./ADR-0030_Finite_Decision_Alpha.md),
[ADR-0031: External Inputs Alpha](./ADR-0031_External_Input_Alpha.md) (`brix.input@1`/`@2`,
`InputSnapshot`, `InputSnapshotId`).

---

## 1. Context and problem statement

Every `brix run` today starts from scratch: a program plus `--input` files produce one
`@Derived` decision, and nothing persists. Nothing records that a value was asserted, nothing
records that it was later corrected or retracted, and there is no way to ask "what did the
decision used to be, and why did it change?" without keeping the old input files and the old
program lying around by hand and re-running `brix run` yourself.

`SOC-LAW-09` ("correction and retraction non-erasure", `spec/SOC_Semantic_Laws.md` §SOC-LAW-09)
already states the rule this needs to obey: *"Retracting revision-scoped support invalidates
dependent current conclusions without deleting or rewriting durable theorem history. A corrected
claim is a new contextual judgement; prior evidence remains attributable to its original context
and revision."* Today that law's "Authority and evidence" note says plainly: *"A complete
invalidation traversal and historical context model have not landed."* This ADR is that history
model, built at the layer users actually touch: the CLI's finite-decision programs and their
external inputs, not a new kernel primitive.

## 2. Decision

A **knowledge base** is an ordinary directory. It holds:

- content-addressed **program sources** (`programs/<program-id>.brix`);
- content-addressed **input snapshots** (`snapshots/<snapshot-id>.json`, `brix.input@2`);
- an append-only, hash-chained sequence of **revision records**
  (`revisions/<seq>.json`, schema `brix.kb.revision@1`);
- a static **manifest** (`kb.json`, schema `brix.kb@1`);
- a **`HEAD`** pointer (schema `brix.kb.head@1`) to the current revision.

Nine operations act on it — `init`, `assert`, `retract`, `program`, `log`, `show`, `diff`,
`audit`, `verify` — as both a library (`crates/brix-kb`) and a CLI surface
(`brix kb <op>`, `crates/brix-cli/src/commands/kb.rs`). `init`/`assert`/`retract`/`program`
each produce exactly one new, immutable revision. Nothing is ever deleted or rewritten:
retracting a value is a new revision that removes it going forward, never an edit erasing that
it was ever asserted.

### 2.1 Why a new crate, not new CLI-command code

`brix-kb` is a new library crate (`crates/brix-kb`), not code folded into
`crates/brix-cli/src/commands/*.rs`. Two reasons:

1. **Layering.** The knowledge base's core (revision chain, canonical digests, replay,
   dependency graph, diff) is reusable outside a CLI process — a future service or a `brix test`
   fixture could open the same directory. Putting it in `brix-cli` would trap it behind a
   `PathBuf`/`stdout` interface.
2. **Merge safety.** This work lands alongside parallel changes to
   `crates/brix-cli/src/commands/*.rs` (a shared loading-pipeline refactor) and to
   `crates/brix-lower/src/finite_decision/` (why/whynot explanation trees). A new crate, plus
   exactly one new file (`commands/kb.rs`) and one added line (`pub mod kb;`) in the files those
   changes touch, has no surface area to conflict with them.

`brix-kb` depends on `brix-canon`, `brix-semantic`, `brix-syntax`, and `brix-lower` (the last for
`finite_decision`, `input`, `imports`, and `audit_bundle`). It does **not** depend on
`brix-cli`, and nothing in `brix-canon`/`brix-semantic`/`soc-core`/`brix-kernel` — the trusted
computing base per `docs/audit/issue-63/` — depends on it. Only `brix-cli` depends on `brix-kb`
(checked by `scripts/check_tcb_dependencies.py`).

### 2.2 Content-addressed programs and snapshots

A program is stored by the `FiniteDecisionProgramId` its resolved, `show`-stripped module hashes
to (ADR-0030/ADR-0031); a snapshot by its `InputSnapshotId` (ADR-0031). Both stores are
write-once (`create_new`) and idempotent: writing the same address twice is a no-op, since the
address *is* a hash of the exact bytes held there. A revision names its program and snapshot by
these ids and by the relative path the address implies (`programs/<id>.brix`,
`snapshots/<id>.json`) — never a raw filesystem path outside the knowledge base, so the
directory stays portable.

Storing the *program's own source text* (not the imports it might pull in via `use`) keeps a
knowledge base's own directory self-contained and matches how `brix run`/`brix audit`/
`brix verify` already treat packages: resolved from `--package-path` roots supplied at each
invocation, never embedded into an artifact (ADR-0031 §D-COMPAT lists `brix.soc` as the one
exception, embedded in the binary itself). Every `brix kb <op>` therefore accepts
`--package-path <dir>...`, exactly like the existing commands. **Limitation:** if a knowledge
base's program uses `use`, later operations against it must be given the same package roots
again; the knowledge base does not vendor imported packages. This mirrors the existing tool's
behavior and did not need solving specially here.

`brix-lower::input::InputSnapshot` had no way to build a snapshot directly from an in-memory
value map — only by decoding a shard file. `assert`/`retract`/`program` need exactly that (start
from the stored snapshot's values, upsert or remove some, build a fresh snapshot, hash it). Rather
than round-tripping through a JSON encode/decode for every edit, this ADR adds one small public
constructor, `InputSnapshot::from_values`, in `crates/brix-lower/src/input.rs` (not
`crates/brix-lower/src/finite_decision/`, which is out of scope for this change; `input.rs` is
not). It is purely additive, touches no canonical encoding, and is covered by the existing
`InputSnapshot` test suite plus this crate's own round-trip tests.

### 2.3 The revision record

Schema `brix.kb.revision@1`:

```json
{
  "schema": "brix.kb.revision@1",
  "seq": 2,
  "parent": "<64-hex, or null for revision 1>",
  "program_id": "<64-hex>",
  "program_path": "programs/<program_id>.brix",
  "snapshot_id": "<64-hex>",
  "snapshot_path": "snapshots/<snapshot_id>.json",
  "change": { "kind": "assert", "names": ["stock"] },
  "result": {
    "status": "selected",
    "candidate": "hold",
    "decision_digest": "<64-hex, or null>",
    "context_id": "<64-hex, or null only for missing-inputs>",
    "facts_digest": "<64-hex, or null>",
    "outcomes_digest": "<64-hex, or null>",
    "diagnostics": []
  },
  "digest": "<64-hex — this record's own identity>"
}
```

`change.kind` is one of `init`, `assert` (with the asserted names), `retract` (with the retracted
names), or `program` (with the previous program id and the names dropped because the new program
no longer declares them, or declares them at a different type).

`status` is one of `selected`, `quiescent`, `unknown` (the deliberation itself faulted — a
`FiniteDecisionStop::Unknown`), or **`missing-inputs`**: the declared input contract was not
fully satisfied by the snapshot. A knowledge base may start incomplete (`init` with no, or partial,
`--input`), and `retract` may make a previously-complete contract incomplete again — both are
honest, first-class outcomes, not usage errors, matching the spirit of ADR-0031's
`validate_completeness` vs. `validate_against_declarations` split.

**Why the record stores digests of the facts/dispositions and the decision value, not copies of
them.** Every read operation (`log`/`show`/`diff`/`audit`/`verify`) replays the revision fully —
parses, resolves imports, strips `show`, lowers, builds the runtime, and runs it — because
finite-decision programs are bounded and this is cheap (§2.5). The replay is the source of truth
for anything a person reads; the record's digests exist only so `verify` can confirm a replay
still reproduces exactly what was recorded, without a second, duplicated encoding of
`L3ValueV2`/`DerivedFact`/`CandidateDisposition` on disk that could quietly drift from the code
that actually renders them (`crate::commands::fact_to_json`/`fmt_value_human` in `brix-cli`,
reused as-is by `brix kb show`/`log`). A digest can't be un-hashed, so this is a deliberate
asymmetry: the record proves a replay is faithful, it does not substitute for one.

**Why canon, not JSON, for the revision digest.** The record's `digest` field is computed over a
canonical `brix_canon::CanonWriter` preimage — tag `"brix.kb.revision@1"` under `Domain::Value`,
the same pattern `ContextId`/`ConfigId` already use for a generic identity (ADR-0031
§D-IDENTITY) — never over the record's JSON bytes. JSON has no single canonical byte form in this
codebase (whitespace, key order, and number formatting are all free choices of the encoder), so
hashing JSON text would make the chain's integrity depend on exactly reproducing formatting
decisions nothing else in the toolchain treats as meaningful. The JSON file additionally *carries*
the digest it was written with (the `digest` field), which lets `verify` catch a single-field
hand-edit immediately, before it even walks the parent chain (§2.6).

Two digests in the record are deliberately **not** built from a portable canonical encoding:
`facts_digest` folds in `CandidateStatus`'s `Display` rendering (a hand-written, deterministic
string, not `Debug`), and it, plus `outcomes_digest`, exist purely as this crate's own
tamper/drift check — never compared against another implementation's bytes the way `SOC-LAW-01`
governs a semantic identity. `decision_digest`, by contrast, reuses `InputScalarValue`'s real
`Canonical` encoding (the same bytes that value would carry if it were later re-supplied as an
input), because a decision's value is exactly the kind of thing that identity governs.

`outcomes_digest` covers every decision the program declares, not only the first commit pool:
each commit pool (ADR-0039) and each instance of each `decide` block (ADR-0043), with its stop,
selected candidate and value, and candidate dispositions. `status` is `unknown` when any of them
failed closed, the same rule `brix run` uses for its status and exit code. `candidate` and
`decision_digest` describe the first commit pool, as `brix run`'s top-level decision does.

### 2.4 Strict, duplicate-key-rejecting decoding

`crates/brix-lower/src/input.rs` already has a hand-written strict JSON parser
(`StrictJsonParser`) that rejects duplicate keys — the point of ADR-0031 §D-NODUPKEYS is that
`serde_json`'s default map/struct decoding does not: a duplicate key is either silently
last-write-wins or, for a `#[derive(Deserialize)]` struct, matched to a field twice with no error
at all. That parser is private and shaped exactly for the `brix.input` tagged-value envelope, so
this ADR does not reuse it as-is; instead `crates/brix-kb/src/strict_json.rs` is a small,
independent equivalent in the same spirit — a generic `Value` tree (`Obj`/`Arr`/`Str`/`Int`/
`Bool`/`Null`) that rejects a repeated object key the moment it is parsed, plus typed accessors
that reject an unrecognized field via an explicit allow-list. `brix-kb`'s own files
(`kb.json`, `HEAD`, every `revisions/<seq>.json`) are decoded through it — never through
`serde_json`'s `Value`/derive path — while writing continues to use `serde_json` (already
whitelisted in `DEPS.md` for "diagnostics/manifests only", the exact category these files fall
into): encoding has no adversarial-duplicate-key hazard, since this crate is the only writer.

### 2.5 Honest impact analysis, not incremental evaluation

Every `assert`/`retract`/`program` re-runs the *entire* decision from the stored program and the
freshly built snapshot (`brix_kb::pipeline::replay`). There is no incremental evaluator here.
`soc-core::IncrementalEngine` exists and is exercised by other tests
(`crates/soc-regimes/tests/finite_frontier_incremental_differential.rs`) for the settlement
layer's own delta-vs-naive parity law (SOC-LAW-08); wiring finite-decision programs through it is
explicitly **future work**, worth doing once knowledge bases hold relation-scale data where
full re-evaluation per edit stops being cheap. Today's finite-decision programs are small and
bounded (ADR-0030's `MAX_EXPR_NODES`, etc.), so full re-derivation is the honest choice: it can
never disagree with itself the way an incremental path and a naive path could.

What this ADR adds on top is purely **explanatory**, in `crates/brix-kb/src/deps.rs`: a static
dependency graph over one plan's rules and proposals (a rule's declared `depends_on`, a
proposal's declared `deps`, and every `LetRef`/`RuleFact` identifier actually mentioned in a rule
body or a proposal's guard/value — because a rule can read an input or a `let` binding directly,
without ever declaring it as a dependency; only inter-rule fact reads are declared). `brix kb
diff` computes both revisions' facts and inputs by full replay, and only *afterward* uses the
graph to narrow a changed fact's explanation down to the changed inputs/rules it can reach. If the
graph is ever wrong, replay is still what a person reads; only the "why" line would say less than
it should, never something false. A dedicated test in `crates/brix-kb`
(`test_diff_every_fact_outside_the_affected_set_is_unchanged`) confirms independently — by
literally comparing the two fresh replays, not by trusting the graph — that every fact
**outside** the "why"-implicated set is byte-identical across the compared revisions; the CLI
integration test (`crates/brix-cli/tests/kb_integration.rs`) additionally checks the same
property end to end through the real binary's `diff` output.

### 2.6 Locking and non-erasure

A single `.lock` file (`OpenOptions::create_new`) serializes writers; a concurrent `init`/
`assert`/`retract`/`program` against a locked directory fails immediately with a clear message
and exit 2, rather than blocking or corrupting state. Readers (`log`/`show`/`diff`/`verify`/
`audit`) take no lock.

`HEAD` is the commit point. A revision file and `HEAD` are each written as a temporary sibling
that is fsynced, renamed into place, and followed by an fsync of the directory, so a crash leaves
either the old file or the complete new one. A crash between the two writes leaves the previous,
still-valid `HEAD` and a revision file numbered past it; that file was never committed, no reader
reaches it (readers go through `HEAD`), and the next writer, holding the lock, replaces it rather
than refusing to continue. `init` writes `kb.json`, which marks the directory as a knowledge base,
last, so an interrupted `init` can simply be rerun. Once `HEAD` names a revision, its file is never
written again.
`verify` walks the whole chain from revision 1: it strictly decodes each record (which alone
catches a single hand-edited field, since the stored `digest` no longer matches — §2.3), checks
`parent` against the previous revision's freshly recomputed digest, recomputes the program id from
the stored source and the snapshot id from the stored snapshot file (catching an edited program
or snapshot file even if the revision record itself was left untouched), and replays the revision
to confirm the recorded result. Any mismatch fails closed: exit 1, with a message naming the
revision and which check failed. **Limitation, by design, shared with every hash chain (Git
included):** an attacker who rewrites a revision *and* every digest and parent link after it,
*and* `HEAD`, produces a chain that is internally consistent again. `verify` detects tampering
that does not also correctly redo the rest of the chain; it is not a defense against a fully
privileged filesystem attacker who rewrites the entire tail and the pointer to it. Nothing in this
ADR's tests exercises that case, because no on-disk integrity scheme without an external witness
can.

### 2.7 Operation semantics worth calling out

- **`assert`** upserts the named values from the given `--input` shard(s) into the current
  snapshot (a new value for an existing name is a correction) and validates the *merged* result
  against the program's declarations before writing a revision; an undeclared name or a type
  mismatch is rejected with no side effect (no revision is created).
- **`retract`** requires every named input to currently be set; retracting a name that was never
  asserted is a usage error (exit 2), not a silent no-op, so a `retract` always means something
  actually changed.
- **`program`** re-checks every currently-set value against the new program's input declarations;
  a value whose name is no longer declared, or is declared at a different type, is dropped and
  named in the revision's `change.dropped_inputs` — never rejected outright, since a program
  change legitimately changes what inputs mean.
- **Exit codes** follow the CLI's existing convention (0 / 1 / 2). For an operation that produces
  a decision (`init`/`assert`/`retract`/`program`), exit 1 means the resulting decision is not a
  clean `selected`/`quiescent` outcome — `unknown` or `missing-inputs` — exactly as `brix run`
  already exits 1 on `Unknown`. `log`/`diff` always exit 0 on success (they report on possibly
  many revisions of mixed status, like `git log`); `show` reports one revision faithfully and
  also always exits 0 on success, since displaying history is not asserting a fresh claim;
  `verify` exits 1 on any integrity failure.

---

## 3. Architecture

```text
                    brix kb <op> (crates/brix-cli/src/commands/kb.rs)
                                   |  parses nothing further — crate::cli::KbOp
                                   |  is already parsed; renders human/--json
                                   v
                         brix-kb::ops / brix-kb::diff
        +--------------------------+--------------------------+
        |                          |                          |
        v                          v                          v
  pipeline::load_plan      revision::RevisionRecord      deps::build_dep_graph
  (parse -> resolve        (canon digest, chain,          (rule/proposal ->
   imports -> strip show   strict JSON via                 transitive inputs
   -> lower)                strict_json.rs)                 + rule facts)
        |                          |
        v                          v
  pipeline::replay          paths.rs (content-addressed
  (build runtime, run)       programs/ + snapshots/,
        |                     revisions/<seq>.json, kb.json, HEAD)
        v
  FiniteDecisionRun (facts, dispositions, decision, context) — same runtime
  crates/brix-cli/src/commands/run.rs and audit.rs already use.
```

---

## 4. Implementation mapping

| Requirement | Anchor |
|---|---|
| Library crate | `crates/brix-kb` (new workspace member) |
| On-disk layout | `crates/brix-kb/src/paths.rs` |
| Revision record, canonical digest, chain | `crates/brix-kb/src/revision.rs` |
| Manifest (`kb.json`) / `HEAD` | `crates/brix-kb/src/manifest.rs` |
| Strict, duplicate-key-rejecting decode of `brix-kb`'s own files | `crates/brix-kb/src/strict_json.rs` |
| `brix.input@2` snapshot encoder + round-trip tests | `crates/brix-kb/src/snapshot_io.rs`; constructor `InputSnapshot::from_values` in `crates/brix-lower/src/input.rs` |
| Parse → resolve imports → strip `show` → lower → build → run, independent of `brix-cli` | `crates/brix-kb/src/pipeline.rs` |
| Dependency graph for `diff`'s "why" | `crates/brix-kb/src/deps.rs` |
| `init`/`assert`/`retract`/`program`/`log`/`show`/`audit`/`verify`, locking | `crates/brix-kb/src/ops.rs` |
| `diff` | `crates/brix-kb/src/diff.rs` |
| Unified operation error (`code`/`message`/`status`/`exit_code`, mirrors `CliInputError`) | `crates/brix-kb/src/error.rs` |
| Package loading (`use`) independent of `brix-cli` | `crates/brix-kb/src/packages.rs` |
| Library-level lifecycle tests (init/assert/retract/program/log/show/diff/audit/verify, tamper, lock, non-erasure) | `crates/brix-kb/src/lifecycle_tests.rs` |
| CLI surface: `Command::Kb`, `KbOp`, argument parsing, help text | `crates/brix-cli/src/cli.rs` |
| CLI dispatch, module doc comment | `crates/brix-cli/src/main.rs` |
| `pub mod kb;` | `crates/brix-cli/src/commands/mod.rs` |
| CLI rendering (human/`--json`), reusing `fact_to_json`/`fmt_value_human`/`format_finite_decision_human`/`candidate_disposition_to_json`/`decision_to_json`/`bound_input_to_json` | `crates/brix-cli/src/commands/kb.rs` |
| CLI integration tests, incl. a `kb audit` bundle verifying with plain `brix verify` | `crates/brix-cli/tests/kb_integration.rs` |
| Workspace membership, dependency inventory | root `Cargo.toml`, `docs/audit/issue-63/workspace-dependencies.{json,dot}` (regenerated via `scripts/check_tcb_dependencies.py --write`) |

---

## 5. Status and compatibility

- **Status:** Accepted and implemented. `brix run`/`brix audit`/`brix verify`/`brix check`/
  `brix why`/`brix whynot` and their existing tests are untouched — this ADR adds a new crate and
  a new CLI subcommand, nothing else.
- **SOC-LAW-09:** this ADR is a *consumer* of the law, made concrete and user-facing at the
  finite-decision/CLI layer; it does not redefine the shared rule, and the law's registry entry in
  `spec/SOC_Semantic_Laws.md` (owned by the evidence-durability taxonomy and future invalidation
  engine, `#59`/`#178`) is unchanged by it.
- **Compatibility:** strictly additive. No existing canonical encoding, program/context/snapshot
  identity, or audit bundle format changed; `vectors/` is untouched.
