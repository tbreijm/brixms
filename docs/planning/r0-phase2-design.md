# R0 Phase 2 design (Opus review, 2026-10-09)

Companion to `r0-cold-open-spec.md`. Paths relative to `crates/`.

## Decisions (user, 2026-10-09)
1. Corrupt/mismatched persisted state at open: **hard typed error**; operator calls `open_replay` explicitly. No silent fallback.
2. Legacy (@2) or validity-invalidated worlds: **replay on open until the next commit**, which writes @3. No migrate command.
3. soc-core pack index rework (M6): **in scope** (required for G1).
4. Query/explain APIs (`get_derived`, `all_settlements`, `explain_decision*`, `get_settlement`) return **`Result`**.
5. **Intern DerivationIds now** (content-addressed) instead of storing full trees in join indexes/support sets.

## Findings that change the spec
- Distinct must NOT key by TupleRecord: `IntermediateTuple::to_tuple_record` (network.rs:80) goes via `Value::to_bytes` (Display), so `Int(1)`/`Str("1")` collide. Key by typed IntermediateTuple.
- `DerivationId::canon_write` Group arm writes `v.to_bytes()`: undecodable and collides. New storage codec needed; leave `digest()` unchanged.
- Plain TrieMap ops treat `Lazy` nodes as empty (get->None store.rs:389, iter skips 739, remove no-op 720, insert panics 583). `decision_tree` uses them (network.rs:1880-1890). Only fallible store-aware ops may be reachable.
- `FileNodeStore::new` loads every pack index record (store/file.rs:178-265): O(total nodes) per open.
- revision schema checks `== V2` at session.rs:1763, session.rs:1807, verify.rs:995 must accept @3.

## Design
- `world/state_store.rs`: `StateStore` (Arc, Clone; FileNodeStore or MemoryNodeStore; bounded LRU of decoded nodes keyed (TypeId,Digest), sized by `node_cache_bytes`); `SMap<K,V>` wrapper over TrieMap exposing only fallible store-aware ops; `SubRoot{root,len}` for nested collections (pattern: secondary indexes session.rs:1168-1232).
- Layouts: join index `SMap<Vec<Value>,SubRoot>`->`SMap<DerivationId,IntermediateTuple>`; Distinct `SMap<IntermediateTuple,SubRoot>`->`SSet<DerivationId>`; GroupedCount `SMap<Vec<Value>,SubRoot>` (len = count); derived `SSet<TupleRecord>`; frontier `SMap<(decide,entity),BTreeMap<cand,CandidateRec{..,supports:SubRoot}>>`; decisions `SMap`.
- apply never writes the store; on success persist network tries beside decision_tree (session.rs:1317), same pack/flush; after HEAD rename **dehydrate** (all tries -> `from_root_digest`).
- Join continuations: inner-trie root snapshot + `(hash,key)` page cursor.
- Codecs (`world/state_codec.rs`): injective `brix.world.state-value@1` for Value (F64 bits, Decimal int128+scale); DerivationId storage codec (depth bound = dag.nodes.len()); IntermediateTuple sorted list; TupleRecord wrapper; CandidateEntry with recomputed/checked calendar_key.
- `network_state_root` = digest of header node `brix.world.network-state@1`: validity key (program_digest, exec_profile digest, compiler/lowering version, state-codec version), decision root+len, frontier root+len, derived roots, per-OperatorId kind+roots+lens. Quantum/limits excluded. Revision `REVISION_SCHEMA_V3` adds presence-tagged root to `compute_digest` (revision.rs:200, dispatch 272-290); `from_json` accepts @1/@2/@3.
- Open: `from_program` (subscribers/downstream/compiled exprs, no store) -> if @3 load header verified -> check validity key + operator kinds/arity -> check header.decision_root == rev.decision_root (replaces session.rs:946) -> install via `from_root_digest`, skip `set_network`. Else replay; report `open_mode`.
- Remove `base_relations` and `Scan.records`: `BaseView::old_tuple(rel,key)->Result<Option<WorldTuple>>` passed to `apply_ops_staged_bounded`; session passes pre-batch `&self.relations`.
- Pre-existing gap: `derived_relations` is a set without support counts; collapse of two tuples to the same TupleRecord is order-sensitive. Add support counts (or key by IntermediateTuple).
- Also fix: receipts/revisions scan for idempotency keys (session.rs:797-838) is O(revisions) -> persisted idempotency trie. soc-core: sorted fanout-addressed per-pack `.idx` + lazy index cache.

