//! The **authority publication fence** — who may construct a [`Judgement`]
//! with which outcome, on what support (ADR-0016).
//!
//! ADR-0002 §4.1 froze the verifier-authority table in prose, and
//! [`Outcome::authority`] encodes half of it as data. Nothing called it. This
//! module is the other half and the enforcement: [`ROUTES`] enumerates every
//! legal (authority, outcome, evidence-kind) triple, and
//! [`Judgement::publish`] is the only door outside this crate through which a
//! `Judgement` value can be obtained.
//!
//! The shape is forced by one fact about the substrate: **a [`Judgement`]
//! carries an [`EvidenceId`] — a digest — not the [`Evidence`].** No check
//! applied to a finished judgement can recover what supports it, so the fence
//! sits at construction and is handed the supporting *artifact*. That is what
//! [`Support`] is: [`Evidence`] with the bodies replaced by the things they
//! digest.
//!
//! Two doors, and the distinction is normative (ADR-0016 §3):
//!
//! | Door | Yields | Claims authority |
//! |---|---|---|
//! | [`Judgement::publish`] | a `Judgement` **value** | yes — checked against [`ROUTES`] |
//! | [`crate::JudgementId::recompute`] | a `JudgementId` | **no** |
//!
//! A checker re-deriving the id of a judgement it is *auditing* is not
//! publishing it, and holds a digest rather than the artifact. Forcing it
//! through `publish` would make it claim an authority it does not have.
//!
//! Fail closed throughout: every rejection returns a [`PublicationError`] and
//! constructs nothing. A refused publication is never a downgraded outcome,
//! never `Unknown`, and never `Refuted` (ADR-0016 §5).

use brix_canon::Digest;

use crate::{
    Authority, CertificateId, DecompVerification, Decomposition, Evidence, EvidenceId, Judgement,
    Outcome, PropositionId, VerifierId,
};

/// What a publisher presents as support for an outcome — [`Evidence`] with the
/// opaque bodies replaced by the artifacts they digest, so [`ROUTES`] can
/// inspect them.
///
/// [`Support::Settlement`] borrows the [`Decomposition`] rather than taking its
/// id precisely because the route conditions in ADR-0016 §4 turn on
/// [`DecompVerification`], which an id cannot expose.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Support<'a> {
    /// A proof kernel accepted a certificate that the proposition holds.
    KernelCertificate {
        verifier: VerifierId,
        certificate: CertificateId,
    },
    /// A proof kernel accepted a refutation.
    KernelRefutation {
        verifier: VerifierId,
        certificate: CertificateId,
    },
    /// A settlement decomposition — `Recorded` supports `Derived`,
    /// `ReplayVerified` supports `Audited` (ADR-0002 §5.1/§5.2).
    Settlement(&'a Decomposition),
    /// A raw ground assertion at the current revision.
    Ground { body: Digest },
    /// A measurement, simulation, or estimate.
    Measurement { body: Digest },
    /// A result certified by a named external system.
    ExternalResult { body: Digest },
    /// A non-authoritative suggestion.
    Suggestion { body: Digest },
    /// **Provisional (ADR-0016 §7, `spec/errata/0004-tree-realization-audited-support.md`).**
    /// Tree-realization support with no replay-verified [`Decomposition`]
    /// behind it — today its body is a digest of the proposition being
    /// claimed, which distinguishes nothing. This variant exists so the hole
    /// is *named in the table* rather than invisible inside a general-purpose
    /// constructor. It is reported, not blessed.
    TreeRealization { body: Digest },
}

impl Support<'static> {
    /// **Provisional (ADR-0016 §7).** The tree-realization support body, in
    /// one place.
    ///
    /// The formula — a value-domain digest of the proposition's own digest —
    /// is what `soc-regimes::audited_type_check_tree` publishes today, and it
    /// is exactly what makes the route unsound: the evidence is computable
    /// from the claim. It lives here rather than being open-coded at each
    /// site so the erratum has a single anchor, and so the day it is replaced
    /// with a real artifact there is one definition to delete.
    ///
    /// See `spec/errata/0004-tree-realization-audited-support.md`.
    pub fn tree_realization(proposition: PropositionId) -> Self {
        Support::TreeRealization {
            body: Digest::of(brix_canon::Domain::Value, proposition.digest().as_bytes()),
        }
    }
}

