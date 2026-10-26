# Alpha.8: Concrete Generic Schemas, Grouped Collection, Decision Evidence, and Protected Authority

Baseline: `v0.1.0-alpha.7` (`6822203`).

## Scope

Alpha.8 delivers the BrixMS runtime and transport changes for the Atlas workflow, together with the arithmetic proofing and crash fixes. Downstream Atlas compiler migration and runtime pin updates remain separate consumption work:

- **Arithmetic proofing and crash repair**:
  - Kernel-closed arithmetic typing leaves and iterative `RealizesComp` spine traversal avoid the deep proof stack overflow while retaining independent certificate checks.

- **Small expression forms (R9 and R14)**:
  - `only` extracts a singleton and reports a typed fault for zero or multiple elements.
  - Final wildcard match arms and structurally typed empty-list ascriptions reduce exhaustive boilerplate.

- **Review Hardening & Concrete Generic Identity (R8 & R13)**:
  - Exact module-qualified nominal matching (`actual == expected`) preventing generic nominal conflation.
  - Contextual specialization fills unresolved internal constructor arguments without relabeling concrete types or repairing invalid external inputs.
  - Strict typed query decoding (`world query --typed`) through declared field schemas via `Value::from_typed_bytes_with_schemas`. Missing schemas or decode errors fail closed in executable worlds.
  - Explicit `QueryCursor::Versioned` and `Legacy` representation with strict scope enforcement (world ID, revision digest, relation, ordering kind). Versioned cursors never fall back to legacy parsing.
  - Structural annotation schema resolution in lowering and canonical plan encoding.

- **R10: Incremental Grouped Collection (`collect`) & Provenance Separation**:
  - Incremental aggregation operator `GroupProjection::Collect` with deterministic primary sorting and canonical provenance-leaf tie-breaking (`derivation_provenance_key` / `Provenance::ordering_key`) across both the incremental network and reference evaluators.
  - Explicit `EvaluatedRow { values: Row, provenance: Provenance }` separation in the reference evaluator so `Distinct` compares only relational values while `EquiJoin`, `GroupedCount`, `DisjointUnion`, and `Collect` preserve composite and derived provenance identities.
  - Full multiplicity preservation for distinct child records producing identical projected values.
  - Parent-anchored aggregation model (`union nonempty_slots, empty_slots` with `where not exists` and annotated `fn no_slots(): List<Str> = []`): retraction of the final child retains the parent settlement with an empty list `[]`.
  - Bounded cardinality limits: overflow aborts the transaction with an explicit error, leaving HEAD and settlements unchanged.
  - Verified against naive reference evaluator differential fuzzing.

- **R11: Portable Settled Decision Evidence (`brix.world.decision-evidence@1`)**:
  - Canonical artifact binding world manifest digest, program digest, revision sequence, revision digest, decide name, entity key, candidate, value digest, and program-bound claim context (`CertifiedAnswer { path, caller, release_id, grants }`).
  - Mandatory independent audit bundle replay (`build_genesis_bundle_up_to_revision` ending at the requested revision) under explicit decode and replay work budgets (`DecisionEvidenceDecodeLimits`) and decision trie Merkle proof verification before issuing verifier-restricted `VerifiedDecisionEvidence` (`Authority::RuntimeCheck`); `include_bundle` defaults to `true` across CLI, RPC, and Python SDK.
  - Separation of execution-profile `ExecProfileV1.crate_version` from the modeled release catalogue ID (`CertifiedAnswer.release_id`), and strict settlement-only fallback when no `CertifiedAnswer` is declared.
  - End-to-end support across CLI (`brix world evidence` / `verify-evidence`), RPC (`brix serve --stdio`), and Python SDK (`BrixClient.get_decision_evidence` / `verify_decision_evidence`), requiring consumers to specify expected candidate and value digest claims explicitly.

- **R12: Protected Finite Authority Types**:
  - Protected `DocumentedStep` and `RuntimeOutcome` types lowerable exclusively via `validate<T>(...)`.
  - Empty default `DocumentaryRegistry`; documentary authority requires a digest-verified `DocumentarySnapshot` checking evidence ID, content digest, evidence kind, authority scope, and disposition; runtime authority requires admitting a verifier-issued `VerifiedDecisionEvidence` and matching the exact qualified decision, entity, world, revision, candidate, result digest, caller, release, and grants.
  - Evidence environment digest bound into `FiniteDecisionRuntime::context` (`ContextId`) and threaded through execution, `explain_candidate`, helper calls, and `evaluate_shows_exprs`.
  - Direct literal construction and wire input injection fail closed with compile-time and runtime diagnostics.
  - Honest three-valued disposition: `ValidationOutcome::Satisfied`, `Violated`, `Unknown`.
  - Opaque verified handles are defined beside the complete world verifier with private fields. Rust type names, public witness constants, and caller-built Merkle proofs cannot mint a handle.
  - The lower-level evaluator exposes an explicitly trusted host adapter; custom adapter implementations are part of the host trust boundary, not portable evidence verification.
  - CLI execution and audit verification reload and independently replay runtime certificates from bytes, pinned through `BRIX_EXPECT_RUNTIME_CLAIMS`; documentary inputs require the separately configured `BRIX_EXPECT_EVIDENCE_DIGEST`.

- **Packaged World & Atlas Qualification (Checkpoint 9)**:
  - `scripts/smoke_world_atlas.py` exercises both the baseline alpha.6 world workflow (`examples/world-atlas/atlas.brix`) and the alpha.8 surface/binding/collect/evidence/freshness workflow in isolated CLI processes.
  - Downstream Atlas repository integration (`semantics/compile.py`, full Atlas domain/surface schema migration, and `model/runtime-lock.json` binary hash pinning) is tracked in the downstream Atlas repository upon consuming the `v0.1.0-alpha.8` release artifact.

## Storage and Compatibility

- Workspace package version bumped to `0.1.0-alpha.8`.
- `ExecProfileV1` supports world sessions and audit bundles created under `0.1.0-alpha.5`, `0.1.0-alpha.6`, `0.1.0-alpha.7`, and `0.1.0-alpha.8`.
- Backward-compatible legacy cursor decoding maintained exclusively for unpinned live queries.
