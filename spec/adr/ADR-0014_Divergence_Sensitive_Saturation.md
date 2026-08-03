# ADR-0014 — Divergence-Sensitive Saturation, the Settlement Interface, and Weak Bisimulation (v1 Saturation Profile)

Status: **Proposed** (2026-08-02) (refines [ADR-0002](./ADR-0002_SOC_Constitution.md) §1.3, §5.3, §8, §10; bounded against [ADR-0012](./ADR-0012_L3_Executable_Settlement.md) §7; governs `crates/soc-core/src/saturate/` and its frozen vectors).

Date: 2026-08-02.

Foundation documents: [ADR-0002: SOC Constitution](./ADR-0002_SOC_Constitution.md) (§1.3 saturation, §5.3 divergence-sensitivity, §8 the ratified behavior signature ⟨D-FO⟩, §10 CJ-1), [ADR-0012: Brix L3 Executable Settlement](./ADR-0012_L3_Executable_Settlement.md) (§7, the boundary this ADR is the other side of), [ADR-0013: Canonical Certificate Envelope](./ADR-0013_Canonical_Certificate_Envelope.md) (the envelope discipline reused here), `spec/Build_Plan_v3_SOC.md` Step 8 (S5), `spec/SOC_Semantic_Laws.md` SOC-LAW-10.

This is the S5 semantic pin tracked by **#61**. It defines no proof rules, no language semantics, and no CLI surface.

---

## 1. Context

ADR-0002 §8.3 ratified ⟨D-FO⟩: the committed coalgebra targets `F_O(X) = D_O(X) = 1 + O × X` with `O = O_min`, and **quiescence is the `1` summand**. `soc-core` implements this: `commit::Committed` is the sum, `Observation` is `O_min`, `Frontier::select_least` is `select_K`.

What the engine lacks is any notion of an *administrative* step, so the `1` summand carries far less than its name suggests. `crates/soc-core/src/commit.rs:73-76` admits it:

> Quiescence (ADR-0002 §2 — divergence-sensitive saturation is a later slice; here quiescence is simply "the oracle-shared enumeration found nothing to commit").

Three operationally distinct situations are indistinguishable: provably nothing left to do; a finite administrative prefix before the next visible step; an administrative search that never terminates.

The sharpest symptom is `commit.rs:298-330`. `run(..., max_ticks) -> (Journal, Vec<CostRecord>)` breaks on `Committed::Quiescent` *and* falls out of its `for` loop at `max_ticks`. **Both exits return the same value**, so no caller can distinguish "settled" from "ran out of budget" — exactly the collapse ADR-0002 §5.3 forbids: *"A search that has not terminated has proved nothing."*

### 1.1 Three facts about the current substrate that shape this design

1. **`ExecConfig.history` is not canonical and is not a state.** `ExecConfig = { world: Handle, policy: Handle, history: Digest }`, and `oracle.rs`'s `CandidateStep::canon_write` folds `successor.raw()` and `witness.raw()` — raw interner indices — into the history chain. So `history` is allocation-order dependent *and* strictly grows on every applied candidate. **`ExecConfig` equality is therefore useless as a state identity:** every configuration along an administrative loop is distinct, which would make cycle detection vacuous and every state space infinite. This forces §4's projection.
2. **`Observation` is history-free.** `try_commit_selected` builds `Judgement::new(context, Realizes(witness, src, dst), Derived, …)` — no key, no phase, no history. So `τ*;o` and a direct `o` from the same world yield the **same `Observation`** but **different successor `ExecConfig`s**. This is why §11 reads #61's first acceptance criterion as observation equality plus coinductive successor equivalence, not `ExecConfig` equality.
3. **`PropositionId::of(&impl Canonical)` takes any canonical value**; there is no closed proposition enum. A quiescence proposition is a pure addition with no ordinal ABI risk.

## 2. Decision

This ADR pins: `τ`/realizing labels as a *profile projection* ⟨D-TAU⟩; a concrete v1 observation profile ⟨D-OBS⟩; the observable state projection and its declared assumptions ⟨D-PROJ⟩; a total settlement interface in which certified divergence is its own summand ⟨D-DIV⟩; quiescence and divergence certificates; a quiescence proposition shape ⟨D-QP⟩; and divergence-sensitive weak bisimulation with directional refinement ⟨D-REF⟩.

It does **not** re-open ⟨D-FO⟩. `F_O`, `O_min`, `select_K`, `K`, `Committed`, `Observation`, and the `Journal`/`CommittedStep` ABI are all unchanged.

## 3. Administrative versus realizing steps ⟨D-TAU⟩

### 3.1 Why τ cannot be a new committed summand

