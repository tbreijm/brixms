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
