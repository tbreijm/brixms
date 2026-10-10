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

## M3 results (derived, frontier, decisions; Result APIs; M7 trace hook)
Commits: d0dd24e (soc-core plain TrieMap ops panic on lazy nodes), d4474df, b138387, e9909bd, 52052bf.

- Derived relations: every lowered `select` ends in a Distinct, so a derived relation is a view over that operator's typed support index (`DerivedRel::DistinctOutput`); other output operators get a support-counted `Counted` index (unused by lowered programs today).
- Frontier: `SMap<(decide, entity), FrontierBucket>`; candidate supports inline (one) or `Sub` (two or more, hot map until dehydrated). Buckets are read once, edited in memory and written once per entity per batch.
- Decision trie: `SMap<WorldKey, WorldTuple>`, node encoding and roots unchanged. Network state names no `TrieMap`; test `network_state_cannot_reach_plain_trie_ops`.
- `WorldError` gains `CorruptState`, `MissingState`, `ValidityMismatch`; query APIs return `Result`.
- M7 hook: `StateStore::enable_policy_trace`, schema `brix.world.policy-trace@1`; default off.

Bench (100k, release, post-M2 re-measured same machine): load 13.27 s -> 14.5-14.8 s (+9.5..+11.5%), cold open 9.03 -> 8.8-9.0 s, one-row batch 54.8 -> 50-74 ms (noisy), objects 44 -> 50, RSS after open 1236 -> 724-747 MB, disk 446 -> 498 MB, `decisions_ms` 44 -> ~900 (cold read from store).

### Pre-existing bug found
The frontier applied all retractions before all insertions per relation. A (tuple, derivation) derived and retracted within one batch left a stale candidate and settlement. Reproduced on 57cd4e1 (post-M2). Fixed by netting the batch's deltas first (e9909bd); regression `transient_derivation_within_a_batch_leaves_no_candidate`.

### New gaps
1. M4 header must hash `frontier_root` (supports nested in it), `decision_sub_root` and operator roots; `derived_roots()` for `DistinctOutput` relations returns the Distinct root, so list derived roots only for `Counted` relations.
2. `get_candidates`/`current_state`/explain expand supports to full derivations (O(supports)); bulk readers should use `get_settlement`/`all_settlements`.
3. `all_settlements` pages and verifies every bucket: ~0.9 s per 80k entities cold; M7 `warm_set` could prefetch.
4. `legacy_*` oracle shadows must be removed at M4; the per-delta `legacy_derived` assert would diverge if a `Counted` relation ever has multiple supports.
5. Retracting a never-inserted support is a silent no-op; netting assumes a well-formed delta stream.
6. Load remains ~+10% over post-M2 (extra frontier trie persisted and scanned); further wins would need cheaper candidate support encoding (Inline supports carry the full Distinct tuple key).

## M4 results (restore from persisted state; sidecar; open surfacing)

Open now restores from the HEAD revision's persisted roots instead of replaying history. A world whose HEAD cannot be restored (legacy revision, validity-key or schema change) replays once, then writes a `state-cache.json` sidecar (`brix.world.state-cache@1`) so the next open restores. `open_replay` / `--replay` never writes. Open info (`open_ms`, `open_mode`, `replay_reason`, `state_source`, `state_refreshed`, `tail_batches`) is surfaced in `world.*` results; `brix world open-check [--verify-state] [--refresh-state]` checks a world.

### Bench (release, `r0_open_bench`, serial runs)

| Metric | M3 final (100k) | 10k run 1 | 10k run 2 | 100k run 1 | 100k run 2 |
|---|---|---|---|---|---|
| Load (s) | 14.5-14.8 | 1.51 | 1.51 | 14.13 | 14.25 |
| Cold open, restored (ms) | 8800-9000 | 2.9 | 2.8 | 14.3 | 14.3 |
| of which FileNodeStore::new (ms) | n/a | 0.54 | 0.51 | 0.54 | 0.58 |
| RSS after open | 724-747 MB | 4.0 MB | 4.2 MB | 12.6 MB | 12.3 MB |
| First all_settlements after restore (ms) | ~900 | 53 | 58 | 879 | 866 |
| One-row batch (ms) | 50-74 | 51 | 48 | 49 | 48 |
| One-row objects written | 50 | 47 | 47 | 51 | 51 |
| Disk | 498 MB | 49 MB | 49 MB | 498 MB | 498 MB |

