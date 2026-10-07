# ADR-0046 — Persistent World Runtime: Linked Models, Deterministic Incremental Storage, and Bounded Truth Maintenance

Status: **Draft — proposed, not ratified**, 2026-10-03. Follows `v0.1.0-alpha.3` baseline commit `44d46ef`. Implementation plan governed by `docs/planning/persistent-world-runtime-plan.md`.

> **Read this as a proposal, not a settled contract.** Concrete parameters, names and syntax below (branching factor, work-quantum size, error and command names, the source-syntax example) are *illustrative placeholders chosen by the drafter*, not decisions. The source syntax in §3.5 has not been checked against the existing parser and has no lowering yet; the plan requires a complete parseable example with its precise lowering before this ADR can be accepted. See §6 "Open items before ratification".

Foundation documents:
- [ADR-0002: SOC Constitution](./ADR-0002_SOC_Constitution.md) (§5.3 fail closed, §8.1 calendar commitment/deliberation split, §9.1 $O(\Delta)$ invariant, §9.2 state and calendar architecture)
- [`spec/SOC_Semantic_Laws.md`](../SOC_Semantic_Laws.md) (SOC-LAW-01 canonical identity, SOC-LAW-09 correction and retraction non-erasure, SOC-LAW-10 observable-behavior fidelity, SOC-LAW-12 verifier closure)
- [ADR-0040: Finite Relations](./ADR-0040_Finite_Relations.md)
- [ADR-0041: Persistent Knowledge Base](./ADR-0041_Persistent_Knowledge_Base.md)
- [ADR-0043: Per-Entity Decisions](./ADR-0043_Per_Entity_Decisions.md)
- [ADR-0044: Serve Protocol](./ADR-0044_Serve_Protocol.md)
- [ADR-0045: Explicit Numeric Arithmetic](./ADR-0045_Explicit_Numeric_Arithmetic.md)

---

## 1. Context and Problem Statement

BrixMS currently maintains decision states through two execution modes:
1. Pure finite-decision evaluation (`brix run`, `brix test`), where each step reconstructs inputs and re-evaluates all rules from scratch.
2. The `brix.kb@1` persistent knowledge base (`crates/brix-kb`), which appends revisions but implements assert/correct/retract by serializing full JSON snapshots, re-parsing and re-lowering modules, and rebuilding the full runtime on every fact change.

Furthermore, within `soc-core`:
- [`soc_core::store::ArcMap::insert`](../../crates/soc-core/src/store.rs) clones the entire `BTreeMap` ($O(N)$ entry copies) rather than providing node-level structural sharing.
- [`soc_core::calendar::Frontier::apply_delta`](../../crates/soc-core/src/calendar.rs) clones the candidate map on every delta commit (`let mut staged = self.entries.clone()`).
- [`soc_core::commit::commit_tick`](../../crates/soc-core/src/commit.rs) enumerates candidates broadly rather than maintaining an incremental least-key index.

While the existing `soc-core` $O(\Delta)$ routing gate (`crates/soc-core/tests/o_delta_gate.rs`) verifies that inert configurations do not trigger un-registered provider callbacks, the physical storage and calendar execution paths are strictly $O(|world|)$ in entry copies, allocations, and re-evaluation. Under a large, linked world model (target: 1,000,000 resident keyed facts across 100+ linked modules), these full-state paths degrade catastrophically.

This ADR defines the **Persistent World Runtime Profile** (`brix.world@1`): an execution strategy over SOC configurations and witnesses that maintains a large, linked world model through small, durable changes with work proportional strictly to affected state ($D + A + R + P$).

---

## 2. Complexity Contract and Complexity Equation

Let:
- $D$: changed source tuples (delta input set)
- $A$: affected operator work (actual join matches, filtered evaluations, and invalidation traversals)
- $R$: changed derived results (proposals, per-entity decisions, output tuples)
- $P$: physical storage and index paths touched (tree nodes visited, copied, and hashed)

### THE Invariant
Work per committed transaction MUST be proportional to:
$$\mathcal{W} \propto D + A + R + P$$
with documented logarithmic/radix index overheads. There MUST NOT be any term proportional to $|world|$ (unrelated resident facts), unrelated modules, the total candidate count, or historical transaction depth.

