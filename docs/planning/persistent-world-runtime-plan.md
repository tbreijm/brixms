# Persistent incremental world runtime — agy implementation handoff

Status: **Proposed build plan**, 2026-10-03. Requested by Tony after
`v0.1.0-alpha.3`; baseline commit `44d46ef`. This document describes work to
build, not capabilities already shipped. Luna reviewed the architectural gaps
and acceptance requirements. Runtime implementation is delegated to agy.

## 1. Objective and completion boundary

Make Brix maintain a large, linked world model through small durable changes.
An update must avoid scanning, copying, hashing, serializing, recompiling, or
re-evaluating unrelated state. Large differences in submodel size or activity
must not force equal-sized partitions or whole-submodel invalidation.

The first complete release of this work must demonstrate:

- one million resident keyed facts, across at least 100 linked model modules;
- a workload with over 4 MiB of canonical data, over 256 records in a relation,
  over 256 linked helpers, and over 128 linked schemas;
- insertion, correction, and retraction through the actual public API;
- persistent restart, incremental derivation, affected decisions and explanations;
- immutable, consistently versioned reads and independent audit verification;
- work that follows the affected dependency closure, including under skew;
- unchanged alpha.3 finite-decision artifacts and behavior in their existing profile.

These are **acceptance workload targets**, not measured capacity claims or new
hard-coded model ceilings. Publish measurements and resource settings with the
release. Finishing a larger input decoder or a new collection alone is not done.

### Complexity contract

Let `D` be changed source tuples, `A` the affected operator work (including
actual join matches and invalidations), `R` changed derived results, and `P`
the storage/index paths touched. The update target is work proportional to
`D + A + R + P`, with documented index overhead. No scan term proportional
to the unrelated world, unrelated modules, all candidates, or all history.

Keep the existing SOC O(Δ) routing gate. Add separate measurements for physical
index work: a counted B-tree lookup is not evidence that its comparisons or
allocations cost O(1). Fixed-width trie paths have bounded depth; ordered
indexes may have logarithmic overhead. Expose that distinction instead of
weakening the constitution or hiding work from counters.

One changed fact may legitimately affect every result. Full export, first
ingestion, cold index construction, complete verification, and a genuinely
global aggregate have their own size-dependent costs; report them separately.
Do not disguise recurring update work as setup or deferred cleanup.

## 2. Baseline: reuse the foundations, replace the full-state paths

| Existing component | What it provides | Gap this work must close |
| --- | --- | --- |
| [`soc-core/src/engine.rs`](../../crates/soc-core/src/engine.rs) | Footprint-indexed provider routing and materialized candidates | Static configuration-handle footprints; provider internals and physical index work are not fully metered |
| [`soc-core/src/store.rs`](../../crates/soc-core/src/store.rs) | `PersistentMap` seam and immutable snapshots | `ArcMap::insert` copies the whole map |
| [`soc-core/src/calendar.rs`](../../crates/soc-core/src/calendar.rs) | Canonical least-key selection and transactional delta semantics | `Frontier::apply_delta` clones the whole frontier |
| [`soc-core/src/commit.rs`](../../crates/soc-core/src/commit.rs) | Settlement semantics and a recomputation reference | `commit_tick` enumerates candidates; it must not become the new incremental hot path |
| [`brix-kb/src/ops.rs`](../../crates/brix-kb/src/ops.rs), [`pipeline.rs`](../../crates/brix-kb/src/pipeline.rs) | Durable revisions, snapshot history and full replay | Updates clone/validate/encode snapshots and rebuild/replay the runtime |
| [`brix-kb/src/deps.rs`](../../crates/brix-kb/src/deps.rs) | Explanatory dependency analysis | Not a sound, maintained runtime subscription index |
| [`brix-lower/src/imports.rs`](../../crates/brix-lower/src/imports.rs) | Transitive imports and cycle/conflict errors | Flat declaration namespace; unused imports count against global caps; no aggregate import budget |
| [`brix-lower/src/l3_v2.rs`](../../crates/brix-lower/src/l3_v2.rs) | Shared scalar evaluator, exact current arithmetic semantics | List expressions are not persistent relation operators |
| [`soc-core/tests/o_delta_gate.rs`](../../crates/soc-core/tests/o_delta_gate.rs) | Armed routing-cost gate and a negative naive baseline | Small static-provider fixture does not cover durable end-to-end updates |