Fallback (legacy `@2` HEAD, fresh process each): 10k first open 621-639 ms (replay + sidecar refresh), second 1-2 ms; 100k first 8.7-8.9 s, second 3-11 ms from the sidecar.

Cold open is not exactly flat: ~3 ms at 10k, ~14 ms at 100k (was ~9 s). The store floor is 0.55 ms. The remainder is attributed to parsing the HEAD revision record, whose changed-key list scales with the last batch size; this is inferred from the batch sizes and the receipt-first scan fix, not instrumented. The idempotency scan now parses a revision only when its receipt is missing (14 ms vs 18.6 ms). No 1M run (disk).

### A/B summary

Restored state equals replayed state (sidecar restore == replay tests), a key-change refresh works, and the first commit after a refresh is `@3` writing only the delta (<100 objects). Negative controls: stale HEAD/key, truncated or garbage sidecar, damage behind a valid sidecar, held lock, forced replay writes nothing, and three sidecar crash points.

### Gaps

- The latched `FileNodeStore` read masks corruption as missing; fix is `NodeStore::get_node` returning a `Result` (M6).
- Public `set_network` can attest foreign decisions.
- No GC: the sidecar root and the HEAD root must both be GC roots in M6.
- The `r0-oracle` feature and legacy shadows are removed; CI no longer needs it.
- The open scan is still O(files); the persisted idempotency trie is M6.
- Restored networks have no base copy (`base_relations` empty); use `verify_state`. Base copy is M5-lite.

### Remaining

M5 (base copy / M5-lite) and M6 (GC with both roots, `Result`-returning node reads, persisted idempotency trie, O(1) open scan).

## Profiling: load growth and the decisions read (2026-10-10, on 883dd8d)

Bench: `r0_open_bench`, release, serial, M-series Mac with 16 GB. The bench now keeps per-batch commit profiles (`load_batches[].profile`, from `WorldSession::last_commit_profile`: phase timings plus state-store node reads and bytes), and takes `R0_OPEN_BENCH_KEEP=1` (keep the world dir) and `R0_OPEN_BENCH_NO_BATCH=1` (child stops after the decisions read, so a kept world can be re-probed).

### Problem 1: load growth (1M: 2.4 s, 2.4 s, 16 s, 36 s ... 53 s per 100k rows)

Reproduces at 300k (batches of 30k: 0.6, 0.6, 3.3, 5.0, 6.8, 7.2, 7.5, 8.1, 8.8, 9.0 s). Nothing in a commit scans resident facts: `clone_network` (20-70 ms), `decision_encode` (O(batch), 50-260 ms), `publish` (40-60 ms), `stage_base` (flat, 50-230 ms), idempotency check and header are flat or zero. The growth is in four phases (300k, batch 2 -> batch 9, ms):

| Phase | 300k b2 | 300k b9 | 1M b2 | 1M b9 (before) | Cause |
|---|---|---|---|---|---|
| apply_ops | 1790 | 5200 | 8266 | 33841 | decoded-node-cache misses: state node reads 0 -> 342k (300k), 99k -> 1.70M (1M), each 7-20 us |
| persist_state | 1030 | 2590 | 4344 | 10502 | nodes written per commit 0.38M -> 0.63M (300k), 1.3M -> 2.1M (1M); about 4-5 us per node |
| persist_base | 70 | 510 | 298 | 3680 | hydrated, unchanged siblings of path-copied base nodes are probed with `contains` (see below) |
| flush | 218 | 352 | 723 | 1249 | bytes written per commit 115 -> 200 MB (300k), 394 -> 678 MB (1M) |