| Framing | Consequence | Verdict |
|---|---|---|
| τ as a new `Committed` variant | `F_O(X) = 1 + (O+1)×X` — a widening of the frozen signature | **Forbidden** by ADR-0002 §8.3 |
| τ as a committed step with its `Observation` suppressed | Same shape in disguise, and it breaks audit: `audit_step` requires `step.observation.judgement_digest == derived_id.digest()`, so a suppressed observation is unauditable | **Forbidden** |
| **τ as a declared projection over fully committed steps** | `F_O` and every existing type untouched | **Adopt** |

The load-bearing observation: **`F_O` is the functor of the *saturated* behavior, not of the immediate step.** ADR-0002 §1.3 says saturation *"distinguishes administrative steps from realizing steps"*; CJ-1 speaks of a world *"whose **saturated** realizing semantics is an `F_O`-coalgebra"*; SOC-LAW-10 grades today's fixtures as *"the **unsaturated immediate-step** prerequisite only."* Today's `commit_tick` is the underlying transition `γ`; its value is `D_O`-shaped only because the unsaturated system happens to have no τ. Saturation layers **above** `γ`, consuming a run of ticks and exporting one `D_O`-shaped value.

SOC-LAW-10's own domain clause settles *how* the label is assigned: *"Implementations of **one declared observation profile**, including their administrative (τ) and visible realizing steps."* τ-ness is **declared**, not intrinsic.

### 3.2 Normative statement

> Every step the calendar commits — administrative or realizing — is a full `Committed::Step`: it carries a real `Observation`, mints a `Derived` judgement through `try_commit_selected` alone, and is appended to the `Journal`. `Committed`, `Observation`, and `F_O = 1 + O×X` are unchanged.
>
> An **observation profile** is a total, canonically identified classifier assigning each committed step a `StepLabel`. `Administrative` means only that the declared observation boundary does **not export** that step's `Observation` across saturation. It does not mean the step was uncommitted, unjournaled, ungraded, or unauditable.

```rust
pub enum StepLabel { Administrative, Realizing }

/// Fail closed: never defaults to `Realizing` (fabricating an observation)
/// nor to `Administrative` (silently hiding one).
pub enum ProfileError { MixedDecomposition, UnregisteredGenerator, EmptyDecomposition }

pub trait ObservationProfile {
    fn id(&self) -> ObservationProfileId;
    /// Classifies from `CommittedStep` alone — durable canonical material
    /// (`Key`, `Observation`, `Decomposition`, `ConfigId`, `WitnessId`), never
    /// a raw `Handle` — so the label is replayable from the journal.
    fn label(&self, step: &CommittedStep) -> Result<StepLabel, ProfileError>;
}
```

Consequences worth stating: the **same journal under two profiles has two visible traces and one identical committed trace**; and a profile with `𝒢_τ = ∅` makes saturation degenerate exactly to today's behavior, so every existing regime keeps its current meaning with no migration.

## 4. The v1 observation profile ⟨D-OBS⟩ and the state projection ⟨D-PROJ⟩

### 4.1 v1 profile: the generator partition

"Observation profile" is a named-but-undefined placeholder gated behind #59. This ADR gives it **one** concrete v1 instance with a canonical identity, and nothing more:

```rust
/// v1: the generator partition 𝒢 = 𝒢_τ ⊎ 𝒢_o. A step is `Administrative` iff
/// every generator of its decomposition is in 𝒢_τ, `Realizing` iff every one
/// is in 𝒢_o, and a `ProfileError` otherwise.
pub struct GeneratorPartitionProfile { administrative: GeneratorRegistry, realizing: GeneratorRegistry }
```

Identity preimage (frozen, ADR-0013 discipline): marker `b"brix.soc.obs-profile"`; version `1`; kind `"brix.soc.obs-profile.generator-partition@1"`; `𝒢_τ` as a canonical set of `GeneratorId` digests; `𝒢_o` likewise. The two sets MUST be disjoint (fallible constructor).

> **Forward compatibility.** `ObservationProfileId` is an **opaque digest** everywhere it appears. #59 may define richer profiles under new `kind` strings; it MUST NOT reinterpret the v1 `generator-partition@1` preimage. **#61 pins the slot, not the taxonomy.**

### 4.2 The observable state projection

Per §1.1(1), `ExecConfig` cannot be a state identity.

```rust
/// The state identity saturation, cycle detection, and bisimulation use.
/// `history` is deliberately excluded: it folds `Handle::raw()` and grows on
/// every applied candidate, so including it makes every state distinct.
pub struct ObservableState { pub world: Handle, pub policy: Handle }
pub fn project(e: &ExecConfig) -> ObservableState { .. }
```

Excluding history is not free — it **asserts** something about the presentation, so the assertion is named and carried:

