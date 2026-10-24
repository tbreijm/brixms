# Alpha.8: Concrete Generic Schemas, Grouped Collection, Decision Evidence, and Protected Authority

Baseline: `v0.1.0-alpha.7` (`6822203`).

## Scope

Alpha.8 delivers complete, integrated support across Checkpoints 1 through 9:

- **Review Hardening & Concrete Generic Identity (R8 & R13)**:
  - Exact module-qualified nominal matching (`actual == expected`) preventing generic nominal conflation.
  - Contextual specialization fills unresolved internal constructor arguments without relabeling concrete types or repairing invalid external inputs.
  - Strict typed query decoding (`world query --typed`) through declared field schemas via `Value::from_typed_bytes_with_schemas`. Missing schemas or decode errors fail closed in executable worlds.
  - Explicit `QueryCursor::Versioned` and `Legacy` representation with strict scope enforcement (world ID, revision digest, relation, ordering kind). Versioned cursors never fall back to legacy parsing.
  - Structural annotation schema resolution in lowering and canonical plan encoding.

- **R10: Incremental Grouped Collection (`collect`)**:
  - Incremental aggregation operator `GroupProjection::Collect` with deterministic primary sorting and canonical child `WorldKey` (`child_key.to_hex()`) tie-breaking across both the incremental network and reference evaluators.
  - Full multiplicity preservation for distinct child records producing identical projected values.
  - Parent-anchored aggregation model (`union nonempty_slots, empty_slots` with `where not exists` and annotated `fn no_slots(): List<Str> = []`): retraction of the final child retains the parent settlement with an empty list `[]`.
  - Bounded cardinality limits: overflow aborts the transaction with an explicit error, leaving HEAD and settlements unchanged.
  - Verified against naive reference evaluator differential fuzzing.

- **R11: Portable Settled Decision Evidence (`brix.world.decision-evidence@1`)**:
  - Canonical artifact binding world manifest digest, program digest, revision sequence, revision digest, decide name, entity key, candidate, value digest, and caller/grants context (`DECISION_CLAIM_BUNDLE_PREFIX` sealed inside replay bundle bytes).
  - Mandatory independent audit bundle replay (`build_genesis_bundle_up_to_revision` ending at the requested revision) and decision trie Merkle proof verification before issuing `Authority::RuntimeCheck` receipts; `include_bundle` defaults to `true` across CLI, RPC, and Python SDK.
  - End-to-end support across CLI (`brix world evidence` / `verify-evidence`), RPC (`brix serve --stdio`), and Python SDK (`BrixClient.get_decision_evidence` / `verify_decision_evidence`).

- **R12: Protected Finite Authority Types**:
  - Protected `DocumentedStep` and `RuntimeOutcome` types lowerable exclusively via `validate<T>(...)`.
  - Documentary authority validated against `DocumentaryRegistry` (`doc://` registered URIs); runtime authority validated against structured decision outcome records rather than substring matching.
  - Direct literal construction and wire input injection fail closed with compile-time and runtime diagnostics.
  - Honest three-valued disposition: `ValidationOutcome::Satisfied`, `Violated`, `Unknown`.
  - Verified preservation and rechecking on reload and replay.

- **Packaged World & Atlas Qualification (Checkpoint 9)**:
  - `scripts/smoke_world_atlas.py` exercises both the baseline alpha.6 world workflow (`examples/world-atlas/atlas.brix`) and the alpha.8 surface/binding/collect/evidence/freshness workflow in isolated CLI processes.
  - Downstream Atlas repository integration (`semantics/compile.py`, full Atlas domain/surface schema migration, and `model/runtime-lock.json` binary hash pinning) is tracked in the downstream Atlas repository upon consuming the `v0.1.0-alpha.8` release artifact.

## Storage and Compatibility

- Workspace package version bumped to `0.1.0-alpha.8`.
- `ExecProfileV1` supports world sessions and audit bundles created under `0.1.0-alpha.5`, `0.1.0-alpha.6`, `0.1.0-alpha.7`, and `0.1.0-alpha.8`.
- Backward-compatible legacy cursor decoding maintained exclusively for unpinned live queries.
