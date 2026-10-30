# The SOC Resolution Contract

Status: **Proposed** (2026-10-27). The shared resolution contract over existing candidate discovery, admissibility, selection, commitment, evidence, and invalidation machinery (issue [#52](https://github.com/tbreijm/brixms/issues/52)).

Governing decisions: [ADR-0001](./adr/ADR-0001_Proof_Substrate.md) (§4 outcome lattice, §4.1 authority, §5 artifact identities, §6 resolvers vs kernels, §7 retraction), [ADR-0002](./adr/ADR-0002_SOC_Constitution.md) (§1 category of configurations and witnesses, §4 verifier-authority table, §5 epistemic lattice, §8 committed coalgebra into $D_O = 1 + O \times X$, §8.1 calendar key $K$, §9.1 $O(\Delta)$ invariant, §9.2 dataflow providers), [ADR-0014](./adr/ADR-0014_Divergence_Sensitive_Saturation.md) (divergence-sensitive saturation, quiescence and divergence certificates), [ADR-0016](./adr/ADR-0016_Authority_Publication_Fence.md) (authority publication fence and `ROUTES`).

Implementation: `crates/soc-core`, `crates/brix-semantic`, `crates/soc-core/tests/resolution_contract.rs`.

---

## 0. How to read this document

Every normative clause carries an **evidence status**. The status is part of the clause, not commentary:

| Marker | Meaning |
|---|---|
| **[Pinned: `test`]** | Implemented, and a named test fails if the clause is violated. |
| **[Partial: …]** | Implemented, but the pin is weaker than the clause — the exact gap is stated. |
| **[Specified]** | Normative, **no implementation exists**. Binding on the implementation when it lands. |

This brief is not a normative ruling. Follow `CONTRIBUTING.md`: unresolved normative choices need a drafted erratum and Tony's ruling before dependent implementation. Existing behavior can be mapped and tested while decisions are pending. Mark proposed, implemented, and independently pinned clauses separately; creating this document does not make its requirements implemented.

---

## 1. Concept-to-code map

This contract reconciles the conceptual model of SOC resolution with existing live types and interfaces across `soc-core` and `brix-semantic`.

| Concept | Existing starting point | Live identity and authority | Concrete gap / reconciliation |
|---|---|---|---|
| **Scoped goal / question** | `ContextId`, `PropositionId`, `ExecConfig` (`x, p, h`) | Opaque content-addressed `ContextId` / `PropositionId`; caller authority | No unified generic resolution request envelope; `ExecConfig` is process-local settlement state, while `saturate::PresentationV1` is saturation-specific. |
| **Candidate** | `witness_provider::Candidate` | Process-local `Handle`s (`witness`, `successor`) | Handles are process-local interner offsets (`Handle.raw()`), not portable canonical identities (`WitnessId`, `ConfigId`). Canonical candidate identity requires projection via `Interner`. |
| **Discovery** | `WitnessProvider`, `IncrementalWitnessIndex`, `oracle::cand` | Stateless trait (`WitnessProvider`) or dataflow operator (`IncrementalWitnessIndex`); no authority to commit or publish | Historical "Regime" terminology in issue #52 conflated discovery providers with semantic regimes. Provider = enumeration operator; `RegimeId` = semantic interpretation $\rho_w$. |
| **Admissibility** | `Adm`, `AdmAll`, `AdmNone`, `AdmWitnessAllowlist`, `AndAdm` | Pure predicate over `(ExecConfig, Candidate)` | Admissibility is a hard filter defining candidate validity. Soft ranking and tie-breaking must be decoupled from admissibility. |
| **Ranking / selection** | `calendar::Key`, `Frontier`, deterministic `select_K` | `Key = (phase, priority, tiebreak)`; unique-key frontier $B^{uk}_{K,O}$ | Mechanical least-key selection guarantees forward progress, but choosing a winner does *not* establish logical uniqueness. Ambiguity must be evaluated before mechanical tie-breaking. |
| **Commitment** | `try_commit_selected`, `commit_tick`, `Committed`, `Journal` | `Outcome::Derived`; authority: `Authority::SettlementKernel` | Hot loop records (unverified) `Decomposition` and commits $O_{min}$ observation; hot loop never publishes `Proven` or `Audited`. |
| **Support / invalidation** | `Dependency` (`EdgeKind`), `Delta`, `CandidateDelta`, `Footprint` | Typed dependency edges (`brix-semantic`); content-addressed sets (`Delta`) | Dependency edges record provenance, and `Delta` drives view maintenance, but a unified multi-support retraction-closure engine is not yet centralized across all domains. |
| **Verification / evidence** | `AuditChecker` (`audit_step`), `ProofKernel`, `Judgement::publish` | Verification-gated `ROUTES`; authorities: `AuditChecker`, `ProofKernel` | The issue mentions `Estimated`, but `Outcome` has six variants and no `Estimated`: live code uses `Measured` (`Authority::ExternalDriver`). Discrepancy is recorded explicitly. |
| **Search history** | `DiscoveryRun` | Excluded by design from `Judgement` identity (proof irrelevance to provenance) | No concrete `DiscoveryRun` struct exists in live code; telemetry/search order is excluded from durable `JudgementId` by construction. |

---

## 2. Canonical inputs, scoped questions, and identity

**2.1** A resolution question SHALL bind:
1. Exact assumption context (`ContextId`),
2. Goal proposition (`PropositionId`),
3. Admissibility and governance policy (`PolicyId` / policy handle),
4. Supported semantic profile (`ProfileId` / `RegimeId`),
5. Target verifier version (`VerifierId`),
6. Explicit operational limits (step, time, or memory budgets). **[Partial: `ExecConfig` binds world/policy/history handles; `saturate::PresentationV1` binds context, profile, policy, and assumptions; `ResolutionQuery` formalizes the envelope in `soc-core::resolution`.]**

**2.2** The canonical identity of a resolution result is search-invariant: it depends strictly on the semantic inputs `(context, proposition, outcome, evidence)`, NEVER on provider registration order, discovery attempt order, heuristics, wall-clock timing, or search telemetry. **[Pinned: `scenario_10_order_and_telemetry_changes_preserve_canonical_result` in `crates/soc-core/tests/resolution_contract.rs`, and `JudgementId::of`]**

**2.3** Two candidates with identical canonical representations SHALL NOT produce distinct duplicate keys in the deliberation frontier. Attempting to bind conflicting values to the same calendar key SHALL be rejected fail-closed (ADR-0002 §8.1). **[Pinned: `buk_divergent_duplicate_key_is_rejected_idempotent_duplicate_is_accepted` in `crates/soc-core/tests/calendar_commit.rs`]**

**2.4** The generic resolution request envelope SHALL be canonically encodable via `brix-canon` under domain tag `"brix.soc.resolution.query"`, capturing `(context: ContextId, goal: PropositionId, policy: Digest, budget: Option<u64>)`. Decoders SHALL fail closed on length mismatch or unknown tags (SOC-LAW-01). **[Pinned: `scenario_12_resolution_query_and_scope_round_trip_canon`]**

**2.5** Candidates discovered via process-local interner handles (`Candidate { witness: Handle, successor: Handle }`) SHALL be projectable to canonical, portable identities (`CanonicalCandidate { witness: WitnessId, successor: ConfigId }`) via the active `Interner`. Canonical candidates SHALL encode and decode canonically (SOC-LAW-01). **[Pinned: `scenario_15_canonical_candidate_projection_and_round_trip`]**

---

## 3. Resolution outcomes and epistemic lattice

**3.1** Resolution outcomes classify the cardinality and completeness of candidate solutions to a scoped question. They are distinct from the epistemic outcome lattice (ADR-0001 §4, SOC-LAW-04):

| Resolution State | Meaning | Permitted Epistemic Status (`Outcome`) |
|---|---|---|
| `Unique(c)` | Exactly one candidate satisfies the goal and admissibility policy. | May support `Derived`, `Audited`, or `Proven` depending on verification. |
| `Ambiguous { selected, candidates }` | Multiple admissible candidates exist without an explicit logical disambiguation rule. | May commit `selected` as `Derived` for forward progress; logical status remains ambiguous. |
| `NoSolution` | No candidate satisfies the goal in the target domain, and the domain search was **certified complete**. | May support certified quiescence (`Derived`) or `Refuted` (if proof-kernel certified). |
| `Unknown(reason)` | Search was incomplete, interrupted, resource-exhausted, or hit an unsupported fragment/cycle. | MUST map to `Outcome::Unknown`. MUST NOT be reported as `NoSolution` or `Refuted`. |

**[Pinned: `scenario_1_two_admissible_candidates_distinguishes_winner_from_logical_ambiguity`, `scenario_3_empty_complete_domain_yields_nosolution`, and `scenario_4_bounded_incomplete_search_yields_unknown_never_nosolution_or_refuted`]**

**3.2** A deterministic commitment winner (via `select_K`) DOES NOT establish logical uniqueness. When multiple admissible candidates exist, the calendar's selection is an operational scheduling commitment, not a proof that alternative candidates are logically invalid. **[Pinned: `scenario_1_two_admissible_candidates_distinguishes_winner_from_logical_ambiguity`]**

**3.3** Empty candidate enumeration SHALL NOT be treated as evidence of `NoSolution` or `Refuted` unless the search domain has an independently certified completeness condition (`CompletenessScope::Complete` / `EnumerationCompleteness`) (SOC-LAW-04). **[Pinned: `scenario_3_empty_complete_domain_yields_nosolution` and `scenario_4_bounded_incomplete_search_yields_unknown_never_nosolution_or_refuted`]**

**3.4** Detection of a search cycle or repeated administrative state (a lasso) establishes non-termination or divergence of that path. It SHALL NOT be treated as a logical contradiction, inconsistency, or refutation of the goal proposition (ADR-0014 Stage B). **[Pinned: `scenario_5_cycle_detection_yields_cycle_unknown_never_refuted`]**

**3.5 Frozen v1 Saturation Certificate Envelope:**
The v1 saturation certificate (`QuiescenceCertificateV1`) envelope (`vectors/soc_quiescence_v1.json`, ⟨D-QCERT⟩) encodes `EnumerationCompleteness::Complete` (ordinal 0) and is **frozen ABI**. It promises complete enumeration and MUST NOT be retrofitted with a partial, budgeted, or heuristic completeness claim. Any future bounded discovery certificate or partial quiescence claim requires an explicit compatibility/version decision and a new envelope version (v2) (ADR-0014 §6.2, `crates/soc-core/OWNER.md`). **[Pinned: `decode_rejects_an_incomplete_enumeration_ordinal` in `tests/saturation_certificates.rs`]**

**3.6 Logical Uniqueness Requires Complete Enumeration:**
A resolver SHALL NOT classify a candidate as `ResolutionOutcome::Unique` unless the candidate enumeration was certified complete (`CompletenessScope::Complete`). Under bounded or incomplete enumeration, finding exactly one candidate does not eliminate the possibility of alternative candidates in the unsearched space; such an outcome SHALL be classified as `ResolutionOutcome::Unknown(ResolutionUnknown::IncompleteEnumeration)`. **[Pinned: `scenario_13_incomplete_enumeration_cannot_claim_uniqueness`]**

---

## 4. Policies and selection hierarchy

**4.1** The resolution pipeline strictly separates concerns in the following execution order:
1. **Admissibility Filter (`Adm`):** Hard gating deciding candidate validity ($c \in \text{cand}(e)$).
2. **Preference / Ranking:** Ordering of valid candidates by semantic priority (`Key.priority`).
3. **Tie-Resolution Policy:** Disambiguation among candidates with equal priority.
4. **Mechanical Selection (`select_K`):** Deterministic choice using the unique digest tie-break (`Key.tiebreak`).
5. **Verifier Acceptance (`ROUTES`):** Post-resolution validation and upgrade by the designated authority.

**[Pinned: `scenario_1_two_admissible_candidates_distinguishes_winner_from_logical_ambiguity` and `governance_conservation`]**

**4.2** Ambiguity evaluation SHALL take place BEFORE arbitrary or mechanical tie-breaking. If semantic ranking leaves more than one candidate at the least priority level, the query result is logically `Ambiguous`, even if the calendar proceeds to commit a deterministic winner. **[Pinned: `scenario_1_two_admissible_candidates_distinguishes_winner_from_logical_ambiguity`]**

**4.3** Tightening an admissibility policy ($\text{Adm}' \Rightarrow \text{Adm}$) monotonically shrinks the admissible candidate set ($\text{cand}'(e) \subseteq \text{cand}(e)$). A prohibited candidate cannot survive as the selected winner. **[Pinned: `scenario_2_tightened_admissibility_shrinks_candidates_and_displaces_winner` and `tests/governance_conservation.rs`]**

**4.4** A resolution result is strictly bound to its policy identity. A changed policy has a distinct bound identity and SHALL NOT reuse previously resolved conclusions as if the question were unchanged. **[Pinned: `scenario_9_binding_change_prevents_old_result_reuse`]**

**4.5 Selected Candidate Validity Invariant:**
A candidate SHALL NOT be presented as the `selected` winner in `ResolutionOutcome::Ambiguous` unless it is a verified member of the admissible candidate set. Inadmissible candidates supplied as selected winners SHALL be filtered out to `None` (SOC-LAW-07). **[Pinned: `scenario_14_prohibited_selected_cannot_survive`]**

---

## 5. Authority and checked publication

**5.1** Every published judgement SHALL be constructed exclusively through `Judgement::publish` and verified against the immutable `ROUTES` table. Direct construction outside `brix-semantic` is structurally denied. **[Pinned: `crates/brix-semantic/tests/publication.rs`]**

**5.2** Publisher authority is strictly partitioned:
- `Authority::ProofKernel` is the SOLE publisher of `Outcome::Proven` and `Outcome::Refuted`.
- `Authority::SettlementKernel` is the SOLE publisher of `Outcome::Derived`.
- `Authority::AuditChecker` is the SOLE publisher of `Outcome::Audited`.
- `Authority::ExternalDriver` is the SOLE publisher of `Outcome::Measured`.
- `Authority::AnyResolver` may emit `Outcome::Unknown(reason)`, but MAY NEVER downgrade a stronger outcome or publish `Proven`/`Refuted`.

A discovery provider, resolver, or optimizer SHALL NOT mint `Proven`, `Refuted`, or `Audited`. **[Pinned: `crates/brix-semantic/tests/publication.rs`]**

**5.3 Epistemic Enum Reconciliation (Issue #52 vs Live ABI):**
Historical text in issue #52 referenced an `Estimated` outcome. In live frozen ABI (`crates/brix-semantic/src/outcome.rs`), `Outcome` has exactly six variants:
1. `Proven` (ordinal 0, `ProofKernel`)
2. `Refuted` (ordinal 1, `ProofKernel`)
3. `Derived` (ordinal 2, `SettlementKernel`)
4. `Measured` (ordinal 3, `ExternalDriver`)
5. `Unknown` (ordinal 4, `AnyResolver`)
6. `Audited` (ordinal 5, `AuditChecker`)

External simulations, empirical estimates, and certified measurements are published exclusively as `Outcome::Measured` under `Authority::ExternalDriver`. No `Estimated` variant exists, and none may be added without an approved ADR and canonical version bump. **[Pinned: `outcome_ordinals_are_frozen` in `crates/brix-semantic`]**

**5.4** Audit verification is independent log replay (`audit::audit_step`), never self-asserted confidence by the discovery engine. The hot loop records an unverified `Decomposition`; only replay verification upgrades `Derived` to `Audited`. **[Pinned: `tests/audit_factorization.rs`]**

---

## 6. Support, revision, and invalidation

**6.1** Invalidation is exact: retracting or removing an input configuration, assumption, or dependency premise invalidates any live conclusion that relies upon it. **[Partial: Live candidate retraction is demonstrated via empty provider substitution and journal immutability in `scenario_8_last_support_removal_invalidates_live_state_while_retaining_history`; workspace-wide automatic invalidation-closure traversal is an unlanded engine stage.]**

**6.2 Immutable Historical Evidence vs Live State:**
Retracting a support invalidates the *live applicability* of a conclusion. However, historical records in the append-only `Journal` (including prior `CommittedStep` records) are immutable and remain permanently valid witnesses of the state transitions that occurred. **[Pinned: `scenario_8_last_support_removal_invalidates_live_state_while_retaining_history`]**

**6.3 Multi-Support Preservation:**
When a proposition is justified by multiple independent derivations ($D_1$ and $D_2$), retracting the premises of $D_1$ SHALL NOT invalidate the live proposition if $D_2$ remains fully justified. **[Partial: Multi-derivation premise tracking is demonstrated via manual set removal over explicit `Dependency` edges in `scenario_7_alternate_support_preserves_justified_conclusion`; automated multi-support dependency-graph retraction is an outstanding engine capability (Build_Plan_v3_SOC.md).]**

**6.4 Binding Invalidation:**
Any modification to:
- Program / world configuration,
- Assumption context (`ContextId`),
- Governance policy (`PolicyId`),
- Verifier identity / version (`VerifierId`),
invalidates the reuse of a prior resolution result under the new binding. **[Pinned: `scenario_9_binding_change_prevents_old_result_reuse`]**

---

## 7. Discovery, telemetry, and explanation

**7.1** All search strategies, candidate attempt sequences, exploration order, pruning heuristics, and wall-clock timings belong to discovery telemetry (`DiscoveryRun`) and SHALL remain strictly outside durable `Judgement` identity. Proof irrelevance applies to provenance: different search paths reaching the same conclusion on identical evidence produce the byte-identical `JudgementId`. **[Pinned: `scenario_10_order_and_telemetry_changes_preserve_canonical_result`]**

**7.2** The resolution contract requires explainability for:
1. Why a candidate was proposed (provider candidate generation),
2. Why a candidate was rejected (admissibility failure or policy restriction),
3. Why a candidate was selected (least-key priority and tie-break),
4. Why verification failed (replay or proof checking rejection),
5. Why a live conclusion was invalidated (dependency retraction).

**7.3 Bounded Explanation Minimality:**
Any explanation produced by a resolver claims minimality ONLY within the bounded search trace or heuristic subset explored. Global minimality is NOT claimed unless accompanied by a decision procedure and formal proof of exhaustive search. **[Specified]**

---

## 8. Determinism and cost boundaries

**8.1** Canonical observable behavior and selection order SHALL NOT depend on:
- Provider registration order in `providers: &[&dyn WitnessProvider]`,
- Hash table iteration order (`clippy.toml` denies `HashMap`/`HashSet` in Ring 0; `BTreeMap`/`BTreeSet` is mandated),
- Wall-clock time or thread scheduling.

**[Pinned: `scenario_10_order_and_telemetry_changes_preserve_canonical_result`]**

**8.2 Preservation of the $O(\Delta)$ Commitment Boundary:**
Per-step commitment cost in the incremental engine SHALL scale with the size of the delta and active provider fanout ($O(|\Delta| \times \text{fanout})$), never with the size of the inert world. Verification replay and complete domain search SHALL NOT be silently moved into the hot commitment path. **[Pinned: `tests/o_delta_gate.rs`]**

---

## 9. Parallel interface with context management (#59)

**9.1 Boundary and Ownership:**
- Issue #59 owns context payload contents, schema, canonical bytes, identity correspondence, validation routines, and cross-boundary transport.
- Issue #52 (this contract) consumes `ContextId` as an opaque content-addressed identifier and defines the consumer validation contract.

**9.2 Consumer Contract for Context Validation:**
Before resolving a question scoped to `ContextId`, the resolver SHALL validate context integrity through the validation seam:
1. Validate that the provided context payload hashes to the expected `ContextId`.
2. Check that the assumption tree and profile are compatible with the resolver.
3. Classify context failures into the taxonomy:
   - `Unavailable`: Context payload cannot be resolved from storage.
   - `Malformed`: Context bytes fail canonical decoding.
   - `Mismatched`: Provided context digest does not match expected identity.
   - `Unsupported`: Context features or version outside resolver capability.
   - `Exhausted`: Resource bounds in context have been exceeded.

**[Partial: Consumer contract trait `ContextValidator` and error taxonomy are pinned via `StubContextValidator` in `scenario_6_mismatched_context_fails_validation_and_blocks_publication`; publication fence gating on context validation is owned normatively by #59.]**

**9.3 Prohibition of Ambient Cross-Context Reuse:**
A judgement or derivation valid under `ContextId` $C_1$ SHALL NOT be reused under `ContextId` $C_2$ without explicit context subsumption or transport evidence authorized by #59. **[Partial: Context validation rejection is demonstrated via `StubContextValidator` in scenario 6 and presentation binding checks in scenario 9; end-to-end transport gating across contexts is parked under #59.]**

**9.4 Test Isolation:**
Any context validator used in #52 conformance tests is explicitly a test double (`StubContextValidator`), not a production validation engine. **[Pinned: `crates/soc-core/tests/resolution_contract.rs`]**

---

## 10. Worked reference instances

### 10.1 Native Typing Regime (#53)
- **Goal:** Resolve type realization for expression-context pair `(e, \Gamma)`.
- **Candidate Discovery:** Syntax-directed elaboration generators matching expression constructors.
- **Admissibility:** Structural typing rules in `soc-regimes::type_realization`.
- **Outcome:**
  - `Unique(Ty)`: Expression has a unique canonical principal type.
  - `Ambiguous`: Overloaded operator with multiple valid types and no disambiguating annotation.
  - `NoSolution`: Ill-typed expression (type error in closed fragment).
- **Evidence:** `Support::Tree(TreeDerivation)` upgraded to `Outcome::Audited` under `Authority::AuditChecker` with `TreeVerification::StructureVerified`.
- **Status:** **[Partial: Tree derivation and audit gates exist; incremental invalidation is outstanding under #53.]**

### 10.2 Proof-Candidate Checking (#55)
- **Goal:** Resolve proof certificate for proposition $P$ in context $\Gamma$.
- **Candidate Discovery:** Proof search resolvers generating candidate proof terms.
- **Selection:** Ranking by proof size, proof cost, or tactic preference.
- **Outcome:**
  - `Unique(Term)` / `Ambiguous`: Candidate terms presented to proof kernel.
  - `Unknown`: Proof search budget exhausted.
- **Evidence:** Candidate term crossed over `EdgeKind::ElaborationBoundary` into `Authority::ProofKernel`. Published as `Outcome::Proven` with `Support::KernelCertificate`.
- **Status:** **[Specified: Proof candidate generation and elaboration boundary defined; full kernel integration parked under #55.]**

### 10.3 Compatibility and Migration Obligations (#54)
- **Goal:** Resolve behavioral preservation under schema/version migration $\Sigma_1 \to \Sigma_2$.
- **Candidate Discovery:** Migration relation mapping rules and adapter generators.
- **Admissibility:** Preservation of canonical observables and invariants.
- **Outcome:**
  - `Unique(Adapter)`: Compatible migration path discovered.
  - `NoSolution`: Incompatible breaking change in closed fragment.
  - `Unknown`: Undecidable or unverified equivalence.
- **Evidence:** Bi-simulation witness or equivalence certificate.
- **Status:** **[Specified: Parked under #54; referenced here to establish contract uniformity.]**

---

## 11. Conformance matrix and residual implementation gaps

### Conformance Matrix

| Scenario | Tested Property | Test Pin | Status |
|---|---|---|---|
| **1. Two admissible candidates** | Least-key commitment picks winner; classified as `Ambiguous` | `scenario_1_two_admissible_candidates_distinguishes_winner_from_logical_ambiguity` | **[Pinned]** |
| **2. Tightened admissibility** | Candidate set shrinks; excluded winner displaced | `scenario_2_tightened_admissibility_shrinks_candidates_and_displaces_winner` | **[Pinned]** |
| **3. Empty complete finite domain** | Empty candidates under complete scope yields `NoSolution` | `scenario_3_empty_complete_domain_yields_nosolution` | **[Pinned]** |
| **4. Interrupted / bounded search** | Budget exhaustion yields `Unknown`, never `NoSolution`/`Refuted` | `scenario_4_bounded_incomplete_search_yields_unknown_never_nosolution_or_refuted` | **[Pinned]** |
| **5. Cycle or repeated state** | Lasso yields declared cycle `Unknown`, never contradiction | `scenario_5_cycle_detection_yields_cycle_unknown_never_refuted` | **[Pinned]** |
| **6. Rejected / wrong-context** | Context mismatch detected via `StubContextValidator`; publication gating halts | `scenario_6_mismatched_context_fails_validation_and_blocks_publication` | **[Partial: Gated on StubContextValidator check; end-to-end publication fence integration owned by #59]** |
| **7. Multi-support removal** | Alternate derivations (`Judgement` + `Dependency`) preserve conclusion | `scenario_7_alternate_support_preserves_justified_conclusion` | **[Partial: Manual set removal on Dependency edges; centralized invalidation engine outstanding]** |
| **8. Last support removal** | Live state invalidated; immutable journal history retained & replayable | `scenario_8_last_support_removal_invalidates_live_state_while_retaining_history` | **[Partial: Manual empty provider substitution; centralized invalidation engine outstanding]** |
| **9. Binding change** | Policy/context change prohibits reusing old resolution | `scenario_9_binding_change_prevents_old_result_reuse` | **[Pinned]** |
| **10. Order / telemetry invariance** | Candidate set & winner invariant under provider order & telemetry | `scenario_10_order_and_telemetry_changes_preserve_canonical_result` | **[Pinned]** |
| **11. Controlled edit sequence** | Incremental additions/retractions agree with naive oracle | `scenario_11_controlled_edit_sequence_incremental_vs_naive_resolution` | **[Pinned]** |
| **12. Canonical serialization** | Canonical encoding & decoding round-trip fails closed | `scenario_12_resolution_query_and_scope_round_trip_canon` | **[Pinned]** |
| **13. Incomplete uniqueness check** | 1 candidate under incomplete enumeration yields `Unknown(IncompleteEnumeration)` | `scenario_13_incomplete_enumeration_cannot_claim_uniqueness` | **[Pinned]** |
| **14. Inadmissible winner filter** | Prohibited/inadmissible candidates passed as selected are sanitized to `None` | `scenario_14_prohibited_selected_cannot_survive` | **[Pinned]** |
| **15. Candidate handle projection** | Projection from process-local `Candidate` to `CanonicalCandidate` round-trips | `scenario_15_canonical_candidate_projection_and_round_trip` | **[Pinned]** |
| **16. Outcome canonical codecs** | Full serialization round-trip across all four resolution outcome states | `scenario_16_resolution_outcome_canonical_round_trip` | **[Pinned]** |
| **17. Negative decode & invalid rejection** | Cardinality, ordering, duplicate, selected membership, and limit violations rejected fail-closed | `scenario_17_negative_decode_coverage_rejects_noncanonical_and_invalid_outcomes` | **[Pinned]** |
| **18. Verified completeness evidence** | Current v1 quiescence certificates and caller-declared domains cannot certify goal-specific candidate completeness; empty/singleton results remain `Unknown` absent a supported proof | `scenario_18_verified_query_bound_completeness_evidence` | **[Pinned]** |
| **19. Candidate limit enforcement** | Classifier rejects > 1024 candidates fail-closed as `BudgetExhausted`; output round-trips cleanly | `scenario_19_classifier_enforces_candidate_limit_and_round_trips` | **[Pinned]** |

### Residual Implementation Gaps

The following capabilities are specified by this contract but remain intentionally unlanded or owned by adjacent issues:

1. **Production Context Validation Seam (Issue #59):**
   `ContextValidator` currently runs against a test double (`StubContextValidator`). Real context envelope parsing, transport across boundaries, and store validation will land under #59.
2. **Centralized Retraction-Closure Engine:**
   Multi-support preservation and invalidation are verified locally in `resolution_contract.rs` and through `Dependency` edges, but a workspace-wide incremental invalidator traversing all dependency graphs is a subsequent engine stage (`Build_Plan_v3_SOC.md`).
3. **General Completeness Checker:**
   No production proof currently connects a resolution goal to an exhaustive candidate set. Caller-declared finite domains and `QuiescenceCertificateV1` establish no such connection and are rejected by the verified-evidence constructors. `CompletenessScope::Complete` remains an operational classification input only; it carries no independent proof authority. A future proof must bind the query and exact candidate result set and establish that the supported search relation is exhaustive.
4. **Concrete `DiscoveryRun` Telemetry Schema:**
   `DiscoveryRun` is normatively excluded from `JudgementId`, but its concrete serializable schema for studio diagnostics is not yet implemented in Ring 0.