1. **apply_ops is cache thrash, not O(resident) work.** Every row touches about 9 hash-keyed tries (two join sides, Distinct x2, GroupedCount, frontier, decision tree, plus the base relation), each path-copy reads about log32(N) nodes, and the bench touches every sku group in every batch, so a batch's working set is close to the whole state. The decoded-node cache (128 MB accounted, which is about 4-5x that in RSS: `cost = 2 x encoded` against a real decoded size of roughly 8-10x) is full from batch 2 on. At 300k the first order batch has 0 reads; from batch 3 on, reads/row rise 0.3 -> 11 (300k) and 1 -> 17 (1M). The 1M cliff at batch 3 (99k -> 1.29M reads, apply_ops 8 s -> 20 s) is the working set outgrowing the cache. Evidence: with a 6 GB cap apply_ops is flat at 2.3 s across all batches at 300k and total batch 9 drops 9.0 -> 6.0 s; cap sweep (batch 9): 128 MB 9.0 s (RSS after load 3.0 GB), 512 MB 6.7 s (4.2 GB), 1 GB 6.3 s (5.9 GB). Disabling the persist write-through made it worse (reads 342k -> 502k, batch 9 9.6 s). 17 reads per row is near the layout's floor (each of about 9 tries contributes 1-2 distinct nodes per row below the depth where paths diverge); only cheaper misses or a larger or denser cache can move it.
2. **persist_state grows with nodes written**, which grows B x log(N/B): nodes/key 1.3 -> 2.1 as the tries deepen. Per-component (300k, batch 9): frontier 63k nodes/322 ms, decisions 64k/234 ms, join sides 111k each/about 500 ms, Distinct 64k/361 ms, GroupedCount 111k/461 ms. Inherent to the current node layout, mild.
3. **persist_base is O(visited hydrated nodes)**, a genuine resident-size term. Base relation tries are never dehydrated, so after a path-copy the unchanged siblings of every new branch are in-memory nodes, and `persist_node_recursive` must call `store.contains()` on each (it cannot tell they were already written). Counted at 300k: visits per commit 40k -> 174k, of which 0 -> 110k are such probes; each probe is a `known`-set hit or an idx lookup (`KNOWN_CAP` is 262k, cleared when full). State tries do not show it (their children are `Lazy`). The same hydrated base copy is why RSS after load is 5.1 GB at 1M (M5-lite).
4. Disk is 5.0 GB at 1M (5 KB per fact; no GC until M6), so at 1M the page cache holds only part of the packs and per-read cost rises 7 -> 20 us.

**Fix landed** (does not change any output, root or digest): the `find()` idx lookup now narrows with an in-memory sparse sample (first 8 digest bytes of every 32nd idx record, built at flush for free and lazily with the Bloom filter for packs found at open) and reads one window of at most about 64 records instead of 4-6 bisection reads plus a 20 KB bucket; `pack_file()` compares `Arc` identity instead of walking path components on every read. Result (same machine, same bench):

| | before | after |
|---|---|---|
| 1M load, per batch (s) | 2.2 2.3 14.7 31.3 33.1 35.5 40.5 42.5 45.4 50.9 | 2.2 2.4 14.3 25.6 26.3 27.9 31.4 33.8 36.0 38.1 |
| 1M load total (s) | 298.5 | 238.0 (-20%) |
| 1M batch 9 apply_ops (s) | 33.8 | 23.9 |
| 300k batch 9 (s) | 9.0 | 7.8 |

The shape is unchanged (the cliff at batch 3, then slow growth), so this is a constant-factor win, not the fix for the growth.

**Recommended next** (not done; each is larger or changes policy):
- Make a cache miss cheaper than a syscall pair and make the cache denser. Cache the raw encoded node (cost = its length) and decode on hit (1-2 us), so the same RSS holds 4-5x more nodes; or calibrate `cost` to the real decoded size and raise the default with `node_cache_bytes` (a deployment choice: RSS grows about 5x the accounted bytes today). Expected: apply_ops flat once the previous batch's written nodes (about 400 MB encoded at 1M) fit.
- Prefetch with parallel reads: a batch's touched paths are known after sorting ops by trie hash; issue the node reads on a small thread pool before the serial apply. Expected 3-5x on the miss-bound part (apply_ops 24 s -> under 8 s at 1M) with no format change.
- Remove the persist_base term: dehydrate base tries after persist (needs a decoded cache on the base store; M5-lite) or mark nodes persisted per store. Expected -3.7 s of 38 s at 1M and about -3 GB RSS.
- Fewer nodes per row: fatter leaves (several entries per leaf node) would cut both reads and writes by about 1.4x but change node encodings and roots (needs its own ADR and a state-format version).

### Problem 2: the decisions read (1M: 12.7 s for 800k decisions, about 15.9 us each)

