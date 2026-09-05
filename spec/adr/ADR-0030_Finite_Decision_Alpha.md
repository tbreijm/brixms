# ADR-0030 — Finite-Decision Alpha: An Executable Deliberation Profile

Status: **Accepted** (2026-09-05). Supersedes [ADR-0029](./ADR-0029_L3_Witness_Frontier_Profile.md)
(`brix.l3.witness-frontier@1`). Governs the finite-decision alpha release slice (`0.1.0-alpha.2`)
and defines the executable profile `brix.l3.finite-decision@1`.

Date: 2026-09-05.

Foundation documents: [ADR-0002: SOC Constitution](./ADR-0002_SOC_Constitution.md) (§1 dynamics,
§4.1 epistemic lattice, §5.3 fail closed, §7 realization regimes, §8 the behavior signature,
§8.1 commitment/deliberation split, §9 calendar and interning, §9.1 $O(|\Delta|)$ invariant),
[ADR-0010: SOC Language Design](./ADR-0010_SOC_Language_Design.md) (§7a ⟨D-OPARROW⟩),
[ADR-0012: L3 Executable Settlement](./ADR-0012_L3_Executable_Settlement.md) (⟨D-STATUS⟩, ⟨D-PROFILE⟩),
[ADR-0013: Canonical Certificate Envelope](./ADR-0013_Canonical_Certificate_Envelope.md),
[ADR-0014: Divergence-Sensitive Saturation](./ADR-0014_Divergence_Sensitive_Saturation.md),
[ADR-0015: Judgment-Scoped Tightness](./ADR-0015_Judgment_Scoped_Tightness.md),
[ADR-0016: Authority Publication Fence](./ADR-0016_Authority_Publication_Fence.md),
[ADR-0027: L3 v2 Derivation](./ADR-0027_L3_V2_Derivation.md) (Stages A–C),
[ADR-0028: Witness Provider Ontology](./ADR-0028_Witness_Provider_Ontology.md).

---

## 1. Context and Problem Statement

ADR-0012 (`brix.l3.rule-agenda-saturated@1`) defined an executable settlement profile over closed,
zero-argument constants. As established in ADR-0027 §1, v1 commits each rule once in a serial agenda
and quiesces; it cannot express derivation or decision.

ADR-0027 introduced derivation (Stages A–C: expressions, evaluator, acyclic dependencies), allowing
rules to derive facts from earlier rules. ADR-0029 proposed presenting rule witnesses concurrently
in a witness frontier (`brix.l3.witness-frontier@1`), but lacked explicit propose-versus-commit
syntax, complete deliberation frontier semantics, structured rejection reasons, fail-closed handling
for evaluation faults and key conflicts, certified quiescence upon total candidate rejection, and
canonical program identity bindings over guards and priorities.

The **finite-decision alpha** profile (`brix.l3.finite-decision@1`) specifies an operational,
deterministic decision engine over a finite pool of candidate proposals. It realizes the core
deliberation/commitment split (ADR-0002 §8.1) in the executable language: rules derive semantic
facts; proposals define candidate state transitions with guards, values, and priorities; a complete
frontier is evaluated against admission policy; and a deterministic keyed calendar selects and
commits exactly one winning step into the journal.

---

## 2. Decision Points

### ⟨D-PROFILE⟩ Profile Identifier: `brix.l3.finite-decision@1`

The profile marker for this execution profile is:
```text
brix.l3.finite-decision@1
```

Per ADR-0012 §10 and ADR-0027 §2, this profile does not widen `brix.l3.rule-agenda-saturated@1` or
modify SOC core semantics. It is a distinct, additive execution profile with its own plan representation,
runtime adapter, and canonical program identity.

### ⟨D-GRAMMAR⟩ Propose-Plus-Commit Grammar

A finite-decision program extends the module grammar with explicit candidate proposal declarations
and a terminating commit block. Declarations are newline-delimited; the surface grammar contains
no trailing semicolons:

1. **Definitions and Rules:** Closed type configurations (`config`), top-level bindings (`let`), and
   derived rules (`rule`) compute the prerequisite fact environment.