impl Support<'_> {
    /// The [`Evidence`] this support encodes. Total.
    ///
    /// Note that [`Support::Settlement`] and [`Support::TreeRealization`] both
    /// project to [`Evidence::SettlementReplay`]: they are the same evidence
    /// *variant* distinguished by what stands behind it. That distinction is
    /// exactly what the id-only surface could not express, and the reason the
    /// §7 finding stayed invisible for so long.
    pub fn evidence(&self) -> Evidence {
        match *self {
            Support::KernelCertificate {
                verifier,
                certificate,
            } => Evidence::KernelCertificate {
                verifier,
                certificate,
            },
            Support::KernelRefutation {
                verifier,
                certificate,
            } => Evidence::KernelRefutation {
                verifier,
                certificate,
            },
            Support::Settlement(decomposition) => Evidence::SettlementReplay {
                body: decomposition.id().digest(),
            },
            Support::Ground { body } => Evidence::GroundAssertion { body },
            Support::Measurement { body } => Evidence::Measurement { body },
            Support::ExternalResult { body } => Evidence::CertifiedExternalResult { body },
            Support::Suggestion { body } => Evidence::Suggestion { body },
            Support::TreeRealization { body } => Evidence::SettlementReplay { body },
        }
    }

    /// The `Copy` tag [`ROUTES`] matches on.
    pub const fn kind(&self) -> SupportKind {
        match self {
            Support::KernelCertificate { .. } => SupportKind::KernelCertificate,
            Support::KernelRefutation { .. } => SupportKind::KernelRefutation,
            Support::Settlement(_) => SupportKind::Settlement,
            Support::Ground { .. } => SupportKind::Ground,
            Support::Measurement { .. } => SupportKind::Measurement,
            Support::ExternalResult { .. } => SupportKind::ExternalResult,
            Support::Suggestion { .. } => SupportKind::Suggestion,
            Support::TreeRealization { .. } => SupportKind::TreeRealization,
        }
    }

    /// The content-addressed id of the evidence this support encodes — the
    /// value that lands in [`Judgement::evidence`].
    pub fn evidence_id(&self) -> EvidenceId {
        self.evidence().id()
    }
}

/// The evidence-kind column of the ADR-0016 §4 route table. Carries no
/// canonical encoding and no ABI ordinal — it is a routing tag, never hashed.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum SupportKind {
    KernelCertificate,
    KernelRefutation,
    Settlement,
    Ground,
    Measurement,
    ExternalResult,
    Suggestion,
    /// Provisional — see [`Support::TreeRealization`].
    TreeRealization,
}

/// Whether a [`Route`] is settled doctrine or a named, reported hole.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum RouteStatus {
    /// Blessed by ADR-0002 §4.1 / ADR-0016 §4.
    Settled,
    /// Exists so a currently-live path keeps compiling and behaving exactly as
    /// it does today, while the soundness question is reported rather than
    /// quietly resolved. **Not blessed.**
    Provisional,
}

/// What a route additionally demands of its support beyond the evidence kind.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum RouteCondition {
    /// Nothing beyond the evidence kind.
    None,
    /// The [`Decomposition`] must carry this verification form. `Derived`
    /// takes `Recorded` (the hot loop's unverified record, ADR-0002 §5.1);
    /// `Audited` takes `ReplayVerified` (the checker's replay, §4.1).
    Decomposition(DecompVerification),
}

/// One legal publication route: an authority may publish this outcome on this
/// kind of support, subject to this condition.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Route {
    pub authority: Authority,
    pub outcome: Outcome,
    pub support: SupportKind,
    pub condition: RouteCondition,
    pub status: RouteStatus,
}

