//! Adversarial test suite for the authority publication fence (ADR-0016).
//!
//! Every test here tries to construct a `Judgement` the fence is supposed to
//! refuse, and asserts it fails **closed**: a typed `PublicationError` and
//! nothing constructed — never a downgraded outcome, never `Unknown`, never
//! `Refuted` (ADR-0016 §5). The exhaustive sweep proves the negative space
//! (every `(Authority, Outcome)` pair outside `ROUTES` is unreachable
//! regardless of what evidence is offered); the per-condition tests prove the
//! specific failure a naive fence would get wrong (wrong pole, wrong
//! verification form, wrong authority); the legal-route tests prove the
//! positive space is intact and that publication changed no `JudgementId`.

use brix_canon::Digest;
use brix_semantic::{
    AuditedSource, Authority, CertificateId, ConfigId, ContextId, DecompVerification,
    Decomposition, GeneratorId, Judgement, JudgementId, Outcome, PropositionId, PublicationError,
    Route, RouteCondition, Support, SupportKind, VerifierId, ROUTES,
};

const ALL_AUTHORITIES: [Authority; 5] = [
    Authority::ProofKernel,
    Authority::SettlementKernel,
    Authority::ExternalDriver,
    Authority::AnyResolver,
    Authority::AuditChecker,
];

const ALL_OUTCOMES: [Outcome; 6] = [
    Outcome::Proven,
    Outcome::Refuted,
    Outcome::Derived,
    Outcome::Measured,
    Outcome::Unknown,
    Outcome::Audited,
];

fn context() -> ContextId {
    ContextId::root()
}

fn proposition(tag: &str) -> PropositionId {
    PropositionId::from_canon(tag.as_bytes())
}

/// A `Recorded` two-endpoint chain — the hot loop's unverified record form.
fn recorded_decomposition(tag: &str) -> Decomposition {
    let g = GeneratorId::named(&format!("fence.{tag}.g@1"));
    let x0 = ConfigId::from_canon(format!("fence.{tag}.x0").as_bytes());
    let x1 = ConfigId::from_canon(format!("fence.{tag}.x1").as_bytes());
    Decomposition::recorded(vec![g], vec![x0, x1]).expect("well-formed chain")
}

/// A `ReplayVerified` two-endpoint chain — the audit checker's verified form.
fn verified_decomposition(tag: &str) -> Decomposition {
    let g = GeneratorId::named(&format!("fence.{tag}.g@1"));
    let x0 = ConfigId::from_canon(format!("fence.{tag}.x0").as_bytes());
    let x1 = ConfigId::from_canon(format!("fence.{tag}.x1").as_bytes());
    Decomposition::replay_verified(vec![g], vec![x0, x1]).expect("well-formed chain")
}

/// Build the support a given legal route demands, so the "every route
/// succeeds" and "byte-identity" tests can drive `ROUTES` generically instead
/// of hand-listing each row twice.
fn support_for_route<'a>(
    route: &Route,
    proposition: PropositionId,
    recorded: &'a Decomposition,
    verified: &'a Decomposition,
    verifier: VerifierId,
    certificate: CertificateId,
    body: Digest,
) -> Support<'a> {
    match route.support {
        SupportKind::KernelCertificate => Support::KernelCertificate {
            verifier,
            certificate,
        },
        SupportKind::KernelRefutation => Support::KernelRefutation {
            verifier,
            certificate,
        },
        SupportKind::Settlement => match route.condition {
            RouteCondition::Decomposition(DecompVerification::Recorded) => {
                Support::Settlement(recorded)
            }
            RouteCondition::Decomposition(DecompVerification::ReplayVerified) => {
                Support::Settlement(verified)
            }
            // The AnyResolver/Unknown row has no condition: either form
            // satisfies it, so pick one arbitrarily.
            RouteCondition::None => Support::Settlement(recorded),
        },
        SupportKind::Ground => Support::Ground { body },
        SupportKind::Measurement => Support::Measurement { body },
        SupportKind::ExternalResult => Support::ExternalResult { body },
        SupportKind::Suggestion => Support::Suggestion { body },
        SupportKind::TreeRealization => Support::tree_realization(proposition),
    }
}

