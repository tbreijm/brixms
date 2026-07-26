# ADR-0003 — Proof Kernel Profile: Calculus Subset, Independent TCB, and Elaboration Gate

Status: **Proposed** (refines [ADR-0002](./ADR-0002_SOC_Constitution.md) §3 D1, §5.2, §8, §10; governs `brix-kernel` and `crates/brix-kernel`).

Date: 2026-07-26.

Foundation documents: [ADR-0002: SOC Constitution](./ADR-0002_SOC_Constitution.md) (§3, §4, §5, §6, §10), [ADR-0001: Proof Substrate](./ADR-0001_Proof_Substrate.md). This ADR defines the architecture, Trusted Computing Base (TCB) boundaries, total acceptance contract, verdict vocabulary, first calculus profile, and verification gate for `brix-kernel` — the dependent proof kernel of BrixMS.

---

## 1. Thesis & Dependency Isolation (TCB Boundary)

Per [ADR-0002](./ADR-0002_SOC_Constitution.md) §3 (Decision D1), BrixMS relies on exactly **two trusted kernels**: the settlement kernel (`soc-core` / `brix-oracle` reference) and the dependent proof kernel (`brix-kernel`). The proof kernel's sole purpose is to answer: *Does this canonical explicit proof term prove this proposition in this exact context?*

To maintain absolute trust, `brix-kernel` MUST maintain total structural independence from the rest of the engine:

- **Strict Dependency Tree:** `brix-kernel` depends **ONLY** on `brix-semantic` (and transitively `brix-canon`).
- **No Reverse Linkage:** `brix-kernel` never imports, links, or loads `brix.type`, `brix.proof`, `soc-core` (the settlement runtime), `brixc` (the compiler), or any language frontend.

```text
               +-----------------------+
               |      brix-kernel      |  <-- Dependent Proof Kernel (TCB)
               +-----------+-----------+
                           |
                           v
               +-----------------------+
               |     brix-semantic     |  <-- Canonical Artifacts & Outcomes
               +-----------+-----------+
                           |
                           v
               +-----------------------+
               |      brix-canon       |  <-- Binary Encoding & Cryptographic Hashes
               +-----------------------+
```

### 1.1 Trusted Computing Base (TCB) Exclusions

Every feature included in a proof kernel increases the attack surface for unsoundness, non-termination, and logical corruption. The TCB of `brix-kernel` explicitly **EXCLUDES** the following subsystems:

| Excluded Subsystem | Why Exclusion Is Mandatory for Trust |
|---|---|
| **Parsing & String Lexing** | Text grammars and AST parsers are prone to ambiguous syntax rules, stack overflows, and parser divergence. Terms entering the kernel are already canonical binary artifacts (`brix-canon`). |
| **Proof Search & Tactics** | Heuristics, unification loops, and tactic resolution are complex, non-deterministic, and prone to infinite loops. Search lives entirely off-kernel in realization regimes (`brix.proof`). |
| **Metavariables & Implicit Holes** | Uninstantiated unification placeholders can leak unconstrained wildcards into verified judgements, corrupting logical soundness. Kernel terms must be fully explicit and fully instantiated. |
| **General Recursion** | Unbounded fixpoints and arbitrary recursive functions break total evaluation and introduce logical inconsistency (Curry's paradox). Evaluation within the kernel must be strictly terminating. |
| **Provenance Ranking & Heuristics** | Epistemic path sorting and scoring are non-logical metadata operations. The kernel evaluates strict structural proof rules, uninfluenced by path preference or cost ranking. |
| **Term Optimization & Rewriting** | Compiler optimization passes can introduce semantic drift or code generation bugs. The kernel verifies explicit canonical terms directly without rewriting. |
| **Settlement Runtime (`soc-core`)** | Isolates logical proof checking from operational state machine bugs, scheduler behavior, and calendar tie-breaking in the execution engine. |
| **Compiler & Type Checker (`brix.type`)** | Isolates proof verification from frontend surface syntax, type inference bugs, and elaboration errors. `brix.type` is a realization regime that proposes candidate terms off-kernel. |

---

## 2. Acceptance API and Independence Gate

The proof kernel exposes a single, total entry point for term verification.

```rust
pub fn acceptance(
    context: &Context,
    proposition: &PropositionId,
    term: &ExplicitTerm,
) -> Verdict
```

### 2.1 API Characteristics

1. **Total Evaluation:** `acceptance` is a total function. It is guaranteed to terminate in finite time and within bounded memory for any input, returning a single, exhaustive [`Verdict`](#3-verdict-vocabulary-and-epistemic-mapping).
2. **Explicit Canonical Terms:** `term` is a content-addressed, canonical, fully elaborated explicit proof term artifact (`ExplicitTerm`). It contains no implicit parameters, no unresolved holes, and no metavariables.
3. **The Independence Gate:** The acceptance API MUST be fully compilable, testable, and callable in a build environment containing **only** `brix-kernel`, `brix-semantic`, and `brix-canon`. If calling `acceptance` requires linking `brix.type`, `brix.proof`, `soc-core`, or `brixc`, the independence gate is violated and the build MUST fail.

---

## 3. Verdict Vocabulary and Epistemic Mapping

The acceptance API returns one of exactly **six exhaustive verdict variants**. Each verdict maps precisely to an epistemic [`Outcome`](file:///Users/tonyreijm/Projects/brixms-v3/crates/brix-semantic/src/outcome.rs#L43-L67) and enforces strict publisher authority per [ADR-0002](./ADR-0002_SOC_Constitution.md) §4.1.

```rust
pub enum Verdict {
    Accepted(Certificate),
    Rejected(RejectionReason),
    Malformed(String),
    Unsupported(UnsupportedConstruct),
    ContextMismatch { claimed: ContextId, term_context: ContextId },
    ResourceExhausted(ResourceBudgetReason),
}
```

### 3.1 Verdict Mapping Table

| Verdict Variant | Description | Epistemic Outcome Mapping | Authority Permitted |
|---|---|---|---|
| **`Accepted(Cert)`** | Term is well-formed, in-profile, matches `Context` and `PropositionId`, and validly proves the proposition. | `Outcome::Proven` (or `Outcome::Refuted` if refutation term) | `Authority::ProofKernel` |
| **`Rejected(Reason)`** | Term is well-formed and in-profile for `Context`, but its logical steps fail to establish `PropositionId`. | *None* (Candidate rejected; no judgement published) | N/A |
| **`Malformed(Err)`** | Term artifact is corrupt, unparseable, or violates canonical structural tree invariants. | *None* (Invalid payload; no judgement published) | N/A |
| **`Unsupported(C)`** | Term contains a logical construct or rule outside this profile's declared calculus subset. | *None* (Out of profile; no judgement published) | N/A |
| **`ContextMismatch`** | Term's embedded assumption context digest does not match the target `ContextId`. | *None* (Scope mismatch; no judgement published) | N/A |
| **`ResourceExhausted`** | Kernel hit memory, evaluation depth, or step budget limits during verification. | `Outcome::Unknown` (Bottom of epistemic lattice) | `Authority::AnyResolver` / `Authority::ProofKernel` |

> [!CRITICAL]
> **Resource Exhaustion Is Never a Logical Rejection:**
> `ResourceExhausted` indicates budget depletion, NOT logical falsity. It MUST map strictly to `Outcome::Unknown` (the bottom of the epistemic lattice per [ADR-0002](./ADR-0002_SOC_Constitution.md) §4). It MUST NEVER be collapsed to `Rejected`, `Refuted`, or `false`.

---

## 4. Authority and Revision Invariance

Per [ADR-0002](./ADR-0002_SOC_Constitution.md) §4.1 ([`Authority::ProofKernel`](file:///Users/tonyreijm/Projects/brixms-v3/crates/brix-semantic/src/outcome.rs#L76-L81)):

1. **Sole Publisher:** `brix-kernel` acceptance is the **SOLE** publisher of `Outcome::Proven` and `Outcome::Refuted` judgements across the entire system.
2. **Regime Bounds:** Realization regimes (`brix.type`, `brix.proof`, external provers) may search, optimize, and construct candidate proof terms, but they are strictly forbidden from asserting acceptance or publishing `Proven`/`Refuted` outcomes directly.
3. **Revision Invariance:** Judgements published via `Accepted` certificates are **revision-invariant**. Because they are closed over explicit contexts (`ContextId`) containing full program snapshots and assumption sets, their logical validity survives settlement retractions and program revision updates.

---

## 5. Declared Calculus Subset (Profile 1)

This first profile defines the initial, core calculus supported by `brix-kernel`. Any proof term invoking constructs beyond this specification MUST return `Verdict::Unsupported`.

### 5.1 Admitted Logic & Judgement Rules

The profile supports intuitionistic propositional logic extended with finite products, finite sums, existential witnesses, equality with substitution, and transformation preservation.

#### 1. Hypothesis & Contexts ($\text{Hyp}$)
$$\frac{x : P \in \Gamma}{\Gamma \vdash x : P} \quad (\text{Hyp})$$

#### 2. Implication ($\rightarrow$)
$$\frac{\Gamma, x : P \vdash t : Q}{\Gamma \vdash \lambda x. t : P \rightarrow Q} \quad (\rightarrow I) \qquad \frac{\Gamma \vdash f : P \rightarrow Q \quad \Gamma \vdash a : P}{\Gamma \vdash f \, a : Q} \quad (\rightarrow E)$$

#### 3. Composition / Cut ($\text{Comp}$)
$$\frac{\Gamma \vdash a : P \quad \Gamma, x : P \vdash b : Q}{\Gamma \vdash \mathsf{cut}(a, x. b) : Q} \quad (\text{Comp})$$

#### 4. Finite Products ($\times$)
$$\frac{\Gamma \vdash a : P \quad \Gamma \vdash b : Q}{\Gamma \vdash \langle a, b \rangle : P \times Q} \quad (\times I)$$

$$\frac{\Gamma \vdash p : P \times Q}{\Gamma \vdash \pi_1(p) : P} \quad (\times E_1) \qquad \frac{\Gamma \vdash p : P \times Q}{\Gamma \vdash \pi_2(p) : Q} \quad (\times E_2)$$

#### 5. Finite Sums ($+$)
$$\frac{\Gamma \vdash a : P}{\Gamma \vdash \mathsf{inl}(a) : P + Q} \quad (+ I_1) \qquad \frac{\Gamma \vdash b : Q}{\Gamma \vdash \mathsf{inr}(b) : P + Q} \quad (+ I_2)$$

$$\frac{\Gamma \vdash s : P + Q \quad \Gamma, x : P \vdash u : R \quad \Gamma, y : Q \vdash v : R}{\Gamma \vdash \mathsf{case}(s, x. u, y. v) : R} \quad (+ E)$$

#### 6. Existential Witnesses ($\exists$)
$$\frac{\Gamma \vdash w : A \quad \Gamma \vdash p : P(w)}{\Gamma \vdash \mathsf{pack}(w, p) : \exists x. P(x)} \quad (\exists I) \qquad \frac{\Gamma \vdash e : \exists x. P(x) \quad \Gamma, x:A, y:P(x) \vdash t : R}{\Gamma \vdash \mathsf{unpack}(e, x.y. t) : R} \quad (\exists E)$$

#### 7. Equality & Substitution ($=$)
$$\frac{a \equiv b}{\Gamma \vdash \mathsf{refl}(a) : a = b} \quad (= I) \qquad \frac{\Gamma \vdash e : a = b \quad \Gamma \vdash p : P(a)}{\Gamma \vdash \mathsf{subst}(e, p) : P(b)} \quad (= E)$$

#### 8. Transformation Preservation ($\text{Trans-Pres}$)
$$\frac{\Gamma \vdash w : \mathsf{Realizes}(w, x, y) \quad \Gamma \vdash \pi : \mathsf{Preserves}(w, P) \quad \Gamma \vdash p : P(x)}{\Gamma \vdash \mathsf{pres}(w, \pi, p) : P(y)} \quad (\text{Trans-Pres})$$

> [!CRITICAL]
> **Preservation is dimension-scoped, never universal.** A witness realizes a
> settlement *possibility* $x \realizes{w} y$; it does **not** make $x$ and $y$
> logically interchangeable. Transport of $P$ across $w$ is admitted **only**
> with an explicit $\mathsf{Preserves}(w, P)$ premise — evidence that $P$ lies in
> $w$'s declared preservation profile (SOC lax semantics; ADR-0002 §7,
> "equivalent always names *which* dimensions are preserved"). Dropping $\pi$ and
> transporting an arbitrary $P$ would be **unsound** — it collapses the lax
> functor to an isomorphism and must never be admitted.

### 5.2 Side conditions (freshness)

The eliminators binding fresh variables — $(+ E)$, $(\exists E)$, and $(\text{Comp})$ — carry the standard **eigenvariable condition**: the bound variables ($x, y$ in $\mathsf{case}$; $x, y$ in $\mathsf{unpack}$; $x$ in $\mathsf{cut}$) MUST be fresh for $\Gamma$ and MUST NOT occur free in the conclusion type $R$. A term violating a freshness side condition is `Verdict::Malformed` (a structural well-formedness failure), not `Rejected`.

---

## 6. Verifier Identity and Kernel-Agnostic Certificate Contract

An `Accepted` verdict yields a [`Certificate`](file:///Users/tonyreijm/Projects/brixms-v3/crates/brix-semantic/src/evidence.rs#L48) containing a [`VerifierId`](file:///Users/tonyreijm/Projects/brixms-v3/crates/brix-semantic/src/evidence.rs#L30) and a [`CertificateId`](file:///Users/tonyreijm/Projects/brixms-v3/crates/brix-semantic/src/evidence.rs#L48).

### 6.1 Kernel-Agnostic Interface Design

The acceptance contract is explicitly designed to be kernel-agnostic so that future external proof kernels (e.g., Lean 4, Coq) can be integrated behind the same interface shape across elaboration boundaries without altering the downstream evidence structure.

```rust
pub struct Certificate {
    pub verifier: VerifierId,
    pub certificate_id: CertificateId,
}
```

- **Native Proof Kernel:** Produces `Certificate` with `verifier = VerifierId::named("brix.kernel@0.1")`.
- **Future External Kernel (e.g., Lean):** Produces `Certificate` with `verifier = VerifierId::named("lean@4")`.

No Lean adapter is built in this phase; the interface is strictly adapter-ready.

---

## 7. First Theorem Target: Decomposition-Validity

The first operational job of `brix-kernel` is certifying **Decomposition-Validity**, fulfilling [ADR-0002](./ADR-0002_SOC_Constitution.md) §5 and §10 (PD-1).

### 7.1 The `Derived → Audited → Proven` Upgrade Path

The epistemic lattice grades the transition from runtime commitment to verified theorem through a three-stage upgrade pipeline:

```text
   +--------------------+
   |  Engine Hot Loop   |  --> Commits step, records compact support
   +---------+----------+      Outcome: Derived  (Authority: SettlementKernel)
             |
             v
   +--------------------+
   |   Audit Checker    |  --> Replays factorization k = g_n ∘ ... ∘ g_1
   +---------+----------+      Outcome: Audited  (Authority: AuditChecker)
             |
             v  [ElaborationBoundary Edge]
   +--------------------+
   |    brix-kernel     |  --> Elaborates & certifies decomposition term
   +--------------------+      Outcome: Proven   (Authority: ProofKernel)
```

1. **`Derived` (Runtime Commit):** The `soc-core` hot loop commits a state step and records an unverified `Decomposition` artifact (`DecompVerification::Recorded`). Outcome is `Outcome::Derived`.
2. **`Audited` (Replay Verification):** The reference replayer (`brix-oracle` role) replays the generator sequence $k = g_n \circ \dots \circ g_1$ ($g_i \in \mathcal{G}$), verifying exact relational composition. It updates the artifact to `DecompVerification::ReplayVerified` and publishes `Outcome::Audited`.
3. **`Proven` (Kernel Elaboration):** `brix-kernel` elaborates a decomposition-validity proof term certifying that $\rho_k = \rho_{g_n} \circ \dots \circ \rho_{g_1}$ holds extensionally in the proof calculus. The proof crosses an [`ElaborationBoundary`](file:///Users/tonyreijm/Projects/brixms-v3/crates/brix-semantic/src/dependency.rs#L39) dependency edge (`EdgeKind::ElaborationBoundary`), upgrading the judgement to `Outcome::Proven`.

### 7.2 Subsequent Targets (Future Profiles)

Later proof debt items from [ADR-0002](./ADR-0002_SOC_Constitution.md) §10 are explicitly deferred to future profile specifications:
- **Governor Monotonicity (PD-1 / §5.5):** Certifying that tightening admissibility $\mathsf{Adm}$ pointwise shrinks candidates without fabricating false outcomes.
- **Counter-Machine Trace Correspondence (PD-2):** Certifying 2-counter machine halting and control policy encodings for Turing completeness proofs.

---

## 8. Verification Gate (Definition of Done)

The implementation of `crates/brix-kernel` for Profile 1 is complete when all of the following conditions are met:

### 8.1 Adversarial Certificate Test Suite

The kernel test harness MUST pass an adversarial suite verifying all six verdict paths:

1. **False Proof Terms:** Well-formed terms asserting invalid logical deductions MUST return `Verdict::Rejected`.
2. **Corrupt Payloads:** Structurally damaged, incomplete, or undecodable canon bytes MUST return `Verdict::Malformed`.
3. **Unadmitted Rules:** Terms invoking general recursion, implicit holes, tactics, or unadmitted rules MUST return `Verdict::Unsupported`.
4. **Context Mismatches:** Terms referencing context digests unequal to the target `ContextId` MUST return `Verdict::ContextMismatch`.
5. **Budget Depletion:** Terms exceeding step, depth, or evaluation limits MUST return `Verdict::ResourceExhausted`, mapping to `Outcome::Unknown` (NEVER `Rejected` or `Refuted`).

### 8.2 Standalone Build Isolation Gate

`crates/brix-kernel` MUST build, pass all unit/integration tests, and run acceptance checks in complete isolation:

```bash
cargo check -p brix-kernel --no-default-features
```

The build graph for `brix-kernel` MUST NOT contain `brix.type`, `brix.proof`, `soc-core`, `brixc`, or any compiler dependency.

---

## 9. Non-goals

- **Not** implementing proof search, unification, or automated tactic resolution in `brix-kernel`.
- **Not** building external theorem prover adapters (e.g., Lean 4, Coq) in this slice (interface readiness only).
- **Not** admitting dependent type inference, general recursion, or inductive family definitions in Profile 1.
- **Not** re-deciding any frozen constitution decisions from [ADR-0001](./ADR-0001_Proof_Substrate.md) or [ADR-0002](./ADR-0002_SOC_Constitution.md).

---

### One-line summary

`brix-kernel` defines a total, TCB-isolated acceptance API over a minimal intuitionistic calculus profile (Profile 1), returning six exhaustive verdicts, acting as sole publisher of revision-invariant `Proven`/`Refuted` outcomes, and enabling the `Derived → Audited → Proven` upgrade path for decomposition validity across elaboration boundaries without external engine dependencies.