/// **The** authoritative enumeration of legal publishers (ADR-0002 §4.1 +
/// ADR-0016 §4). This is the thing the code consults; the ADR table is its
/// prose rendering, not a second source of truth.
///
/// Every triple absent from this list is illegal. Adding a publisher is a
/// deliberate act: a row here plus a reviewed reason in the ADR.
pub const ROUTES: &[Route] = &[
    // --- ProofKernel: the two revision-invariant poles, and they are not
    // interchangeable. A settlement replay can never support either.
    Route {
        authority: Authority::ProofKernel,
        outcome: Outcome::Proven,
        support: SupportKind::KernelCertificate,
        condition: RouteCondition::None,
        status: RouteStatus::Settled,
    },
    Route {
        authority: Authority::ProofKernel,
        outcome: Outcome::Refuted,
        support: SupportKind::KernelRefutation,
        condition: RouteCondition::None,
        status: RouteStatus::Settled,
    },
    // --- SettlementKernel: the hot loop records; it never asserts verification.
    Route {
        authority: Authority::SettlementKernel,
        outcome: Outcome::Derived,
        support: SupportKind::Settlement,
        condition: RouteCondition::Decomposition(DecompVerification::Recorded),
        status: RouteStatus::Settled,
    },
    // --- AuditChecker: only a replayed-and-verified chain upgrades to Audited.
    Route {
        authority: Authority::AuditChecker,
        outcome: Outcome::Audited,
        support: SupportKind::Settlement,
        condition: RouteCondition::Decomposition(DecompVerification::ReplayVerified),
        status: RouteStatus::Settled,
    },
    // PROVISIONAL — ADR-0016 §7, spec/errata/0004-tree-realization-audited-support.md.
    // `soc-regimes::audited_type_check_tree` publishes Audited on evidence
    // computed from the proposition it is claiming. Reported, not blessed;
    // this row keeps behaviour identical while the hole has a name.
    Route {
        authority: Authority::AuditChecker,
        outcome: Outcome::Audited,
        support: SupportKind::TreeRealization,
        condition: RouteCondition::None,
        status: RouteStatus::Provisional,
    },
    // --- ExternalDriver: a named driver, via a certified-result envelope.
    Route {
        authority: Authority::ExternalDriver,
        outcome: Outcome::Measured,
        support: SupportKind::Measurement,
        condition: RouteCondition::None,
        status: RouteStatus::Settled,
    },
    Route {
        authority: Authority::ExternalDriver,
        outcome: Outcome::Measured,
        support: SupportKind::ExternalResult,
        condition: RouteCondition::None,
        status: RouteStatus::Settled,
    },
    // --- AnyResolver: bottom takes any support, deliberately. The discipline
    // on `Unknown` is that nobody may *downgrade* to it to hide a failure
    // (ADR-0001 §4, ADR-0014 §5.1); fencing honest failure would obstruct
    // without preventing any escalation.
    Route {
        authority: Authority::AnyResolver,
        outcome: Outcome::Unknown,
        support: SupportKind::KernelCertificate,
        condition: RouteCondition::None,
        status: RouteStatus::Settled,
    },
    Route {
        authority: Authority::AnyResolver,
        outcome: Outcome::Unknown,
        support: SupportKind::KernelRefutation,
        condition: RouteCondition::None,
        status: RouteStatus::Settled,
    },
    Route {
        authority: Authority::AnyResolver,
        outcome: Outcome::Unknown,
        support: SupportKind::Settlement,
        condition: RouteCondition::None,
        status: RouteStatus::Settled,
    },
    Route {
        authority: Authority::AnyResolver,
        outcome: Outcome::Unknown,
        support: SupportKind::Ground,
        condition: RouteCondition::None,
        status: RouteStatus::Settled,
    },
    Route {
        authority: Authority::AnyResolver,
        outcome: Outcome::Unknown,
        support: SupportKind::Measurement,
        condition: RouteCondition::None,
        status: RouteStatus::Settled,
    },
    Route {
        authority: Authority::AnyResolver,
        outcome: Outcome::Unknown,
        support: SupportKind::ExternalResult,
        condition: RouteCondition::None,
        status: RouteStatus::Settled,
    },
    Route {
        authority: Authority::AnyResolver,
        outcome: Outcome::Unknown,
        support: SupportKind::Suggestion,
        condition: RouteCondition::None,
        status: RouteStatus::Settled,
    },
    Route {
        authority: Authority::AnyResolver,
        outcome: Outcome::Unknown,
        support: SupportKind::TreeRealization,
        condition: RouteCondition::None,
        status: RouteStatus::Settled,
    },
];