Read before implementation: [CONTRIBUTING](../../CONTRIBUTING.md),
[ADR-0002](../../spec/adr/ADR-0002_SOC_Constitution.md),
[semantic laws](../../spec/SOC_Semantic_Laws.md),
[ADR-0041](../../spec/adr/ADR-0041_Persistent_Knowledge_Base.md),
[ADR-0040](../../spec/adr/ADR-0040_Finite_Relations.md),
[ADR-0043](../../spec/adr/ADR-0043_Per_Entity_Decisions.md),
[ADR-0044](../../spec/adr/ADR-0044_Serve_Protocol.md), and the
[beta contract](../../spec/Beta_Contract.md).

## 3. Decisions to carry into the implementation ADR

Create the next available ADR before changing semantics or artifact formats.
Use the following as the proposed contract; record deviations explicitly.
This plan does not ratify a change to an accepted ADR. Follow the repository's
erratum process for an actual conflict, not for routine implementation choices.

### 3.1 Separate the world runtime from the legacy finite profile

Add an explicitly versioned world profile and storage/API schemas. Keep
`brix.input@1`–`@4`, existing program/context identities, `brix.kb@1`, and
the finite-decision profile readable with their original meanings. Do not
rewrite them in place or raise every constant and call that world support.

The new profile is an additional execution strategy over SOC configurations
and witnesses. Keyed records and relations are a concrete modeling surface,
not a new privileged ontology or a third authority kernel.

Start with one process, local durable storage, and a single writer. Preserve
an independent recomputation implementation. Multi-machine distribution and
consensus are outside this plan; multiple storage partitions are local first.

### 3.2 Separate stable addresses from immutable versions

- A logical model/relation/entity key identifies what can be updated or read.
  Use explicit canonical keys, initially `Int` or `Str`; no approximate keys.
- A value/version digest identifies immutable content. Correcting a logical
  key removes the old version and introduces the new version in one batch.
- Derived tuple identity includes its qualified relation and canonical tuple.
  Multiple derivations support one tuple rather than creating accidental duplicates.
- A revision names coherent source-state, program-graph, result, and history
  roots. An equal logical state can have an equal state root while a different
  transaction history still has a different revision/history identity.

For the new profile, propose a context whose world component is the canonical
source-state root, including pinned connected-model versions. Bind it with the
program-graph identity, assumptions, evaluation profile and recorded semantic
limits. Keep the transaction/history identity in the revision/evidence envelope;
do not include derived-result roots in their own judgement context. Freeze the
exact encoding in P0. Preserve the existing `ContextId::root()` migration anchor.

Dependencies subscribe to logical keys, relation indexes and query predicates,
not merely to old immutable handles. Otherwise an insertion under a new digest
or a previously missing key cannot discover its consumers.

### 3.3 Deterministic persistent storage

Use a path-copying, content-addressed radix trie over canonical key digests
for point-addressed maps. Specify canonical node encoding, path compression,
empty nodes, and collision buckets before freezing a root format. Collision
buckets compare complete canonical keys and have deterministic order; test
collisions with an injectable test hasher. Hash equality is not key equality.

Store immutable values once and reference them from tree leaves. Updates copy
and hash only changed paths. Ordered/range indexes and the calendar need
appropriate ordered structures; do not force lexicographic range scans through
a digest index. An ordered index's representation is rebuildable unless its
canonical form has deliberately been made part of the world profile.

The logical state root must not depend on insertion order, memory addresses,
page splits, physical shard placement, caches, randomized hashing, or thread
interleaving. Canonical logical trees and physical disk packing are distinct.
All semantic encodings go through `brix-canon`; preserve existing frozen vectors.
Any external collection dependency needs the repository's `DEPS.md` review.

### 3.4 Linked modules and explicit model interfaces

Introduce qualified symbols and explicit exported interfaces in the new
profile. A library import brings definitions; a model connection binds typed
output relations to another model's inputs. Do not silently turn existing
`use` into execution of another model's decisions.

Pin transitive dependency content digests in a program-graph manifest. Store
the dependency closure needed for historical replay; do not re-resolve an old
revision from today's mutable package paths. Validate each imported module,
then link the executable reachable definitions. Unused definitions may cost
load/validation work but must not occupy the active rule network's capacity.

Keep the source/provenance manifest and executable-plan identity distinct.
Changing unused source can change the manifest without forcing unrelated
operator recompilation. Neither artifact may omit a dependency it claims to pin.
Cache compilation by content digest, compiler/profile version, and the imported
interfaces it actually depends on. Bounds cover both each module and the full
import graph; resolution is iterative, cycle-checked, and bounded while loading.

### 3.5 First relational fragment

Specify a minimal versioned source surface for keyed input relations, derived
relation rules, exported relations, and per-key decisions. Existing scalar
expressions and nominal record definitions should be reused where possible.
The ADR must include one complete parseable example and its precise lowering;
do not invent a second runtime language in an undocumented JSON manifest.