---

## 3. Architecture and Semantic Decisions

### 3.1 Separation of World Runtime from Legacy Finite Profiles

The world runtime is an additive execution profile:
- Legacy schemas `brix.input@1`–`@4`, `brix.kb@1`, `brix.kb.revision@1`, `brix.serve@1`, and the finite-decision profile (`ADR-0030`, `ADR-0043`) remain readable with their frozen specifications and unmodified byte-level behavior.
- The new profile introduces explicit version tags:
  - `brix.world@1` (world manifest and catalog)
  - `brix.world.batch@1` (transactional mutation envelope)
  - `brix.world.revision@1` (compact delta revision record)
  - `brix.world.audit@1` (independent audit receipt bundle)
- Execution starts single-process, local durable storage, single-writer. Multi-host distributed consensus is explicitly deferred.

### 3.2 Stable Addresses vs. Content-Addressed Versions

To support sound truth maintenance and incremental reactivity without whole-model scans:
- **Logical Key:** Entities, input relation rows, and model bindings are addressed by explicit canonical keys (initially canonical `Int` or `Str`). Approximate or float keys are forbidden.
- **Content Digest:** Immutable tuple contents, value payloads, and program artifacts are identified by their canonical content digest (`Digest::of(Domain::Value, ...)`).
- **Derived Tuple Identity:** A derived tuple's identity is defined by its fully qualified relation name and its canonical tuple values. Multiple derivations of an identical tuple merge into a single logical tuple with an explicit **support count** / provenance set.
- **Revision Identity:** A revision records the root digests of source state, program graph, derived outputs, and history chain. Two histories producing equal logical state share the identical state root digest, while retaining distinct revision chain digests.

### 3.3 Deterministic Persistent Storage Substrate

`soc-core`'s `PersistentMap` seam is backed by a native, path-copying, content-addressed radix trie / HAMT built strictly on `brix-canon`:
- **TCB Whitelist Compliance:** Under `scripts/check_tcb_dependencies.py`, `soc-core` dependencies remain strictly `{"brix-canon", "brix-semantic"}`. No external collection crates (`im`, `rpds`) are introduced into Ring 0.
- **Radix Trie / HAMT Design:**
  - Radix trie over canonical `brix-canon` key digests (domain-separated BLAKE3). **Open:** branching factor (e.g. 16 or 32), to be decided from P1 measurements rather than fixed here.
  - Path compression for single-child intermediate branches.
  - Deterministic canonical node encoding via `brix-canon`.
  - Collision buckets compare complete canonical keys and order entries deterministically by canonical key order. Tested with an injectable synthetic hasher.
- **Canonical Root Invariant:** Equal logical maps produce byte-identical root digests regardless of insertion order, batching, page layout, or memory addresses.
- **Ordered Structures for Range & Calendar:** Range queries and the deliberation calendar use a persistent balanced ordered tree structure (or transactional change overlay) so that least-key selection is $O(\log C)$ without copying the full tree.

### 3.4 Linked Modules and Explicit Model Interfaces

- **Qualified Symbols:** Identifiers in the world profile use qualified paths (`model_name::relation_name`, `module::helper`).
- **Interfaces and Connections:**
  - `import path::to::module;` imports pure helper declarations, schema definitions, and pure scalar functions.
  - `connect source_model::rel_out -> local_rel_in;` explicitly binds an exported relation from a foreign model into an input relation of the local model at a pinned revision.
- **Program Graph Closure:** Transitive module dependencies are content-addressed and pinned by digest in a `ProgramGraphManifest`. Historical replays resolve against the pinned closure, never against ambient filesystem paths.
- **Compilation Caching:** Operator DAG compilation is cached by `(source_digest, compiler_version, imported_interface_digests)`. Changes to unused functions do not trigger operator invalidation.

### 3.5 First Relational Fragment

