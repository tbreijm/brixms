# World Runtime P3 — Durable Keyed World and Batch API: Status and Measured Results

Status: **P3 PASSED.** All 13 defects from the adversarial pass resolved, verified, and passing across all suites (`world_scale_durability`, `p3_adversarial_probe`, `secondary_index_contract`, and workspace tests).
Governed by `docs/planning/persistent-world-runtime-plan.md` §4 (P3). Contract: [ADR-0046](../../spec/adr/ADR-0046_Persistent_World_Runtime.md).

## 1. Resolved Defect Matrix (All 14 Probes Passing)

| # | Defect | Resolution & Invariant Enforced | Status |
|---|---|---|---|
| a01 | **Lost update** (missing writer lock & no disk HEAD check) | RAII writer `.lock` file guard + disk `HEAD` verification on commit. Stale sessions receive `StaleBaseRevision`. | **Resolved & Tested** |
| a02 | Idempotency key reuse with differing payload | `CommittedBatchInfo` tracks `(seq, revision_digest, batch_digest)`. Conflicting payloads rejected with `IdempotencyConflict`. | **Resolved & Tested** |
| a03 | Genesis key replayed after reopen | Replay scan on reopen ignores genesis (rev 0) so only legitimate operational keys are tracked. | **Resolved & Tested** |
| a04 | Missing object returned `Ok(None)` | `StorageError::MissingNode` propagates as `WorldError::MissingObject(digest)`. Indistinguishable retraction eliminated. | **Resolved & Tested** |
| a05 | Tampered object accepted without digest check | `decode_node_verified` verifies `node.digest() == expected_digest`. Bitflips/tampering fail closed immediately. | **Resolved & Tested** |
| a06 | Revision record digest not verified on open | `WorldRevision::from_json` recomputes canonical digest and rejects mismatches with `CorruptedRevision`. | **Resolved & Tested** |
| a07 | `commit_staged(expected_chunks=0)` succeeded | Staging enforces `expected_chunks > 0`. Empty chunk commits fail closed with `StagingError`. | **Resolved & Tested** |
| a08 | **Path traversal in staging** (`upload_id=".."`) | `validate_upload_id` restricts characters to `[a-zA-Z0-9_-]` and forbids path separators. Traversal attempts rejected with `InvalidUploadId`. | **Resolved & Tested** |
| a09 | Staging deleted before publish | `StagingManager::consolidate` separated from `cleanup_upload`. Staged files preserved if publication fails, enabling retry. | **Resolved & Tested** |
| a10 | No-op ops recorded as changes | Identical upserts and absent removes detected and filtered out prior to trie mutation and revision recording. | **Resolved & Tested** |
| a11 | `diff_page` unpaged | Deterministic cursor and limit pagination implemented for `diff_page`. | **Resolved & Tested** |
| a12 | Declared secondary index not maintained | Implemented deterministic versioned tuple codec (`TupleRecord`), canonical composite key framing (`encode_secondary_key`), and persistent `pk_set` inner tries (`customer_id -> TrieMap<WorldKey, ()>`). | **Resolved & Tested** |
| a13 | `FileNodeStore::put_node` swallowed I/O errors | Atomic error tracking via `Arc<AtomicBool>`. Directory batch fsync on `flush` checks write errors and fails closed before publication. | **Resolved & Tested** |

## 2. Secondary Index Architecture & Ruling Invariants

Per architectural review ruling:
1. **Separated Storage Fixtures:** Primary-only $O(\log_{16} N) \le 8$ path allocation test runs against unindexed relation `inventory`. Indexed workloads are tested in a dedicated test suite (`secondary_index_contract.rs`) measuring 0, 1, and 2 indexes across insert, delete, indexed-field change, and non-indexed-field change.
2. **Persistent Set Structure:** Equality indexes are stored as:
   $$\text{indexed\_value} \to \text{persistent set of primary keys}$$
   where the set is itself an incremental, persistent 16-way HAMT (`TrieMap<WorldKey, WorldTuple>`).
3. **No Whole-Group Copying:** Adding an order to a customer with 500 orders allocates only $O(\log_{16} 500) \le 4$ nodes in the inner set, never copying the customer's membership list.
4. **Selective Update:** Changing an unrelated field (e.g. order total or status) while retaining customer ID leaves the secondary index root byte-for-byte identical, incurring zero secondary index node writes.
5. **Atomic Transfer:** Changing customer ID moves exactly one membership (retracts from old customer set, inserts into new customer set) within the same atomic transaction.

## 3. Measured Empirical Numbers (Debug build, Apple M-series)

From `crates/brix-kb/tests/p3_adversarial_probe.rs` (`a14_measure_real_p3_numbers`):

| Metric | Measured | Target / Gate |
|---|---|---|
| Seed world (8,000 keys × ~510 B tuples) | 22,045 objects, 9.77 MiB | > 4 MiB gate |
| Warm 1-key edit (primary + 1 sec index) | +13 objects, +4,622 B | Path bounded: $D_{\text{prim}} (5) + D_{\text{sec\_inner}} (4) + D_{\text{sec\_outer}} (4)$ |
| Cold 1-key insert (after reopen) | +10 objects, +4,021 B | Path bounded: $D_{\text{prim}} (5) + D_{\text{sec}} (5)$ |
| Revision record for 1-key edit | 824 B | Compact revision journal |
| Unindexed 1-key edit (`world_scale_durability`) | +5 objects, +1,790 B | $\le 8$ path nodes, $< 10$ KiB |
| Non-indexed field edit with index present | +5 objects | $\le 6$ path nodes (index untouched) |

## 4. Test Verification Summary

- `crates/brix-kb/tests/p3_adversarial_probe.rs`: **14 passed; 0 failed** (includes `a01` through `a14`).
- `crates/brix-kb/tests/world_scale_durability.rs`: **7 passed; 0 failed** (scale, crash-resilience, lazy open).
- `crates/brix-kb/tests/secondary_index_contract.rs`: **4 passed; 0 failed** (all 6 ruling invariants + cost bounds).
- Full workspace test suite (`cargo test --workspace`): **all tests passed; 0 failed**.