The first complete profile admits acyclic relation dependencies, selection,
projection, indexed equality joins, distinct/existence, grouped count, and
per-entity decisions. Preserve set membership with explicit derivation supports.
Group existence/count must track zero-to-one and one-to-zero transitions.
Unindexed predicates and scalar helpers may recompute their affected input
region, but that work must be charged and their locality honestly described.

Do not claim a general incremental evaluator for arbitrary recursive helper
functions. Reuse the existing evaluator for declared pure scalar computations,
memoized by arguments and code identity where useful. Initially reject recursive
relation SCCs and stratified negation with a clear unsupported-profile error.
Those require a subsequent fixed-point/deletion contract; support counters alone
are unsound for circular self-support.

Numeric semantics remain those of ADR-0045. No F64 reassociation, approximate
equality, or new numeric-key admission. General numeric sums are a follow-up:
deletion and update order must not change rounding or checked-overflow behavior.
Do not optimize a sum by changing the expression tree and assuming equivalence.

### 3.6 Atomic changes and versioned observation

A proposed batch carries its expected base revision, canonical upserts/removals,
and an idempotency key. Normalize operations deterministically; reject conflicting
duplicate operations rather than taking transport order as an implicit override.
Type/schema errors and stale conflicting bases reject the batch.

Stage source changes, index changes, subscriptions, supports, candidates and
required decisions together. Publish a revision only after the entire affected
settlement scope is complete. On exhaustion return `Unknown` with a resumable
or retryable attempt identity; leave the previously committed revision intact.
Do not advertise the old decision as the successful result of the attempted edit.
This is the **new world transaction contract**, not a silent change to KB v1's
revision-on-unknown behavior.

Persist objects and the revision record, fsync them as required, then atomically
publish the new root/HEAD and fsync its directory. Readers pin one immutable
revision. Recovery sees the old commit or the complete new commit. A crash or
retry must not create duplicate logical transactions or partially advance indexes.

Do not store a mutable foreign submodel pointer in a supposedly immutable world.
Connections pin revisions; advancing a connected model is an explicit delta.
Physical repartitioning alone does not revise the model's semantic state.

### 3.7 Dynamic dependencies and truth maintenance

Maintain direct subscriptions and reverse indexes; do not materialize every
node's transitive dependency closure. Track absent-key and indexed-range reads,
as well as present tuples. A new matching tuple must activate a previously
empty join or false existence predicate without scanning every rule.

Switching a conditional branch replaces the relevant read subscriptions
transactionally. A correction/retraction removes its precise supports; a tuple
survives while another valid derivation supports it. Keep per-provider candidate
supports as well: one provider retracting a candidate cannot remove another
provider's identical contribution. Content-addressed provenance DAGs share
unchanged proof/support fragments; ordinary updates do not expand full histories.

Reuse cached derivation payloads without falsely reusing an old contextual
judgement. The current revision/context remains explicit. Avoid rewriting all
cached entries merely because the global root changed; validate their retained
dependencies and construct contextual result envelopes as needed. Reused payloads
are untrusted inputs to judgement/evidence construction under the exact current
context; an old contextual judgement ID cannot stand for the new one. No cache
hit earns `Audited` or `Proven` authority.

### 3.8 Skew, scheduling and resource policy

Logical submodels are not allocation units. Split large/hot data by relation
and key range or digest prefix; allow small models to share storage machinery.
Keep per-partition counters for rows, bytes and work, and a global memory budget.
Large partitions cannot reserve all available queue or cache capacity.

Use bounded work quanta with a deterministic ready-queue tie-break. Resume large
operators from continuations rather than repeatedly restarting them. Fairness
applies to **computation**; the canonical least-key settlement rule is unchanged.
An unfinished provider that could produce a smaller key prevents that settlement
scope from committing. Never treat partial enumeration as quiescence.

Independent settlement scopes may progress separately only when the new profile
defines their version and dependency boundaries. A connected transaction still
waits for its required closure. The first single-writer implementation must not
claim concurrent independent commits; subsequent scheduling/rebase work must
prove serializability and preserve the recorded settlement order.

Separate configuration into:

| Kind | Examples | Identity/behavior |
| --- | --- | --- |
| Model contract | Relation schema, keys, a declared logical cardinality restriction | Semantic; changes are program/model revisions |
| Evaluation profile | Deterministic work accounting, recursion/derivation bounds, arithmetic rules | Versioned and recorded with attempted execution/evidence |
| Operational policy | Cache bytes, page sizes, disk quota, request chunk size, concurrency | May refuse work; does not change successful logical results |

