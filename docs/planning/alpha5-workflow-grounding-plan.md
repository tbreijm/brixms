# Alpha.5 workflow grounding implementation plan

Status: **Proposed implementation handoff**, 2026-10-04. Intended implementer:
Luna, working in small reviewed changes. This is a plan, not shipped support or
an accepted semantic specification. Alpha.4's full P8 qualification remains a
prerequisite; this document does not move unfinished alpha.4 work into alpha.5.

## 1. Release objective

Make Brix a shared grounding layer for different LLM-powered workflows. A
workflow can obtain bounded context from a particular world revision, attach
observations to their evidence, propose a change, and have Brix revalidate and
commit it. A second workflow can inspect the same evidence and decisions.
Historical verification uses recorded model responses without recalling a model.

The release promise is: **a workflow can identify what it knew, where that
knowledge came from, what changed, and whether its proposed change is still
valid.** Brix verifies declared relationships and constraints over recorded
inputs. It does not certify that an external source or model interpretation is
true, or that unrestricted generated prose faithfully states every fact.

The required demonstration has three workflows over one service-incident world:
intake, remediation planning, and review. They use at least two interchangeable
model adapters, with deterministic offline fixtures for release qualification.
Jev is a useful optional live adapter; credentials and vendor availability must
not be required to install Brix, run CI, or verify a historical run.

## 2. Scope and boundaries

### Required capabilities

1. Source artifacts and typed claims with provenance, correction and retraction.
2. Named, parameterized context views with revision pins, evidence references,
   explicit coverage, and hard work/output budgets.
3. A common library facade, CLI/stdio methods, Python client, and local MCP adapter.
4. Typed proposals, bounded previews, policy evaluation, and atomic commit with
   stale-read protection and idempotency.
5. Dependency-aware invalidation and durable change feeds, including absent-key
   and empty-index-match dependencies.
6. Durable model-attempt records and offline grounding verification.
7. End-to-end examples, adversarial tests and measured locality through the
   public interfaces.

### Deferred capabilities

Alpha.5 does not add a general agent scheduler, arbitrary SQL, a vector database,
automatic ontology induction, probabilistic theorem proving, automatic checking
of arbitrary natural-language answers, distributed writers, or a hosted
multi-tenant service. Existing retrieval systems may supply source artifacts;
semantic search ranking is not authoritative evidence.

Keep external effects in host adapters. Alpha.5 demonstrates durable action
intents and retry handling with a fake executor; it does not promise exactly-once
delivery to arbitrary remote systems. Native TypeScript bindings and a broad
connector catalog can follow the Python/MCP paths.

## 3. Start from the released alpha.4 interfaces

Read [CONTRIBUTING](../../CONTRIBUTING.md), the
[world runtime plan](persistent-world-runtime-plan.md),
[P6 audit contract](p6-audit-contract.md),
[ADR-0046](../../spec/adr/ADR-0046_Persistent_World_Runtime.md),
[ADR-0044](../../spec/adr/ADR-0044_Serve_Protocol.md),
[numeric semantics](../../spec/adr/ADR-0045_Explicit_Numeric_Arithmetic.md), and
[beta compatibility contract](../../spec/Beta_Contract.md).

At planning time, relevant seams include:

| Existing seam | Reuse and verify before implementation |
| --- | --- |
| `brix-kb/src/world/session.rs` | Atomic multi-relation `WorldBatch`, writer lock, pinned snapshots, query pages, index pages, revision diffs |
| `world/revision.rs` and the final alpha.4 audit implementation | Revision/program pins, historical decisions, independent verification and checkpoint scope |
| `world/network.rs`, `world/reference.rs` | Maintained evaluation, independent relational evaluation, affected-result reporting |
| `brix-lower/src/relation_dag.rs`, `world_expr.rs` | Existing relational operators and shared checked scalar semantics |
| `brix-canon`, `soc-core/src/store.rs` | Canonical identity, persistent indexes, verified object storage and physical cost meters |
| `brix-cli/src/serve.rs`, `commands/world.rs` | CLI/stdio parity, existing failure envelopes and exit codes |
| `bindings/python/brix/client.py` | Existing subprocess transport and timeout semantics |

These are anchors, not instructions to copy current prototypes. Record the actual
alpha.4 tag and commit in the F0 implementation results. Confirm historical
derived reads, independent audit, operational limits and read pins against the
released implementation. Missing alpha.4 requirements block starting dependent
alpha.5 work. New alpha.5 integration seams belong in explicit tasks below.

