# World Runtime P1 — Persistent Primitives and Delta-Updated Calendar Results

Status: **P1 Complete**, 2026-10-03, branch `feature/persistent-world-runtime`.  
Governed by `docs/planning/persistent-world-runtime-plan.md` §4 (P1). Contract text: [ADR-0046](../../spec/adr/ADR-0046_Persistent_World_Runtime.md).

Every measurement below was captured from real output of the reproduction commands in §2.

---

## 1. What P1 Establishes

1. **Delta-updated calendar frontier (`Frontier::apply_delta`):**
   - Replaced whole-frontier cloning (`self.entries.clone()`) with a transactional rollback overlay (`RollbackOverlay`).
   - Clones **0** existing entries when applying a delta (previously copied all $N$ entries).
   - Atomic rollback via RAII guard on any error (`InsertConflict`, `RemoveMissing`, `RemoveMismatch`).
   - Complexity reduced from $O(N)$ to $O(|\Delta| \log N)$.
   - Wall-clock speedup: 32× at $N=1\text{k}$, 457× at $N=10\text{k}$, 1,686× at $N=50\text{k}$.

2. **Persistent Radix Trie / HAMT (`TrieMap`):**
   - Implemented behind the `PersistentMap` trait seam with safe Rust structural sharing (`Arc`).
   - 16-way branching (nibble-based) with path compression on intermediate branch nodes.
   - Contraction on deletion ensuring canonical minimal representation regardless of history.
   - Incremental Merkle root hashing caching sub-tree digests in nodes.
   - Deterministic collision buckets sorting complete keys for hash collision resistance.
   - Single-key insert clones **0** existing entries (compared to $N$ clones in `ArcMap`).
   - Wall-clock speedup: 135× faster insert at $N=100\text{k}$ (7.25 µs vs 978.6 µs).
   - Structural operations strictly logarithmic: depth bounded by $\le \log_{16} N + 1$ (visited/allocated/hashed $\le 5$ at $N=10\text{k}$).

3. **Durable Node Persistence (`NodeStore`):**
   - Content-addressed store interface (`NodeStore`, `MemoryNodeStore`) and canonical node serialization (`encode_node`) for durable persistence.

4. **Negative Controls:**
   - Retained `ArcMap::insert` and `Frontier::naive_apply_delta` as active negative controls in `world_scale_negative_controls.rs`. The cost harness continues to verify that naive paths fail flat scale gates.

---

## 2. Reproduce

```sh
# 1. Delta calendar scale & rollback verification:
cargo test -p soc-core --test frontier_delta_scale -- --nocapture

# 2. TrieMap scale & structural bound verification:
cargo test -p soc-core --test trie_map_scale -- --nocapture

# 3. TrieMap differential & canonical root verification:
cargo test -p soc-core --test trie_map_differential -- --nocapture

# 4. Negative controls (verify that naive paths continue to fail scale gates):
cargo test -p soc-core --test world_scale_negative_controls -- --nocapture --test-threads=1

# 5. Wall-clock benchmarks:
cargo bench -p soc-core --bench world_frontier_delta
cargo bench -p soc-core --bench trie_map_insert
```

**Environment:** Apple M1 Pro, 16 GiB RAM, macOS 15.6.1 (arm64), rustc/cargo 1.96.1.

---

## 3. Measurements

### 3.1 `Frontier::apply_delta` vs `naive_apply_delta`

Measured on 1 removal + 1 addition delta:

#### Deterministic Entry Clones (Observed via `Counted` Value Tracker)

| Frontier Size N | Naive (P0 Baseline) | `apply_delta` (P1) | Existing Entries Copied |
|---:|---:|---:|---:|
| 500 | 501 | **1** | **0** |
| 1,000 | 1,001 | **1** | **0** |
| 2,000 | 2,001 | **1** | **0** |
| 4,000 | 4,001 | **1** | **0** |

*Note: The single clone in P1 is the newly inserted candidate value being stored. Existing entries copied is strictly 0.*

#### Wall Clock (`world_frontier_delta`, 1 rm + 1 add, median / p90)

| N | Naive Median | P1 Median | P1 p90 | Speedup | P1 Scaling Ratio (vs 1k) |
|---:|---:|---:|---:|---:|---:|
| 1,000 | 10.625 µs | 334.125 ns | 382.750 ns | **31.8×** | 1.00× |
| 10,000 | 209.458 µs | 458.208 ns | 499.500 ns | **457.1×** | 1.37× |
| 50,000 | 983.167 µs | 583.083 ns | 648.792 ns | **1,686.1×** | 1.75× |

*Over a 50× increase in frontier size (1k to 50k), execution time increases by only 1.75×, demonstrating true $O(\log N)$ logarithmic scaling.*

---

### 3.2 `TrieMap::insert` vs `ArcMap::insert`

#### Deterministic Entry Clones for 1 Key Insert

| Store Size N | `ArcMap::insert` (P0 Baseline) | `TrieMap::insert` (P1) |
|---:|---:|---:|
| 1,000 | 1,000 | **0** |
| 2,000 | 2,000 | **0** |
| 4,000 | 4,000 | **0** |

#### Structural Metrics (`TrieOpStats`) for 1 Key Insert

| Store Size N | Nodes Visited | Nodes Allocated | Nodes Hashed | Key Comparisons | Max Depth Bound |
|---:|---:|---:|---:|---:|---:|
| 100 | 3 | 3 | 3 | 1 | $\le 8$ |
| 1,000 | 4 | 4 | 4 | 1 | $\le 8$ |
| 10,000 | 5 | 5 | 5 | 1 | $\le 8$ |

*Structural work tracks $\le \log_{16} N + 1$ path allocations without touching or re-hashing unaffected sibling nodes.*

#### Wall Clock (`trie_map_insert` vs `arc_map_insert`, median / p90)

| N | `ArcMap` Median | `TrieMap` Median | `TrieMap` p90 | Speedup | `TrieMap` Scaling Ratio (vs 1k) |
|---:|---:|---:|---:|---:|---:|
| 1,000 | 16.708 µs | 4.195 µs | 4.498 µs | **4.0×** | 1.00× |
| 10,000 | 76.583 µs | 5.485 µs | 5.751 µs | **14.0×** | 1.31× |
| 100,000 | 978.625 µs | 7.248 µs | 7.643 µs | **135.0×** | 1.72× |

`TrieMap::get` read latency across sizes:
- N=1,000: 167 ns
- N=10,000: 183 ns
- N=100,000: 208 ns

---

## 4. Status Against P1 Exit Criteria

| Exit Criterion | Verification | State |
|---|---|---|
| **Equal maps from different insertion histories produce byte-identical logical roots** | Proptest 100 random traces with shuffled insertion order + insert/remove cycles assert `m1.root_digest() == m2.root_digest()`. Contraction on remove preserves canonical branching. | **MET** |
| **Old snapshots remain valid and untouched after derivation** | Persistent structural sharing with `Arc` preserves root and node handles. Verified across all tests. | **MET** |
| **Single-key updates and rollback do not copy/hash the whole store or candidate frontier** | `TrieMap::insert` entry clones = 0; path allocations $\le 5$ at 10k. `Frontier::apply_delta` entry clones = 1 (the added candidate); existing entries cloned = 0. Rollback overlay restores state in $O(\Delta)$. | **MET** |
| **Differential collection tests cover removal, overwrite, collisions, and adversarial key distributions** | Differential test suite compares against `BTreeMap` reference oracle; covers sequential keys, shared dense prefixes, sparse powers of two, and deliberate hash collisions via `ModuloHasher(3)`. | **MET** |
