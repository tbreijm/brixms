# World Runtime P2 — Module Graph and Relational Frontend Results

Status: **P2 Complete**, 2026-10-04, branch `feature/persistent-world-runtime`.  
Governed by `docs/planning/persistent-world-runtime-plan.md` §4 (P2). Contract text: [ADR-0046](../../spec/adr/ADR-0046_Persistent_World_Runtime.md).

Every measurement and assertion below is captured from the real output of the reproduction commands in §2.

---

## 1. What P2 Establishes

1. **Qualified Module Graph & Visibility (`brix_lower::module_graph`):**
   - Qualified symbol resolution (`QualifiedName { module, name }`).
   - Module visibility enforcement: `export` modifier exposes symbols for cross-module consumption; non-exported items remain private to their declaring module.
   - Cross-module access to unexported items is rejected with `ModuleLinkError::NonExportedAccess`.
   - Coexistence of private helpers: identical private symbol names across distinct modules coexist simultaneously in the linked plan without flat-name collisions.
   - Dead-code reachability pruning: active executable plans include only reachable dependencies from the root; unused library exports are pruned.

2. **Bounded Iterative Loading (`ModuleLoader`, `SizedLoader`):**
   - Host operational limits (`ModuleLoaderLimits`): `max_import_depth`, `max_import_modules`, `max_module_source_bytes`, and `max_total_source_bytes`.
   - Pre-allocation limit enforcement: `ModuleLoader::module_size` probes module size before allocating the source buffer into memory. `SizedLoader` rejects oversize modules strictly before calling `load_module`, preventing unbounded heap allocation.
   - Cycle detection: cyclic import paths are identified and rejected with `ModuleLinkError::Cycle(chain)`.
   - Pinned `ProgramGraphManifest`: tracks content digests, interface digests (exported signatures), and transitive manifest hashes.

3. **Interface-Based Invalidation (`compute_affected_modules`):**
   - Precise compile-cache invalidation: an edit to an internal function body alters only the module's implementation content digest while leaving its interface digest unchanged.
   - Downstream importers are only marked affected if the modified module's public interface digest changes.
   - Unrelated modules imported into the graph are never marked affected by changes to independent libraries.

4. **Contract Validation (`LinkedProgram::validate_contracts`):**
   - Schema interfaces: record configs validate unique field names; sum configs validate unique variant names; relational input schemas validate non-empty key fields that exist in the schema.
   - Helper contracts: functions validate unique parameter names and valid signatures.
   - Violations are rejected before graph activation with `ModuleLinkError::InvalidSchema` and `ModuleLinkError::InvalidHelper`.

5. **Relational Source Grammar & Compatibility (`brix-syntax`):**
   - Semicolon-free relational declarations: `rel input <name>: <type> key <fields>` and `rel derived <name> = select <expr> from <bindings> [where <predicates>] [group by <fields>]`.
   - Anonymous record expressions (`{ f: e, ... }`), record types (`{ f: T, ... }`), and qualified paths (`module::item`).
   - Contextual keywords: `key`, `group`, `by`, `rel`, `select`, and `export` function as ordinary identifiers in let bindings, parameter names, record fields, and expressions without breaking legacy code (`let select = 1`, `let export = 1`, `let rel = 1`).
   - Bounded export modifier parsing: consecutive `export` modifiers are checked immediately; recursion is bounded by `ParseLimits::max_nesting_depth`, safely rejecting 20,000 chained `export`s without stack overflow.

6. **Relational Operator DAG Lowering (`brix_lower::relation_dag`):**
   - Lowers relational AST items to an explicit operator DAG: `Scan`, `Filter`, `Project`, `EquiJoin`, `Distinct`, and `GroupedCount`.
   - Equijoin inference: automatically extracts equality predicates (`left.k == right.k`) to construct hash equijoins, rejecting cross-joins that lack equality keys (`MissingEquiJoinPredicate`).
   - Recursion cycle detection: detects and rejects recursive relation SCCs (`RelationalLowerError::RecursiveRelationCycle`).
   - True unstratified negation detection: identifies mutual cyclic dependencies involving negation and rejects them with `RelationalLowerError::UnstratifiedNegation`.
   - Unsupported negation refusal: rejects relational negation outside of admitted profile operators with `RelationalLowerError::UnsupportedNegation`.

---

## 2. Reproduce