`all_settlements` reads the candidate frontier (not the decision trie): `CandidateFrontier::all_buckets` pages the whole `SMap`, verifies each bucket, and `settle`s it. Breakdown at 100k (80k decisions, 838 ms; same shape at 200k: 160k, 1.91 s): node reads inside `page` 762 ms (91%), `verify_bucket` 31 ms, `settle` 25 ms, result map 37 ms. There are 109k node reads for 80k buckets (1.36 per bucket: the trie holds a leaf per key plus branches), 190 bytes each, so the cost is 7 us per node read: with the old lookup about 5 pread syscalls to bisect the idx and one for the 20 KB bucket, one pread for the body, two blake3 passes (pack checksum, then trie digest), the allocation and decode, and two cache inserts. Decode, verify and settle are under 10%. Page size is irrelevant (256 vs 4096: same time). The decoded cache has zero reuse in a scan.

Fix landed: the sparse idx window above (1M: 12.7 -> 7.15 s; 200k: 1.91 -> 1.19 s). It is a constant factor: 800k decisions need at least 1.1M serial node reads, so the spec's under 1 s at 1.09M facts is out of reach by tuning the read path. Atlas's 363k orders mean about 363k decisions, about 3.2 s now.

Recommended: (a) a settlement ledger: the `{seq}.decisions` delta bodies already exist per revision; compact them into a sorted flat checkpoint (derived cache, verified against `decision_root` by recompute in `world verify`), so the read is one sequential pass (800k x about 100 B = 80 MB, around 0.1 s); (b) otherwise parallel subtree reads (N threads over the 32 top-level branches), roughly linear speedup; (c) a paged API (`settlements_page(cursor, limit)`) for consumers that do not need everything, which `for_each_bucket` now supports without materialising.

**RSS 2.9 GB after the one-row batch.** It is the high-water mark of the decisions read, kept by the allocator (the bench also holds the result map alive). Measured at 200k (160k decisions, `ps`/`time -l` peak): 930 MB with the old `all_settlements`; with a 1 MB decoded cache 447 MB; so the decoded-node cache contributes about 480 MB (128 MB accounted, 3.7x real) and the materialised `Vec<(FrontierKey, FrontierBucket)>` of every bucket (about 2 KB each, held until the settlement map is built) and the result map the rest. The one-row batch itself adds little (its staged network is dehydrated). Landed: `all_settlements` now streams (`for_each_bucket`), so only the settlements are retained: 200k peak 930 -> 617 MB; at 1M RSS after the one-row batch 2.95 GB -> 1.46 GB. Remaining about 450 MB is the decoded cache filled by a scan; a scan-resistant admission rule (do not insert pages read by bulk iteration) removes it.

### Gaps found
- `KNOWN_CAP` (262k) and the 65,536-node raw cache in `FileNodeStore` are shared by the base store and the state store (clones share `State`) and are far below a 1M world's node counts.
- GC (M6) must drop the per-pack sparse and Bloom tables and the `known` set along with any removed pack.
- Receipts do not carry the commit profile (observation only, by design); `WorldSession::last_commit_profile` is in-process.

## M6 / Lane C results (storage correctness, GC, disk)

**Result-returning reads.** `NodeStore::get_node -> Result<Option<Vec<u8>>, StorageError>`. `FileNodeStore` maps a checksum/format failure to `CorruptedNode` and OS errors to `Io`; the store-wide latch is kept (later reads, cached ones included, stay errors, `CorruptedNode` if the first failure was damage). Pack damage is `WorldError::CorruptState` end to end; the three r0_restore negative controls now require it.

**GC (`brix world gc <dir> [--dry-run] [--keep-revisions N]`, `world::gc::gc_world`).** Roots: for each retained revision its `relation_roots`, `secondary_index_roots`, `decision_root`; for HEAD also `network_state_root`; plus the state-cache sidecar root. Default retains every revision on disk (so `pin_revision`, `explain --rev`, `decision_delta`, audit builders and `world verify` behave as before); `--keep-revisions K` keeps the newest K plus genesis, HEAD and checkpoint pins, older revisions then error with a missing node (journal files are never touched). Older revisions' network-state tries are not roots: they are the bulk of the garbage. Reachability is a conservative byte scan for the canonical digest embedding (`01 20` + 32 bytes) that checks presence in the store, uniform over all trie types, the header and derivation objects; a false match only keeps a node. Live nodes are copied to new packs while marking, packs are published, then old packs are unlinked one by one (invariant: every live node is always in a published pack, so a crash leaves old, new or a superset, never a loss). Single-writer lock; stale `.tmp`/orphan `.idx` swept; removed packs drop Bloom/sparse/idx handles/`known`/node cache. Crash points `GcCrashPoint` x5 tested. `verify_after` reopens, audits all packs and checks each live node resolves.

