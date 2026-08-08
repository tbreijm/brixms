//! The audit-factorization checker (ADR-0002 §4.1, §5 point 1; §6; PD-1's
//! *operational* discharge; `Build_Plan_v3_SOC.md` Step 4 gate).
//!
//! This module is the **sole authority** for the `Outcome::Audited` route
//! (ADR-0002 §4.1 verifier-authority table): it is the reference replayer —
//! `brix-oracle`'s role at the audit boundary — that replays a committed
//! step's recorded (unverified) [`brix_semantic::Decomposition`] against the
//! log and, if and only if the exact relational composition
//! `ρ_k = ρ_gn ∘ … ∘ ρ_g1` checks out over the intermediate-configuration
//! chain, publishes the upgraded `Audited` judgement. The engine's hot loop
//! (`crate::commit`) may *record* a decomposition; it may never assert its
//! verification — that is exactly the line this module draws.
//!
//! **Two different judgements, one `Dependency` edge (ADR-0002 §5 point 1).**
//! A committed step is `Derived` (published at the commit boundary,
//! `crate::commit::commit_tick`). Its replay-verified decomposition is a
//! **new**, separate `Audited` judgement — different evidence
//! (`Decomposition::replay_verified` mints a different `DecompositionId` than
//! `Decomposition::recorded` even over identical generators/configs, by
//! design — see `brix_semantic::decomposition`'s module doc), hence a
//! different `JudgementId`. The two are linked by a `Dependency` edge
//! (`EdgeKind::Premise`, target = the `Derived` judgement's digest). The
//! canonical upgrade path (ADR-0002 §5 point 2) is `Derived → Audited →
//! Proven`; this module discharges exactly the first arrow.
//!
//! **Fail-closed (ADR-0002 §4, §4.1).** A replay that does not complete
//! exactly yields `Unknown(reason)`, never a pass, never a mutation of the
//! pre-existing `Derived` judgement. There is no code path in this module
//! that publishes `Derived` or `Proven`.

use brix_semantic::{
    Authority, ConfigId, ContextId, DecompVerification, Decomposition, DecompositionError,
    Dependency, EdgeKind, Evidence, GeneratorId, GeneratorRegistry, Judgement, JudgementId,
    Outcome, Realizes, Support,
};

use crate::journal::{CommittedStep, Journal};

/// The relation `ρ_g` of each generator `g ∈ 𝒢`, as the checker replays it.
/// `realizes(g, src, dst)` is true iff the primitive logged witness `g`
/// relates configuration `src` to `dst` under `ρ_g`. The checker verifies the
/// EXACT relational composition `ρ_k = ρ_gn ∘ … ∘ ρ_g1` by walking the
/// decomposition's intermediate-configuration chain one generator at a time
/// (ADR-0002 §6, `Build_Plan_v3_SOC.md` Step 4 gate).
pub trait GeneratorSemantics {
    fn realizes(&self, g: &GeneratorId, src: &ConfigId, dst: &ConfigId) -> bool;
}