// --- Exhaustive illegal-pair sweep -----------------------------------------

#[test]
fn illegal_authority_outcome_pairs_fail_for_every_support_kind() {
    let ctx = context();
    let prop = proposition("sweep");
    let recorded = recorded_decomposition("sweep");
    let verifier = VerifierId::named("sweep-verifier@1");
    let certificate = CertificateId::from_canon(b"sweep-certificate");
    let body = Digest::of(brix_canon::Domain::Value, b"sweep-body");

    // One representative value per `SupportKind` variant — kept in sync with
    // the enum by the length assertion below.
    let supports: [Support; 8] = [
        Support::KernelCertificate {
            verifier,
            certificate,
        },
        Support::KernelRefutation {
            verifier,
            certificate,
        },
        Support::Settlement(&recorded),
        Support::Ground { body },
        Support::Measurement { body },
        Support::ExternalResult { body },
        Support::Suggestion { body },
        Support::tree_realization(prop),
    ];
    assert_eq!(
        supports.len(),
        8,
        "one representative per SupportKind variant — update this list if SupportKind grows"
    );

    let mut illegal_pairs_checked = 0usize;
    for authority in ALL_AUTHORITIES {
        for outcome in ALL_OUTCOMES {
            if authority == outcome.authority() {
                // The one legal pair for this outcome — covered by the
                // legal-route tests below, not this sweep.
                continue;
            }
            illegal_pairs_checked += 1;
            for support in &supports {
                let err = Judgement::publish(authority, ctx, prop, outcome, *support).expect_err(
                    "an authority that is not this outcome's sole authority must never publish it, on any support",
                );
                assert_eq!(
                    err,
                    PublicationError::WrongAuthority {
                        outcome,
                        claimed: authority,
                        sole: outcome.authority(),
                    },
                    "a wrong-authority claim must be rejected as WrongAuthority specifically, not laundered into a different error"
                );
            }
        }
    }
    // 5 authorities x 6 outcomes, minus the 6 legal (authority, outcome) pairs.
    assert_eq!(illegal_pairs_checked, 24);
}

// --- Specific illegal-support-for-correct-authority cases -------------------

#[test]
fn proven_on_settlement_replay_is_unsupported_evidence() {
    // A settlement replay must never support either revision-invariant pole.
    let ctx = context();
    let prop = proposition("proven-on-settlement-replay");
    let verified = verified_decomposition("proven-on-settlement-replay");

    let err = Judgement::publish(
        Authority::ProofKernel,
        ctx,
        prop,
        Outcome::Proven,
        Support::Settlement(&verified),
    )
    .expect_err("a settlement replay must never support Proven");
    assert_eq!(
        err,
        PublicationError::UnsupportedEvidence {
            authority: Authority::ProofKernel,
            outcome: Outcome::Proven,
            support: SupportKind::Settlement,
        }
    );
}

#[test]
fn refuted_on_kernel_certificate_is_unsupported_evidence() {
    // Wrong pole: a certificate proves, it does not refute.
    let ctx = context();
    let prop = proposition("refuted-on-kernel-certificate");
    let support = Support::KernelCertificate {
        verifier: VerifierId::named("wrong-pole-verifier@1"),
        certificate: CertificateId::from_canon(b"wrong-pole-certificate"),
    };

    let err = Judgement::publish(Authority::ProofKernel, ctx, prop, Outcome::Refuted, support)
        .expect_err("a KernelCertificate does not refute — Refuted demands KernelRefutation");
    assert_eq!(
        err,
        PublicationError::UnsupportedEvidence {
            authority: Authority::ProofKernel,
            outcome: Outcome::Refuted,
            support: SupportKind::KernelCertificate,
        }
    );
}