Use a new `crates/brix-kb/src/grounding/` facade over the world runtime. Keep
provider HTTP calls and MCP transport outside the trusted evaluator. Draft the
next available ADR without assuming ADR-0047 is still free. Adopt additive,
versioned formats; never reinterpret finite-profile or alpha.4 artifacts.

## 4. Contracts to implement

The choices below are proposed defaults for the implementation ADR. Resolve real
conflicts with accepted specifications through the repository's erratum process.
Routine implementation choices do not require repeated user confirmation.

### 4.1 Identity and persistence

All semantic identifiers use domain-separated, versioned `brix-canon` encodings.
JSON is transport only. Reject duplicate fields, ambiguous key encodings, unknown
semantic fields, invalid numeric values and out-of-budget payloads. Specify
canonical order, optional-field encoding and domain separation with vectors before
building adapters. Numeric values retain ADR-0045 types and checked semantics.

Reserve draft tags `brix.grounding.source@1`, `observation@1`, `transition@1`,
`view@1`, `context@1`, `proposal@1`, `validation@1`, `run@1` and `audit@1`, each
with the full `brix.grounding.` prefix. F1a specifies complete field encodings;
these names do not authorize updating existing frozen vectors. Include an
initialized world-instance identifier and revision digest wherever a receipt or
cursor refers to a particular world. Sequence numbers alone are not identity.

Represent grounding metadata as explicitly declared, runtime-owned world
relations, addressed under a reserved `brix_grounding` namespace. Register their
schemas and indexes in the grounding profile's pinned manifest. They use the
same transaction and audit path as domain facts; there is no independently
committed mutable side database. Use a **profile-owned relation catalog**,
canonically bound into a new additive manifest/profile version. Domain relations
must still exactly match executable declarations; only registered metadata
relations are admitted additionally. Do not inject generated source into the
user's program to defeat reachability pruning. Independent verification receives
the catalog, checks metadata transitions and reconstructs all relation roots;
metadata is not silently omitted from audit because it is runtime-owned.

Large source/request/response bodies live in a bounded content-addressed artifact
store. Persist and fsync referenced blobs before publishing their metadata in a
world batch. A failed commit may leave unreachable blobs, never visible metadata
referencing missing bodies. Retention observes live revisions, contexts, runs and
audit pins. Charge storage growth and use alpha.4's maintenance/admission controls.

Grounding initialization creates a new world with domain and metadata declarations
already present. Attaching grounding to an existing alpha.4 world requires an
explicit export/import into a new directory; preserve the original directory,
identities and provenance. No implicit manifest rewrite on open.

Metadata commits advance the world revision, but must not invalidate a context
merely because its own receipt or model response was recorded. Dependency
validation compares the actual domain reads plus program/profile/policy identity,
not just the latest global revision number. Under the writer lock, the final
batch still uses the current revision as its expected base.

Persist context definitions/tickets, observation transitions, proposals,
validation receipts, model attempts, idempotency results, subscriptions, feed
events and action intents in these versioned relations. Large immutable bodies
are referenced artifacts. Mutable lifecycle rows have append-only transition
records, so the retained world revisions can reconstruct old states. Index
idempotency and subscription lookups; never rebuild them by scanning all history
on every workflow operation or restart.

The integration seam is one internal `WorldSession` transaction coordinator:
under its existing writer lock it validates read witnesses, stages domain changes
and required decisions, stages metadata/invalidations, then publishes through the
existing durability path. Factor a prepare/inspect/finalize seam if necessary.
Do not call `apply_batch` and then separately save the receipt, and do not copy
the HEAD publication algorithm into the grounding module.

### 4.2 Sources and observations

| Record | Minimum semantic contents |
| --- | --- |
| `SourceArtifact` | Content digest, media type, byte length, supplied origin label, explicit observation timestamp and optional source-supplied effective time |
| `EvidenceRef` | Source digest and a validated UTF-8 byte range or structured field pointer; full-source references are explicit |
| `Observation` | Subject/entity, declared claim type, typed payload, evidence references, producer identity, optional model attempt, optional probabilities and validity interval |
| `ObservationTransition` | Observation ID, operation, actor, policy pin, reason, optional superseded ID, committed revision |