**`open-check --verify-store`** (`world::gc::verify_store`): framing, idx, every body checksum; damage is `corrupt-state`.

### Disk (release, `r0_open_bench`; GC with default options)
| | before | live nodes / bytes | dead nodes | after | packs |
|---|---|---|---|---|---|
| 100k | 487 MB (498 MB at load) | 948,969 / 150 MB bodies | 643,005 (40% of 1.59M) | 302 MB | 11 -> 1 |
| 1M | 4.90 GB | 9,464,294 / 1.52 GB bodies | 6,482,875 (41% of 15.9M) | 3.04 GB | 10 -> 6 |

GC time 10.7 s (100k), 167 s (1M; dry-run mark alone 96 s). After GC `open-check --verify-state --verify-store` passes (restored, 8 ms / 358 ms open). So about 40% of nodes (38% of bytes) are garbage; the remaining 1.26-1.5x over alpha.4's 86 MB is live data, not garbage. Live overhead: bodies are ~50% of the compacted store; the rest is per-node framing, an 80-byte record in the pack footer plus an 80-byte record in the derived `.idx` (about 160 B per node vs 158 B average body). Not changed (pack format/idx layout out of scope for a no-risk pass); candidate next steps: drop the pack-embedded record table in favour of the idx, or store only a 16-byte checksum. Node encodings and roots are untouched.

### Gaps / notes
- Sessions open before a GC must be reopened. Concurrent readers in other processes are not supported during GC.
- Legacy loose objects: primitive covered in `soc-core/tests/pack_gc.rs`; no world-level legacy GC test (cannot construct a legacy world).
- The 1M figures were taken while another lane's 1M bench was running on the same machine.
## Lane B (alpha.5): fast decisions read and O(1) idempotency

Both are caches (ADR-0046 s5). No revision schema, network-state header, root or digest changes, so no M4 version handling is needed: every cache carries its own schema string, and any mismatch is a miss that falls back to the authoritative path with identical output.

### Settlement checkpoint (`settlements.ckpt`, `brix.world.settlement-checkpoint@1`)
- Flat file: magic, revision seq/digest, validity-key digest, decision root, count, sorted body (per decide: entities with canonical decision tuples), trailing digest of everything before it. Written tmp + rename (process-unique tmp; not fsynced, a torn file fails the trailer).
- Read (`WorldSession::all_settlements[_with_source]`, so CLI and serve get it): strict decode, validity key match, then at HEAD revision digest and decision root must match; otherwise the checkpoint is older and the `{seq}.decisions` deltas (digest-authenticated by `decision_delta`) are applied (max tail 256; missing/compacted delta = miss). The result is spot-checked against the persisted decision trie: same count, 64 evenly spread entries equal to the trie value, plus every key a tail touched. A full recompute of the decision root from the content is `verify_settlement_checkpoint` (also run by `open-check --verify-state`).
- Where it is written: lazily by the first frontier-path read (and by a read that applied more than 32 tail revisions), eagerly by `open-check --refresh-state` (`refresh_settlement_checkpoint`, which also checks that the settlements hash to the decision root). Never at commit: rewriting ~80 MB per commit would break O(delta); the tail mechanism keeps the cache current without touching commits. `open_replay`/`refresh_after_replay=false` never write (`checkpoint_autowrite`). Cost of the first read is the old frontier walk plus one encode and write.
- Paged variant: `settlements_page(cursor, limit)` over `CandidateFrontier` buckets (trie order, frontier path only).
- Crash point `CheckpointAfterTmpBeforeRename`.