#[test]
fn proven_on_kernel_refutation_is_unsupported_evidence() {
    // The other direction of the same non-interchangeability.
    let ctx = context();
    let prop = proposition("proven-on-kernel-refutation");
    let support = Support::KernelRefutation {
        verifier: VerifierId::named("wrong-pole-verifier@2"),
        certificate: CertificateId::from_canon(b"wrong-pole-certificate-2"),
    };

    let err = Judgement::publish(Authority::ProofKernel, ctx, prop, Outcome::Proven, support)
        .expect_err("a KernelRefutation does not prove — the two poles are not interchangeable");
    assert_eq!(
        err,
        PublicationError::UnsupportedEvidence {
            authority: Authority::ProofKernel,
            outcome: Outcome::Proven,
            support: SupportKind::KernelRefutation,
        }
    );
}

#[test]
fn measured_on_kernel_certificate_is_unsupported_evidence() {
    let ctx = context();
    let prop = proposition("measured-on-kernel-certificate");
    let support = Support::KernelCertificate {
        verifier: VerifierId::named("measured-verifier@1"),
        certificate: CertificateId::from_canon(b"measured-certificate"),
    };

    let err = Judgement::publish(
        Authority::ExternalDriver,
        ctx,
        prop,
        Outcome::Measured,
        support,
    )
    .expect_err("a kernel certificate must never support Measured");
    assert_eq!(
        err,
        PublicationError::UnsupportedEvidence {
            authority: Authority::ExternalDriver,
            outcome: Outcome::Measured,
            support: SupportKind::KernelCertificate,
        }
    );
}

#[test]
fn audited_from_recorded_chain_is_decomposition_verification_mismatch() {
    let ctx = context();
    let prop = proposition("audited-from-recorded-chain");
    let recorded = recorded_decomposition("audited-from-recorded-chain");

    let err = Judgement::publish(
        Authority::AuditChecker,
        ctx,
        prop,
        Outcome::Audited,
        Support::Settlement(&recorded),
    )
    .expect_err("a recorded chain must never support Audited");
    assert_eq!(
        err,
        PublicationError::DecompositionVerificationMismatch {
            outcome: Outcome::Audited,
            expected: DecompVerification::ReplayVerified,
            found: DecompVerification::Recorded,
        }
    );
}

#[test]
fn derived_from_replay_verified_chain_is_decomposition_verification_mismatch() {
    let ctx = context();
    let prop = proposition("derived-from-replay-verified-chain");
    let verified = verified_decomposition("derived-from-replay-verified-chain");

    let err = Judgement::publish(
        Authority::SettlementKernel,
        ctx,
        prop,
        Outcome::Derived,
        Support::Settlement(&verified),
    )
    .expect_err("a replay-verified chain must never be published as Derived — the hot loop records, it never asserts verification");
    assert_eq!(
        err,
        PublicationError::DecompositionVerificationMismatch {
            outcome: Outcome::Derived,
            expected: DecompVerification::Recorded,
            found: DecompVerification::ReplayVerified,
        }
    );
}

