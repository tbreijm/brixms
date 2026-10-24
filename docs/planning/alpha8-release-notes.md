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
  - Incremental aggregation operator `GroupProjection::Collect` with deterministic primary sorting and child `WorldKey` tie-breaking.
  - Full multiplicity preservation for distinct child records producing identical projected values.
  - Parent-anchored aggregation contract: retraction of the final child cleanly settles the parent to an empty list `[]`.
  - Bounded cardinality limits: overflow aborts the transaction with an explicit error, leaving HEAD and settlements unchanged.
  - Verified against naive reference evaluator differential fuzzing.

- **R11: Portable Settled Decision Evidence (`brix.world.decision-evidence@1`)**:
  - Canonical artifact binding world manifest digest, program digest, revision sequence, revision digest, decide name, entity key, candidate, value digest, and caller/grants context.
  - Cryptographic verification via decision trie Merkle proof paths and independent audit bundle replay.
  - End-to-end support across CLI (`brix world evidence` / `verify-evidence`), RPC (`brix serve --stdio`), and Python SDK (`BrixClient`).

- **R12: Protected Finite Authority Types**:
  - Protected `DocumentedStep` and `RuntimeOutcome` types lowerable exclusively via `validate<T>(...)`.
  - Direct literal construction and wire input injection fail closed with compile-time and runtime diagnostics.
  - Honest three-valued disposition: `ValidationOutcome::Satisfied`, `Violated`, `Unknown`.
  - Verified preservation and rechecking on reload and replay.

## Storage and Compatibility

- Workspace package version bumped to `0.1.0-alpha.8`.
- `ExecProfileV1` supports world sessions and audit bundles created under `0.1.0-alpha.5`, `0.1.0-alpha.6`, `0.1.0-alpha.7`, and `0.1.0-alpha.8`.
- Backward-compatible legacy cursor decoding maintained exclusively for unpinned live queries.