Observation payloads are immutable. Lifecycle transitions are `proposed ->
accepted | rejected` and `accepted -> retracted | superseded`. A correction
creates a new observation and links the old one; it does not edit old evidence.
Conflicting accepted observations remain separately attributable. Domain policy
chooses how to resolve or escalate them; insertion order is never that policy.

Keep observation acceptance, model confidence, source attribution and Brix
verification grade as different fields. Only the appropriate verifier can issue
verification outcomes. A model cannot assert that its own response is `Audited`.

An explicit host-configured policy decides whether observations may be accepted
automatically. Otherwise they remain proposals. Accepting an observation and
applying its declared domain mapping happen in the same transaction. Mapping is
typed and deterministic, not arbitrary code supplied in the response. Derived
facts with independent surviving support must survive withdrawal of one claim.

Start with plain UTF-8 text and strict structured JSON import. CSV schema mapping
can be a subsequent adapter. Evidence span validation proves source inclusion,
not semantic entailment of the extracted claim. No automatic fetching of arbitrary
URLs embedded in model output.

Text spans are zero-based, end-exclusive UTF-8 byte offsets on immutable original
bytes. JSON references use RFC 6901 string-form pointers: decode `~0` and `~1`,
support zero-based array indices, reject invalid escapes and nonexisting targets;
duplicate-key JSON refuses. Keep raw-byte content identity separate from source
metadata identity. Initially admit `text/plain` and `application/json` with UTF-8;
reject other media/encoding combinations rather than guessing normalization.

`observed_at` is the recorded host capture time; `source_effective_at` is an
attributed source claim. Time-dependent policy evaluates an explicit recorded
`as_of` input. Clock reads belong to event capture and lease management, not a
hidden dependency of deterministic replay. Accepted observations cannot transition
to rejected; use retraction or supersession. All edges check host capabilities and
the pinned transition policy and use the same idempotency rules as proposals.

### 4.3 Context views and read dependencies

A `ContextViewSpec` is a versioned, host-registered definition with typed
parameters, named output fields, evidence expansion rules, declared budgets and
allowed domain scope. The model invokes a registered view; it does not submit an
unbounded executable query. Compile once per definition/program identity.

Initial view operators are keyed lookups, indexed equality selections and bounded
joins/projections over the alpha.4 relational fragment. Reuse lowering/evaluation;
do not implement another scalar or relational language. A versioned manifest may
name and parameterize existing relations/operators. Unsupported plans fail during
registration. Unindexed scans require explicit admission and are fully metered.

Initial configurable operational defaults, to measure and tune in F8: 128 output
rows and 64 KiB per context page, 32 KiB expanded evidence per page, 100,000 query
work units and 4,096 dependency witnesses per context operation; source artifacts
up to 16 MiB through 64 KiB upload chunks; 100 proposed domain operations per
proposal. These are per-operation admission limits, not world-size caps. Record
effective limits; never accept higher limits from an untrusted artifact. F0 also
sets aggregate storage, concurrent attempt, pin-retention and maintenance limits
for the qualification host. An unfinished upload cannot reserve unlimited space.

`ContextBundle` contains:

- world instance, pinned revision digest, program/profile/view identities and
  canonical parameters;
- authorized structured rows, evidence references and relevant policy results;
- `coverage = complete | partial | unknown`, with field-level missing/conflict
  reasons, pagination state and explicit truncation;
- an opaque server-issued dependency ticket and a digest of the exact delivered
  context; a deterministic text rendering is an optional presentation;
- counters for source probes, operator work, evidence bytes and serialized bytes.

Hard budgets cover query work, result rows, evidence bytes, total output bytes and
dependency records. A caller may request an additional tokenizer-specific token
budget, but byte/work limits remain the enforceable baseline. Budget overflow
must not silently drop required policy facts. Unavailable required evidence yields
`unknown`; an intentionally paginated result is `partial`. Empty results mean
absence only within the authorized, completed query scope.

Track key reads including missing keys, equality-index buckets including empty
buckets, and dependencies of joins and derived rows. Reading a result that does
not yet exist must subscribe to possible insertions. Filtering by permissions is
part of the query definition and dependency identity. Unsupported predicate
tracking may temporarily use a whole-relation root with an explicit conservative
invalidation flag; it cannot earn the precise-locality release gate.