#[test]
fn wrong_claimed_authority_is_rejected_with_the_right_sole_field() {
    let ctx = context();

    // Derived claimed by the audit checker.
    let prop = proposition("wrong-authority-derived-claimed-by-audit-checker");
    let recorded = recorded_decomposition("wrong-authority-derived-claimed-by-audit-checker");
    let err = Judgement::publish(
        Authority::AuditChecker,
        ctx,
        prop,
        Outcome::Derived,
        Support::Settlement(&recorded),
    )
    .expect_err("only the settlement kernel may publish Derived");
    assert_eq!(
        err,
        PublicationError::WrongAuthority {
            outcome: Outcome::Derived,
            claimed: Authority::AuditChecker,
            sole: Authority::SettlementKernel,
        }
    );

    // Audited claimed by the settlement kernel.
    let prop = proposition("wrong-authority-audited-claimed-by-settlement-kernel");
    let verified = verified_decomposition("wrong-authority-audited-claimed-by-settlement-kernel");
    let err = Judgement::publish(
        Authority::SettlementKernel,
        ctx,
        prop,
        Outcome::Audited,
        Support::Settlement(&verified),
    )
    .expect_err("only the audit checker may publish Audited");
    assert_eq!(
        err,
        PublicationError::WrongAuthority {
            outcome: Outcome::Audited,
            claimed: Authority::SettlementKernel,
            sole: Authority::AuditChecker,
        }
    );

    // Proven claimed by the settlement kernel.
    let prop = proposition("wrong-authority-proven-claimed-by-settlement-kernel");
    let support = Support::KernelCertificate {
        verifier: VerifierId::named("wrong-authority-verifier@1"),
        certificate: CertificateId::from_canon(b"wrong-authority-certificate"),
    };
    let err = Judgement::publish(
        Authority::SettlementKernel,
        ctx,
        prop,
        Outcome::Proven,
        support,
    )
    .expect_err("only the proof kernel may publish Proven");
    assert_eq!(
        err,
        PublicationError::WrongAuthority {
            outcome: Outcome::Proven,
            claimed: Authority::SettlementKernel,
            sole: Authority::ProofKernel,
        }
    );
}

// --- Every legal route actually succeeds ------------------------------------

#[test]
fn every_legal_route_actually_succeeds() {
    let ctx = context();
    let verifier = VerifierId::named("legal-route-verifier@1");
    let certificate = CertificateId::from_canon(b"legal-route-certificate");
    let body = Digest::of(brix_canon::Domain::Value, b"legal-route-body");

    for route in ROUTES {
        let tag = format!(
            "legal-route-{:?}-{:?}-{:?}",
            route.authority, route.outcome, route.support
        );
        let prop = proposition(&tag);
        let recorded = recorded_decomposition(&tag);
        let verified = verified_decomposition(&tag);
        let support = support_for_route(
            route,
            prop,
            &recorded,
            &verified,
            verifier,
            certificate,
            body,
        );

        let judgement = Judgement::publish(route.authority, ctx, prop, route.outcome, support)
            .unwrap_or_else(|err| panic!("legal route {route:?} was rejected: {err:?}"));

        assert_eq!(
            judgement.outcome, route.outcome,
            "{route:?}: published judgement carries the wrong outcome"
        );
        assert_eq!(
            judgement.evidence,
            support.evidence().id(),
            "{route:?}: published judgement's evidence must be exactly the support's evidence id"
        );
    }
}

// --- Byte-identity guard: the fence moved no JudgementId --------------------

#[test]
fn every_legal_route_yields_a_judgement_id_byte_identical_to_recompute() {
    // This proves the fence moved no `JudgementId`: for every legal route,
    // `Judgement::publish(...).unwrap().id()` is byte-identical to
    // `JudgementId::recompute` over the same four fields. Publication is a
    // new *door*, not a new *encoding* — the ADR-0016 fence changed who may
    // construct a judgement, never what a judgement's identity is.
    let ctx = context();
    let verifier = VerifierId::named("byte-identity-verifier@1");
    let certificate = CertificateId::from_canon(b"byte-identity-certificate");
    let body = Digest::of(brix_canon::Domain::Value, b"byte-identity-body");

    for route in ROUTES {
        let tag = format!(
            "byte-identity-{:?}-{:?}-{:?}",
            route.authority, route.outcome, route.support
        );
        let prop = proposition(&tag);
        let recorded = recorded_decomposition(&tag);
        let verified = verified_decomposition(&tag);
        let support = support_for_route(
            route,
            prop,
            &recorded,
            &verified,
            verifier,
            certificate,
            body,
        );

        let published = Judgement::publish(route.authority, ctx, prop, route.outcome, support)
            .unwrap_or_else(|err| panic!("legal route {route:?} was rejected: {err:?}"));

        let recomputed = JudgementId::recompute(ctx, prop, route.outcome, support.evidence_id());
        assert_eq!(
            published.id(),
            recomputed,
            "{route:?}: publish() must yield exactly the id recompute() derives from the same four fields"
        );
    }
}