/// Why a publication was refused. Every variant means **nothing was
/// constructed** — a refusal is never a downgraded outcome, never `Unknown`,
/// and never `Refuted` (ADR-0016 §5).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PublicationError {
    /// The claimed authority is not this outcome's sole authority
    /// ([`Outcome::authority`], ADR-0002 §4.1).
    WrongAuthority {
        outcome: Outcome,
        claimed: Authority,
        sole: Authority,
    },
    /// No route pairs this evidence kind with this outcome — the support
    /// cannot bear the claim.
    UnsupportedEvidence {
        authority: Authority,
        outcome: Outcome,
        support: SupportKind,
    },
    /// `Derived` presented with a replay-verified chain, or `Audited` with a
    /// merely recorded one (ADR-0002 §5.1/§5.2).
    DecompositionVerificationMismatch {
        outcome: Outcome,
        expected: DecompVerification,
        found: DecompVerification,
    },
    /// [`AuditedSource::verify`]: the presented artifact is not what the
    /// judgement's evidence id names.
    EvidenceBindingMismatch {
        expected: EvidenceId,
        found: EvidenceId,
    },
    /// [`AuditedSource::verify`]: the source judgement's outcome is not
    /// `Audited`, so it may not cross an elaboration boundary (ADR-0002 §5 ¶2).
    NotAudited { found: Outcome },
}

/// The route matching `(authority, outcome, support)`, or `None` if that
/// triple is illegal.
pub fn route_for(
    authority: Authority,
    outcome: Outcome,
    support: SupportKind,
) -> Option<&'static Route> {
    ROUTES
        .iter()
        .find(|r| r.authority == authority && r.outcome == outcome && r.support == support)
}

/// Check a claimed publication against [`ROUTES`] without constructing
/// anything. [`Judgement::publish`] is this plus the construction.
pub(crate) fn check_route(
    authority: Authority,
    outcome: Outcome,
    support: Support<'_>,
) -> Result<&'static Route, PublicationError> {
    let sole = outcome.authority();
    if authority != sole {
        return Err(PublicationError::WrongAuthority {
            outcome,
            claimed: authority,
            sole,
        });
    }

    let route = route_for(authority, outcome, support.kind()).ok_or(
        PublicationError::UnsupportedEvidence {
            authority,
            outcome,
            support: support.kind(),
        },
    )?;

    match (route.condition, support) {
        (RouteCondition::None, _) => Ok(route),
        (RouteCondition::Decomposition(expected), Support::Settlement(decomposition)) => {
            if decomposition.verification == expected {
                Ok(route)
            } else {
                Err(PublicationError::DecompositionVerificationMismatch {
                    outcome,
                    expected,
                    found: decomposition.verification,
                })
            }
        }
        // A decomposition condition on a non-settlement support is a table
        // authoring error, not a caller error. Refuse rather than pass.
        (RouteCondition::Decomposition(_), other) => Err(PublicationError::UnsupportedEvidence {
            authority,
            outcome,
            support: other.kind(),
        }),
    }
}

/// A verified elaboration source: a judgement that really is `Audited`, whose
/// evidence id really is the id of a presented, route-legal artifact
/// (ADR-0016 §6, audit finding A-2).
///
/// The binding in step 3 of [`AuditedSource::verify`] is what makes this
/// non-forgeable rather than merely well-typed. Without it a caller could hand
/// over a genuine `Audited` judgement alongside an *unrelated* verified
/// decomposition and elaborate the wrong claim.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AuditedSource {
    judgement: Judgement,
}

impl AuditedSource {
    /// Verify that `judgement` may cross an elaboration boundary on `support`.
    ///
    /// Three checks, in order, each failing closed:
    ///
    /// 1. `judgement.outcome == Outcome::Audited` — ADR-0002 §5 ¶2: "only
    ///    `Audited`-supported settlement evidence may enter an
    ///    `elaboration-boundary` edge."
    /// 2. `(AuditChecker, Audited, support.kind())` is a route in
    ///    ADR-0016 §4 and its condition holds — so a settlement support must
    ///    be replay-verified.
    /// 3. **The binding.** `support.evidence().id() == judgement.evidence`.
    pub fn verify(judgement: &Judgement, support: Support<'_>) -> Result<Self, PublicationError> {
        if judgement.outcome != Outcome::Audited {
            return Err(PublicationError::NotAudited {
                found: judgement.outcome,
            });
        }

        check_route(Authority::AuditChecker, Outcome::Audited, support)?;

        let found = support.evidence_id();
        if found != judgement.evidence {
            return Err(PublicationError::EvidenceBindingMismatch {
                expected: judgement.evidence,
                found,
            });
        }

        Ok(AuditedSource {
            judgement: *judgement,
        })
    }