2. **Proposals (`propose`):** Each proposal declares a candidate transition with explicit dependencies,
   priority, guard condition, and successor expression (newline-delimited, no trailing semicolon):
   ```brix
   propose <name>(<deps>...) priority <p> when <guard> = <value>
   ```
   - `<name>`: Unique identifier for the proposal within the module.
   - `(<deps>...)`: Comma-separated list of rule or fact dependencies required by this candidate.
   - `priority <p>`: Non-negative integer priority level (`u64`). Lower numerical values represent
     higher priority / greater urgency (`0` is highest priority).
   - `when <guard>`: Boolean guard predicate evaluated against the current world and derived facts.
     A guard evaluating to `false` causes immediate candidate rejection under structured reason code.
   - `= <value>`: Closed expression evaluating to the candidate successor configuration.
3. **Commit Block (`commit`):** Exactly one nonempty `commit` declaration is defined per decision slice
   (newline-delimited, no trailing semicolon):
   ```brix
   commit <name> from (<candidate_1>, <candidate_2>, ...)
   ```
   The commit declaration defines the active candidate pool eligible for frontier deliberation. Programs
   with zero commit declarations, multiple commit declarations, or an empty candidate list in `commit`
   are refused before execution with a deterministic syntax/lowering error.

### ⟨D-ONECOMMIT⟩ Exactly One Nonempty Commit Step

A finite-decision deliberation cycle evaluates the proposals admitted to the frontier and commits
**at most one** step. Settlement does not loop infinitely or construct unbounded serial agendas; it
resolves the candidate frontier into a single operational commitment or halts in certified quiescence.

### ⟨D-COMPLETEFRONTIER⟩ Complete Deliberation Frontier After Rules

Deliberation evaluates the **complete** candidate pool specified by the commit block after all
prerequisite rules have been derived:
1. Every candidate proposal $c \in \text{Candidates}$ is evaluated against the current execution
   configuration $\text{ExecConfig}(x, p, h)$.
2. Evaluation does not short-circuit upon encountering the first admissible candidate. The full
   frontier is partitioned into:
   - **Admitted Set:** All candidates whose guards evaluate to `true` and which pass the active
     admission policy (`AdmissionPolicy`).
   - **Rejected Set:** All candidates whose guards evaluate to `false` or which are rejected by the
     admission policy, each accompanied by its structured reason code (`ReasonCode`).
3. The complete evaluated frontier is retained in `EvaluatedFrontier` for audit and explanation
   (`explain_why`, `explain_why_not`).

### ⟨D-PHASEZERO⟩ Calendar Ordering at Phase Zero

Candidate calendar keys are evaluated strictly at **phase zero**:
```text
phase = 0
```
In this alpha profile, scheduling does not advance across simulated time steps or discrete phases.
All candidate proposals compete within the single initial deliberation phase $\tau = 0$.

### ⟨D-KEYORDER⟩ Lower Priority Value Then Canonical Digest Tie-Break

Deliberation key ordering obeys the standard SOC calendar order (ADR-0002 §8.1, §9.2):
$$\text{Key} = (\text{phase}, \text{priority}, \text{tiebreak})$$

1. **Phase:** Fixed to `0`.
2. **Priority:** Sorted by numerical value in ascending order: **lower numeric values are more urgent**
   (priority `0` dominates priority `1`).
3. **Tie-Break:** When phase and priority are identical, ties are broken deterministically by the
   **canonical digest** of the candidate's canonical lowered triple:
   $$\text{CanonicalCandidateV1} = (\text{regime\_id}, \text{witness\_digest}, \text{successor\_digest})$$
   The digest is resolved through boundary interning (`Interner::resolve`), tagged with
   `CANONICAL_TIEBREAK_TAG` (`"brix.regimes.NamedCandidate.tiebreak@1"`), and hashed under `Domain::Value`.
   Raw integer handles and surface syntactic order are never used for tie-breaking.

### ⟨D-DISPOSITION⟩ Tripartite Candidate Disposition

Deliberation classifies every proposal in the commit pool into one of three mutually exclusive
dispositions:

1. **`Selected`:** The candidate was admitted and possesses the unique minimal `Key` in the evaluated
   frontier. It is committed into the settlement journal.
2. **`AdmittedNotSelected` (Overshadowed):** The candidate was admitted (guard evaluated to `true`, policy
   admitted), but was not selected because another admitted candidate had a strictly smaller `Key`
   (higher priority or winning canonical tiebreak).
