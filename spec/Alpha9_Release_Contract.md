# Alpha.9: reliable collections, composable relations, and scoped requests

Status: **Proposed release contract**, 2026-10-28. Baseline:
`v0.1.0-alpha.8`, commit `25b130a`. Target: `v0.1.0-alpha.9`.

This specification translates Atlas AI's requirements into ordered delivery
slices built on issues #52, #53, and #59. Atlas reports that its alpha.8 migration
reduces approximately 7,800 source facts to 696 catalogue entries with parity.
Its corruption reproduction and performance numbers below are downstream
observations, not measurements independently reproduced by this document.

All new clauses below are **Specified**, unless explicitly identified as
existing alpha.8 behavior. A test name or implementation location is a starting
point, not evidence that a requirement passes. Delivery must replace each
Specified status with a named passing gate and the tested commit, or state the
remaining gap. This document does not approve unresolved normative amendments;
follow [CONTRIBUTING](../CONTRIBUTING.md) for rulings before dependent code.

## 1. Release scope and delivery order

| Order / ID | Priority | Deliverable | Dependency |
|---|---|---|---|
| F | Foundation | Reviewed #52 resolution, #53 typing, #59 context slices and their integration | Release prerequisite; see §2 |
| A9-1 | P0 | Repair cold restore of large `collect` groups | Start immediately on alpha.8; no need to wait for F |
| A9-2 | P0 | First-class unions, distinct union, composite decision keys, indexed nested-field joins | A9-1; F contracts govern semantics |
| A9-3 | P0 | Typed named queries, host scope, pinned revision and bound cursors | A9-2; #52/#59 integration |
| A9-4 | P0 | Immutable evidence context per request across execution and verification | A9-3; #59 validation and #52 authority boundaries |
| A9-5 | P1 | Reject unsupported string operations at check time; bounded library optional | A9-4; #53 typing boundaries |
| A9-6 | P1 | Child-key collection ordering and explicit truncation | A9-5; repaired A9-1 persistence |
| A9-7 | P1 | Reproducible published 8k / 80k / 800k benchmark and size budget | Qualify final A9-1–6 artifact |

Deliver Atlas slices in this order. Earlier reproductions, baseline benchmark
capture, and independent design work may run alongside foundation review.
Do not hold the data-corruption repair for unrelated API work. Alpha.9 includes
all seven slices; a partial delivery must be named and scoped as such.

Already available in alpha.8 and retained: `--rev`, `--limit`, `--cursor`,
`--typed`, `expect_digest`, historical reads, joins, absence, retraction, atomic
batches, versioned revision-bound cursors, and replay-checked decision evidence.
Extend those paths rather than creating alternative storage, resolver, or
verification engines. See [alpha.8 release notes](../docs/planning/alpha8-release-notes.md),
[ADR-0046](adr/ADR-0046_Persistent_World_Runtime.md),
[ADR-0047](adr/ADR-0047_Atlas_World_Profile.md), and
[ADR-0048](adr/ADR-0048_Evidence_Bearing_Types.md).

## 2. Foundation integration gate

These issues provide bounded foundations, not a claim that every historical
acceptance item in each issue has landed. Record the accepted contract, merged
commit, actual supported fragment, and outstanding work at integration.