```rust
pub struct DeclaredAssumptions {
    /// P1 — `Regime::candidates` and `Adm::admits` depend on `e` only through `project(e)`.
    pub history_independent: bool,
    /// P6 — the keyer's `priority`/`tiebreak` do not depend on `phase`.
    pub phase_stable_keying: bool,
}
```

| | Assumption | Status | Buys |
|---|---|---|---|
| P1 | history independence | declared + bounded check | `ObservableState` is a state; repetition ⇒ recurrence |
| P2 | finite branching | structural (`candidates -> Vec<_>`) | finite frontier; `select_K` total |
| P4 | total effective enumeration | structural (no bounded regime API exists) | an empty frontier really is empty |
| P5 | total profile classification | checked at run time | every hidden step is labelled, none defaulted |
| P6 | phase-stable keying | declared + bounded check | γ well-defined on `ObservableState` |
| P3 | finite administrative reachability | **not assumed** — the budget stands in | termination without a budget |

### 4.3 Theorem (τ-decidability for a v1-conformant presentation)

> Let a presentation satisfy P1, P2, P4, P5, P6. Because `γ = select_K ∘ δ` is deterministic (⟨D-FO⟩ Candidate A) and P1+P6 make the choice a function of `ObservableState` alone, the administrative orbit of any state is a **lasso**: a stem of length `m ≥ 0` followed by either a terminal event (empty frontier, or a realizing step) or a cycle of length `ℓ ≥ 1`.
>
> **(a) Soundness of the divergence certificate needs only P1 + P6.** If `project(e_i) = project(e_{i+ℓ})` and every step in `[i, i+ℓ)` is `Administrative`, determinism re-selects the same candidate forever: `↑_τ`. This is a *finite* observation of an *infinite* behavior, and it fires even when the state space is infinite.
>
> **(b) Decidability of `sat_step` additionally needs P3.** Under P3, `sat_step` terminates within `|S_τ(e₀)| + 1` γ-ticks and returns exactly one of `Realizing`, `Quiescent`, `Divergent` — no `Unknown`.
>
> **(c) Without P3, `sat_step` under budget `B` returns one of those three within `B` γ-ticks, or `Unknown`.** It never returns `Quiescent` for a non-quiescent state, and never returns anything but `Unknown` for an exhausted search.

This is the ADR's precise answer to *"state the exact assumptions under which quiescence is decidable"*: **certification is cheap (P1+P6); decidability is the expensive one (P3), and we buy it with a budget rather than assuming it.**

## 5. The total settlement interface ⟨D-DIV⟩

ADR-0012 §7 forbids reinterpreting `PlanComplete`/`FrontierStalled`. Every name below is new; none collides, and `run_saturated` never emits a v1 status.

```rust
/// The total one-step settlement interface. Deliberately a **strictly larger**
/// vocabulary than `F_O`: `Divergent` and `Unknown` are not `F_O`-values. The
/// `F_O`-coalgebra is defined exactly on the sub-carrier where this returns an
/// `F_O`-value — that partiality is the honest content of the interface, and is
/// what CJ-1 will be stated against.
pub enum SaturatedStep {
    /// The `O × X` summand, after hiding a finite τ-prefix.
    Realizing { observation: Observation, successor: ExecConfig, hidden_steps: u64 },
    /// The `1` summand, **certified**.
    Quiescent(Box<QuiescenceCertificateV1>),
    /// `↑_τ`, **certified** by a closed lasso. Graded `Unknown` for the
    /// completion/quiescence question — never `Refuted`, and never the `1` summand.
    Divergent(Box<DivergenceCertificateV1>),
    /// Nothing established. Never a pass, never a certificate, never `Refuted`.
    Unknown(SaturationUnknown),
}

pub enum SaturationUnknown {
    AdministrativeBudgetExhausted { hidden_steps: u64, budget: SaturationBudget },
    AdministrativeStateBudgetExhausted { states: u64, budget: SaturationBudget },
    VisibleBudgetExhausted { visible_steps: u64, budget: SaturationBudget },
    ProfileError { at_step: u64, error: ProfileError },
    CommitFailed { at_step: u64, error: CommitError },
    UndeclaredAssumption(AssumptionId),
    AssumptionViolated { assumption: AssumptionId, at_step: u64 },
    KeyConflict { at_step: u64 },
}
```

### 5.1 Why divergence is a summand, not an `Unknown` reason

#61's goal list buckets *"administrative divergence, resource exhaustion, or unsupported analysis as `Unknown`"*. But the Step 8 gate demands *"a divergence-sensitivity conformance test — a terminal state and an infinitely-searching state are distinguished."* **If divergence were merely an `Unknown` reason it would be indistinguishable from exhaustion and that test could not exist.**