// --- AuditedSource::verify ---------------------------------------------------

#[test]
fn audited_source_rejects_a_derived_judgement() {
    let ctx = context();
    let prop = proposition("audited-source-rejects-derived");
    let recorded = recorded_decomposition("audited-source-rejects-derived");
    let derived = Judgement::publish(
        Authority::SettlementKernel,
        ctx,
        prop,
        Outcome::Derived,
        Support::Settlement(&recorded),
    )
    .expect("a Recorded chain legally publishes Derived");

    let err = AuditedSource::verify(&derived, Support::Settlement(&recorded))
        .expect_err("a Derived judgement must never cross the elaboration boundary");
    assert_eq!(
        err,
        PublicationError::NotAudited {
            found: Outcome::Derived
        }
    );
}

#[test]
fn audited_source_rejects_a_chain_other_than_the_one_its_evidence_names() {
    let ctx = context();
    let prop = proposition("audited-source-binding-mismatch");
    let bound = verified_decomposition("audited-source-binding-mismatch-bound");
    let other = verified_decomposition("audited-source-binding-mismatch-other");

    let audited = Judgement::publish(
        Authority::AuditChecker,
        ctx,
        prop,
        Outcome::Audited,
        Support::Settlement(&bound),
    )
    .expect("a ReplayVerified chain legally publishes Audited");

    // A genuine Audited judgement, but the presented artifact is a different
    // chain than the one its own evidence id names.
    let err = AuditedSource::verify(&audited, Support::Settlement(&other)).expect_err(
        "a judgement's evidence id must bind to the exact artifact presented, not merely one of the right shape",
    );
    assert_eq!(
        err,
        PublicationError::EvidenceBindingMismatch {
            expected: audited.evidence,
            found: Support::Settlement(&other).evidence_id(),
        }
    );
}

#[test]
fn audited_source_refuses_a_recorded_chain_even_for_a_genuinely_audited_judgement() {
    let ctx = context();
    let prop = proposition("audited-source-refuses-recorded-chain");
    let verified = verified_decomposition("audited-source-refuses-recorded-chain");
    let recorded = recorded_decomposition("audited-source-refuses-recorded-chain");

    let audited = Judgement::publish(
        Authority::AuditChecker,
        ctx,
        prop,
        Outcome::Audited,
        Support::Settlement(&verified),
    )
    .expect("a ReplayVerified chain legally publishes Audited");

    // The judgement really is Audited; a recorded chain must still be
    // refused when presented as its support (ADR-0016 §6 step 2).
    let err = AuditedSource::verify(&audited, Support::Settlement(&recorded)).expect_err(
        "a recorded chain must never support Audited, even against a genuinely Audited judgement",
    );
    assert_eq!(
        err,
        PublicationError::DecompositionVerificationMismatch {
            outcome: Outcome::Audited,
            expected: DecompVerification::ReplayVerified,
            found: DecompVerification::Recorded,
        }
    );
}

#[test]
fn audited_source_honest_case_succeeds_and_returns_the_same_judgement() {
    let ctx = context();
    let prop = proposition("audited-source-honest-case");
    let verified = verified_decomposition("audited-source-honest-case");

    let audited = Judgement::publish(
        Authority::AuditChecker,
        ctx,
        prop,
        Outcome::Audited,
        Support::Settlement(&verified),
    )
    .expect("a ReplayVerified chain legally publishes Audited");

    let source = AuditedSource::verify(&audited, Support::Settlement(&verified)).expect(
        "the honest case — Audited outcome, replay-verified chain, bound evidence — must verify",
    );
    assert_eq!(*source.judgement(), audited);
}