Host operational limits must never be relaxed by an untrusted artifact. A
verifier can refuse insufficiently provisioned work; it cannot accept partial
replay. Wall-clock cancellation is recorded as operational exhaustion, not a
deterministic proof of nontermination. Keep existing default behavior in old
profiles. New world limits bound a batch or computation, not total model size
through inherited 256-item/4-MiB constants. Storage is still bounded by explicit
operator policy; "large" never means unaccounted or infinite.

## 4. Build sequence and review-sized deliverables

Each stage can span several focused PRs. Land contracts before consumers;
keep the naive path runnable throughout. Stage numbers express dependencies,
not calendar estimates. No production release is implied by an intermediate gate.

### P0 — Contract and executable cost baseline

**Owners:** architecture/integration and test-infrastructure lanes.

- Write the implementation ADR covering section 3, new schema tags, grammar,
  authority boundaries, migration and exhaustion semantics.
- Add fixture generators and meters before optimizing: tuple probes, operator
  invocations and inspected tuples, tree nodes visited/copied, bytes allocated,
  hashed/read/written, candidate operations, and recompilations.
- Add full-store and full-frontier negative controls. Baseline the real public
  KB update path separately from the routing-only engine test.
- Record a copyable benchmark command, environment, seeds, dataset digests,
  raw measurements, and setup/update/read/verification breakdowns.

**Exit:** existing world-proportional paths visibly fail the proposed scale
gate, while the existing routing gate stays green. No counter is allowed to
omit delegated/unmeasured work by reporting zero.

### P1 — Persistent primitives and delta-updated calendar

**Owners:** storage/core lane. Depends on P0 artifact and cost contracts.

- Implement insert/update/remove/batch operations, immutable snapshots and
  path-level sharing behind `PersistentMap` or an additive compatible seam.
- Implement canonical incremental root hashing and durable node storage.
- Remove whole-frontier transactional cloning using a persistent ordered
  structure or validated change overlay with safe atomic publication.
- Preserve key-conflict, stale-removal, least-key, and rollback behavior.

**Exit:** equal maps from different insertion histories have equal logical
roots; old snapshots remain valid; single-key updates and rollback do not
copy/hash the whole store or candidate frontier. Differential collection tests
include removal, overwrite, collisions and adversarial key distributions.

### P2 — Module graph and relational frontend

**Owners:** compiler/linker lane. Depends on P0; can overlap P1.

- Add qualified symbols, typed exports/connections, bounded iterative import
  resolution, pinned transitive source closure and compile-cache invalidation.
- Add the new relation source surface and lower it to an explicit operator DAG.
- Detect unsupported cycles at lowering. Validate scalar-helper contracts and
  schema interfaces before graph activation.
- Split module validation budgets from reachable executable-plan budgets.

**Exit:** at least 100 modules link without flat-name collisions; 1,000 helpers
and 512 small schemas can be validated under an explicit test profile. An edit
to one implementation recompiles its affected dependency region; an unused
library export does not fill the active plan or invalidate unrelated operators.
Import count/bytes/depth limits fail at loading, not after unbounded allocation.

### P3 — Durable keyed world and batch API

**Owners:** world-runtime/KB lane. Depends on P1 and P0 schemas; consumes P2.

- Introduce the library session API: open/create, pin revision, apply batch,
  query/diff page, close/reopen. Names can follow repository conventions.
- Implement stable keyed records, immutable version handles, primary and
  declared secondary indexes, revision journal and atomic root publication.
- Add chunked bulk ingestion with a manifest and final atomic publication;
  partial uploads are staging data, not partially visible worlds.
- Record changed keys and roots per revision, not full JSON snapshots. Expose
  paged results/diffs so a one-key update does not serialize the entire world.
- Retain a bounded full-recompute result path as the initial correctness oracle.

**Exit:** a world exceeding 4 MiB survives create/change/retract/reopen; a
one-key durable edit writes changed objects and a compact revision only.
Inject crashes at each persistence boundary; old/new visibility and retry
idempotence hold. Cold open reads metadata lazily rather than replaying all history.

### P4 — Maintained operator network and precise invalidation

**Owners:** incremental-runtime lane. Depends on P1–P3 interfaces.

- Connect relation deltas to retained operators and reverse subscriptions.
  Adapt existing SOC provider seams without pretending the current static
  `Footprint::Configs` alone represents all keyed/range subscriptions.
- Implement selection, projection, equality-join indexes, distinct/existence,
  grouped count and acyclic rule propagation, with retractions and supports.
- Maintain the candidate frontier incrementally, including guard/key changes
  and duplicate support. Use the existing canonical settlement discipline.
- Reuse compiled plans across data revisions. Detect semantic code changes
  separately and rebuild only affected operators where supported.