```sh
# 1. Module graph scale, bounded loading, and contract validation:
cargo test -p brix-lower --test module_graph_scale -- --nocapture   # 9 tests

# 2. Relational operator DAG lowering and cycle/negation validation:
cargo test -p brix-lower --test relational_dag_test -- --nocapture  # 7 tests

# 3. Relational syntax parser, legacy compatibility, and bounded export tests:
cargo test -p brix-syntax --test relational_syntax -- --nocapture   # 10 tests

# 4. Workspace lint & formatting check:
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

**Environment:** Apple M1 Pro, 16 GiB RAM, macOS 15.6.1 (arm64), rustc/cargo 1.96.1.

---

## 3. What Each Test Asserts

| Test | Asserts |
|---|---|
| `test_100_modules_link_without_flat_name_collisions` | 100 modules each declare an identical private `internal_helper` and private `InternalMeta` config, plus an exported `compute_{i}` calling both. Root imports and calls all 100 modules. All 201 functions (1 root + 100 exported compute + 100 private internal_helper) and all 100 `InternalMeta` configs coexist simultaneously in `linked.functions` and `linked.configs` without flat-name collisions. |
| `test_1000_helpers_and_512_schemas_validate` | 1,000 helper functions and 512 schema records load, link (`graph.link()`), and validate contracts (`linked.validate_contracts()`) cleanly, producing 1,001 linked functions and 512 linked configs. |
| `test_invalid_contract_rejected_at_link` | Contract violations are rejected at linking: duplicate field in a schema fails with `InvalidSchema`; duplicate parameter in a helper fails with `InvalidHelper`. |
| `test_interface_based_invalidation` | App imports `lib_b` and `unrelated_c`. Implementation-only edit in `lib_b` affects only `lib_b` (0 downstream invalidations). Interface edit in `lib_b` affects `lib_b` and `app_a`, while `unrelated_c` is never affected in either case. |
| `test_unused_library_exports_pruned_from_active_plan` | 50 exported functions and 20 exported configs unused by root are pruned: linked active plan contains exactly 2 functions (`root::main_entry` and `big_lib::target_fn`) and 0 configs. |
| `test_bounded_loading_depth_limit_refusal` | Limit 2, chain `m0 -> m1 -> m2 -> m3` fails with `ImportDepthExceeded { limit: 2, found: 3, chain: ["m0", "m1", "m2"] }`. |
| `test_bounded_loading_module_count_limit_refusal` | Limit 2, root + 3 imports fails with `ModuleCountExceeded { limit: 2, found: 3 }`. |
| `test_bounded_loading_module_bytes_limit_refusal` | Limit 200, 500-byte module fails with `ModuleBytesExceeded { limit: 200, found: 500 }`. Verified with `SizedLoader`: `load_module` panics if called, proving refusal happens strictly before source allocation. Fallback post-load check for unsized loaders also verified. |
| `test_bounded_loading_cycle_detection` | `a -> b -> c -> a` fails with `Cycle(["a", "b", "c", "a"])`. |
| `test_legacy_identifiers_and_contextual_keywords` | Validates that `select`, `export`, `rel`, `key`, `group`, and `by` parse validly as let bindings (`let select = 1`), function parameters (`fn f(select: Int): Int = select`), record fields (`config T = { rel: Int, select: Str, export: Bool }`), and anonymous record expressions. |
| `test_export_chain_bounded_and_no_stack_overflow` | 20,000 chained `export` modifiers under `ParseLimits::strict()` fail immediately with `ParseError` in <1 ms without stack overflow. |
| `test_adr0046_fulfillment_lowering_to_operator_dag` | Lowers the ADR-0046 fulfillment pipeline to typed `Scan`, `Filter`, `Project`, `EquiJoin` (`sku == sku`), and `Distinct` operator nodes. |
| `test_grouped_count_lowering_to_operator_dag` | Lowers grouped line item aggregation to typed `GroupedCount` with grouping keys `["sku"]` and alias `pending_orders`. |
| `test_recursive_relation_cycle_rejected` | Mutual recursion cycle (`r1 -> r2 -> r1`) rejected with `RecursiveRelationCycle`. |
| `test_self_recursive_relation_rejected` | Self-recursive relation (`r -> r`) rejected with `RecursiveRelationCycle`. |
| `test_unstratified_negation_rejected` | Cyclic dependency with negation (`active_orders -> !pending_orders -> active_orders`) rejected with `UnstratifiedNegation`. |
| `test_unsupported_relational_negation_rejected` | Negation of relation outside cycle (`where !cancel`) rejected with `UnsupportedNegation`. |
| `test_missing_equijoin_predicate_rejected` | Join without equality predicate rejected with `MissingEquiJoinPredicate`. |

---

## 4. Status Against P2 Exit Criteria

| Exit Criterion | Verification | State |
|---|---|---|
| **$\ge 100$ modules link without flat-name collisions** | `test_100_modules_link_without_flat_name_collisions` links 101 modules; 201 functions and 100 configs (including 100 identical private helper names and 100 identical private config names) coexist simultaneously under qualified names. | **MET** |
| **1,000 helpers and 512 schemas validate under explicit test profile** | `test_1000_helpers_and_512_schemas_validate` links and validates contracts across 1,000 helpers and 512 schemas. `test_invalid_contract_rejected_at_link` verifies contract enforcement. | **MET** |
| **Edit to one implementation recompiles only affected dependency region** | `test_interface_based_invalidation` confirms body edits do not invalidate importers, and unrelated modules are never marked affected. | **MET** |
| **Unused library exports do not fill active plan or invalidate unrelated operators** | `test_unused_library_exports_pruned_from_active_plan` prunes 100% of unused library exports (50 functions, 20 configs) from active plan. | **MET** |
| **Import count/bytes/depth limits fail at loading, before unbounded allocation** | `SizedLoader` enforces module and total byte limits strictly before source buffer allocation (`load_module` is never called); depth and count limits refuse before descending/enqueuing. | **MET** |
| **Relational source surface lowered to explicit operator DAG** | `lower_relations` lowers input/derived queries to `Scan`, `Filter`, `Project`, `EquiJoin`, `Distinct`, and `GroupedCount`. | **MET** |
| **Unsupported recursive relation SCCs and unstratified negation rejected** | Recursive SCCs trigger `RecursiveRelationCycle`; mutual cyclic negation triggers `UnstratifiedNegation`; non-admitted relation negation triggers `UnsupportedNegation`. | **MET** |