| Foundation | Required alpha.9 use | Gate before relying on it |
|---|---|---|
| [#52](https://github.com/tbreijm/brixms/issues/52): resolution | Separate admissibility, selection, ambiguity, complete absence and incomplete search; retain support and authority distinctions | Canonical outcome decoding is bounded and round-trips supported outputs. Any completeness claim is checked and bound to its exact question, or explicitly a trusted-caller classification with no independent proof authority. Coverage labels match executable tests. |
| [#53](https://github.com/tbreijm/brixms/issues/53): typing | Context-bound, independently checked typing evidence for the supported arithmetic fragment; clear admission checks for new query/string forms | Arithmetic bridges are closed or retired under accepted rules with negative tampering tests and bounded kernel checking. Structural identity cannot collide across distinct objects. No global tightening of generators to manufacture a grade. |
| [#59](https://github.com/tbreijm/brixms/issues/59): context | One versioned context vocabulary and validator used by scoped queries and request evidence | Decode, digest correspondence, supported profile, reference availability, adjacent ancestry rules and authorization are separate checks. Parent lookup verifies the requested identity. Envelope authentication alone cannot mint a fully validated context. |

Integrate #52's consumer seam with #59's actual validator; a matching pair of
caller-supplied IDs or a fixture stub is insufficient. Exercise it in an actual
request path with wrong/missing context and unsupported profile cases. #59's
first profile may require an explicit extension for host scope and evidence;
do not disguise unsupported dimensions as validated opaque bytes.

Keep semantic contexts distinct from authentication credentials. A named
principal, grant or scope in a payload does not authorize its use. Bind an
authorized host scope to the semantic context through the accepted profile.
Request evidence augments that context rather than replacing lexical, program,
world, revision or verifier bindings. Unsupported context transport fails closed.

Typing proof is not a proof of an evaluated result, termination or external
observation. World winners remain runtime settlements. A deterministic winner,
a complete relation scan, and a verified theorem are different claims. Scope
failures, budget exhaustion and malformed evidence cannot become `NoSolution`
or `Refuted`. Multi-row query success is not automatically resolver ambiguity.

## 3. A9-1 — large collection persistence

### Reported reproduction

```brix
rel input record: { id: Str, kind: Str, ordinal: Int } key id
rel derived o = select { kind: r.kind, ids: collect(r.id, r.ordinal, 1000) }
  from r in record group by r.kind
```

One group with 257 members reportedly commits successfully, then the next
process fails `world open-check`, `world query` and further batches with
`corrupt persisted world state: state store: corrupted node: <hash>`.
256 members pass. A declared limit of 300 with 300 members and six batches of
50 also fail. Controls: 700 members distributed over ten groups, and `count()`
over one 700-member group, pass. Preserve these controls in the regression.

Investigate grouped accumulator encoding/decoding, storage container bounds,
complete persistence of child references, dehydration/rehydration and paged
traversal. Starting points: `crates/brix-kb/src/world/{network,state_store,
chunk_policy}.rs`. The 256-row traversal quantum is a hypothesis, not a diagnosed
cause. Fix the invariant; merely increasing a hidden constant is insufficient.

### Required behavior and acceptance

- No internal page, chunk or container boundary imposes a semantic group limit.
  Declared cardinality and explicit cumulative resource budgets still apply.
  Resource exhaustion returns a typed failure before publishing HEAD.
- A successful batch publishes a completely restorable reachable state graph.
  All new nodes use compatible encoder/decoder limits; every persisted reference
  exists and validates. Do not turn normal commits into full-world verification
  scans to obtain this property.
- Exercise group sizes **1, 255, 256, 257, 1,000 and 5,000**. For each size N,
  use a declared limit of N and test: insert N; attempt N+1 and confirm the
  rejected child remains absent; retract N-1 existing members in a batch; retract
  the final remaining child separately; reinsert. For N=1, omit the N-1 step.
  Include retractions crossing the 257/256/255 boundaries in the larger cases.
  Include successful updates replacing a member without changing cardinality.
- After **every successful or rejected batch**, use a fresh process for
  `world open-check --verify-store`, typed query and an independent
  `WorldSession::verify_oracle` check. Add a harness for the latter; do not claim
  that an existing CLI flag invokes it. Verify the next batch also succeeds.
- Overflow refuses the entire transaction. HEAD sequence, HEAD digest, source
  rows, derived rows, settlements and observable history remain unchanged;
  orphan scratch objects are not published state. Include an unrelated write
  in the failing batch to detect partial application.
- Run one-batch, 50-row-batch and forced chunk-policy variants, especially around
  255/256/257. Compare equivalent histories under different internal chunk
  policies; batch-history differences need not have identical revision digests.
- Ordinary groups disappear after their final child. Existing parent-anchored
  empty-list constructions retain their parent with `[]`. Test both behaviors.

Preserve damaged-world evidence. Never reinterpret an actually corrupt node as
an empty group. Distinguish supported old-cache rebuilding from corruption under
ADR-0047. Document whether already affected alpha.8 worlds can be recovered by
verified journal replay into a separate destination; never silently rewrite them.

## 4. A9-2 — unions, composite keys and nested join indexes

### Union schema and support

A union exposes the resolved schema of its sources, including exact nominal
types and nested fields. All sources must be schema-compatible at check time;
reject incompatible fields/types with a source-specific diagnostic. Resolve
schemas through the acyclic relation DAG so a select, group or join can consume
a union, including a union of derived relations. No fallback `value` field may
replace the source record schema.

**Proposed semantics:** preserve plain `union` as the existing disjoint/key
collision-checking operation for compatibility. Add explicit `union distinct`
with set semantics over canonical typed row values, independent of provenance.
Identical rows from overlapping sources appear once. Retaining any supporting
source retains the output; withdrawing the final support retracts it. Two rows
with the same source key but different values remain distinct relational rows;
an explicit downstream key constraint still rejects conflicting values. Never
silently choose a winner by source order. Specify a versioned stable output key
for distinct rows, separate from their source-support identities.

Atlas's overlapping catalogue inputs use `union distinct`. If Atlas instead
needs deduplication by a subset of fields, that requires an explicit key/conflict
policy; it is not silently inferred from this operator.

### Decision key and indexed field paths

**Proposed syntax:** `decide choice for r in rows per (tenant, id) { ... }`.
Retain `per id` and its existing identities. New composite keys are ordered,
typed tuples with unambiguous canonical encoding, not concatenated display
strings. Every key field is present and type-checked. Preserve the full key in
winner relations, RPC/CLI/Python transport, explanations, history and evidence.
Test collisions such as `("a", "bc")` versus `("ab", "c")` and equal local IDs
belonging to different tenants.

The implementation preserves literal string expectations with
`--expect-entity`; JSON-looking scalar string IDs are never inferred to be
tuples. Use `--expect-entity-key-json <JSON-array>` for composite CLI evidence
expectations. The SDK accepts a list/tuple and sends a distinct
`brix.entity-key@1` object containing typed components; `Decimal` components carry
an explicit decimal tag. Verification canonicalizes that caller-supplied shape
without adopting the evidence bundle's claimed identity. Wrong key values or
component types fail verification. Evidence files and response metadata use the
declared key schema to preserve scalar versus composite presentation.

Equality joins on statically resolved paths such as `f.value.b` use maintained
indexes over that path, including when `f` is a decision relation. Validate the
entire path and terminal type. A path through a variant requires a supported
narrowing rule; do not index an undefined field. Winner replacement, nested-field
updates, retraction and cold restore update/reconstruct the index correctly.

Acceptance: select/group/join over each union shape; mismatched-schema failures;
duplicate supports across two and three sources; removal of one then all
supports; composite-key evidence round trips; and nested-field indexed joins
equal to full recomputation after edits and cold restore. Assert index probes
and bounded unrelated-row visits with work counters/plan inspection. Equal
answers alone do not establish that the join used an index.

Starting points: `crates/brix-lower/src/relation_dag.rs`, shared frontend AST and
checker, world lowering/network, decision transport and Python bindings.

## 5. A9-3 — parameterized queries in host-controlled scope

A program declares a named, read-only query with typed parameters and a typed
result schema. It uses the existing admitted relational fragment. Query source
syntax and wire schema are versioned additions; parameter values are data, never
interpolated Brix expressions. Missing, extra or ill-typed parameters fail before
execution. Parameter names cannot shadow host-scope bindings.

Proposed declaration shape (new syntax, not an alpha.8 example):

```brix
query catalogue(kind: Str) = select { id: c.id, kind: c.kind }
  from c in catalogue_entries where c.kind == kind
```

The host supplies scope separately from `kind`. Admission resolves the named
query's parameter and result schemas once against its bound program; execution
accepts typed argument values rather than arbitrary source text.

The request model is `(query identity, typed arguments, pinned revision,
host scope, read budget, optional cursor)`. Resolve HEAD at most once when a
revision is omitted, then pin that exact revision and digest for the whole read.
The next page stays at that revision even if a writer advances HEAD. Unknown or
unavailable revisions fail explicitly, with no fallback to current state.

The embedding host authenticates the caller and supplies an immutable authorized
scope. Untrusted query arguments, cursor contents and input envelopes cannot
set or widen it. A Python caller may be the trusted embedding host; expose that
trust boundary separately from ordinary query parameters. Remote/stdio use must
define who owns the authenticated channel before accepting a context handle.

Scope enforcement precedes joins, aggregation, absence and decision visibility.
Do not compute global aggregates or winners and merely filter their final rows.
Alpha.9's initial profile uses explicitly declared partition fields and
scope-preserving dependencies. A materialized relation may be reused only when
its plan establishes that partition property; otherwise evaluate a supported
scoped plan or reject it. Cross-partition dependencies and undeclared global
relations fail admission unless explicitly authorized by the scope profile.
Decision partition keys must preserve the authorized partition: a winner
selected globally across tenants cannot be reused as a tenant-local winner.

Extend alpha.8 cursor binding to include query/program identity, canonical typed
argument digest, host-scope identity, evidence-context identity when relevant,
world identity, revision sequence/digest and ordering version. Cursor tampering
must not widen authority or alter the bound query. A cursor is a continuation,
not an authorization credential. Reject any changed binding. Scoped named
queries never accept legacy unbound cursors.

Reads need no parameter upserts, temporary input relations or committed facts.
They leave HEAD, source rows, history, settlements and idempotency records
unchanged and work on a read-only world. Execution must not require persistent
cache writes or a writer lock; any acceleration can be ephemeral.

Acceptance: named-query results equal the corresponding filtered relation at the
same revision and scope; typed failures; two tenants sharing local IDs; joins,
counts and absence that could otherwise leak another partition; writer advance
between pages; rejection after changing any cursor binding; tampered cursors;
read-only filesystem execution; and concurrent scoped requests in one process.
Exercise CLI, Rust/stdio and Python paths. Preserve existing historical reads.

Starting points: `crates/brix-kb/src/world/session.rs`, existing query cursor
types, `crates/brix-cli/tests/world_pinned_query.rs`,
`crates/brix-kb/tests/world_query_scope_review.rs`, and `bindings/python`.

## 6. A9-4 — immutable evidence context per request

Replace process-global trust selection with an explicit immutable host-created
request context. The host supplies independent expected runtime claim IDs,
documentary snapshot digest, permitted profile/verifier, authorization scope,
and limits. Untrusted evidence bytes are supplied separately and cannot choose
their own expected pins. Empty evidence is explicit for evidence-free operations;
an operation requiring authority fails closed when its evidence is missing.

Use #59's validated context representation, with an explicit versioned extension
if needed. Admit runtime evidence only through existing complete bounded replay
and opaque verifier-issued handles. Pin documentary evidence independently.
Neither caller-set `verified` flags nor certificate IDs alone are authority.

Thread the same context through `run`, `why`, `audit`, `verify`, nested helper
evaluation and applicable world-query/evidence calls. No mutable global registry,
environment-variable switching, or thread-local default may select request
authority. A shared cache must key all scope/profile/pin/revision bindings and
retain verification validity; cache hits cannot bypass admission checks.

Canonical context identity appears in audit artifacts and replay inputs/results.
Verification independently compares the expected context and checks its evidence;
copying an ID from the bundle is not authentication. Include the validated
material needed for portable replay or explicitly bound resolvable references.
Missing references fail closed. Do not serialize host secrets as context content.

Provide Python SDK request-context construction and explicit passing on all four
operations, matching Rust/stdio behavior. Distinguish a trusted host constructor
from an untrusted wire decoder. For standalone CLI compatibility, environment
variables may be read once into an explicit invocation context; server requests
must never inherit ambient trust. Define legacy-bundle verification separately
without silently relabelling old context IDs.

Acceptance: interleave two contradictory trust contexts on the same server/client
and show isolated results; cancellation and exceptions cannot leak state; wrong
pin, forged bytes, missing bundle, changed revision/verifier, unsupported context,
and budget exhaustion all fail closed. Generate and replay audits in separate
processes under correct and incorrect host expectations. Test the Python API,
not only the lower-level Rust verifier.

Starting points: `crates/brix-cli/src/commands/mod.rs`,
`crates/brix-lower/src/evidence.rs`, `crates/brix-kb/src/world/decision_evidence.rs`,
`crates/brix-cli/tests/evidence_environment_review.rs`, and `bindings/python/brix/client.py`.

## 7. A9-5 — honest string admission

The minimum alpha.9 requirement is a check-time error for unsupported `Str + Str`
and `Str < Str`, including helper calls and world expressions that currently
reach the first batch before failing. Reject the operator/type combination at
the actual world admission boundary; a standalone checker test is insufficient.
Preserve supported equality and existing internal canonical ordering.

An optional bounded library can supersede the relevant rejection only if its
semantics are specified and tested: `concat`, `starts_with`, `contains`, `lower`;
typed arguments; maximum input/output UTF-8 bytes and cumulative work; explicit
overflow/exhaustion; and a versioned normalization/case-mapping profile. Proposed
initial normalization is identity (no implicit normalization), with any Unicode
case mapping pinned to a stated data version and independent of locale. Library
availability does not itself authorize overloaded `+` or `<`.

Gate the minimum rejection path even if the library is deferred. Library tests
cover empty strings, Unicode multi-byte/case-expansion cases, canonical identity,
cold persistence and replay. Later lexical search depends on the library, not
merely on this release's ability to reject unsupported operations.

## 8. A9-6 — collection order and explicit truncation

Full `collect(value, sort_key, limit)` retains atomic overflow rejection. Equal
sort keys are ordered by the child's declared typed key using the versioned key
comparator, not a provenance hash. For `Str`, the proposed comparator is lexical
UTF-8 byte order; composite keys compare components lexicographically. Resolve
ties across source relations by qualified source identity. Derived/join children
use a canonical structured source-key tuple; if no stable key can be derived,
require an explicit child key or reject the form. Never fall back silently to a
hash. Preserve multiplicity for different child keys with equal projected values.

**Proposed surface:** `collect(value, sort_key).take(n)` as a grouped projection,
returning `{ items: List<T>, truncated: Bool }`. This is a separate bounded-output
form, not a reinterpretation of the three-argument full collection. `n` is a
non-negative checked integer with an explicit output bound; `take(0)` returns an
empty list and reports truncation iff the existing group has members.

Return the first n children under exactly the full-collection comparator.
`truncated` is true iff the number of supported children exceeds n, counting
distinct child identities rather than distinct projected values. Exceeding n
succeeds with truncation. Exceeding an independent admitted input/work/storage
budget still aborts atomically; truncation does not excuse incomplete search.

Retain sufficient support and ordered state to promote the next child when a
retained member is retracted. Do not discard the tail irrecoverably. Explain
truncation and its limit in typed output/evidence without exposing hidden scope
members. Both the retained list and the flag survive cold restore and replay.

Acceptance: duplicate sort keys with intentionally different hash/key orders;
insertion permutations; member replacement; boundary n-1/n/n+1; n=0; removal
inside and outside the retained prefix; truncated-to-complete transition; final
child; equal projected values; composite child keys; cold restore and oracle
comparison after each edit. Version the changed tie-order semantics and affected
state/lowering identities; preserve verification of alpha.8 historical artifacts.

## 9. A9-7 — scale qualification and size budget

Atlas's reported alpha.8 baseline is **91.5 MB on disk, 2.3 s load, approximately
45 ms per relation on a cold read at 7.8k facts**. Units, machine, cache state,
history shape and workload must be recorded before making direct comparisons.
Existing `docs/planning/r0-baseline-bench.md` is a different workload, not an
Atlas-equivalent baseline.

Publish reproducible results at **8,000, 80,000 and 800,000 source facts** using
both a fixed-seed Atlas-shaped public fixture and, when available, the actual
Atlas model with a shareable workload description/digest. Include balanced and
skewed groups, at least one 5,000-child group, unions, nested joins, decisions,
and scoped named queries. Record source and derived row counts separately.

Report release binary/commit/hash, machine/OS/toolchain, fixture seed/hash,
batch shape, retained revisions, storage settings and all commands. Measure:

| Metric | Required reporting |
|---|---|
| Load | Initialization separately from total insertion/settlement wall time; single and ten-batch profiles |
| Disk | Logical and allocated bytes; source, journal/history, live state/indexes and scratch breakdown; before/after any compaction separately |
| Cold open | Fresh-process startup and world-open times separately; label OS-cache state honestly |
| Query | First-page and complete-result latency/rows/bytes per representative relation/named query; warm versus fresh-process results |
| Memory | Peak RSS during load, cold open and query in separate processes |
| Incremental edits | One-row add/retract latency and work counters with unrelated world size increasing |

Use at least five fresh-process repetitions per size/profile; retain raw JSON
and report median, range and sample count. Do not call a new process a cleared
OS cache. Timeouts/OOMs are results, not omitted points; all three sizes must
complete for release qualification. Benchmark budgets cannot skip correctness
checks; verify stores and oracle equivalence outside timed intervals.

**Proposed initial disk acceptance budget:** total world logical bytes after
ten equal insert batches, retaining all ten revisions, at most
`16 MiB + 12 KiB × source_fact_count` for the specified public workload. This is
approximately 109.75 MiB / 953.5 MiB / 9.17 GiB at the three sizes. It is a release
target, not a measured result or a universal bound over arbitrary fact payloads
and history. Fix the fixture generator, payload-size distribution and history
profile before ratifying or measuring this target. Count all retained required files;
no undisclosed compaction/history deletion. If the target is missed, report the
breakdown and obtain an explicit budget revision before calling the gate met.

No latency or memory improvement percentage is claimed without comparable
alpha.8/alpha.9 runs. Publish the report and machine-readable results with the
release artifact; implementation work may capture a baseline earlier, but final
qualification uses the exact release candidate.

## 10. Compatibility and release evidence

Preserve existing root/context identities, certificate ordinals and unaffected
single-field decision identities. Inventory changes to plan, state-value,
network-state, cursor, request-context, composite-key and audit schemas. Any
changed canonical meaning gets a documented version transition and independent
vectors; historical revisions and evidence are never rewritten in place.

Retain explicit legacy decode/verify behavior where supported. A stale compatible
cache may rebuild from verified bound inputs; corrupt data fails. A format
upgrade must not make old queries silently use new ordering or trust semantics.
Record supported alpha.8 read/replay/upgrade paths and test them with real old
fixtures, not newly encoded approximations.

Release evidence includes:

1. Foundation commits, reviewed conformance status and concrete #52/#59 adapter.
2. The A9-1 boundary matrix, separate-process verification and unchanged-HEAD
   failures, plus a statement of the diagnosed defect and affected versions.
3. A9-2–6 positive, hostile-input, incremental/oracle, transport and cold-replay
   matrices; actual index-work assertions and concurrent request isolation.
4. Alpha.8 compatibility fixtures, independent canonical vectors and explicit
   normative rulings for changed meanings.
5. The repository release gates required by CONTRIBUTING: formatting, workspace
   clippy, workspace tests including doctests, TCB and
   repository canonical/dependency gates; Python tests; packaged CLI/stdio smoke.
6. The A9-7 report/raw data and Atlas downstream parity on the release binary.

Use existing regression suites including `world_grouped_collect`,
`world_operator_network`, `world_pinned_query`, `world_query_scope_review`,
`world_decision_evidence`, `evidence_environment_review` and Python query tests.
Extend the independent oracle, rather than making it call the optimized operator
under test. Assign stable acceptance IDs under A9-1–7 in the delivery ledger.

## 11. Decisions to ratify before dependent implementation

The recommendations above make the draft reviewable. They are not implicit
approval of new language or frozen-format semantics.

| Decision | Proposed ruling | Affected slice |
|---|---|---|
| Union equality/key conflicts | Full typed-row `union distinct`; preserve plain disjoint union | A9-2 |
| Composite decision key | Ordered typed tuple; preserve old single-key encoding | A9-2 |
| Scope model and query surface | Host-authorized partition profile; enforce before relational operators; finalize grammar/wire schema | A9-3 |
| Request context profile | Extend #59 as needed; host expectations separate from untrusted evidence; explicit legacy policy | A9-3/4 |
| String scope | Mandatory check-time rejection; library separately admitted with pinned normalization | A9-5 |
| Collect order/truncation | Versioned typed child-key order; separate `.take(n)` result with `items` and `truncated` | A9-6 |
| Benchmark budget | Specified workload/history, `16 MiB + 12 KiB/fact` target | A9-7 |

Land required ADR amendments/errata before implementing their dependent changes.
The corruption reproduction, regression harness and behavior-preserving repair
can proceed without deciding these new language/API questions.

## 12. Deferred work

After alpha.9: bounded graph operations for process models (ordered paths,
branches and provenance, explicit cycle policy, cumulative budgets), followed
by native lexical search once bounded strings and normalization exist. Neither
adds general recursion or unbounded search by implication. General context
transport, universal completeness checking and a workspace-wide invalidation
engine remain separately scoped where the foundation issues do not deliver them.

## 13. Integration ledger — 2026-10-31

This ledger records integration status rather than ratifying the proposed
language and authority changes in §11.

| Slice | Available work | Integration status / next gate |
|---|---|---|
| F | PR #346, `work/foundation-52-59`; follow-up `2cdeebc` | All 2,158 workspace tests and doctests passed locally; workspace clippy, formatting, canon and dependency/traceability checks passed. Context-report binding and additive record identity coverage are included; CI on this follow-up is tracked in the PR. |
| A9-1 | `5b28a8f`, `work/a9-1-collect-corruption` | Implemented repair and regression matrix; qualify together with F before release. |
| A9-2 | Published source `f18a730`, built from Gemini's implementation and Luna corrections | Union schemas/distinct union, composite keys, nested indexes and key transport fixes are integrated onto F + A9-1. Combined verification passed; see §14. §11 rulings remain explicit. |
| A9-3/4 | Scoped queries and request evidence | Consume the same V2 context validator. Add an explicit supported host-scope/evidence profile and actual request-path tests; the foundation adapter does not supply these features. |
| A9-5–7 | String admission, collection semantics and benchmarks | Retain the order and acceptance gates in §§7–9. |

The foundation discovery API binds context, execution snapshot and policy.
`ResolutionReport` retains an operational classification and the caller's goal;
`WitnessProvider` currently receives `ExecConfig`, so discovery does not establish
that its candidates solve that goal. No production completeness proof is
available. New query code must establish its own query-to-plan binding and must
not infer proof authority from this report.

Arithmetic bridge closure independently checks the accepted conditional typing
proposition. Remaining literal/structural antecedents must remain visible; a
typing certificate does not certify execution. General typing invalidation and
unrestricted context transport remain follow-up engine work rather than hidden
alpha.9 prerequisites.

Keep Gemini's active worktrees intact. Prepare and qualify a separate
`work/alpha9-base` branch containing F plus A9-1, then integrate that branch into
the feature branches when their owners are ready. Record any conflict
resolution and the exact tested commit before publishing release evidence.

The combined code is available at `69dae9c` in `/private/tmp/brix-alpha9-base`.
It contains foundation `2cdeebc` and A9-1's `5b28a8f` patch with no merge
conflicts. The combined collection qualification is recorded separately below;
neither an integrated branch nor a successful foundation test run qualifies the
remaining A9-2–7 features.

At that combined code revision, 2,144 remaining workspace tests and all doctests
passed, with the three collection binaries exercised separately. Grouped collect
(6 tests), list values (9 tests) and 14 scale cases passed in debug mode.
`collect_size_5000` passed its full lifecycle in release mode in 469.01 seconds;
the debug attempt was interrupted after roughly 30 minutes without a result.
Together these runs cover all 2,174 non-ignored tests, with the upper-size case
qualified only in release mode. The complete 5,000-member run used the original
test harness and production code at `1748a6a` (unchanged from `69dae9c`).
Workspace clippy, formatting, the 69-vector
canon cross-check, TCB dependency inventory and semantic-law traceability passed.

The actual CLI 257-member reproduction passed separate-process
`world open-check --verify-store` and typed reads. An overflowing batch including
an unrelated write left HEAD and query output unchanged, and a subsequent
retraction plus cold read succeeded. These smaller checks do not replace the
5,000-member release gate. Keep the A9-1 integration PR in draft until the
required CI checks and review complete. The long debug runtime remains a
qualification cost to address before adding this matrix to an ordinary CI run.

## 14. Reviewed basis through A9-2 — 2026-11-01

`work/alpha9-base` combines the #52/#53/#59 foundation, A9-1 and reviewed A9-2
implementation. Published source `f18a730` has identical production code to the
locally tested revision `f579d40`; only the workflow optimization/test-ignore
attribute was withheld and this ledger updated. PR #347 tracks this incremental
basis, stacked on foundation PR #346. This is not the final alpha.9
release: A9-3–7 and final release qualification remain outstanding.

The reviewed fixes preserve JSON-looking scalar IDs byte-for-byte and use
caller-explicit typed transport for composite evidence expectations. Wrong key
values and component types remain rejected. Direct Rust JSON, saved portable
evidence, CLI/stdio responses and nested upstream links retain the declared key
shape. Presentation metadata is excluded from canonical encodings, proof
identities, logical equality and legacy debug layouts. Numeric type tags remain
explicit, distinguishing F64, Decimal and Str components.

Single-key decision fingerprints retain their exact alpha.8 encoding. The six
frozen semantic/history tests pass with unchanged expected hashes.

Verification evidence:

- At `f579d40`, all **2,192 ordinary debug workspace tests** and doctests passed.
  That run skipped the 5,000-member lifecycle and the 28 existing ignored cases.
  The large lifecycle was qualified separately in release mode. The published tree restores it as
  an ordinary runnable test; its production code is unchanged from the tested
  revision. The published workflow is unchanged.
- All **21 Python SDK tests**, workspace clippy, formatting, whitespace checks,
  the 69-vector independent canon cross-check, TCB dependency inventory and
  semantic-law traceability passed.
- The complete **16-case release collection matrix** passed at `2f82bf8` in
  391.39 seconds. Subsequent changes concern presentation metadata and JSON type
  tags; decision, restore and frozen-history coverage was rerun in the final
  workspace gate. A proposed dedicated release-mode CI job is saved separately
  at `/private/tmp/brix-a9-release-scale-ci.patch`: the current GitHub OAuth token
  lacks workflow-write scope and the SSH signing agent refused publication.
  The optimization was withheld from the published tree, preserving the current
  workflow and its default collection coverage. Debug execution of the 5,000-row
  lifecycle remains slow; release qualification is the recorded completed gate.
- An A9-1-created world successfully cold-opened, verified its store and retained
  derived rows and single-key settlements under the A9-2 CLI.

Gemini's A9-3 worktree is `/private/tmp/brix-a9-3` on
`work/a9-3-scoped-queries`, initially at `171df1d` (Gemini A9-2 plus the earlier
`dae5c72` basis). Bring the updated `work/alpha9-base` into that feature branch at
the owner's next safe checkpoint; the earlier basis lacks these review fixes.
Retain the shared V2 context validator and the host-scope-before-operators rules
in §5. Active feature worktrees are preserved during release-basis integration.

## 15. A9-3 implementation checkpoint (2026-11-01)

Codex and Luna continued from the reviewed `b1f65e1` A9-1/A9-2 foundation.
The initial supported named-query profile now has source declarations, inferred
result schemas, exact `Str`/`Int`/`Bool` arguments, host-owned partition/global
policy, real V2 context validation against saved program and pinned revision
artifacts, bound continuation cursors, and an ephemeral scoped query network.
Both read-only open and query evaluation avoid global network replay; unrelated
relations and decisions are pruned before execution. Decision dependencies are
explicitly rejected by this initial profile rather than reusing global winners.

The CLI surface is `world query-named ... --host-scope FILE`; stdio requires
explicit trusted-host startup. Python supplies an immutable host policy from
its client constructor separately from ordinary query parameters. The host owns
that channel and authenticates external callers. See
[`a9-scoped-queries.md`](../docs/planning/a9-scoped-queries.md) and the proposed
[profile ruling](errata/0006-a9-3-host-scoped-query-profile.md) for exact supported
behavior and limits. Physical scan budgets count all scanned source rows; one
storage node may be decoded before the tuple-byte budget is checked.

Qualification covered the entire workspace (2,212 ordinary unit/integration
checks plus 13 doctests), with the expensive 5,000-child lifecycle run separately
in release mode (passed). Workspace Clippy with warnings denied, TCB inventory,
SOC law-map checks and 29 Python tests passed. Final pruning/reserved-name and
actual read-only-filesystem changes received affected syntax/lowering, CLI,
scoped-world and SDK reruns. Existing frozen vectors and histories were not
updated. This checkpoint is A9-3 work, not the alpha.9 release: A9-4 through
A9-7 and final series qualification remain outstanding.