**Exit:** after each batch, incremental tuples, supports, candidates, decisions
and diagnostics agree with an independent full recomputation. Absent-key
insertion, branch switching, empty-to-nonempty joins, duplicate derivations,
last-support removal and multi-hop retractions have dedicated regressions.

### P5 — First end-to-end usable world slice

**Owners:** integration/CLI lane with runtime and KB owners. Depends on P2–P4.

- Wire the new profile into explicit CLI and stdio/Python operations without
  silently changing `brix kb@1` or `brix.serve@1` response meanings.
- Ship a linked order/inventory/shipping example expressed as relations, with
  one decision per order and a per-tuple explanation. Include imports in its
  stored program closure and run it offline after moving the directory.
- Apply corrections/retractions through the same public path a user calls;
  publish source/index/support/decision roots as one coherent revision.
- Make missing inputs, unsupported operators, exhaustion, and cached prior
  results distinguishable in human and JSON output.

**Exit: MVP.** A real API session keeps a 10k-row linked model open, changes one
fact, updates only its affected region, survives restart, and agrees with the
oracle. This stage is an intermediate usable slice, not the million-row claim.

### P6 — Independent evidence, checkpoint and migration paths

**Owners:** evidence/verification lane. Depends on P5; design begins in P0.

- Define versioned world audit manifests binding source closure, program graph,
  source roots/deltas, execution profile, settlement trace and claimed results.
- Make audit reconstruction independent of fast-runtime caches/indexes. An
  authenticated cached root proves content integrity, not derivation correctness.
- Retain complete verification from genesis. Add checkpoint-plus-suffix replay
  only with an explicit trust basis: previously independently verified checkpoint
  or caller-supplied trusted checkpoint pin. Report its verification scope.
- Bound and meter lazy evidence expansion. Verify cross-model boundaries and
  all required decisions; no first-decision-only success shortcut.
- Provide explicit KB v1 import into a new world directory with provenance,
  preserving the old directory and identities. Unsupported translations refuse
  rather than approximating. Test read-only access to all old artifacts.

**Exit:** corrupt inputs, dependencies, caches, indexes, supports, checkpoints,
trace order, or claimed decisions cannot earn `Audited`. Honest replay succeeds
from a fresh process with caches removed. Historical evidence remains attributed
to its original revision after current support is retracted.

### P7 — Skew controls, residency and maintenance costs

**Owners:** runtime/storage lanes. Depends on P5; correctness constrained by P6.

- Add resumable bounded operator work and deterministic queue fairness.
  Pin settlement scope/readiness; never let fair scheduling change the winner.
- Add page/cache eviction and independent physical partition sizing. Record
  cold-cache I/O and compare partition layouts under identical logical input.
- Implement bounded maintenance: index compaction, cache/checkpoint retention
  and reachability accounting. Roots pinned by readers/history/evidence cannot
  be reclaimed. Do not silently delete committed history to meet a memory target.
- Expose diagnostics identifying the expensive relation, join key, dependency
  fanout, model and exhausted budget; avoid a vague global "model too large".

**Exit:** a 99.9%-large submodel does not make a local update in a small unrelated
submodel scan it. Hot keys are measured as genuine fanout. Maintenance debt and
retained memory remain bounded or trigger explicit admission refusal; they do
not accumulate invisibly until a later world-sized pause.

### P8 — Scale qualification and release

**Owners:** independent test/performance lane and integration owner.

- Run the matrix below through library and public interfaces; publish counters,
  timings, peak/resident memory, bytes written, seeds and build configuration.
- Run long correction/retraction sequences, process restarts, crash recovery,
  cache eviction, and different physical partitions, not just initial insertion.
- Update the language guide, beta contract, law map and README to distinguish
  measured support, fallback recomputation, and deferred operators.
- Add the world example and migration/audit checks to release-package smoke tests.

**Exit: complete handoff objective.** Section 1 targets and all correctness gates
pass. If a target fails, report its limiting path and keep the claim provisional;
do not change the fixture to avoid the work or silently lower the target.

## 5. Required validation matrix