The dependency ticket is an opaque random handle for a **server-side immutable
record**, not a client-authored or self-signed claim. Bind world instance, principal,
capability digest, base revision digest, program/profile/policy/view digests,
parameters, delivered context digest/coverage, expiry and exact read witnesses.
Witness variants initially are `KeyValue(relation,key,value_digest)`,
`KeyAbsent(relation,key)`, `EqualityBucket(index,typed_value,membership_root)` and
the explicitly conservative `RelationRoot(relation,root)`. Include dependencies
of every derived result and policy read, not just displayed rows. Unsupported
range plans use the conservative variant or refuse. Store the ticket with the
context receipt; callers receive the handle and auditable record digest.

Freshness means **the recorded read set still agrees with current state**, not
that global HEAD is unchanged. At commit, load the trusted ticket, check its
scope/definitions, compare each witness against current roots under the writer
lock, and stage at current HEAD. Ordinary run/receipt writes are not domain read
dependencies. Metadata deliberately read by a view, such as observation acceptance
or staleness, does count and must invalidate that view when changed.

Pinned pages and lazy evidence expansion continue at the original revision.
Contexts referencing expired/unavailable pins return a typed failure; they never
silently switch to HEAD. Use bounded pin leases and explicit durable retention for
archived runs, with observable expiry and storage accounting.

### 4.4 Proposals and validation

A `ChangeProposal` binds an immutable normalized operation set to its context
ticket(s), author, reason/evidence, write scope, policy identity and idempotency
key. Models can propose writes only to configured domain operations. Reserved
metadata, program changes, permissions and verification grades are unavailable as
generic workflow mutations.

States are `draft -> validated | rejected | unknown | stale`, followed by
`validated -> committed | stale | rejected | unknown`. Preview and validation
are bound to a proposal digest and evaluated state; they are not transferable
approval tokens for a modified proposal.

`preview` stages changes using persistent snapshots, evaluates affected decisions
and emits a bounded before/after diff. It changes no live domain facts and emits
no external effects. Preview may record its own metadata receipt. Hidden affected
facts are checked internally but must not leak through explanations or counts.

`commit` acquires the writer lock and then:

1. Resolve the authenticated host scope and stored tickets; reject forged,
   expired, cross-world or unauthorized references.
2. Check program, view, policy and relevant data dependencies, including absence
   witnesses. A relevant change returns `stale`; do not silently reinterpret the
   original model response against newer context.
3. Evaluate the proposed delta and all required policy decisions against current
   state. Incomplete evidence, faults or exhaustion cannot produce an allow.
4. Atomically publish domain changes, proposal outcome, run linkage and any action
   intents, using the current world revision as the batch base.

Policies are versioned, pinned Brix programs using the admitted alpha.4 relational
and scalar fragment, with typed inputs for the proposed operations, current
authorized domain state, evidence/coverage status and explicit `as_of`. Their
required decision set is declared in the profile; all must return a recognized
allow for the proposal to pass. A deny rejects; any missing decision, unexpected
output, evaluation fault or budget exhaustion is unknown and refuses commit.
They run in a side-effect-free staged environment and are independently replayed
by the reference evaluator. Arbitrary host callbacks cannot supply an auditable
allow. Host capabilities may further deny an operation, but cannot override an
unknown or denying Brix policy result.

Use the same policy path for preview and commit; commit always rechecks. Host
policies declare which contexts must be complete. Clients cannot opt out of a
policy by marking a context partial or omitting its ticket.

Same idempotency key and payload returns the original receipt, even after a lost
response. Reusing it with another payload rejects. Scope keys to world, principal
and operation. After a timeout, query status or retry the same operation; never
manufacture a new key to guess whether a commit happened.

For optional external actions, commit an outbox intent with a stable delivery ID.
The adapter passes it to executors that support deduplication. States include
pending, acknowledged, failed and outcome-unknown. A crash after sending but before
acknowledgment requires reconciliation; it is not proof that nothing happened.
Replaying a workflow never resends external effects.

### 4.5 Invalidation and model attempts

Maintain reverse subscriptions from reads to contexts/assessments. Following a
commit, emit durable ordered invalidations only for affected subscriptions and
explain the changed dependency within the caller's scope. The feed has a resume
cursor, at-least-once delivery, event IDs and a typed cursor-expired response.
Recovering an expired cursor performs an explicit bounded resynchronization.

Register dependencies and publish invalidation events transactionally with world
metadata. If state changes between the pinned read and subscription registration,
recheck reads before returning a fresh ticket; otherwise return stale. Crashing
between a domain update and notification delivery must not lose its event.
Replace subscriptions atomically on branch changes. Index `(relation,key)` and
`(index,typed_value)` to persistent sets of active subscription IDs; index
conservative subscriptions by relation. Charge large actual subscriber fanout.
Historical run records do not stay in the active subscription set forever.