### Idempotency index (`idempotency.json`, `brix.world.idempotency-index@1`)
- Content-addressed trie (`TrieMap<WorldKey, WorldTuple>`: key = canon(idempotency key), value = seq + revision digest + optional batch digest) in the node store, root named by a sibling sidecar (chosen over a header field because that would change `network_state_root`). Receipts remain the source of truth.
- Commit: the key is inserted on a copy before the flush, its nodes ride the same pack and fsync, the revision is assembled before the flush (it never depended on it), the sidecar is written after HEAD (not fsynced), the copy replaces the session's index only after publication. A lookup is O(log n) node reads.
- Open: sidecar at HEAD (digest must match) = no file scan; sidecar behind HEAD by at most 4096 = caught up from exactly those receipts (crash between HEAD rename and sidecar write); anything else (absent, garbage, other schema/world/revision, root missing in the store, too far behind, a receipt that does not agree) = the previous receipt scan, followed by a best-effort upgrade (default policy only). Old worlds therefore work unchanged and upgrade on first open.
- Policy: a damaged cache is never a hard error at open (it is a miss); a trie node that fails mid-lookup at commit is an error for that commit (refusal, never a different answer).
- GC (M6): the idempotency root must be a GC root alongside HEAD and the state-cache root.

### Measurements (release, M-series Mac, serial; other lanes were running benchmarks, so absolute numbers are noisy)
"Before" = the same binary with the cache file removed (frontier path / receipt scan are the unchanged previous code).

| Metric | 100k before | 100k after | 1M before | 1M after |
|---|---|---|---|---|
| Cold open (ms) | 13-15 | 13-14 | 149 | 133 |
| First all_settlements, fresh process | 590-770 ms | 51-60 ms | 12.05 s (7.15 s unloaded, see above) | 560-590 ms |
| of which first read after upgrade | | frontier, writes checkpoint | | frontier 12.05 s once |
| all_settlements with 1 revision of tail | | 52 ms | | 557 ms |
| One-row batch (ms, 5 samples) | 52-65 | 46-65 (A/B with the index disabled: same within noise) | 79-440 (noisy) | 79-440 |
| Objects written by one-row batch | 51 | 53 | | 59 |
| RSS after decisions read | 331 MB | 47 MB | 1.36 GB | 442 MB |

Open with many one-row batches (each batch is one pack): 1000 batches open 33 ms with the index vs 199 ms scan; 2000 batches 65 ms vs 1477 ms. The index removes the receipt scan; what remains at thousands of batches is `FileNodeStore::new` over one pack (+idx) per commit (110-720 ms at 1000-2000 batches on this contended machine), which is the M6 pack-index/compaction item, not this lane.

### Tests (`crates/brix-kb/tests/r0_fast_read.rs`, 17)
Checkpoint: equals frontier and `open_replay` over four fuzz seeds; tail application over 12 rounds equals replay; long tail refreshes; 15 corruption cases (empty, garbage, truncations, extension, byte flips in header/body/trailer) fall back identically and repair; forged/foreign (wrong digest, key, root, seq, a changed decision, a dropped decision) rejected on read and forged content caught by verify; compacted tail; stale older checkpoint; crash point; explicit refresh and `open_replay` never writes; paging covers everything once. Idempotency: index equals scan (and revision-only worlds), upgrade of old worlds, replay/conflict semantics for both sources, stale sidecar tail catch-up and persistence, seven unusable-sidecar cases, foreign sidecar, crashes before/after the HEAD rename, no file scan when current.

### Open issues
- Checkpoint read trusts self-digest plus probes; a forged-but-consistent file that agrees at all 64 probes and the count survives until `verify`. Full recompute on every read costs seconds.
- The first read of a 1M world is still the frontier walk (writes the checkpoint); `open-check --refresh-state` moves it off the request path.
- Checkpoint is a second copy of the decisions on disk (~80 MB per 800k) and is not GC-managed.
- Commit profile folds the index work into the existing `dehydrate`/`state_header` phases (phase names are asserted by a test).
## Lane A results: load growth and memory (alpha.5, 2026-10-10)

Roots, digests and node encodings are unchanged. Bench: `r0_open_bench` (now also records per-batch `rss_kb` and process `cpu` seconds). The machine was shared with other lanes' builds and benchmarks (load average 3-7), so wall time is unreliable (flush and persist_state swing 2-5x between runs); compare CPU seconds and byte counters.