| Gate | Workload | Required observation |
| --- | --- | --- |
| Inert growth | Hold `D`, affected keys and output fixed; grow resident unrelated rows 1k → 10k → 100k → 1M | Same logical operator/candidate work; physical path work within the selected structure's derived bounds; no unrelated tuple visits |
| Real index growth | Grow rows/providers in live indexes, not only unregistered inert handles | Instrumented lookup/copy/hash work includes real structure depth and allocations |
| Large frontier | Add/remove/re-key one candidate in a growing frontier | Oracle-identical least key; no full clone, sort or provider enumeration |
| Skew | Balanced modules, then 90/10, 99/1 and 99.9/0.1 distributions | Cold large submodels do not penalize unrelated updates; hot-submodel cost follows its real affected region |
| True fanout | One changed join key matches 1, 100, 10k rows | Work/output grows with affected matches; no misleading constant-cost assertion |
| Retraction | Multiple supports, final-support removal, missing-key insertion, changed branch and downstream chains | Correct membership and provenance after every step |
| Graph changes | Duplicate imports, name overlap, diamond dependencies, deep chains, code/schema upgrades | Deterministic linking, pinned historical replay, bounded loading and scoped invalidation |
| Publication | Exhaustion/cancellation and injected crashes before/after every durability boundary | Old revision or complete new revision; never mixed roots or a successful partial result |
| Identity | Different input chunking/insertion orders, physical partitions, restart and eviction | Equal logical roots/results for equal state; history identities may differ for different histories |
| Context reuse | Change unrelated state while a derivation's direct dependencies remain unchanged | Payload can remain shared; judgements requested under the new context have correctly bound IDs/evidence, with no inherited authority or eager rewrite of every cached result |
| Evidence | Tamper caches, supports, dependency code, traces, checkpoints and one later decision | No false audit success; complete and checkpoint-scoped verification are distinguished |
| Retention | Many updates with old readers/checkpoints pinned, then released | Measured retained bytes, no premature reclamation, bounded maintenance and no hidden full-world work per update |
| Legacy | Existing examples, schemas, CLI/serve results, KB history and all frozen vectors | No alpha.3 semantic or identity drift |

For every optimized path include a deliberately naive negative control that
the cost harness rejects. Instrument inside providers, storage and encoders;
the runtime must not count one callback as one unit while it scans a million rows.
Do not fail noisy shared CI on a claimed microsecond threshold. Deterministic
operation/path/byte bounds are hard gates; wall time and peak memory are companion
measurements on a documented machine. Derive and record concrete structural
bounds in P0/P1, rather than inventing a forgiving tolerance after a regression.

PR CI runs 1k/10k fixtures and small randomized differential cases. An explicit
large-workload job runs 100k/1M fixtures and long histories; it is a required
release qualification for the world profile, not a permanently ignored test.
Reference recomputation may run at checkpoints on the million-row workload,
but every batch is checked in the smaller randomized corpus. Clearly label
both coverage levels; periodic comparison is not an every-batch proof.

## 6. Ownership and parallel work

| Lane | Owns | Coordination boundary |
| --- | --- | --- |
| Integration/ADR | Contract, public schemas, migrations, end-to-end example, merge order | Resolves interface changes before consumers edit shared files |
| Storage/core | Persistent map, canonical tree, frontier, disk objects and cost meters | Supplies immutable root and batch APIs; no language semantics |
| Compiler/linker | New source profile, module graph, interfaces, operator DAG | Emits versioned plans and dependency manifests |
| Incremental runtime | Subscriptions, operators, support maintenance and settlement | Consumes store/plan APIs; reports actual work and affected results |
| KB/API/evidence | Revision journal, CLI/serve/Python, replay and audit | Keeps fast caches separate from verifier authority |
| Independent tests | Naive oracle, generators, faults and scale gates | Must not share the fast invalidation/index implementation |

With limited agents, combine integration with KB/API and combine storage with
runtime. P1 and P2 can proceed in parallel after P0. P3 requires stable storage
contracts; P4/P5 integrate before skew optimization. Avoid concurrent edits to
`Cargo.toml`, canonical tags, public plan types or shared CLI dispatch files.
Use separate branches/worktrees and small reviewable PRs. Do not have every
agent build a separate world runtime and try to reconcile them afterward.

## 7. Instructions to agy

1. Inspect the current checkout, applicable instructions and the baseline above;
   adapt paths if later commits have moved them. Preserve unrelated local work.
2. Start a feature branch. Implement P0 and record contracts/measurements before
   changing the engine. Keep a stage/PR checklist in this document or a linked log.
3. Build P1–P5 as one vertical path. A store benchmark alone, a config knob, or
   an isolated provider test cannot satisfy the runtime objective.
4. Preserve old profiles and reference paths. New source/storage/protocol versions
   must be explicit and tested. Never change frozen bytes to make tests green.
5. Finish P6–P8 before calling the world runtime complete. Report deferred
   recursive/negation/numeric-aggregate semantics explicitly, not as implemented.
6. Run focused tests per change and the repository gates at integration points:

   ```sh
   cargo fmt --all --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   python3 scripts/canon_crosscheck.py
   python3 scripts/check_tcb_dependencies.py --check
   python3 scripts/check_soc_law_map.py
   ```

   Add and document the exact commands for the new differential, crash and
   large-workload suites. Update dependency/law negative-gate tests where needed.
   Run Python/protocol tests when those surfaces change.