/// The result of auditing one [`CommittedStep`]. `Unknown` carries a reason
/// and is **never** a pass (ADR-0002 §4: "fail closed to `Unknown`, never a
/// downgrade-hiding pass").
pub enum AuditResult {
    /// Replay completed exactly: the recorded decomposition was flipped to
    /// the `ReplayVerified` form and an `Audited` judgement was published — a
    /// NEW `JudgementId`, linked by a `Dependency` edge to the `Derived` one.
    /// Boxed: `AuditedStep` carries a full `Judgement` + a cloned
    /// `Decomposition` and would otherwise make `Unknown`'s `&'static str`
    /// variant balloon the whole enum (`clippy::large_enum_variant`).
    Audited(Box<AuditedStep>),
    /// Replay did not complete exactly (ADR-0002 §4: fail closed to
    /// `Unknown`, never a downgrade-hiding pass, never a mutation of the
    /// `Derived` judgement). Carries a short, fixed reason.
    Unknown(&'static str),
}

/// The artifacts produced by a successful audit: the new `Audited` judgement,
/// its id, the pre-existing (unchanged) `Derived` judgement's id, the
/// `Dependency` edge linking them, and the `ReplayVerified` decomposition.
pub struct AuditedStep {
    /// The new `Audited` judgement (`Outcome::Audited`).
    pub audited: Judgement,
    /// `audited.id()`, cached.
    pub audited_id: JudgementId,
    /// The pre-existing `Derived` judgement's id (rebuilt from the log,
    /// never mutated by this module — see [`audit_step`] step 1).
    pub derived_id: JudgementId,
    /// The edge `Audited → Derived` (`EdgeKind::Premise`, target =
    /// `derived_id.digest()`), realizing ADR-0002 §5 point 1's "linked by a
    /// `Dependency` edge."
    pub link: Dependency,
    /// The `ReplayVerified` form of the step's decomposition.
    pub verified: Decomposition,
}

/// Audit one committed step: replay its recorded [`Decomposition`] against
/// the log and, iff the exact relational composition checks out end to end,
/// publish the upgraded `Audited` judgement (ADR-0002 §4.1, §5 point 1;
/// `Build_Plan_v3_SOC.md` Step 4 gate). Any failure returns
/// `AuditResult::Unknown(reason)` — never a pass, never a mutated/altered
/// `Derived` judgement.
///
/// Verification procedure, in order:
///
/// 1. **Reconstruct + cross-check the `Derived` judgement "against the
///    log"** — rebuild `Realizes`, the `Derived` evidence
///    (`Evidence::SettlementReplay { body: step.decomposition.id().digest() }`),
///    and `derived_id`; verify `step.observation.outcome_class ==
///    Outcome::Derived` and `step.observation.judgement_digest ==
///    derived_id.digest()`. Also require the input decomposition is in
///    `Recorded` form (this checker upgrades a recorded record; it does not
///    re-audit an already-verified one).
/// 2. **Endpoint match** — `decomposition.configs` starts at `step.src` and
///    ends at `step.dst` (the chain-length invariant is already enforced by
///    `Decomposition`'s constructor; asserted again here defensively).
/// 3. **Exact relational composition** — for every `i`, `generators[i] ∈ 𝒢`
///    (per `registry`) and `semantics.realizes(&generators[i], &configs[i],
///    &configs[i+1])`. This is `ρ_k = ρ_gn ∘ … ∘ ρ_g1` verified stepwise along
///    `x_0, …, x_n`.
/// 4. **Publish `Audited`** — flip the decomposition to `ReplayVerified`,
///    build the new evidence/judgement/link, and return
///    `AuditResult::Audited`.
pub fn audit_step(
    step: &CommittedStep,
    context: ContextId,
    registry: &GeneratorRegistry,
    semantics: &dyn GeneratorSemantics,
) -> AuditResult {
    // This module's one and only outcome route (ADR-0002 §4.1): it never
    // publishes Derived/Proven, only ever Audited — and only Authority::
    // AuditChecker may publish Audited. Asserted here as a standing
    // consistency check on the routing table itself, not on any runtime
    // value.
    debug_assert_eq!(Outcome::Audited.authority(), Authority::AuditChecker);

    // Step 1: reconstruct + cross-check the Derived judgement against the log.
    if step.decomposition.verification != DecompVerification::Recorded {
        return AuditResult::Unknown("decomposition is not in recorded form");
    }

    let proposition = Realizes::new(step.witness, step.src, step.dst).proposition_id();
    let derived_evidence = Evidence::SettlementReplay {
        body: step.decomposition.id().digest(),
    }
    .id();
    // Identity only, never publication (ADR-0016 §3): this checker is
    // *auditing* the settlement kernel's `Derived` judgement, not minting it,
    // and has no standing to claim `Authority::SettlementKernel`.
    let derived_id =
        JudgementId::recompute(context, proposition, Outcome::Derived, derived_evidence);

    if step.observation.outcome_class != Outcome::Derived
        || step.observation.judgement_digest != derived_id.digest()
    {
        return AuditResult::Unknown(
            "log integrity: recorded observation does not match the Derived judgement",
        );
    }

    // Step 2: endpoint match.
    if step.decomposition.configs.first() != Some(&step.src)
        || step.decomposition.configs.last() != Some(&step.dst)
    {
        return AuditResult::Unknown("decomposition endpoints do not match the committed step");
    }
    // The chain-length invariant (configs.len() == generators.len() + 1) is
    // already guaranteed by Decomposition's constructor; assert it again
    // defensively since this checker must never trust an invariant silently.
    debug_assert_eq!(
        step.decomposition.configs.len(),
        step.decomposition.generators.len() + 1,
        "Decomposition's own constructor guarantees this chain-length invariant"
    );

    // Step 3: exact relational composition ρ_k = ρ_gn ∘ … ∘ ρ_g1, walked
    // stepwise along x_0, …, x_n.
    for (i, g) in step.decomposition.generators.iter().enumerate() {
        if !registry.contains(g) {
            return AuditResult::Unknown("decomposition cites a generator outside 𝒢");
        }
        let src = &step.decomposition.configs[i];
        let dst = &step.decomposition.configs[i + 1];
        if !semantics.realizes(g, src, dst) {
            return AuditResult::Unknown(
                "relational composition failed: an intermediate configuration is not realized by its generator",
            );
        }
    }

    // Step 4: publish Audited.
    let verified = match Decomposition::replay_verified(
        step.decomposition.generators.clone(),
        step.decomposition.configs.clone(),
    ) {
        Ok(v) => v,
        Err(DecompositionError::ChainLengthMismatch { .. }) => {
            // Unreachable given the constructor invariant re-asserted above,
            // but never turn an internal impossibility into a silent pass.
            return AuditResult::Unknown("decomposition chain length invalid on replay");
        }
    };
    // This module's one and only publication, through the ADR-0016 §4 fence.
    // The `(AuditChecker, Audited, Settlement)` route demands a
    // `ReplayVerified` chain — which is precisely what steps 1–3 above earned
    // and what `replay_verified` just stamped.
    let audited = match Judgement::publish(
        Authority::AuditChecker,
        context,
        proposition,
        Outcome::Audited,
        Support::Settlement(&verified),
    ) {
        Ok(j) => j,
        Err(_) => {
            // Unreachable on the settled route; never turn a refused
            // publication into a silent pass (ADR-0002 §4: fail closed to
            // Unknown, never to Audited).
            return AuditResult::Unknown("audited publication refused by the authority fence");
        }
    };
    let audited_id = audited.id();
    let link = Dependency::new(EdgeKind::Premise, derived_id.digest());

    AuditResult::Audited(Box::new(AuditedStep {
        audited,
        audited_id,
        derived_id,
        link,
        verified,
    }))
}

/// Audit every step in `journal`, in commit order.
pub fn audit_journal(
    journal: &Journal,
    context: ContextId,
    registry: &GeneratorRegistry,
    semantics: &dyn GeneratorSemantics,
) -> Vec<AuditResult> {
    journal
        .steps()
        .iter()
        .map(|step| audit_step(step, context, registry, semantics))
        .collect()
}