What landed:
1. Base relation and secondary-index tries are reduced to `(root, len)` after each commit (`dehydrate_base`), the state a reopened session already has. `persist_base` no longer probes unchanged hydrated siblings (1M batch 9: 1.70 s -> 1.26 s; 300k batch 9: 455 -> 365 ms). `FileNodeStore` gained a decoded-node cache (disabled until a session sizes it) so base reads stay cheap; its write-through also lets `persist_node_recursive` skip `contains` for cached nodes.
2. Node cache rework. The raw encoded-node cache in `FileNodeStore` is now a byte-bounded hash map (payload + 128 B/entry overhead) instead of a 65,536-node count; `node_cache_bytes` is now the total budget (default 512 MiB), split raw 1/2, operator-state decoded 1/4, base decoded 1/8. Decoded-node cost is 8x the encoding (was 2x; measured RSS grows about 1.7 GB for a 2 GB budget). Bulk iteration (`iter_page_with_store`, so `all_settlements`) is scan resistant: `NodeStore::get_node_scan` never admits to the raw cache, and decoded admission is limited to branches within 3 levels of the root.
3. Not done: deterministic prefetch. The remaining apply_ops time is decode and hit cost, not syscalls, so a read-ahead pool would not help; it needs a larger decoded working set or cheaper decode.
4. Not done: M3 frontier persist/scan overhead (+10%).

Tried and reverted: releasing the network's own base copy after the first commit (`release_base`). It made no RSS difference in a measurement (1.52 -> 1.48 GB at 300k) and broke `world_skew_isolation`, which applies ops to a clone of `session.network`.

### Results (CPU seconds are cumulative over the load; per-batch ms are noisy)

300k, batch = 30k rows:

| | batch 2 ms | batch 9 ms | apply b9 | persist_base b9 | CPU total s | RSS after load |
|---|---|---|---|---|---|---|
| baseline 4c25a80 | 4107 | 11798 | 7228 | 455 | 62.8 | 1.99 GB |
| lane A | 3942 | 11640 | 7037 | 365 | 58.3 | 1.52 GB |

1M, batch = 100k rows (contended machine; baseline 289 s wall / 262 s CPU, lane A 370 s wall / 270 s CPU, i.e. no measurable load-time change):

| | b9 ms | b9 apply | b9 persist_base | RSS after load |
|---|---|---|---|---|
| baseline | 36317 | 21922 | 1703 | 5.34 GB |
| lane A | 58435 | 39005 | 1258 | 4.42 GB |

Per-batch growth is NOT fixed: late batches are still about 5-6x early per-row cost at 300k. A run with a 2 GB budget gave apply_ops 3.5 s at batch 9 (vs 7.0 s) and RSS 4.5 GB, so the lever is holding the working set decoded, which costs memory. RSS above the cache is dominated by the per-batch staged network (all path-copied nodes of a batch hydrated until persist and dehydrate: about 1.3 GB at 300k, growing with nodes written per commit); incremental persist-and-dehydrate inside a batch is the next lever (needs a design for intermediate state writes).

### Tests
`crates/soc-core/tests/node_cache_budget.rs` (byte-bounded raw cache, scan not admitted, decoded scan admission, dehydrated trie reads and writes reach the same root). Existing `r0_restore` differential and the full workspace suite pass.

### Gaps
- `NodeStore::get_node_scan` must be carried through lane C's `Result`-returning `get_node` on merge.
- `DECODED_COST_FACTOR` is empirical; revisit if node types change.
- A deeper `SCAN_ADMIT_DEPTH` or paging-aware admission may help repeated paged scans.

### Chunked-persist gate experiments (lane A, 2026-10-10/11)

New bench knobs: `R0_OPEN_BENCH_BATCHES` (batch count, default 10), `R0_OPEN_BENCH_LOAD_ONLY` (stop after the load), `R0_OPEN_BENCH_REPLAY_ONLY` (child opens by full replay and exits). Peak RSS is `/usr/bin/time -l` maximum resident set size. A smaller batch is a naive chunked persist plus per-batch publish overhead. macOS RSS can read low under memory pressure (other lanes were running), so treat RSS as +-10%. CPU seconds are cumulative process CPU. Default budget (512 MiB) unless noted.

**E0: batch count sweep (load only)**

| N | batches | wall s | CPU s | peak RSS GB | apply us/row | persist_state us/row | state bytes written/row | flush+publish+decision_encode s | last-batch apply us/row |
|---|---|---|---|---|---|---|---|---|---|
| 300k | 10 | 56 | 49.0 | 3.15 | 103 | 54 | 4839 | 3.7 | 183 |
| 300k | 40 | 64 | 62.0 | 2.02 | 96 | 78 | 6177 | 5.8 | 150 |
| 300k | 100 | 92 | 86.2 | 1.66 | 125 | 120 | 7337 | 9.3 | 235 |
| 1M | 10 | 259 | 223.6 | 5.98 | 156 | 66 | 4898 | 12.4 | 263 |
| 1M | 40 | 302 | 268.4 | 5.10 | 164 | 91 | 6098 | 15.0 | 348 |