Resolution: certified divergence is its own **structural summand**, while remaining `Unknown`-*graded* for the completion question. Certified divergence is a positive finite observation of an infinite behavior; budget exhaustion establishes nothing. Collapsing them reintroduces exactly the confusion this ADR removes.

**Uniform grading rule (normative):** every `SaturationUnknown` variant, and `Divergent`, grade the completion/quiescence question as `Unknown`. Exactly one constructor in the crate yields a decided negative: `SaturatedStep::Quiescent`, and only via a verified certificate.

### 5.2 Budget and signature

`brix_kernel::Budget` is unavailable — `soc-core` depends only on `brix-canon` and `brix-semantic`, and adding that edge would violate `scripts/check_tcb_dependencies.py`'s `RULES` and ADR-0002 §3. A local type, with **no `Default` impl** (a caller must state its budget, following `CostRecord`'s honesty discipline):

```rust
pub struct SaturationBudget {
    pub max_hidden_steps: u64,            // τ-steps hidden inside ONE saturated step
    pub max_administrative_states: u64,   // states retained for lasso detection
    pub max_visible_steps: u64,           // saturated steps for a whole run
}

pub fn sat_step<F>(pres: &PresentationV1<'_>, e: &ExecConfig, phase: u64,
                   keyer: &mut F, budget: SaturationBudget)
    -> (SaturatedStep, Vec<CommittedStep>, CostRecord)
where F: FnMut(&Candidate, u64) -> Key;
```