7. Deliver implementation PRs, the ADR, migration instructions, a runnable linked
   example, raw scale results and an honest completion checklist. Do not publish
   a new release as an implicit part of this handoff; Tony will direct release work.

## 8. Explicitly deferred

Distributed consensus, automatic multi-host sharding, arbitrary recursive
relation maintenance, unstratified negation, unrestricted dynamic code loading,
incremental proofs of every computation, and a universal-world theorem are not
prerequisites for this milestone. They also must not be claimed as consequences
of a fast fact-store benchmark. The delivered local runtime must leave clean
interfaces for further work without using those future systems to excuse current
whole-world update paths.

## 9. Execution checklist

- [x] **P0 — Contract and executable cost baseline**
  - [x] Feature branch created (`feature/persistent-world-runtime`).
  - [x] [ADR-0046](../../spec/adr/ADR-0046_Persistent_World_Runtime.md) written and linked.
  - [ ] Physical cost meters: **not delivered.** `PhysicalCost` was removed because nothing populated it (see P0 baseline §1); only deterministic clone counters exist. Structural counters (nodes visited/allocated/hashed) exist for `TrieMap` only (`TrieOpStats`, P1); tuple probes, operator invocations and bytes allocated remain unmeasured.
  - [x] Negative controls implemented: `crates/soc-core/tests/world_scale_negative_controls.rs` (`ArcMap::insert`, `Frontier::apply_delta`) and `crates/brix-kb/tests/kb_scale_negative_controls.rs` (`ops::assert_inputs`).
  - [x] Baseline measurements and commands recorded in [`docs/performance/world-runtime-p0-baseline.md`](../performance/world-runtime-p0-baseline.md).
  - [x] Routing gate confirmed green; negative controls fail flat scale gate.
- [x] **P1 — Persistent primitives and delta-updated calendar**
  - [x] `PersistentMap` extended with `remove`; Radix Trie / HAMT substrate (`TrieMap`) implemented with path compression, branch contraction, and deterministic collision buckets.
  - [x] Canonical incremental Merkle root hashing implemented. [ ] Durable persistence: **prototype only** — `NodeStore` trait, `MemoryNodeStore` and `encode_node` exist; no `decode`, no disk backend, no reload path, and `put_node` does not verify digest-vs-bytes. Collision buckets are a sorted `Vec` (O(bucket) update/rehash); the log-N bound holds only for uniformly distributed hashes (default `CanonHasher` = BLAKE3), not for a weak injected `KeyHasher`.
  - [x] `Frontier::apply_delta` refactored to transactional rollback overlay, eliminating whole-frontier cloning (0 existing entries cloned, $O(|\Delta| \log N)$ complexity).
  - [x] Negative controls preserved in `world_scale_negative_controls.rs`.
  - [x] Scale tests, differential property tests, and benchmarks added; P1 exit criteria verified and recorded in [`docs/performance/world-runtime-p1-results.md`](../performance/world-runtime-p1-results.md).
- [x] **P2 — Module graph and relational frontend**
  - [x] Qualified symbols (`QualifiedName`), module visibility (`export`), cross-module access enforcement (`NonExportedAccess`), and dead-code reachability pruning implemented in `brix_lower::module_graph`.
  - [x] Bounded loading with `ModuleLoader` trait, `SizedLoader`, and `ModuleLoaderLimits`: depth, count, module bytes, and total bytes are checked strictly before source allocation.
  - [x] Content, interface, and transitive manifest digests (`ProgramGraphManifest`) enabling interface-based invalidation (`compute_affected_modules`), with non-invalidation of unrelated modules verified.
  - [x] Semicolon-free relational source grammar added to `brix-syntax` with input/derived relations, projection, filtering, equijoin, and grouping.
  - [x] Legacy identifier compatibility preserved: `rel`, `select`, `export`, `key`, `group`, and `by` function as contextual identifiers across let bindings, parameters, and record fields.
  - [x] Bounded export modifier parsing: recursion charged against `ParseLimits::max_nesting_depth` and consecutive `export` tokens refused immediately, eliminating stack overflow risks.
  - [x] Explicit operator DAG lowering (`brix_lower::relation_dag`) with static recursion SCC cycle rejection, true cyclic unstratified negation rejection, and unsupported negation rejection.
  - [x] Contract validation (`validate_contracts`): helper functions and schema interfaces validated before graph activation.
  - [x] Scale tests (100 modules linking with 201 coexisting functions/configs, 1,000 helpers + 512 schemas linked and validated) and qualification tests added; P2 exit criteria verified and recorded in [`docs/performance/world-runtime-p2-results.md`](../performance/world-runtime-p2-results.md).
  - [x] ADR-0046 §3.5 example updated to semicolon-free syntax and `use` imports in full alignment with the grammar.