(1M with 100 batches was not run: 300k already shows it costs +76% CPU.)

Go criterion (peak RSS falls >= 2 GB at 1M with <= 15% CPU): **FAILED**. 10 -> 40 batches frees 0.9 GB at 1M for +20% CPU (+26% at 300k, which frees 1.1 GB). Chunking rewrites shared upper-trie nodes once per chunk (state bytes written per row +25..50%) and does not touch the per-row apply cost, which keeps growing with trie size (last-batch apply 150-350 us/row vs 7 us in batch 1) even with 3000-row batches. Load growth is cache-miss and decode bound, not batch-size bound.

**E1: 300k, 40 batches, budget +1.1 GB (1536 MiB) with the raw cache at 3/4 (state 1/8, base 1/8)**

| | CPU s | peak RSS GB | apply us/row | last-batch apply us/row |
|---|---|---|---|---|
| 40 batches, default | 62.0 | 2.02 | 96 | 150 |
| 40 batches, 1536 MiB, raw 3/4 | 57.7 | 3.91 | 86 | 130 |
| 10 batches, default | 49.0 | 3.15 | 103 | 183 |
| 10 batches, 1536 MiB, raw 3/4 | 47.0 | 4.46 | 81 | 109 |

Apply_ops at the late batch falls 13-40%, not 2x; and the extra budget eats all the RSS that chunking freed (3.9 GB vs 3.15 GB un-chunked). Not a win.

**E2: heap per hydrated node** (`crates/brix-kb/tests/lane_a_node_heap.rs`, `--ignored`, counting allocator; 20k scattered upserts into a dehydrated 200k-entry `WorldKey -> WorldTuple` trie). `size_of::<Node<WorldKey, WorldTuple>>()` = 104 B, so a lazy child is `Arc<Node::Lazy>` of about 120 B (Opus's estimate holds). Measured 612 B of heap per path-copied node. Boxing the Leaf/Collision/Branch payloads (Node = 40 B, lazy child about 56 B; compiled with no logic change and the full test suite passed) lowered it to 519 B/node (-15%), implying about 1.45 lazy children per path-copied node, i.e. lazy children are about 30% of the hydrated heap, not >= 50%. The rest is leaf key/value allocations, `Arc<V>`, the children `Vec`s and per-allocation overhead. Per the gate, the compact `enum Child` refactor was NOT implemented and the Box experiment was reverted (it changed 42 construction sites in `store.rs` for -15% heap, with no change in RSS or CPU in an end-to-end run: 300k/10 batches peak 3.08 GB vs 3.15 GB, 300k/40 batches 1.94 vs 2.02 GB, CPU +-5% noise). Measured heap is about 4x the encoding, so `DECODED_COST_FACTOR = 8` includes cache-index overhead and is conservative, not unexplained. The compact-child refactor alone does not change the load-growth picture.

**E3: peak RSS of `open_replay`**: 300k: 3.98 GB peak (2.03 GB after open), 36.5 s, versus 34 MB and 41 ms for a normal restore open. About 13 KB per fact: the fallback applies the whole world as one `apply_ops` in `set_network`. 1M was not run (extrapolates to about 13 GB on a 16 GB machine shared with other lanes); the fallback needs chunked replay (apply the base facts in slices of a few 10k rows through the same path, persisting nothing) to be safe at scale. That is the one place where chunking is purely a memory win, since replay output is not persisted per slice.

**Go / no-go**

| Change | Verdict | Evidence |
|---|---|---|
| Naive chunked persist for load (40 batches) | NO-GO | 1M: -0.9 GB peak RSS for +20% CPU; fails the 2 GB / 15% criterion; more bytes written and more publish overhead |
| Chunked persist + larger raw cache (E1) | NO-GO | late apply -13..-40%, RSS ends higher than un-chunked |
| Compact children / boxed Node variants | NO-GO as a load fix | lazy children about 30% of the hydrated heap; -15% heap, no end-to-end change |
| Chunked replay inside `set_network` / `open_replay` (no per-chunk persist) | GO (separate task) | 300k replay peaks at 4 GB (13 KB/fact); output unaffected since nothing is persisted per slice |