3. **`Rejected`:** The candidate's guard evaluated to `false` or the candidate was rejected by the
   admission policy (`AdmissionDecision::Rejected(ReasonCode)`).

Structured explanation APIs (`explain_why` and `explain_why_not`) re-derive these dispositions fresh
from current inputs on every call, never consulting cached or pre-baked strings.

### ⟨D-FAILCLOSED⟩ Evaluation Faults and Key Conflicts Yield Unknown

The profile fails closed under the $B^{uk}$ discipline (ADR-0002 §5.3, §8):

1. **Evaluation Faults:** If evaluating any rule, guard, or candidate value results in an evaluation
   fault (such as arithmetic overflow, division by zero, type mismatch, or missing field), execution
   halts immediately. The settlement outcome is **`Unknown`**, no candidate is selected, and no
   decision is committed to the journal.
2. **Key Conflicts:** If two distinct candidates in the deliberation frontier produce identical
   calendar `Key`s ($\text{Key}_a = \text{Key}_b$ with $c_a \neq c_b$), the frontier records a
   `KeyConflict`. Selection fails closed: the outcome is **`Unknown`**, no candidate is committed,
   and the key conflict diagnostics are preserved for inspection.

### ⟨D-QUIESCENCE⟩ All-Rejected Candidates Yield Certified Quiescence

When every candidate proposal in the commit pool is rejected (i.e. all guards evaluate to `false` or
all candidates are rejected by policy):
1. The admitted candidate set is empty ($\text{Admitted} = \emptyset$).
2. Selection yields `None`.
3. The settlement run halts with certified **quiescence** (`SaturatedStop::Quiescent`).
4. Quiescence is verified via `soc_core::check_quiescence_certificate`, producing a valid
   `QuiescenceCertificateId`. The run certifies that no candidate was admissible at the initial world
   under the declared policy.

### ⟨D-PROGID⟩ Program Identity Binds Full Semantic Specification

The canonical program identity (`ProgramId`) uniquely and deterministically binds the normalized
representation of:
- All configuration definitions (`config`) and top-level bindings (`let`);
- All derived rules (`rule`), their dependency signatures, and their bodies;
- All candidate proposals (`propose`), their normalized names, guards, value expressions, and priority values;
- The exact commit block membership and normalized candidate order;
- Any declared output show/projection directives;
- The profile marker `brix.l3.finite-decision@1`.

Changes in whitespace, comments, or non-semantic syntactic formatting do not alter `ProgramId`. Any
change to rules, guards, candidate expressions, priorities, or commit membership produces a distinct
`ProgramId`.

### ⟨D-EVIDENCE⟩ Strict Runtime Derived and Audit Audited Separation

Evidence grades adhere strictly to the SOC authority publication fence (ADR-0002 §4.1, ADR-0016):
- **Runtime Settlement:** The settlement hot loop executes via `soc_core::try_commit_tick` and publishes
  the committed step at grade **`Derived`**. The runtime does not have the authority to publish `Audited`
  or `Proven`. The runtime decision remains at grade **`Derived`**.
- **Replay Audit:** Auditing is an explicit, separate verification pass (`soc_core::audit_journal`). It
  takes the emitted journal, re-derives generator decompositions via `SettlementWitnessProvider`, and
  verifies them against the registered generator semantics. Successful independent replay issues and
  verifies separate **`Audited`** audit receipts.
- An `Unknown` audit verdict leaves the settlement outcome unchanged and never upgrades evidence.

### ⟨D-PRESERVE⟩ Preservation of L3 v1 Artifacts

All existing L3 v1 data types, artifacts, and test vectors remain completely untouched:
- `L3PlanV1`, `ProgramIdV1`, `FactV1`, `L3WorldV1`, and `SettlementRunV1` are preserved without mutation.
- The v1 profile marker `brix.l3.rule-agenda-saturated@1` continues to govern the serial rule agenda.
- `vectors/l3_plan_v1.json` is not modified.
- All v1 acceptance tests continue to pass byte-identically under the v1 profile.

---

## 3. Architecture Overview