- [x] **P3 — Durable keyed world and batch API**
  - [x] `CanonDecode`, `decode_node_verified`, `Node::Lazy`, lazy `TrieMap` get/insert/remove/page implemented with strict Merkle digest validation.
  - [x] `FileNodeStore`: verified content-addressed layout with atomic write failure tracking, batch directory fsync on flush, and fail-closed error propagation (`a04`, `a05`, `a13`).
  - [x] `WorldSession` API: RAII writer file locking (`.lock`), on-disk HEAD validation, payload-verified idempotency replay, and lazy cold open (`a01`, `a02`, `a03`).
  - [x] Atomic publication: 4 crash points verified (`a06`), revision record digest recomputation & validation on open.
  - [x] Chunked staging: upload ID path-traversal validation, expected chunks validation, staging cleanup deferred until publication succeeds (`a07`, `a08`, `a09`).
  - [x] Revision journal: no-op filtering (`a10`), cursor/limit pagination for `diff_page` (`a11`).
  - [x] Declared secondary indexes: deterministic versioned tuple codec (`TupleRecord`), canonical composite key framing (`encode_secondary_key`), persistent `pk_set` inner tries (`customer_id -> TrieMap<WorldKey, ()>`), unchanged projected field skip, and query APIs (`query_secondary_index`), passing all 6 ruling invariants (`secondary_index_contract.rs`).
  - [x] Oracle differential: point lookups, pagination, and diff events verified against bounded oracle.
  - [x] Measured and qualified: 8,000-key world = 22,045 objects / 9.77 MiB (> 4 MiB gate); 1-key primary edit = +5 objects / +1,790 B (<= 8 path nodes, < 10 KiB); indexed edit bounded by primary + inner/outer trie paths. All 14 adversarial probe tests, 7 durability tests, and 4 secondary index contract tests pass.
- [x] **P4 — Maintained operator network and precise invalidation**
  - [x] Connected relation deltas to `RelationDag` operators (`Scan`, `Bind`, `Filter`, `Project`, `EquiJoin`, `Distinct`, `GroupedCount`) via `WorldNetwork`.
  - [x] Multi-support truth maintenance (TMS) with symmetric canonical derivation tracking (`DerivationId::Distinct`, `DerivationId::Join`).
  - [x] Incremental candidate frontier maintenance per entity (`entity_id -> BTreeMap<String, CandidateEntry>`) with support tracking.
  - [x] Canonical settlement discipline via `(phase, priority, tiebreak)` calendar keys using `soc_core::calendar::Frontier::select_least`.
  - [x] Scratch recompute self-check (`WorldNetwork::recompute_from_scratch`, `verify_differential_correctness`). **Correction 2026-10-04:** this replays through the same network code, so it is a self-consistency check and not independent evidence. Independent agreement: `brix_kb::world::reference` (P6a).
  - [x] All 8 adversarial probes in `crates/brix-kb/tests/p4_adversarial_probe.rs` pass, including empty/nonempty joins, duplicate derivations, 3-hop cascade retractions, branch switching, absent-key stability, zero-one grouped count transitions, and 75-step differential fuzzing.
  - [x] Measured and qualified: 1-key edit in 120-op linked model traverses only 22 intermediate deltas, updating 1 settlement ($O(\Delta)$ bound); absent-key insert generates 0 join matches. Results recorded in [`docs/performance/world-runtime-p4-results.md`](../performance/world-runtime-p4-results.md).
- [x] **P5 — First end-to-end usable world slice (MVP)** — results: [`world-runtime-p5-results.md`](../performance/world-runtime-p5-results.md)
  - [x] 10k-row linked model in an open session (`p01`); one-fact edit: 5 objects, 12 deltas, 1 settlement (`p02`); restart + directory move (`p03`, p08).
  - [x] `brix world` CLI + stdio `world.*` through the public path (`world_public.rs`, `p05`); distinguishable failure classes and idempotent replay.
  - [x] Oracle agreement: self-consistency (`p06`) **and** independent reference evaluator (`world_reference_differential`, P6a, mutation-tested).
  - [ ] Carried: ingestion throughput (~0.5–1k rows/s debug) and edit latency need release-build measurement before P8.
- [ ] **P6 — Independent evidence, checkpoint and migration paths**
- [ ] **P7 — Skew controls, residency and maintenance costs**
- [ ] **P8 — Scale qualification and release**