`Vec<CommittedStep>` (vs `commit_tick`'s `Option<_>`) is the τ-prefix plus the realizing step, in order; **all** are journaled.

**Cost fold rule (normative):** the returned `CostRecord` is `Steps(Σ γ-tick work)` if every tick measured, else `UnknownCost` — never a partial sum, never zero.

**O(Δ) restatement (normative).** ADR-0002 §9.1's invariant is quantified over **committed γ-steps**, not saturated steps. A saturated step hiding `k` τ-steps legitimately costs `k+1` γ-steps' worth. Saturation neither weakens nor extends THE invariant; `tests/o_delta_gate.rs` is unaffected.

## 6. Certificates

### 6.1 What the quiescence certificate asserts

> In context `C`, under observation profile `P`, at presentation revision `R`, from world `x₀` under policy `p`, the recorded finite administrative prefix `σ = s₁…s_m` (every `s_i` labelled `Administrative` by `P`) reaches world `x_m`, at which the admissible frontier under the presentation's declared regime set and `Adm` is **empty under a complete enumeration**, hence `γ(e_m) = inl(*)`.
>
> It asserts nothing about any other context, profile, revision, policy, or regime set.

### 6.2 Frozen field order (ADR-0013 discipline)

marker `b"brix.soc.quiescence"`; version `1`; saturation profile `"brix.soc.saturation@1"`; observation-profile id; `ContextId`; presentation revision; policy digest; source world; terminal world; hidden-prefix length `m`; the `m` prefix step digests; the prefix chain digest; regime-set digest; `Adm` identity; **enumeration-completeness ordinal**; outcome grade; quiescence `JudgementId`.

Three fields deserve their rationale:

- **Prefix chain digest, not raw handles** — `ExecConfig.history` is non-canonical (§1.1); `Journal::replay_chain` is its canonical equivalent.
- **Enumeration completeness** is the load-bearing honesty field. "The frontier is empty" is a decided negative *only if enumeration was exhaustive*. That holds in v1 solely because `Regime::candidates -> Vec<Candidate>` is unbounded and total (P4). The reader accepts **only** the `Complete` ordinal in v1. **If a bounded or fallible regime API is ever added — ADR-0012 §4 already contemplates one as "a later compatible extension" — it MUST come with a v2 certificate and MUST NOT emit v1.**
- **Cost is excluded from identity**, by ADR-0013 §4's argument: identity is a property of the artifacts, not the effort spent. Two runs under different sufficient budgets identify the *same* certificate.

The divergence certificate uses marker `b"brix.soc.divergence"`, version 1, sharing fields 1–8 and then carrying stem length `m`, cycle length `ℓ ≥ 1`, the `m+ℓ` lasso step digests, the cycle-entry world and policy, an assumption-mode ordinal, and the outcome grade.

### 6.3 Checkers — total and fail-closed

```rust
pub fn decode_quiescence_v1(bytes: &[u8]) -> Result<QuiescenceMaterialV1, CertEnvelopeError>;
pub fn validate_quiescence_v1(m: &QuiescenceMaterialV1, expected_context: ContextId,
    expected_profile: ObservationProfileId, expected_presentation: PresentationIdV1)
    -> Result<QuiescenceCertificateId, CertEnvelopeError>;

/// Semantic: re-derive the claim from the presentation and journal.
pub fn check_quiescence_certificate(cert: &QuiescenceCertificateV1,
    pres: &PresentationV1<'_>, prefix: &[CommittedStep]) -> CertificateCheck;

pub enum CertificateCheck {
    /// Independently re-derived. `Derived`-grade in the certificate's exact
    /// context/profile/revision — never a theorem.
    Verified { certificate_id: QuiescenceCertificateId },
    /// Never a pass, never `Refuted`.
    Unknown(CertificateCheckError),
}
```

The semantic checker re-derives, never trusts: envelope decodes exactly with no trailing bytes; context/profile/presentation match; the prefix replays to the recorded chain digest; **every** prefix step is labelled `Administrative` (one realizing step invalidates the certificate); endpoints chain; the frontier at the terminal world is **re-enumerated** and required empty with a measured `CostRecord::Steps` proving the scan ran; regime set and `Adm` match; the grade is `Derived` and the judgement digest matches.

> **Unknown version or profile is rejected outright, never best-effort parsed.**

> **Normative.** A verified divergence certificate establishes `↑_τ`. It MUST NOT be reported as quiescence, as a fixpoint, or as `Refuted`.

### 6.4 The quiescence proposition ⟨D-QP⟩

The certificate's judgement field needs a proposition. Because `PropositionId::of` takes any `Canonical` value, this is a pure addition with no ordinal ABI risk:

```rust
// proposed: crates/brix-semantic/src/quiescent.rs, beside realizes.rs
pub struct Quiescent { pub world: ConfigId, pub policy: ConfigId, pub regimes: Digest, pub adm: Digest }
```

**Recommendation: `brix-semantic`**, not soc-core-local. A quiescence claim is exactly the extensional `F_O` fact ADR-0002 §5.2 says the kernel certifies, and placing it in the substrate is what lets `Derived → Audited → Proven` ever apply to quiescence at S7/Step 11.

## 7. Weak bisimulation and directional refinement ⟨D-REF⟩

### 7.1 Definition, exploiting determinism

`F_O` is **partial deterministic** (⟨D-FO⟩ Candidate A), so saturated behavior is a partial function, not a relation. Write `Sat(X) = O×X ⊎ 1 ⊎ ↑ ⊎ ⊥`.

**`R ⊆ X₁ × X₂` is a divergence-sensitive weak bisimulation** iff for all `(e₁,e₂) ∈ R`, with `sᵢ = satᵢ(eᵢ)`:

1. **Summand agreement.** `s₁` and `s₂` inhabit the same summand of `{O×X, 1, ↑}`. *This single clause is the entirety of divergence-sensitivity: `↑` never matches `1`.*
2. **Observation agreement.** If both are realizing, `o₁ = o₂` in `O_min` and the successors are in `R`.
3. **Fail-closed.** If either is `⊥`, the check returns `Unknown` — never `true`, never a counterexample.

Note the **absence of the usual `∃`-matching-move quantifier**: that is what determinism buys, and why this is a lockstep walk rather than partition refinement.

### 7.2 Refinement direction

With no committed nondeterminism, the only asymmetry available is definedness/divergence. `I ⊑ S` ("I refines spec S") when, along `R`:

- `sat_S = (o, s')` ⟹ `sat_I = (o, i')` with successors in `R` — the implementation must deliver every observation the spec promises;
- `sat_S = 1` ⟹ `sat_I = 1` — the implementation may not invent an observation, **and may not spin forever, where the spec says stop**;
- `sat_S = ↑` ⟹ **no obligation** — the spec is underspecified there;
- `⊥` on either side ⟹ `Unknown`.

**When refinement is the right contract:** when the specification is a *partial* reference — concretely, when a reference oracle loops administratively over a region the fast engine short-circuits. The asymmetry says *replacing a loop with a stop is legal; replacing a stop with a loop is not.* Per SOC-LAW-10 the direction must be **stated in the contract**, so it is a field of the result, never an implicit default.

> **Normative.** For SOC-LAW-08's naive-vs-incremental parity the correct contract is **symmetric `Bisimilar`**, not `Refines`: the incremental engine must be identical, not merely a refinement.

### 7.3 Counterexamples, and why minimality is free

The checker walks pairs, closing coinductively when a `(ObservableState, ObservableState)` pair repeats, and reports the first mismatch. Because `sat` is deterministic there is exactly **one** path from each start pair, so the visible prefix at the first mismatch is the **unique shortest disagreeing visible trace, by construction** — no search, no BFS-by-length, no shrinking. (`proptest` remains useful only for *discovering* fixture pairs, not for minimizing them.)

The counterexample carries context, profile, contract, both presentation ids, the minimal `visible_prefix: Vec<Observation>` (**observations only — never administrative steps**, per #61's non-goal), both summands, and a `MismatchKind` including the `DivergenceVsQuiescence` case that is the divergence-sensitivity clause. Replay aids are excluded from its canonical identity, because the two sides' administrative traces are permitted to differ.

The checker MUST return `Unknown(UndeclaredAssumption(HistoryIndependence))` if either side does not declare P1 — the coinductive close is sound only because `project` loses nothing.

## 8. One-step closure for a safety predicate

ADR-0002 §8.2 Candidate A: *"safety = one-step closure."* The Build Plan's third Step-8 clause requires it; **#61's issue text omits it**, so it is carried here.

Saturation makes the obligation's *scope* a real choice:

```rust
pub enum ClosureMode {
    /// Φ holds at every visible (saturated) state; τ-intermediates are
    /// unconstrained — the profile declared them unobservable. Default reading.
    Visible,
    /// Φ holds at every committed γ-state, τ-intermediates included.
    Raw,
}
```

**Rule (normative).** `Φ` is an invariant from `e₀` iff `Φ(project(e₀))` and, for every saturated-reachable `e` with `Φ(project(e))`, `sat(e) = Realizing(_, e')` implies `Φ(project(e'))`. Then `Φ` holds at every saturated-reachable state. *Proof:* induction on saturated-step count; sound because `D_O` is deterministic (reachability is a path, not a tree) and the `1` summand discharges no successor obligation.

**Acceptance fixture.** One graph `w0 -τ→ w_bad -τ→ w1 -o→ w2` with `Φ = (world ≠ w_bad)`: `Visible` returns closed, `Raw` returns violated. That single fixture shows simultaneously that saturation genuinely hides, that hiding is semantically consequential, that the mode distinction is real, and that the rule detects violations.

## 9. Staging and acceptance fixtures

### Stage A — labels, profile, and the saturated one-step interface

Ships `StepLabel`, `ObservationProfile`(+`Id`, preimage), `GeneratorPartitionProfile`, `ObservableState`/`project`, `PresentationV1`, `DeclaredAssumptions`, `SaturationBudget`, `SaturatedStep`, `SaturationUnknown`, `sat_step`. Certificate *structs* are declared here without `Canonical` impls, so Stage B is purely additive. Divergence detection is **not** enabled yet: every non-terminating τ-orbit is `AdministrativeBudgetExhausted`. Rewrites `Committed::Quiescent`'s doc comment, which currently promises this slice.

Also ships the fix for §1's defect: `run_reason(..) -> (Journal, Vec<CostRecord>, UnsaturatedStop)` with `UnsaturatedStop ∈ {ImmediateFrontierEmpty, TickBudgetExhausted{max_ticks}}`; `run` becomes a thin wrapper. **No caller changes.**

Fixtures: `w0 -τ→ w1 -τ→ w2 -o→ w3` gives `sat_step(e(w0)).observation == commit_tick(e(w2)).observation`, `hidden_steps == 2`, and a **3-step** journal; two runs byte-identical; the same journal under two profiles yields two visible traces and one identical chain digest (pinning ⟨D-TAU⟩ operationally); a mixed-generator decomposition yields `ProfileError` and no observation; and the degenerate `𝒢_τ = ∅` case makes `sat_step ≡ commit_tick`.

*Closes AC-1, AC-4.*

### Stage B — certificates

Ships `Canonical` impls, the fail-closed readers, both semantic checkers, the full `CertEnvelopeError` taxonomy, lasso detection under declared P1+P6, `brix-semantic::Quiescent`, and frozen vectors with an **independent second construction path** built from primitive `CanonWriter` calls that repeats the frozen literals rather than importing the constants and never calls the production encoder. **⟨D-QCERT⟩ and ⟨D-OBS⟩ were ratified 2026-08-03 (§13); the vectors are minted and both field lists are frozen ABI.**

Fixtures: a terminal state certifies and independently verifies; tampering each field yields its exact error; truncation, trailing bytes, non-minimal int, unknown version, unknown profile all reject; a prefix containing one realizing step yields `PrefixNotAdministrative`; `w0 -τ→ w1 -τ→ w0` yields `Divergent{stem 0, cycle 2}` which the quiescence checker refuses and which is not `Refuted`; a 10-long τ-chain under `max_hidden_steps = 3` yields exhaustion with **no certificate of either kind**; **the Build Plan's named conformance test** places a terminal state and an infinitely-searching state side by side and shows structurally distinct, non-interconvertible results; divergence requested without declared P1 yields `UndeclaredAssumption`; and a P1 violation (equal projections, different histories, different candidates) yields `AssumptionViolated`.

*Closes AC-2, AC-3.*

### Stage C — saturated driver, bisimulation, refinement

Ships `run_saturated`/`SaturatedRun`/`SaturatedStop` (no tick-exhaustion ambiguity anywhere), `SaturatedSystem`, `Contract`, `check_saturated`, and the counterexample types.

Fixtures: two `SaturatedSystem` impls over the same presentation — one backed by `naive_view_over`, one by `IncrementalEngine` — compared at every saturated step (the `incremental_differential.rs` model lifted to the saturated level); `τ;τ;o` vs `τ;o` with equal visible behavior holds under `Bisimilar`; an observation mismatch at visible depth 2 yields a counterexample whose prefix length is **exactly** 2 (minimality asserted, not approximated); τ-chain-to-terminal vs τ-loop yields `DivergenceVsQuiescence`; and **the direction fixture** — spec diverges where impl terminates — holds under `Refines` and fails under `Bisimilar`, proving the contracts genuinely differ.

*Closes AC-5, AC-6.*

### Stage D — closure, CJ-1 interface statement, law wiring

Ships §8's closure checker, an explicit statement of the CJ-1 adequacy interface (total + effective + returns the encoded `F_O`-structure + explicit certificates + honest `⊥`, with no proof-search, elaboration, or UI), and the SOC-LAW-10 update.

*Closes AC-7.*

No `brix run` surface is added by any stage.

## 10. SOC-LAW-10 goes `Open → Partial`, not `Enforced`

Four independent reasons, each sufficient:

1. **The law's own quantification clause forbids `Enforced`.** *"Until #59 lands, no law may silently assume that two equal digests carry compatible … observation profiles …"* — and SOC-LAW-10's domain is literally *"one declared observation profile."* #61 gives profiles an identity slot and one instance; #59 owns whether that identity is a valid context dimension. We can claim `generator-partition@1` is *a* boundary, not *the* boundary.
2. **Certification is conditional on declared, unproved hypotheses.** P1 and P6 are asserted by the presentation and only bounded-checked. A gate that requires the subject to assert its own hypothesis covers the profile *plus a promise*.
3. **The law's evolution clause is not discharged.** It requires certificates to include the exact program/world revision, but soc-core has no lowering dependency and cannot validate that a caller derived `PresentationIdV1` canonically.
4. **Precedent.** ADR-0013 fully pinned certificate identity with frozen vectors and independent reproduction, and SOC-LAW-01/12 still stayed `Partial`.

Registry edit at Stage D: status `Partial`, authority "soc-core saturation/certificate/bisimulation checkers", `open_issues: [59, 178]` (61 removed), new implementation anchors for the `saturate/` modules, and executable gates naming the Stage A–D test files.

## 11. Tensions in #61's issue text, resolved here

1. **AC-1 is unsatisfiable as literally read.** The `Observation` is equal, but the successor `ExecConfig` is not — `oracle::apply` folds every applied candidate into `history`. Read as observation equality plus coinductive successor equivalence (§4.2, §7).
2. **The issue buckets divergence under `Unknown`; the Build Plan requires it distinguished from exhaustion.** Resolved by ⟨D-DIV⟩ (§5.1). *This is the most important tension in the issue.*
3. **The issue omits the one-step-closure gate.** Carried in §8/Stage D.
4. **Sequencing inversion.** The issue asks for assumptions "for a presentation", but `Pres` is Build Plan Step 10 (S6), *after* Step 8. Resolved by a minimal `PresentationV1` that S6's `Pres` must later map onto.
5. **"Exact program/world revision" has no identity in soc-core.** Resolved by an opaque caller-supplied `PresentationIdV1`, with the honest admission that soc-core cannot enforce its canonicity.
6. **AC-5 overreaches on "settlement":** `IncrementalEngine` maintains only a candidate *view*. Stage C therefore compares the two as candidate sources feeding one shared saturated driver, entirely inside soc-core.
7. **"Connect to the future `brix run` loop"** collides with ADR-0012 §7. Resolved as an explicit non-goal: no change to `brix run` v1; a future `brix run --saturated` under a new profile marker may consume `SaturatedRun`.
8. **#61 cannot fully close the law it names**, because it lists #59 as a dependency and #59 owns observation profiles. Hence §10.

## 12. Compatibility, evolution, and non-goals

`Committed`, `Observation`, `commit_tick`, `try_commit_selected`, `prospective_successor`, `step_world_delta`, `CommittedStep`, `Journal`, and `replay_chain` are **unchanged**. `run` is retained verbatim as a wrapper over `run_reason`. No new dependency. `brix-semantic` gains one file with a new `Canonical` struct — no new enum ordinal, no reordering. Not breaking for `soc-regimes`, and not breaking for ADR-0012's future L3 adapter (which uses only untouched seams).

**L3 v1 gains nothing yet, by design:** an L3-v1 plan has one generator per rule, all realizing under any profile that does not name them administrative, so `sat_step ≡ commit_tick`. This is the correct degenerate case — and it means **#61 must not be described as "fixing" `PlanComplete`.**

All new artifacts are append-only and versioned. A future nondeterministic or quantitative `F_O` (ADR-0002 §8.2 B/C) would need a new behavior-signature version and a new saturation profile beside it.

**Non-goals:** deciding equivalence for arbitrary Brix programs; hiding nondeterministic external outcomes; requiring internal support layout, scheduling, or administrative traces to match; Studio UI; the multiscale runtime; the `brix run` loop; proving CJ-1 (Step 12 — this ADR supplies the interface it will be stated against); any change to ADR-0012's L3 v1 profile.

## 13. Decisions and their ratification state

| Marker | Decision | Status |
|---|---|---|
| **⟨D-TAU⟩** | τ is a profile projection over fully committed steps; `Committed`/`F_O`/`O_min` unchanged; administrative steps stay journaled and `Derived`. The alternative widens the frozen signature. | Proposed — ratify as stated |
| **⟨D-OBS⟩** | v1 profile = generator partition `𝒢 = 𝒢_τ ⊎ 𝒢_o`, canonically identified; mixed decompositions fail closed. | **Ratified 2026-08-03.** The v1 preimage (`brix.soc.obs-profile` / `…generator-partition@1`) is frozen; the profile *taxonomy* stays with #59 |
| **⟨D-PROJ⟩** | State identity is `(world, policy)`; `history` excluded; P1/P6 declared and bounded-checked. Without it nothing terminates; with it we assert a hypothesis about arbitrary regimes. | Proposed — ratify, with the structural alternative below as follow-up |
| **⟨D-DIV⟩** | Certified divergence is a fourth summand outside `F_O`, `Unknown`-graded for completion, never `Refuted`, never the `1` summand. | Proposed — ratify |
| **⟨D-QP⟩** | Quiescence proposition lives in `brix-semantic` (kernel-targetable) vs soc-core-local. | Proposed — `brix-semantic` |
| **⟨D-REF⟩** | Refinement's sole asymmetry: `sat_S = ↑` imposes no obligation; `sat_S = 1` forbids implementation divergence. | Proposed — ratify |
| **⟨D-QCERT⟩** | The certificates' exact field lists and byte tags (§6.2). | **Ratified 2026-08-03.** Stage B mints `vectors/soc_quiescence_v1.json` and `vectors/soc_divergence_v1.json` against it; both field lists are frozen ABI from that point |

**What ratifying ⟨D-QCERT⟩ committed us to.** The v1 quiescence certificate can only ever assert a *complete* enumeration, because [`EnumerationCompleteness`] admits exactly one ordinal and the reader accepts exactly that one. That is sound only while `Regime::candidates -> Vec<Candidate>` stays unbounded and total. If ADR-0012 §4's contemplated bounded/fallible regime API ever lands, every v1 certificate stays valid for the regimes it was minted against, and the new API MUST mint v2 — see risk 1. This was the load-bearing consideration at ratification, and it is recorded here so the constraint is not rediscovered as a surprise.

**One field-shape refinement made while implementing Stage B.** §6.2's "cycle-entry world and policy" are carried as two `ConfigId`s, not as the in-memory `ObservableState` Stage A declared on `DivergenceCertificateV1`. `ObservableState` holds `Handle`s — allocation-order-dependent interner indices with no canonical encoding — so it can identify a state *inside one run* but can never appear in a durable artifact. The projection ⟨D-PROJ⟩ is unchanged; only its certificate rendering is pinned to digests.

### Risks

1. **Enumeration completeness is true only by accident of the current API.** It rests on `Regime::candidates -> Vec<Candidate>` being unbounded and total. ADR-0012 §4 already contemplates a bounded/fallible regime API; the moment it lands, every v1 completeness claim becomes conditional. Such an API requires a v2 certificate.
2. **P1 is unverifiable in general.** `candidates(&self, e: &ExecConfig)` receives the whole config; a regime branching on `history` silently invalidates every certificate and bisimulation result. Ship the declared assumption plus a bounded differential check now; propose a narrower `HistoryIndependentRegime` trait (making P1 *structural*) as the preferred long-term fix, noting that retrofitting it onto ADR-0012's L3 regime is a real ask on #178.
3. **`Journal::chain_digest()` equality is the wrong parity notion for #61.** Weakly bisimilar implementations have different journals. This must be stated, because existing fixtures train the reader toward chain equality.
4. **The `1` summand is now overloaded** — `Committed::Quiescent` (unsaturated, uncertified) versus `SaturatedStep::Quiescent(cert)` (certified, enumeration-complete, profile/context/revision-bound). Distinct names plus the Stage-A doc rewrite are the mitigation; this is the most likely reading error in the design.