```text
       .brix Source
            |
            v
   [brix-syntax]
      Parser (strict bounds)
            |
            v
    AST with Propose & Commit
            |
            v
   [brix-lower]
      L3 v2 Rules + Proposals Lowering
            |
            +------------------------------------+
            |                                    |
            v                                    v
     Rules Evaluator                    Candidate Pool
  (Derives Fact World)             {NamedCandidate_1..n}
            |                                    |
            +-----------------+------------------+
                              |
                              v
                      [soc-regimes]
                 EvaluatedFrontier::evaluate
                 (Guards & AdmissionPolicy)
                              |
                 +------------+------------+
                 |                         |
                 v                         v
           Admitted Set              Rejected Set
         (Keys: p=0, prio,       (ReasonCode per cand)
          canonical tiebreak)              |
                 |                         v
                 v                 (All rejected?)
           select_least                    |
                  |                   YES -> Certified Quiescence
         (Conflict? Fault?)                 |
           YES -> Unknown                   +------------------+
           NO  -> Selected                                     |
                  |                                            |
                   v                                            v
             [soc-core]                                    Diagnostics
           try_commit_tick                               (Why / WhyNot)
                   |                                            |
                   v                                            |
          Journal Step (@Derived)                               |
                   |                                            |
                   v                                            v
             [soc-core]                                  Exit 0 / Exit 1
            audit_journal
                   |
                   v
          Audit Receipt (@Audited)
```

---

## 4. Implementation Mapping

The core capabilities of this ADR are anchored in the workspace as follows:

| Specification Requirement | Implementation Anchor |
|---|---|
| Profile marker `brix.l3.finite-decision@1` | `crates/soc-regimes/src/finite_frontier.rs` (`FINITE_FRONTIER_REGIME_NAME`) |
| Surface grammar & parser | `propose`, `commit`, `show` in `crates/brix-syntax` (`ast.rs`, `parser.rs`) |
| Named candidates & interning | `NamedCandidate` in `crates/soc-regimes/src/finite_frontier.rs` |
| Canonical identity & tie-break | `CanonicalCandidateV1`, `canonical_tiebreak`, `CANONICAL_TIEBREAK_TAG` |
| Calendar key: phase 0, priority, tiebreak | `NamedCandidate::canonical_key` |
| Complete deliberation frontier | `EvaluatedFrontier::evaluate` |
| Structured rejection reasons | `ReasonCode`, `AdmissionDecision::Rejected` |
| Rejection on guard false | `AdmissionPolicy` evaluation over candidate pool |
| Fail-closed on key conflict | `EvaluatedFrontier::key_conflicts`, `candidate_key_conflicts` |
| Fresh why / why-not re-derivation | `explain_why`, `explain_why_not` |
| Incremental $O(\|\Delta\|)$ view & footprint | `FiniteCandidateRegime`, `Footprint` in `soc-core` |
| Plan lowering & canonical program identity | `FiniteDecisionPlan`, `FiniteDecisionProgramId` in `crates/brix-lower/src/finite_decision/plan.rs` |
| Execution runtime & commit journaling | `FiniteDecisionRuntime` in `crates/brix-lower/src/finite_decision/runtime.rs` |
| Audit input transport & verification | ADR-0026 `SettlementAuditInputBundleV1` (`soc-core`), `check_l3_audit_input_bundle_from_source_v1` (`brix-lower`) |
| CLI subcommands | `check`, `run`, `audit`, `verify`, `why`, `whynot` in `crates/brix-cli` |
| Runtime settlement & audit separation | `soc_core::try_commit_tick` (`Derived` decision) vs `soc_core::audit_journal` (separate `Audited` receipts) |
| L3 v1 preservation | Untouched `crates/brix-lower/src/l3.rs`, `l3_canon.rs`, `vectors/l3_plan_v1.json` |

---

## 5. Status & Compatibility

- **Status:** Accepted (2026-09-05).
- **Supersedes:** ADR-0029 (`brix.l3.witness-frontier@1`).
- **Compatibility:** Additive. Implemented for the `0.1.0-alpha.2` release slice. Implements the complete propose-plus-commit workflow across `brix-syntax` (grammar and parser), `soc-regimes` (`finite_frontier`), `brix-lower` (decision lowering, `FiniteDecisionPlan`, `FiniteDecisionProgramId`, and `FiniteDecisionRuntime`), `soc-core` (commit loop and audit bundles), and `brix-cli` (live subcommands `check`, `run`, `audit`, `verify`, `why`, `whynot`). Canonical program identity is anchored by `FiniteDecisionProgramId` without dependency on or claiming un-landed v2 execution profile identity types.