The source language would be extended in `brix-syntax` with a relational fragment. **The example below is illustrative only**: it has *not* been parsed against the existing grammar (which already has `use`, `input`, `propose`, `commit`) and has no lowering. Every keyword and form (`module`, `import`, `rel input`, `select … from … where`, `group by`, `connect`) is an unvalidated proposal. A complete parseable example with its precise lowering is a ratification requirement (§6).
```brix
module logistics::shipping

use logistics::inventory

rel input order: { id: Str, customer: Str, sku: Str, qty: Int, status: Str } key id
rel input backorder: { sku: Str, available: Int } key sku

rel derived fulfillment =
    select { order_id: o.id, customer: o.customer, sku: o.sku, qty: o.qty }
    from o in order, b in backorder
    where o.sku == b.sku and b.available >= o.qty and o.status == "pending"

rel derived backordered_sku_count =
    select { sku: b.sku, pending_orders: count() }
    from o in order, b in backorder
    where o.sku == b.sku and b.available < o.qty
    group by b.sku

propose fulfill_order(f.order_id)
    priority 1
    when f.qty > 0
    from f in fulfillment

commit decision from (fulfill_order)
```

#### Admitted Operators
1. **Keyed Input Relations:** Explicit unique primary keys (`key <field>`).
2. **Selection ($\sigma$):** Predicates over scalar fields and pure functions.
3. **Projection ($\pi$):** Tuple restructuring and scalar computations.
4. **Indexed Equality Join ($\bowtie$):** Equijoins indexed on matching key attributes.
5. **Distinct / Existence:** Set semantics with derivation support tracking.
6. **Grouped Count:** Monotonic tracking of group sizes, emitting delta events on $0 \to 1$ and $1 \to 0$ transitions.
7. **Per-Entity Decisions:** Decisions scoped to individual entities/keys.

#### Semantic Constraints & Deferrals
- **Acyclic Dependency DAG:** Relation dependencies must be strictly acyclic. Recursive relation strongly connected components (SCCs) and unstratified negation are rejected at lowering with a clear unsupported-profile error (error name and code to be defined; none exists yet).
- **Numeric Semantics:** All arithmetic preserves ADR-0045 exact integer, bounded `Decimal`, and `F64` semantics. Sum aggregates over floats are deferred to prevent order-dependent rounding drift.
- **Scalar Helpers:** Pure functions are evaluated through `brix-lower`'s shared evaluator (`l3_v2.rs`), memoized by `(code_digest, arg_values)`.

**Conjunction in the world profile (decided 2026-10-04).** Inside world-profile expressions (relation
predicates, `decide … when` guards, proposal values, and helper bodies compiled by
`brix_lower::world_expr`), `and` means logical conjunction. It is equivalent to `&&`, short-circuits the
same way, and lowers to `BinOp::AndAnd`. The legacy finite-decision v2 profile still refuses `and`/`then`
as witness composition (ADR-0002). No program accepted before this change changes meaning: v2 never
evaluated `and`. `then` has no world-profile meaning and stays refused.

### 3.6 Atomic Batch Changes and Settlement Discipline

- **Batch Envelope (`brix.world.batch@1`):**
  - `expected_base_revision`: Digest of expected HEAD revision.
  - `idempotency_key`: Client-supplied unique token.
  - `operations`: Deterministically normalized canonical upserts and removals (a correction is one remove-old/introduce-new pair in a single batch). Conflicting duplicate operations on one key are rejected, not resolved by transport order:
    - `Upsert(relation, key, tuple)`
    - `Remove(relation, key)`
- **Transactional Staging:**
  - Changes are staged in an overlay: storage trie deltas, index updates, operator subscriptions, candidate frontier deltas, and decisions are computed together.
  - If evaluation exhausts the operational fuel/memory limit, execution halts, all staged changes are discarded, and an `Unknown` outcome is returned with a resumable or retryable attempt identity. The prior committed revision remains intact and is not advertised as the result of the attempted edit.
- **Atomic Publication:**
  1. Persist new objects and the revision record; `fsync` them.
  2. Atomically publish the new root/HEAD (write temp file + rename).
  3. `fsync` the containing directory.
  Recovery observes either the old commit or the complete new commit.

