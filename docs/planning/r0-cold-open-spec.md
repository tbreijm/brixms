# R0: Cold open without replay

Status: proposal (2026-10-09). Baseline: d38761a (alpha.4). Consumer: Atlas AI (long-lived Rust service embedding brix-kb with a warm WorldSession).
**Decision (2026-10-09): Phase 2 is the goal.** Phase 1 (checkpoint snapshots) is not shipped as a product phase; it is retained below only as a possible interim/fallback.

## 1. Problem

`WorldSession::open` (brix-kb/src/world/session.rs:710) rebuilds the operator network by replaying every resident base fact. `set_network` (session.rs:1841) iterates every base relation trie and calls `network.apply_ops` on all of it, re-evaluating every join, filter, projection, grouped count and decide guard.

Measured on Atlas data (362,991 TMS orders, 1.09M facts, 58-module program):
- cold `world decide`: 158 s, 100% in open -> set_network -> apply_ops_staged_bounded
- 39k-row commit: 154 s, almost all open-time replay
- load time grows superlinearly (each commit re-opens the world)

So ADR-0046's O(Delta) contract holds in-process but not across process starts.

Secondary: `WorldNetwork.base_relations` (network.rs:472) copies every base fact into an in-memory PMap alongside the session's persisted tries; every base fact lives in memory twice.

## 2. Persisted vs not

| State | Persisted | Where |
|---|---|---|
| Base relations | yes | content-addressed TrieMap per relation; roots in revision record |
| Settled decisions | yes | network.decision_tree nodes (P6 G2) |
| Revision chain, batches, program closure | yes | revisions/, program.json |
| Operator state (join indexes, Distinct supports, GroupedCount groups, intermediates) | NO | network.operator_states: PMap<usize, OperatorState> |
| Derived relation contents | NO | network.derived_relations |
| Candidate frontier | NO | network.candidate_frontier |

## 3. Goals / non-goals

- G1: cold-open cost does not grow with resident facts.
- G2: restored network byte-identical to full replay (state roots, settlements, explanations).
- G3: commits stay O(D+A+R+P) (ADR-0046 s2); operator persistence adds only the P term for touched nodes.
- G4: crash safety equals today's: old commit or complete new one (ADR-0046 s3.6).
- G5: persisted state is a cache, never authoritative (ADR-0046 s5). `world.verify` still replays independently from the journal.

Non-goals: multi-writer, distributed state, change to brix.world.batch@1, change to settlement semantics.

## 4. Phase 2: operator state as persisted tries (the goal)

Every piece of mutable network state becomes a content-addressed TrieMap in the node store.

| State | Trie key | Trie value |
|---|---|---|
| Join operator index, per side | (join_key canonical value, input tuple digest) | IntermediateTuple |
| Distinct supports | TupleRecord digest | canonical PSet<DerivationId> |
| GroupedCount | group key values | support set + count |
| Derived relation contents | TupleRecord digest | TupleRecord |
| Candidate frontier | (decide, entity, candidate) | CandidateEntry |
| Settlements | unchanged (decision_tree) | unchanged |

- **Network state root**: digest over (operator id -> trie root) plus derived, frontier and decision roots, canonical order. Revision record gains `network_state_root`; revision digest covers it.
- **Commit**: staging (staging.rs) overlays changes; operator updates become trie path-copies writing only touched nodes, in the same object pack and fsync as base-relation changes. O(Delta log n) node writes, within P.
- **Open**: `TrieMap::from_root_digest(...)` per operator, lazily loaded via a node cache. No replay, no full read.
- **Remove duplicate base copy**: WorldNetwork reads base tuples through a store-backed view of the session's relation tries instead of `base_relations: PMap`.
- **Fallback**: program change or validity-key change -> one full replay, persist new roots, continue incrementally.
- Validity key: program_digest, CompiledProgramEnv version (compiler + world_expr lowering), exec profile (brix.world.exec@1, evaluator id, numeric semantics), result-affecting network limits.

## 5. Correctness and verification

1. Differential invariant (CI): for every test world and fuzz step in `world_reference_differential`, restore-from-persisted vs replay-from-journal must have equal network state roots, settlements, derived relations, `explain_decision`. Negative control: corrupt one persisted node, restore fails closed.
2. Authority unchanged: `world.verify` and the reference evaluator never read persisted operator state.
3. `open --verify-state` (opt-in): restore, run recompute_from_scratch, compare roots.
4. Crash safety: extend `set_crash_point` to state writes; recovery sees old or new HEAD, each with consistent network root.
5. Determinism: equal logical state -> byte-identical network roots regardless of batching/insertion order (ADR-0046 s3.3).

## 6. Interfaces

- `brix.world.revision@1` gains optional `network_state_root`; older revisions open via replay.
- world.json: `node_cache_bytes`.
- CLI/serve: `brix world open-check <dir> [--verify-state]`; world.* JSON results gain `open_ms`, `open_mode` (restored | replay), `tail_batches`.
- Library: `WorldSession::open` restores by default; `WorldSession::open_replay` forces replay (tests, verify). `set_network` unchanged for fresh-program path.
- Optional: `brix serve` caches one open WorldSession per world dir (single-writer lock, invalidate on external HEAD change).

## 7. Acceptance (real Atlas world 1.09M facts + synthetic 100k/1M/10M)

- Cold open + world.decisions (no new batch), 1.09M facts: < 1 s
- Cold open across 100k -> 10M facts: flat (+/-20%)
- One-row world.batch after cold open: < 100 ms; work counters proportional to Delta
- Full load of 362,991 orders (8 batches): total proportional to rows, no superlinear growth
- Restored roots = replay roots in all differential tests
- Crash at every new crash point: old or new HEAD only
- Resident memory at 1.09M facts: <= 0.6x today's

## 8. Rollout

Operator by operator: join index first (dominant in profile), then Distinct, GroupedCount, derived relations, frontier; each step guarded by the root-equality test. Remove in-memory base copy last. Atlas AI reruns `scripts/atlas_ingest_tms.py` + cold world decide at each step.

## Appendix: Phase 1 (interim, not the goal)

`brix.world.network-snapshot@1`: canonical encoding of WorldNetworkState stored in the node store, referenced from the revision record (`network_snapshot`), valid only on matching validity key; written at commit per `snapshot_every_revisions` / `snapshot_min_delta_ops`; open = newest valid snapshot + tail batches via apply_ops. Still O(world) bytes. Only to be built if Phase 2 slips.

## Follow-on (separate spec)

Pluggable `NodeStore` with Postgres/Lakebase implementation (`brix-store-pg`, outside Ring 0), enabled by Phase 2's lazy store-backed tries.
