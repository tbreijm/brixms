//! The judgment-core artifacts assemble into a complete, content-addressed
//! [`Judgement`] with typed provenance — the shape stage 2 (#53) projects a
//! `brix.type` `HasType` derivation into.

use brix_canon::{CanonWriter, Canonical, Digest, Domain};
use brix_semantic::{
    CertificateId, ContextId, Dependency, Durability, EdgeKind, Evidence, Judgement, Outcome,
    PropositionId, VerifierId,
};

fn digest(tag: &[u8]) -> Digest {
    Digest::of(Domain::Value, tag)
}

/// The ADR §8.2 first specimen: a `HasType` judgement in the root context,
/// `Derived`, supported by a settlement replay.
fn has_type_specimen() -> Judgement {
    let context = ContextId::root();
    let proposition = PropositionId::from_canon(b"HasType(subject=x, ty=Int)");
    let evidence = Evidence::SettlementReplay {
        body: digest(b"TypeOfRoleBinding step"),
    }
    .id();
    Judgement::new(context, proposition, Outcome::Derived, evidence)
}

#[test]
fn judgement_id_is_deterministic() {
    assert_eq!(has_type_specimen().id(), has_type_specimen().id());
}

#[test]
fn judgement_id_depends_on_exactly_its_four_fields() {
    let base = has_type_specimen();
    let base_id = base.id();

    // Changing the outcome (Derived -> Proven) is a different judgement.
    let promoted = Judgement::new(
        base.context,
        base.proposition,
        Outcome::Proven,
        base.evidence,
    );
    assert_ne!(promoted.id(), base_id);

    // Changing the proposition is a different judgement.
    let other_prop = Judgement::new(
        base.context,
        PropositionId::from_canon(b"HasType(subject=x, ty=Bool)"),
        base.outcome,
        base.evidence,
    );
    assert_ne!(other_prop.id(), base_id);

    // Changing the evidence is a different judgement (same conclusion, other
    // support) — evidence is part of identity; only *search* is excluded.
    let other_ev = Judgement::new(
        base.context,
        base.proposition,
        base.outcome,
        Evidence::SettlementReplay {
            body: digest(b"a different derivation"),
        }
        .id(),
    );
    assert_ne!(other_ev.id(), base_id);

    // Changing the context is a different judgement.
    let other_ctx = Judgement::new(
        ContextId::from_canon(b"a scoped assumption world"),
        base.proposition,
        base.outcome,
        base.evidence,
    );
    assert_ne!(other_ctx.id(), base_id);
}

#[test]
fn kernel_evidence_is_durable_everything_else_is_revision_scoped() {
    let cert = || CertificateId::from_canon(b"explicit term");
    let verifier = VerifierId::named("brix.kernel@0.1");
    for durable in [
        Evidence::KernelCertificate {
            verifier,
            certificate: cert(),
        },
        Evidence::KernelRefutation {
            verifier,
            certificate: cert(),
        },
    ] {
        assert_eq!(durable.durability(), Durability::Durable);
    }
    for scoped in [
        Evidence::GroundAssertion { body: digest(b"g") },
        Evidence::SettlementReplay { body: digest(b"s") },
        Evidence::Measurement { body: digest(b"m") },
        Evidence::Suggestion { body: digest(b"u") },
        Evidence::CertifiedExternalResult { body: digest(b"e") },
    ] {
        assert_eq!(scoped.durability(), Durability::RevisionScoped);
    }
}

#[test]
fn a_certificate_names_its_verifier() {
    // The Lean/mathlib requirement: a durable certificate is attributable to
    // *which* kernel certified it, so two verifiers yield distinct evidence.
    let cert = CertificateId::from_canon(b"same term");
    let by_native = Evidence::KernelCertificate {
        verifier: VerifierId::named("brix.kernel@0.1"),
        certificate: cert,
    };
    let by_lean = Evidence::KernelCertificate {
        verifier: VerifierId::named("lean@4"),
        certificate: cert,
    };
    assert_ne!(
        VerifierId::named("brix.kernel@0.1"),
        VerifierId::named("lean@4")
    );
    assert_ne!(by_native.id(), by_lean.id());
}

#[test]
fn only_elaboration_boundary_edges_report_as_the_boundary() {
    let target = digest(b"a settlement support");
    assert!(Dependency::new(EdgeKind::ElaborationBoundary, target).is_elaboration_boundary());
    for kind in [
        EdgeKind::Premise,
        EdgeKind::Assumption,
        EdgeKind::Revision,
        EdgeKind::Rule,
        EdgeKind::Checker,
    ] {
        assert!(!Dependency::new(kind, target).is_elaboration_boundary());
    }
}

#[test]
fn evidence_canon_ordinals_are_stable() {
    // Freeze the wire ordinals — a reorder silently changes every EvidenceId.
    let cases: [(Evidence, u64); 7] = [
        (
            Evidence::KernelCertificate {
                verifier: VerifierId::named("k"),
                certificate: CertificateId::from_canon(b"c"),
            },
            0,
        ),
        (
            Evidence::KernelRefutation {
                verifier: VerifierId::named("k"),
                certificate: CertificateId::from_canon(b"c"),
            },
            1,
        ),
        (Evidence::GroundAssertion { body: digest(b"b") }, 2),
        (Evidence::SettlementReplay { body: digest(b"b") }, 3),
        (Evidence::Measurement { body: digest(b"b") }, 4),
        (Evidence::Suggestion { body: digest(b"b") }, 5),
        (Evidence::CertifiedExternalResult { body: digest(b"b") }, 6),
    ];
    for (ev, ordinal) in cases {
        let mut got = CanonWriter::new();
        ev.canon_write(&mut got);
        // The encoding must start with the frozen enum ordinal.
        let mut head = CanonWriter::new();
        head.write_enum(ordinal, |_| {});
        let head = head.finish();
        assert_eq!(
            &got.finish()[..head.len()],
            &head[..],
            "evidence ordinal {ordinal} drifted"
        );
    }
}
