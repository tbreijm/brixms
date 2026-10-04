# World Runtime P5 — First End-to-End Usable World Slice: Measured Results

Status: **P5 exit met** (2026-10-04). Governed by `docs/planning/persistent-world-runtime-plan.md` §4 (P5).
Numbers below are from one orchestrator-run of
`cargo test -p brix-kb --test p5_adversarial_probe -- --nocapture --test-threads=1`
(debug build, Apple M1 Pro). Wall times are companion measurements only and vary
across runs (Lane A observed 10k ingestion between 16 s and 49 s and one-fact edits
between 87 ms and 118 ms in other debug runs); the structural counts are the gates.

## Exit criterion

> A real API session keeps a 10k-row linked model open, changes one fact, updates only
> its affected region, survives restart, and agrees with the oracle.

| Requirement | Evidence | Result |
|---|---|---|
| 10k-row linked model open in one session | `p01_mvp_10k_linked_model_open_session` | 10,000 rows, 16,051 objects, 54,000 intermediate deltas; 20.4 s |
| One-fact edit is local | `p02_local_one_fact_edit_exact_metrics` | objects_written = 5 (gate ≤ 8), intermediate_deltas = 12 (gate ≤ 12), settlements_updated = 1; 59.9 ms |
| Restart + directory move | `p03_restart_and_directory_move_offline_resilience`, p08 | cold open after move 0.9 ms, revision 2 restored |
| Per-tuple / per-decision explanation | `p04_per_tuple_and_per_decision_explanation` | contrastive guards reported |
| Public CLI + stdio path | `p05_cli_and_stdio_protocol_roundtrips`; `crates/brix-cli/tests/world_public.rs` (4 tests) | pass |
| Distinguishable failure classes | `binary_world_errors_distinguish_status_categories` | `unsupported-operator` / `resource-exhaustion` / `missing-input` distinct in JSON + human output; idempotent replay reported |
| Oracle agreement | `p06_full_oracle_agreement_under_adversarial_mutations` **and** `world_reference_differential` (7 tests, 750 fuzz steps) | pass |

**About the oracle:** p06 (and P4's p07) compare against `WorldNetwork::recompute_from_scratch`, which replays
through the *same* network code. That is a self-consistency check, not independent evidence. Independent
agreement comes from `brix_kb::world::reference` (P6a, merged `5dcad84`). That check was mutation-tested:
breaking candidate multi-support or Distinct support counting in `network.rs` makes the fuzz batteries fail.

## p08 empirical report (same run)

| Workload | Measured |
|---|---|
| Seed: 1,200 ops across orders/inventory/shipping | 9.23 s; 1,845 objects; 6,600 intermediate deltas; 1,000 derived rows; 1,000 decisions |
| Targeted one-fact edit | 56.6 ms; 4 objects; 12 deltas; 1 settlement |
| Directory move + cold open | 0.9 ms; revision 2 |

## Changes that closed P5

- `world_expr.rs`: in the world profile, `and` lowers to logical `&&` (`normalize_world_and`). The legacy
  v2 profile still refuses `and`/`then` as witness composition. Recorded in ADR-0046 §3.5.
- `examples/world-linked`: the `decide` block moved to the root module. A plain `use` imports relation
  definitions but must not execute another model's decisions (ADR-0046 §3.4).
- `brix world` error classification (`classify_network_error`).
- p02 fixture: off-by-one in the express flag; the delta bound derived structurally (6 stages × retract+insert = 12).

## Risks carried into P7/P8

- **Ingestion throughput:** about 500–1,000 rows/s in debug. At that rate, 1M rows means tens of minutes of
  first ingestion, so it needs a release-build measurement and probably a bulk-load path before P8.
- A one-fact edit costs about 60 ms in debug, likely dominated by fsync and publication. This needs a
  release-build breakdown.
- The entity id is still the `extract_entity_id` heuristic. It will be replaced by explicit `per <field>`
  (P6 contract §7.1).