## Tests
Differential per fuzz step: restore vs `open_replay` vs fresh replay (roots, all_settlements, derived, explain_decision); proptest for batching/order/quantum-independent roots. Negative controls: flipped byte, deleted node, tampered header, decision_root mismatch (all hard errors); validity-key bump -> replay. Crash: add `AfterBaseObjectsBeforeNetworkObjects`, `AfterNetworkObjectsBeforeFlush` to `CrashPoint` (error.rs:10); after every point HEAD is old or new and restored root == replay root.

## Milestones
M0 codecs, StateStore, SMap, SubRoot, cache, dehydrate, TrieMap hardening | M1 join index (benchmark replay cost early) | M2 Distinct + GroupedCount | M3 derived, frontier, decision_tree via store ops; Result APIs | M4 header, revision @3, restore, `open_replay`, `open_mode`, crash points, full differential | M5 BaseView + base dedupe | M6 soc-core `.idx`, idempotency trie, Atlas acceptance (high risk).
Restore enables only at M4; earlier steps guarded by in-process root equality.

## Release target
R0 Phase 2 ships as **alpha.5**.

## M0 deviations folded into the plan (2026-10-09, M0 = 6348187)
Each is resolved in a named milestone; none may be left as a known gap at alpha.5.

| # | M0 deviation | Resolution | Milestone |
|---|---|---|---|
| 1 | `StateStore` cache holds raw node bytes; `NodeStore` exposes only bytes and TrieMap decodes internally, so every access re-decodes (the problem the design wanted fixed) | soc-core change: decoded-node cache inside the store-aware TrieMap paths (or `NodeStore::get_decoded`), keyed (TypeId,Digest), bounded by `node_cache_bytes`. Benchmark decode share before/after. | M1 (needed before join-index perf is judged); finalised M6 |
| 2 | `NodeStore::get_node -> Option` maps I/O errors to `MissingNode` | Change trait to `Result<Option<_>, StorageError>` (soc-core, all impls and callers); `StateError::Io` then real. Prevents an I/O blip being reported as corruption/absence. | M6 (with pack index rework) |
| 3 | Trie decode failures reported as generic `CorruptedNode` | Carry the codec error as a source in `StorageError`/`StateError` so negative controls can assert the specific cause | M4 (negative-control suite) |
| 4 | `StateError` not integrated with `WorldError` | Add `WorldError::State(StateError)` with typed variants (CorruptState, MissingState, ValidityMismatch) mapped to the hard-error open policy; used by open/commit | M3 (first Result APIs), finished M4 |
| 5 | TrieMap hardening (Lazy-as-empty) skipped because wrapper avoids plain ops | Still required: `decision_tree` uses plain ops until M3 moves it to `SMap`. After M3, make plain ops on Lazy nodes `debug_assert`/error-returning, or make them private to soc-core, so no caller can regress. Add a test that asserts the plain ops are unreachable for network state. | M3 |

Process: after each milestone, Opus re-checks this table and the design for new deviations before the next milestone starts.

## M1 plan (Opus review of 99e8fd8) — new gaps 6-11
| # | Gap | Fix |
|---|---|---|
| 6 | DerivationId interning not implemented (state_codec.rs:293-391 inlines full trees) | Task B: `DNode`/`DerivRef=Digest(brix.world.deriv-node@1)`, raw content-addressed objects, `intern_pending` in network; no GC (record for M4) |
| 7 | Nested SubRoot inner tries have no persistence path and are invisible within a batch | Task C: `JoinSide{outer: SMap<ValueList,SubRoot>, hot: PMap<ValueList,SMap<Digest,JoinEntry>>}`; persist hot inner maps then outer |
| 8 | No decoded-node cache (`resolve_node` soc-core/store.rs:1389 decodes+rehashes each access) | Task A: `DecodedNodeCache` (TypeId,Digest)->Arc<dyn Any>, `NodeStore::decoded_cache()`; replaces StateStore byte LRU |
| 9 | Join match order changes (DerivationId Ord -> hash order) | Oracle compares sets; pull derived support-count fix forward from M3 if it diverges |
| 10 | Replay at open builds tries never persisted -> first commit writes O(world) | Persist+dehydrate at end of `set_network` (session.rs:1841-1861) |
| 11 | SMap lacks Debug; no `WorldError::State` | Bring minimal `WorldError::State(StateError)` + Debug forward from M3 |

