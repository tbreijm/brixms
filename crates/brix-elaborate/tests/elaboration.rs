use brix_elaborate::{elaborate_and_publish, ElaborationResult};
use brix_kernel::{Budget, ExplicitTerm, Prop, TermKind, Var, Verdict};
use brix_semantic::{
    Authority, CertificateId, ContextId, EdgeKind, Evidence, Judgement, Outcome, PropositionId,
    VerifierId,
};

#[test]
fn positive_elaboration_produces_proven_judgement() {
    let context = ContextId::root();
    let prop_atom_id = PropositionId::from_canon(b"P");
    let prop_atom = Prop::Atom(prop_atom_id);
    let kernel_prop = Prop::Impl(Box::new(prop_atom.clone()), Box::new(prop_atom.clone()));

    // Source judgement (e.g. Audited settlement judgement)
    let source_prop_id = PropositionId::from_canon(b"P_implies_P");
    let source_evidence = Evidence::KernelCertificate {
        verifier: VerifierId::named("audit_verifier"),
        certificate: CertificateId::from_canon(b"source_audited_evidence"),
    };
    let source = Judgement::new(
        context,
        source_prop_id,
        Outcome::Audited,
        source_evidence.id(),
    );

    // Explicit term: \x. x (identity term proving P -> P)
    let term_kind = TermKind::Lam {
        var_name: Some("x".to_string()),
        body: Box::new(TermKind::Hyp(Var::Index(0))),
    };
    let term = ExplicitTerm::new(context, term_kind);
    let budget = Budget::new(100, 100);

    let result = elaborate_and_publish(&source, &kernel_prop, &term, budget);

    match result {
        ElaborationResult::Proven { judgement, edge } => {
            // Assert outcome is Proven and authority is ProofKernel
            assert_eq!(judgement.outcome, Outcome::Proven);
            assert_eq!(judgement.outcome.authority(), Authority::ProofKernel);
            assert_eq!(judgement.context, source.context);
            assert_eq!(judgement.proposition, source.proposition);

            // Assert edge is ElaborationBoundary pointing to source
            assert_eq!(edge.kind, EdgeKind::ElaborationBoundary);
            assert_eq!(edge.target, source.id().digest());

            // Assert evidence is a KernelCertificate naming brix-kernel verifier
            let verifier = VerifierId::named("brix.kernel@0.1");
            let expected_cert_payload = format!("{context:?}:{kernel_prop:?}:{term:?}");
            let expected_cert_id = CertificateId::from_canon(expected_cert_payload.as_bytes());
            let expected_evidence = Evidence::KernelCertificate {
                verifier,
                certificate: expected_cert_id,
            };
            assert_eq!(judgement.evidence, expected_evidence.id());
        }
        ElaborationResult::NotElaborated(verdict) => {
            panic!("Expected ElaborationResult::Proven, got NotElaborated({verdict:?})");
        }
    }
}

#[test]
fn negative_well_formed_but_wrong_term_yields_not_elaborated() {
    let context = ContextId::root();
    let p_id = PropositionId::from_canon(b"P");
    let q_id = PropositionId::from_canon(b"Q");

    // Proposition P -> Q
    let kernel_prop = Prop::Impl(Box::new(Prop::Atom(p_id)), Box::new(Prop::Atom(q_id)));

    let source_evidence = Evidence::KernelCertificate {
        verifier: VerifierId::named("audit_verifier"),
        certificate: CertificateId::from_canon(b"source_evidence"),
    };
    let source = Judgement::new(context, p_id, Outcome::Audited, source_evidence.id());

    // Wrong term: identity term \x. x has type P -> P, not P -> Q
    let term_kind = TermKind::Lam {
        var_name: Some("x".to_string()),
        body: Box::new(TermKind::Hyp(Var::Index(0))),
    };
    let term = ExplicitTerm::new(context, term_kind);
    let budget = Budget::new(100, 100);

    let result = elaborate_and_publish(&source, &kernel_prop, &term, budget);

    match result {
        ElaborationResult::NotElaborated(verdict) => {
            assert!(
                matches!(verdict, Verdict::Rejected(_)),
                "Expected Verdict::Rejected, got {verdict:?}"
            );
        }
        ElaborationResult::Proven { .. } => {
            panic!("Resolver term was wrong, but elaborate_and_publish yielded Proven!");
        }
    }
}

#[test]
fn budget_exhaustion_yields_not_elaborated_resource_exhausted() {
    let context = ContextId::root();
    let prop_atom_id = PropositionId::from_canon(b"P");
    let prop_atom = Prop::Atom(prop_atom_id);
    let kernel_prop = Prop::Impl(Box::new(prop_atom.clone()), Box::new(prop_atom.clone()));

    let source_evidence = Evidence::KernelCertificate {
        verifier: VerifierId::named("audit_verifier"),
        certificate: CertificateId::from_canon(b"source_evidence"),
    };
    let source = Judgement::new(
        context,
        prop_atom_id,
        Outcome::Audited,
        source_evidence.id(),
    );

    let term_kind = TermKind::Lam {
        var_name: Some("x".to_string()),
        body: Box::new(TermKind::Hyp(Var::Index(0))),
    };
    let term = ExplicitTerm::new(context, term_kind);

    // Zero step budget => ResourceExhausted
    let budget = Budget::new(0, 100);

    let result = elaborate_and_publish(&source, &kernel_prop, &term, budget);

    match result {
        ElaborationResult::NotElaborated(verdict) => {
            assert!(
                matches!(verdict, Verdict::ResourceExhausted(_)),
                "Expected Verdict::ResourceExhausted, got {verdict:?}"
            );
        }
        ElaborationResult::Proven { .. } => {
            panic!("Budget was zero, but elaborate_and_publish yielded Proven!");
        }
    }
}