### 3.7 Dynamic Truth Maintenance (TMS) & Reverse Subscriptions

- **Direct & Reverse Subscriptions:**
  - Operators register subscriptions with upstream relations by `(relation_id, join_key_val)` or `(relation_id, FullScan)`.
  - When an absent key is inserted, it immediately looks up reverse subscriptions, notifying only matching operators. Unrelated join branches are never evaluated.
- **Multi-Support Tracking:**
  - Every derived tuple tracks its active derivations:
    $$\text{supports}(t) = \{ \langle \text{rule\_id}, \vec{d}_{\text{in}} \rangle \}$$
  - A retraction of an input fact removes the corresponding derivation link. If $|\text{supports}(t)| > 0$, the tuple remains asserted. When $|\text{supports}(t)| = 0$, a retraction event is propagated downstream.
- **Frontier Maintenance:**
  - Candidate additions and retractions from per-entity decision rules update the calendar frontier incrementally.
  - Canonical least-key commit (`select_K`) is preserved identically to ADR-0002 §8.1.

### 3.8 Resource Accounting and Skew Controls

- **Decoupled Allocation Units:**
  - Large or hot relations are partitioned by key hash prefix; small or cold relations share trie structures.
  - Large submodels cannot starve shared memory pools or ready queues.
- **Bounded Computation Quanta:**
  - Operator work is scheduled in bounded quanta with deterministic tie-breaks. **Open:** quantum size, to be set from P7 measurements.
  - Bounded quanta govern execution interleaving; candidate selection continues to follow canonical least-key priority.
- **Policy Classification:**
  1. *Model Contract:* Schemas, keys, cardinalities (determines model revision).
  2. *Evaluation Profile:* Work budgets, recursion bounds, arithmetic rules (recorded in audit trace).
  3. *Operational Policy:* Cache bytes, page size, disk quota (may refuse execution, never alters successful logical output).

---

## 4. Migration and Compatibility

1. **Read-Only Compatibility:**
   - Existing `brix.input@1`–`@4` files and `brix.kb@1` directories remain readable.
   - Standard CLI commands (`brix run`, `brix kb`, `brix verify`) operate unchanged on legacy artifacts.
2. **Explicit KB v1 Import:**
   - An explicit migration (command name and surface not yet decided; e.g. `brix world import-kb <kb-dir> <world-dir>` is only a sketch) converts a `brix.kb@1` directory into a new `brix.world@1` directory with provenance, leaving the old directory and identities untouched. Translations that cannot be done exactly are refused, never approximated.

---

## 5. Authority and Audit Boundaries

- Fast runtime caches, indices, and memoized tables are strictly non-authoritative.
- An `Audited` or `Proven` tag (ADR-0019) requires independent verification:
  - An independent verifier reads the raw journal and canonical program closure, reconstructing all derived states without relying on persistent index structures or fast runtime caches.
  - Checkpoint verification is supported only when chained to a previously independently verified checkpoint pin.

---

## 6. Open items before ratification

This draft does not yet meet the plan's own bar for acceptance. Outstanding:

1. **Source syntax.** One complete example that parses with the real grammar, plus its precise lowering to the operator DAG. §3.5's example is unverified.
2. **Frozen storage format.** Canonical node encoding, path compression, empty-node and collision-bucket encoding, and the branching factor must be specified before any root format is frozen.
3. **Ordered structures.** Choice of persistent ordered structure (or change overlay) for the calendar and range indexes, and whether its canonical form is part of the profile.
4. **Concrete schemas.** Field-level definitions for `brix.world@1`, `brix.world.batch@1`, `brix.world.revision@1`, `brix.world.audit@1`; these are names only so far.
5. **Structural cost bounds.** The plan asks for concrete structural bounds derived in P0/P1; none are derived yet (see `docs/performance/world-runtime-p0-baseline.md` §1).
6. **Law and dependency gates.** Whether `spec/conformance/soc-semantic-laws.json` and the dependency/law negative-gate tests need updating has not been assessed.
7. **Conflict check.** Confirm no accepted ADR is contradicted; if one is, use the repository erratum process.