Tasks: A (soc-core cache + state_store.rs) and B (interning/codecs/error.rs) parallel; then C (network.rs join conversion + `join_oracle.rs`, old PMap impl kept as oracle until M4) with D (session wiring, manifest `node_cache_bytes`) in parallel against agreed signatures; then E (tests/r0_join_store.rs, bench fields).
Gate (100k release, 2 runs vs baseline): load <=1.3x (13.8 s); one-row batch <100 ms and objects_written <=3x; cold open <=1.25x; RSS <= baseline; decode share down vs cache disabled.

## M7: operational policy seam (heuristic now, learned policy later)
Decision (user, 2026-10-09): ship a heuristic policy plus a clean seam so a learned/RL policy can replace it later. Must not change output: policies are operational per ADR-0046 s3.8 (may affect latency/memory/refusal, never state roots, settlements, or explanations).

**Seam.** `trait StatePolicy: Send + Sync` in brix-kb (world/policy.rs), held by `StateStore`/`WorldSession`, default `HeuristicPolicy`. All methods are pure functions of an explicit, loggable `PolicyInput` (no hidden clocks or randomness; any randomness takes a seed from the input) so runs are reproducible and replayable:
- `warm_set(open_ctx) -> Vec<Digest>`: nodes to prefetch at open (top decision-trie levels, each operator's root branch, hot paths from the access sidecar).
- `cache_admit(node_meta) -> Admit` / eviction hints for the decoded-node cache.
- `scheduling_quantum(program_stats, queue_depth) -> usize`.
- `persist_plan(delta_stats) -> {flush_now, dehydrate, compact_packs}`.
- `admission(op_ctx) -> Admit | Defer | Refuse(reason)` (feeds existing verification budgets).
Policy outputs are clamped by hard invariants in the engine (min/max quantum, never skip persisting before HEAD rename, never refuse a replay verify), so a bad or learned policy can degrade speed but not correctness.

**Heuristic v1.** Frequency/recency access counters per trie root and depth (CLOCK second chance already in the decoded cache), warm top-K levels plus last-session hot set from a small `access.sidecar` (best-effort cache, never authoritative, ignored if stale), quantum from observed per-quantum latency with a fixed bound, compaction when pack count exceeds a threshold, persist after every commit.

**Trace for RL.** Structured, opt-in `PolicyTrace` log (JSONL): `{seq, input, decision, outcome{latency_us, bytes_read, cache_hits/misses, rss}}` per decision point, versioned schema `brix.world.policy-trace@1`, written outside the node store (never part of revisions or roots). A `replay-policy` harness re-runs a recorded trace against any `StatePolicy` offline (counterfactual cache-hit simulation for warm_set/cache_admit; latency model for quantum), and a `PolicyEnv` adapter exposes the decision points as a gym-style (obs, action, reward) interface behind a cargo feature so an external trainer can drive it. Reward is defined by the host (e.g. cold-open ms + weighted RSS); the repo ships only a reference reward and the Atlas benchmark as eval.

**Rules.** Default build contains no ML dependency. Policy selected by `world.json` `policy: "heuristic" | "external:<id>"`; recorded in the operational policy record. Differential tests run every fuzz world under heuristic, a random policy and an adversarial policy and require identical roots/settlements/explanations (proves the policy cannot change output). Benchmark gates are evaluated with the heuristic policy only.
Milestone: M7, after M4 (restore) since warm_set needs the restore path; trace logging hooks can land earlier in M1-M4 at StateStore (record node touches per query).