    /// The verified source judgement.
    pub fn judgement(&self) -> &Judgement {
        &self.judgement
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ConfigId, GeneratorId};

    const ALL_OUTCOMES: [Outcome; 6] = [
        Outcome::Proven,
        Outcome::Refuted,
        Outcome::Derived,
        Outcome::Measured,
        Outcome::Unknown,
        Outcome::Audited,
    ];

    fn decomposition(verification: DecompVerification) -> Decomposition {
        let generators = vec![GeneratorId::named("g0@1")];
        let configs = vec![ConfigId::from_canon(b"x0"), ConfigId::from_canon(b"x1")];
        match verification {
            DecompVerification::Recorded => Decomposition::recorded(generators, configs),
            DecompVerification::ReplayVerified => {
                Decomposition::replay_verified(generators, configs)
            }
        }
        .expect("well-formed chain")
    }

    #[test]
    fn every_route_agrees_with_the_frozen_authority_table() {
        // ROUTES may never disagree with ADR-0002 §4.1: a route's authority is
        // exactly its outcome's sole authority, by construction.
        for route in ROUTES {
            assert_eq!(
                route.authority,
                route.outcome.authority(),
                "{route:?} contradicts Outcome::authority()"
            );
        }
    }

    #[test]
    fn every_outcome_has_at_least_one_route() {
        // Totality in the direction that matters: no outcome is unpublishable
        // by its own authority.
        for outcome in ALL_OUTCOMES {
            assert!(
                ROUTES.iter().any(|r| r.outcome == outcome),
                "{outcome:?} has no publication route"
            );
        }
    }

    #[test]
    fn only_the_tree_realization_route_is_provisional() {
        // Freeze the size of the reported hole (ADR-0016 §7). A second
        // provisional row must be a deliberate, reviewed act.
        let provisional: Vec<_> = ROUTES
            .iter()
            .filter(|r| r.status == RouteStatus::Provisional)
            .collect();
        assert_eq!(
            provisional.len(),
            1,
            "unexpected provisional routes: {provisional:?}"
        );
        assert_eq!(provisional[0].outcome, Outcome::Audited);
        assert_eq!(provisional[0].support, SupportKind::TreeRealization);
    }

    #[test]
    fn routes_are_unique_per_triple() {
        // A duplicated triple would make `route_for`'s first-match arbitrary.
        for (i, a) in ROUTES.iter().enumerate() {
            for b in &ROUTES[i + 1..] {
                assert!(
                    !(a.authority == b.authority
                        && a.outcome == b.outcome
                        && a.support == b.support),
                    "duplicate route triple: {a:?}"
                );
            }
        }
    }

    #[test]
    fn support_kind_agrees_with_evidence_projection() {
        // Settlement and TreeRealization deliberately share an Evidence
        // variant while staying distinct routing tags — the distinction the
        // id-only surface could not express (ADR-0016 §5).
        let verified = decomposition(DecompVerification::ReplayVerified);
        let settlement = Support::Settlement(&verified);
        let tree = Support::TreeRealization {
            body: Digest::of(brix_canon::Domain::Value, b"prop"),
        };
        assert!(matches!(
            settlement.evidence(),
            Evidence::SettlementReplay { .. }
        ));
        assert!(matches!(tree.evidence(), Evidence::SettlementReplay { .. }));
        assert_ne!(settlement.kind(), tree.kind());
    }

    #[test]
    fn decomposition_condition_is_checked_in_both_directions() {
        let recorded = decomposition(DecompVerification::Recorded);
        let verified = decomposition(DecompVerification::ReplayVerified);

        // Audited from a merely recorded chain: the hot loop may record, never
        // assert verification (ADR-0002 §4.1).
        let err = check_route(
            Authority::AuditChecker,
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

        // Derived from an already-verified chain: ADR-0002 §5.1's hot-loop
        // record is the unverified form.
        let err = check_route(
            Authority::SettlementKernel,
            Outcome::Derived,
            Support::Settlement(&verified),
        )
        .expect_err("a replay-verified chain must never be published as Derived");
        assert_eq!(
            err,
            PublicationError::DecompositionVerificationMismatch {
                outcome: Outcome::Derived,
                expected: DecompVerification::Recorded,
                found: DecompVerification::ReplayVerified,
            }
        );
    }
}
