# World Runtime P4 — Maintained Operator Network and Precise Invalidation: Status and Measured Results

Status: **P4 PASSED.** All 8 adversarial probes and 9 operator network tests passed, verified, and qualified across all suites (`world_operator_network`, `p4_adversarial_probe`, and full workspace check).
Governed by `docs/planning/persistent-world-runtime-plan.md` §4 (P4). Contract: [ADR-0046](../../spec/adr/ADR-0046_Persistent_World_Runtime.md) §3.5, §3.7.

## 1. Verified Invariant Matrix (All 8 Probes Passing)

| # | Probe | Invariant Enforced & Verified | Status |
|---|---|---|---|
| p01 | **Empty-to-nonempty join & reverse** | Join on empty input produces 0 tuples. Unmatched row preserves 0 tuples. Single match transitions $0 \to 1$. Multi-match activates $1 \to 2 \to 4$. Retraction cleanses down $4 \to 2 \to 0$. | **Verified** |
| p02 | **Duplicate derivations & multi-support** | Same tuple derived via two distinct paths receives 2 supports. Retracting first path preserves tuple. Retracting second path cleanly eliminates tuple. | **Verified** |
| p03 | **3-hop cascade retraction** | Chain $A \to B \to C \to \text{Decide}$. Retracting base tuple in $A$ propagates through $B$ and $C$, cleanly retracting candidate proposal from entity decision pool. | **Verified** |
| p04 | **Branch switching & re-deliberation** | Flipping a fact's field falsifies winning guard. Winner immediately re-deliberates to lower-priority alternative candidate or none, strictly following `(phase, priority, tiebreak)` calendar discipline. | **Verified** |
| p05 | **Absent-key insertion & range stability** | Inserting unindexed / non-matching keys into base relations executes $O(\Delta)$ without touching or invalidating unrelated join matches, candidate supports, or settled decisions. | **Verified** |
| p06 | **Grouped count transitions** | Grouped aggregation correctly transitions $0 \to 1$ (row created), $1 \to 2$ (row updated), $2 \to 1$ (row updated), and $1 \to 0$ (row retracted without leftover count=0 record). | **Verified** |
| p07 | **Differential oracle fuzz (75 mutations)** | 75 random mixed mutations (upserts, updates, retractions, branch switches) tested against independent scratch-recomputed oracle (`WorldNetwork::recompute_from_scratch`). 100% equivalence in derived tuples, candidate frontiers, and settled decisions. | **Verified** |
| p08 | **Empirical workload scaling** | Measured intermediate delta volume, derived row generation, and settlement update bounds under batch mutations. | **Verified** |

## 2. Measured Empirical Numbers (Debug build, Apple M-series)

From `crates/brix-kb/tests/p4_adversarial_probe.rs` (`p08_measure_real_p4_numbers`):

| Metric | Measured | Target / Gate |
|---|---|---|
| Seed workload (120 ops across base inputs) | 1,150 intermediate deltas, 300 derived tuples, 100 candidates | Correct multi-relation propagation |
| Single-key edit in linked model | 22 intermediate deltas, 1 settlement updated | $O(\Delta)$ affected region, no full scan |
| Absent-key insert (no join match) | 0 join matches, 1 group row update | Strict locality |
| Differential oracle equivalence | 75/75 mutations matching 100% | Zero discrepancy against scratch oracle |

## 3. Test Verification Summary

- `crates/brix-kb/tests/world_operator_network.rs`: **9 passed; 0 failed**
- `crates/brix-kb/tests/p4_adversarial_probe.rs`: **8 passed; 0 failed**
- `cargo check --workspace`: **clean (0 errors, 0 warnings)**