Feed cursors bind world instance, principal/capability scope, canonical filter,
revision and event ordinal. Event identity/order is deterministic within a
commit. A bounded snapshot token accompanies resynchronization. Raw world diff
pages are not automatically a correctly scoped feed.

Include view, question, model configuration and program changes in invalidation.
A retained assessment is reusable only when every declared input still agrees.
The host assembles the entire model request from recorded context and a versioned
template. Extra tool results, conversation history or other inputs must also be
recorded and declared. A host that cannot supply complete dependency information
must mark the result non-reusable. Do not assume the LLM disclosed all it read.

Invalidation marks an assessment stale; it does not automatically run a model or
erase history. Hosts explicitly request refresh under cost/rate limits. If an
accepted derived observation depends on stale context, affected policies must see
that status until a replacement is accepted. Apply this through maintained
dependencies, not a scan of all old runs. Suppress self-triggering metadata loops.

A `ModelAttempt` records provider, requested and returned model identifiers,
template/options, context and complete request/response artifacts, attempt ID,
timing/usage when available, and outcome. Credentials are excluded. Retries are
distinct attempts linked to one logical request. Late responses remain historical
evidence but cannot authorize a current-state commit with stale dependencies.

An attempt has `pending -> completed | failed | timed_out | cancelled`; a late
response is an additional linked completion event after timeout/cancellation,
never deletion of the earlier outcome. Responses and outputs are immutable.
One logical request may have several attempts; accepting an assessment explicitly
names which attempt and context it used. No last-arrival-wins inference policy.

Jev probabilities and general-LLM structured outputs use the same observation
contract. Do not fabricate confidence values where a provider supplies none.
Do not require model determinism or claim repeat calls reproduce stored answers.

### 4.6 Access and transport

First release deployment: one local world service with scoped clients. The host
binds principal and capabilities to a session; a model-supplied `actor` string
does not grant authority. Capabilities name registered views, evidence scope,
observation types and writable operations. Check authorization before reading,
expanding evidence, previewing, subscribing or writing. The trusted local
administrator remains able to manage the underlying world directly.

Concretely, the host launches each scoped stdio/MCP session with an administrator-
owned scope file through a startup option, outside tool arguments. Principal ID
and allowed view/operation parameters come from that file and bind all calls in
the process. The SDK receives an already configured transport; it cannot change
scope through a method call. Session handles expire on disconnect/restart;
durable tickets may be resolved only by a new session with the same principal
and still-valid capabilities. Revocation changes the scope digest and is rechecked
on every write. This boundary assumes the model cannot modify the service's files
or launch an unrestricted administrator process; it is not an OS sandbox.

Failure details, counts, citations, diffs and invalidation notifications must not
reveal data outside the scope. Do not promise constant-time access or a hardened
remote multi-tenant boundary. Source excerpts are data; text in an artifact cannot
change the host's capabilities or policy.

Use additive `grounding.*` methods in `brix.serve@1`, matching CLI results:

| Surface | Initial operations |
| --- | --- |
| Evidence | `source.put`, `evidence.read` |
| Observations | `observation.propose`, `observation.transition`, `observation.get` |
| Context | `context.query`, `context.page`, `context.status` |
| Proposals | `proposal.create`, `proposal.preview`, `proposal.commit`, `proposal.get` |
| Change feed | `changes.page` |
| Runs | `run.record`, `run.get`, `run.audit`, `run.verify` |

The table abbreviates the `grounding.` prefix. Admin profile/view registration
and initialization are separate CLI configuration operations. Artifact upload and
feed reads are bounded. Freeze argument/result schemas in F1 before adapters.
`run.audit` and `run.verify` are implemented in F7 and advertised by protocol
discovery only when available; F6 must not register placeholder successes.
Reuse ADR-0044's protocol-versus-domain-error distinction. Keep stale, denied,
partial, unknown, expired and idempotent replay distinguishable in typed results.

The MCP adapter is a separate Python process using the existing Brix client. It
implements MCP protocol negotiation, tools and resources; Brix's JSON-lines
protocol is not itself MCP. Pin the supported MCP revision and dependency under
repository dependency policy during F0. Expose typed tools for workflow operations,
revision-addressed resources for context/evidence, and optional resource update
notifications backed by the durable feed. Tools never bypass the facade's checks.

Protocol references: [MCP resources](https://modelcontextprotocol.io/specification/2025-11-25/server/resources)
and [MCP tools](https://modelcontextprotocol.io/specification/2025-11-25/server/tools).
The transport does not supply Brix's evidence, revision or authorization semantics.

### 4.7 Offline verification

A versioned grounding run bundle contains the necessary alpha.4 world evidence,
profile/view/policy definitions, delivered contexts and dependency tickets,
recorded model artifacts, observations, proposals and receipts. Export has an
explicit disclosure scope. Missing confidential evidence produces an incomplete
verification scope, not a full success over redacted material.

The verifier takes external world/program/run pins and local resource limits.
It reconstructs context and declared policy outcomes using the alpha.4 independent
reference path, verifies source excerpts, artifact digests, transitions, commit
preconditions and receipt linkage. It must not trust cached context, fast indexes,
support tables or a producer's claimed verification grade. Checkpoint trust and
scope follow the released alpha.4 contract.

Report separately: artifact integrity, world verification scope, context
reconstruction, policy/commit verification, and external claims that remain
unverified. A valid source excerpt is not proof of a model's interpretation. No
network access, fresh inference or external effects are required during replay.

## 5. Luna implementation sequence

Each row is a bounded lane, not permission for one large patch. Split at the
listed seams into changes of roughly 500 non-generated lines or less, following
CONTRIBUTING. Prefer one library change and its regression tests per review. Run
targeted tests before the lane's integration checks; do not mark gates from mocks
of the component being qualified.

| Task | Depends on | Owned files or seams | Concrete deliverable and acceptance |
| --- | --- | --- | --- |
| F0 Baseline and ADR | Released alpha.4 P8 | New grounding ADR; `docs/planning/alpha5-grounding-results.md` | Pin alpha.4 commit, map actual APIs, settle reserved relation registration and protocol versions, record budgets/compatibility. No fabricated audit or historical-read support. |
| F1a Canonical types | F0 | New `grounding/types.rs`, `codec.rs`; canonical tests | Typed IDs, records, statuses and envelopes; canonical vectors and strict decoder negatives. Different profiles/contexts/proposals cannot collide by omitted fields. |
| F1b Metadata transaction facade | F1a | New `grounding/store.rs`, `mod.rs`; narrow session integration | Profile-owned catalog, initialization, and metadata in world transactions. Factor the one prepare/finalize seam. Crash tests prove no partial domain/receipt commit. |
| F1c Artifact storage | F1b | New `grounding/artifacts.rs`; storage tests | Bounded uploads, content verification, durability before metadata, failed-upload cleanup and retention pins. Crash tests prohibit missing referenced blobs. |
| F1d Explicit migration | F1b, F1c | New `grounding/migration.rs`; isolated fixtures | Import existing alpha.4 data into a new grounding world with provenance; old directory/identities unchanged. Refuse unsupported mappings. |
| F2a Sources and claims | F1c | New `grounding/evidence.rs`, `observations.rs` | Validate evidence references, preserve conflicts, transitions and deterministic mapping. Correction/retraction leaves independent support and historical citations intact. |
| F2b View registration | F1a | New `grounding/views.rs`; minimal lowerer seam | Typed parameters and bounded admitted query plans; deterministic definition digest; compiler cache keyed by definition/program; no new evaluator. |
| F3a Context materialization | F2a, F2b | New `grounding/context.rs`; snapshot/query seams | Revision-pinned structured output, pagination, coverage and budgets. Empty, truncated, denied and unknown results remain distinct. |
| F3b Read dependency tickets | F3a | New `grounding/dependencies.rs`; index/revision seams | Track found/missing keys, empty/populated equality buckets, join dependencies and scoped policy/program identity. Forged tickets refuse; own metadata writes do not make every ticket stale. |
| F4a Preview and policy | F3b | New `grounding/proposal.rs`, `policy.rs`; staged evaluator seam | Validate operations; persistent preview and bounded effects; no live domain change. Policy faults or missing mandatory evidence never allow. |
| F4b Commit and retries | F4a | Same proposal module; single integrator owns `world/session.rs` | Check tickets and policy under writer lock, atomically commit outcome and facts. Two competing proposals cannot both pass incompatible preconditions; lost replies replay original receipt. |
| F4c Action intents | F4b | New `grounding/outbox.rs`; fake executor tests | Durable intent and stable delivery ID, retry/unknown/reconcile states. Fake external executor deduplicates; replay does not execute. |
| F5a Invalidation feed | F3b, F4b | New `grounding/invalidation.rs`; maintained subscriptions | Durable paged feed, recovery and scoped diagnostics. Insertion into an empty observed bucket invalidates; unrelated growth does not enumerate subscriptions. |
| F5b Model attempt ledger | F2a, F3b, F5a | New `grounding/runs.rs`; provider fixture helpers | Record complete requests/responses; stale completion and refresh states; request completeness controls reuse; deadlines do not guess commit outcome. |
| F6a CLI and Python facade | F1 contracts; integrate after F4b/F5b | New `brix-cli/src/commands/grounding.rs`; integrator edits `cli.rs`, `serve.rs`, `commands/mod.rs`; Python client | Real binary/stdio tests for each lifecycle, restart, errors and correlation. Existing finite/world methods remain compatible. |
| F6b MCP adapter | F6a | New `bindings/mcp/`; own tests/package metadata | Actual MCP handshake, resources, typed tools, paging and scoped sessions over real Brix; no fake local replacement for dispatch. |
| F7a Run audit producer | F4b, F5b | New `grounding/audit.rs`; CLI/stdio audit seam | Bounded versioned export containing independently reconstructible evidence and explicit scope. Works after restart and directory move. |
| F7b Independent verifier | F7a | New `grounding/verify.rs`; independent tests | Fresh-process offline verification from external pins. Tampered claims/contexts/policies/receipts fail; deliberately broken fast context implementation is caught. |
| F8a Workflow demonstration | F6b, F7b | New `examples/grounded-incidents/`; provider adapters outside core | Intake, planner and reviewer share one world through public APIs. Two adapter contracts tested with offline fixtures; optional live Jev run documented separately. |
| F8b Qualification and packaging | All tasks | New grounding scale tests; smoke script; docs/results | Adversarial and locality matrix below, package smoke, compatibility gates, measured release notes. |

No dates are promised by these task sizes. Alpha.5 is complete when the exit
criteria pass, not when all modules exist.

## 6. Delegation and integration discipline

Luna can implement each lane. A separate integration owner reviews contract
changes, joins the lanes, and runs the cross-feature gates. Before each task:
read current status, diff and applicable ownership instructions; verify no active
Claude/agy worker owns the same files. Use separate worktrees for concurrent
builders. Never reset another worker's changes or let two lanes format/edit the
same central module concurrently.

Suggested waves:

1. F0 then F1a/F1b/F1c sequentially; review format and transaction decisions first.
2. F2a and F2b in parallel; F1d can use a third lane. Then F3a/F3b sequentially.
3. F4a/F4b on the runtime lane; F6a transport scaffolding may use frozen schemas
   in parallel, but does not qualify until it calls the real facade.
4. F4c and F5a in separate files; F5b follows. F6a/F6b finish integration.
5. F7a/F7b sequentially for format availability, with verifier tests authored
   independently. F8a can proceed against the integrated public surface.
6. F8b and an independent adversarial review precede tagging.

Only the integration owner edits shared registration points (`lib.rs`, `mod.rs`,
`cli.rs`, `serve.rs`, workspace manifests) while lanes run. Export names and small
integration patches travel with each lane's handoff. If a builder is blocked on
an unavailable seam, report the concrete dependency; do not substitute a mock and
claim completion.

### Copyable Luna task prompt

```text
Implement task <F#> from docs/planning/alpha5-workflow-grounding-plan.md.
Baseline: <released alpha.4 tag and commit>. Working branch: <task branch>.
Own only: <files>. Shared integration owner: <owner>.
Read CONTRIBUTING.md, the accepted grounding ADR, prerequisite task receipts,
and the actual code before editing. Preserve alpha.4 and finite-profile behavior.
Implement the task's explicit contracts using the existing canonical encoder,
world transactions and scalar evaluator. No provider calls in semantic core.
Keep this change small; split it if the next independent seam exceeds review size.
Add the relevant adversarial regression tests, run targeted tests and report exact
commands/results. Do not claim P8/locality/audit gates from logical counters alone.
Return changed files, behavior, test evidence, known gaps and required integration
edits. Do not edit other lanes, publish releases or weaken acceptance criteria.
```

## 7. Required adversarial and locality gates

| Gate | Required scenario and result |
| --- | --- |
| Evidence binding | Edited source bytes, invalid excerpt boundaries, wrong source digest and nonexistent pointers refuse. A correct excerpt with an unsupported interpretation remains an external claim. |
| Conflicts and corrections | Two sources disagree; both remain visible with attribution. Retract one support; independent support and old revision explanations survive. |
| Model authority | A response requests reserved metadata writes, permission changes or an `Audited` grade; every public adapter refuses equivalently. |
| Stale inference | Change a used fact while a model request is in flight. Its response records successfully as historical evidence but cannot authorize the old proposal. |
| Negative dependencies | Read a missing key or an empty indexed selection; insert a matching row. Invalidate and refuse the stale proposal. An unrelated insertion does not invalidate a precisely tracked view. |
| Policy freshness | Policy/program/capability changes invalidate applicable tickets even when returned fact values are unchanged. Commit rechecks full mandatory scope. |
| Context coverage | Force every row/byte/work/evidence/dependency limit. Pagination never implies completeness; truncated policy data cannot authorize a commit. No silent revision change between pages. |
| Scope isolation | Two host scopes receive different views. Hidden data is not exposed through citations, preview diffs, existence errors, counts or change feeds. |
| Atomicity and contention | Crash around artifact fsync, batch publication and receipt delivery; recover old or complete new state. Race conflicting proposals under the writer lock. |
| Retry and effects | Lost commit response returns original receipt on retry. Changed payload conflicts. Crash after sending action but before acknowledgment yields reconciliation state; replay sends nothing. |
| Selective refresh | Grow unrelated facts 1k/10k/100k/1M and idle subscriptions 100/1k/10k, keeping the affected view fixed. Source probes, invalidation visits and physical allocation/I/O follow affected work plus index depth. |
| Skew and real fanout | Reuse alpha.4's balanced and 99.9/0.1 fixtures; local small-model view stays local. Matching fanout 1/100/10k is charged and bounded, never disguised as constant work. |
| Maintenance | Exercise context expiry, abandoned attempts, repeated refresh, retained history and artifact pins. Reclaim only unreachable artifacts; bounded maintenance or explicit admission refusal prevents hidden world-sized cleanup. |
| Independent replay | Remove caches, move the directory and deny networking. Reconstruct contexts, transitions and policy results. Tamper with each artifact class and ensure no full verification success. |
| Mutation controls | Break fast-view filtering, missing-key invalidation or preview policy handling deliberately in test controls. The independent/differential suites must fail. |
| Provider interchange | Two provider adapters return different assessments over identical context. Both remain attributable; deterministic Brix rules and validation contracts remain unchanged. |

Measure CPU/wall time, peak/resident memory, tuple/index probes, operator work,
physical copies/allocations, bytes read/written, artifact growth and model-call
count. Publish hardware, release build flags, seeds and limits in
`docs/performance/alpha5-grounding-results.md`. Derive structural bounds from the
implemented structures; use regression envelopes for timings, not guessed
millisecond promises. Preserve alpha.4's negative controls and frozen artifacts.

## 8. Release completion checklist

- [ ] Alpha.4 full qualification and tag recorded as the baseline.
- [ ] Grounding ADR and versioned API/encoding vectors reviewed.
- [ ] F1 through F7 contracts implemented and verified through the real facade.
- [ ] All three workflows operate through real SDK/MCP calls with offline fixtures.
- [ ] Independent audit and the full matrix above pass, including armed failures.
- [ ] Existing alpha.4 locality and legacy finite-profile gates remain green.
- [ ] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  and `cargo test --workspace` pass.
- [ ] `python3 scripts/canon_crosscheck.py`,
  `python3 scripts/check_tcb_dependencies.py --check`, and
  `python3 scripts/check_soc_law_map.py` pass.
- [ ] Python SDK and MCP adapter test commands are documented and pass from the
  packaged distribution; package smoke includes context, stale proposal refusal,
  committed change and offline replay without vendor credentials.
- [ ] README, language/API documentation, compatibility scope, changelog and
  measured limitations are updated. Raw observations are never advertised as
  proven facts and external execution is never advertised as exactly once.
- [ ] Independent integration review completed before the release tag.

## 9. First implementation assignment

After alpha.4 is released, give Luna **F0 only**, then F1a. The first handoff must
include the actual baseline, the accepted/draft status of each governing contract,
the metadata relation registration design, exact transaction integration seams,
and the smallest F1a patch boundary. This makes the following lanes executable
without asking Luna to invent the architecture while implementing it.
