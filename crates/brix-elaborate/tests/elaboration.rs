use brix_elaborate::{
    elaborate_and_publish, elaborate_decomposition, ElaborationResult, RealizesTree, TreeObj,
};
use brix_kernel::{Budget, ExplicitTerm, ObjectTerm, Prop, TermKind, Var, Verdict};
use brix_semantic::{
    AuditedSource, Authority, ConfigId, ContextId, Decomposition, EdgeKind, Evidence, GeneratorId,
    GeneratorRegistry, GeneratorSemantics, Judgement, Outcome, PropositionId, Support,
    TreeDerivation,
};

/// A `StructureVerified` single-leaf tree derivation — the typing lane's
/// support for `Audited` (ADR-0017). Used here purely as a generic
/// `AuditChecker`/`Audited` source for `elaborate_and_publish`, which does
/// not itself inspect the tree; the tests that exercise tree structure live
/// in `elaboration_fence.rs`.
fn tree_derivation(tag: &str) -> TreeDerivation {
    let g = GeneratorId::named(&format!("elaboration.{tag}.g@1"));
    let x0 = ConfigId::from_canon(format!("elaboration.{tag}.x0").as_bytes());
    let x1 = ConfigId::from_canon(format!("elaboration.{tag}.x1").as_bytes());
    TreeDerivation::structure_verified(RealizesTree::Leaf {
        generator: g,
        src: TreeObj::Atom(x0),
        dst: TreeObj::Atom(x1),
    })
}

#[test]
fn positive_elaboration_produces_proven_judgement() {
    let context = ContextId::root();
    let prop_atom_id = PropositionId::from_canon(b"P");
    let prop_atom = Prop::Atom(prop_atom_id);
    let kernel_prop = Prop::Impl(Box::new(prop_atom.clone()), Box::new(prop_atom.clone()));

    // Source judgement (e.g. Audited typing judgement), published via the
    // tree lane's route (ADR-0017) and bound to its own proposition, since
    // this test exercises `elaborate_and_publish` directly.
    let source_prop_id = PropositionId::from_canon(b"P_implies_P");
    let derivation = tree_derivation("positive-elaboration");
    let source = Judgement::publish(
        Authority::AuditChecker,
        context,
        source_prop_id,
        Outcome::Audited,
        Support::Tree(&derivation),
    )
    .expect("AuditChecker/Audited/Tree(StructureVerified) is a legal route");
    let audited_source = AuditedSource::verify(&source, Support::Tree(&derivation))
        .expect("source binds to its own tree derivation support");

    // Explicit term: \x. x (identity term proving P -> P)
    let term_kind = TermKind::Lam {
        var_name: Some("x".to_string()),
        body: Box::new(TermKind::Hyp(Var::Index(0))),
    };
    let term = ExplicitTerm::new(context, term_kind);
    let budget = Budget::new(100, 100);

    let result = elaborate_and_publish(&audited_source, &kernel_prop, &term, budget);

    match result {
        ElaborationResult::Proven { judgement, edge } => {
            // Assert outcome is Proven and authority is ProofKernel
            assert_eq!(judgement.outcome, Outcome::Proven);
            assert_eq!(judgement.outcome.authority(), Authority::ProofKernel);
            assert_eq!(judgement.context, source.context);
            assert_eq!(judgement.proposition, kernel_prop.proposition_id());
            assert_ne!(judgement.proposition, source.proposition);

            // Assert edge is ElaborationBoundary pointing to source
            assert_eq!(edge.kind, EdgeKind::ElaborationBoundary);
            assert_eq!(edge.target, source.id().digest());

            // Assert evidence is a KernelCertificate naming brix-kernel verifier
            let verifier = brix_kernel::native_verifier();
            let expected_cert_id = brix_kernel::certificate_id_v1(
                &brix_kernel::CertificateMaterialV1::new(&context, &kernel_prop, &term),
            );
            let expected_evidence = Evidence::KernelCertificate {
                verifier,
                certificate: expected_cert_id,
            };
            assert_eq!(judgement.evidence, expected_evidence.id());
        }
        ElaborationResult::NotElaborated(verdict) => {
            panic!("Expected ElaborationResult::Proven, got NotElaborated({verdict:?})");
        }
        ElaborationResult::Refused(err) => {
            panic!("Expected ElaborationResult::Proven, got Refused({err:?})");
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

    let derivation = tree_derivation("negative-well-formed-wrong-term");
    let source = Judgement::publish(
        Authority::AuditChecker,
        context,
        p_id,
        Outcome::Audited,
        Support::Tree(&derivation),
    )
    .expect("AuditChecker/Audited/Tree(StructureVerified) is a legal route");
    let audited_source = AuditedSource::verify(&source, Support::Tree(&derivation))
        .expect("source binds to its own tree derivation support");

    // Wrong term: identity term \x. x has type P -> P, not P -> Q
    let term_kind = TermKind::Lam {
        var_name: Some("x".to_string()),
        body: Box::new(TermKind::Hyp(Var::Index(0))),
    };
    let term = ExplicitTerm::new(context, term_kind);
    let budget = Budget::new(100, 100);

    let result = elaborate_and_publish(&audited_source, &kernel_prop, &term, budget);

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
        ElaborationResult::Refused(err) => {
            panic!("Resolver term was wrong, but elaborate_and_publish yielded Refused({err:?})!");
        }
    }
}

#[test]
fn budget_exhaustion_yields_not_elaborated_resource_exhausted() {
    let context = ContextId::root();
    let prop_atom_id = PropositionId::from_canon(b"P");
    let prop_atom = Prop::Atom(prop_atom_id);
    let kernel_prop = Prop::Impl(Box::new(prop_atom.clone()), Box::new(prop_atom.clone()));

    let derivation = tree_derivation("budget-exhaustion");
    let source = Judgement::publish(
        Authority::AuditChecker,
        context,
        prop_atom_id,
        Outcome::Audited,
        Support::Tree(&derivation),
    )
    .expect("AuditChecker/Audited/Tree(StructureVerified) is a legal route");
    let audited_source = AuditedSource::verify(&source, Support::Tree(&derivation))
        .expect("source binds to its own tree derivation support");

    let term_kind = TermKind::Lam {
        var_name: Some("x".to_string()),
        body: Box::new(TermKind::Hyp(Var::Index(0))),
    };
    let term = ExplicitTerm::new(context, term_kind);

    // Zero step budget => ResourceExhausted
    let budget = Budget::new(0, 100);

    let result = elaborate_and_publish(&audited_source, &kernel_prop, &term, budget);

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
        ElaborationResult::Refused(err) => {
            panic!("Budget was zero, but elaborate_and_publish yielded Refused({err:?})!");
        }
    }
}

#[test]
fn decomposition_n2_well_formed_produces_proven() {
    let context = ContextId::root();
    let source_prop_id = PropositionId::from_canon(b"audited_decomp_prop");

    let g1 = GeneratorId::named("g1@1");
    let g2 = GeneratorId::named("g2@1");
    let x0 = ConfigId::from_canon(b"x0");
    let x1 = ConfigId::from_canon(b"x1");
    let x2 = ConfigId::from_canon(b"x2");

    let decomp = verified_chain(vec![g1, g2], vec![x0, x1, x2]);

    // The source must be Audited from *this* decomp: `elaborate_decomposition`
    // binds the source's evidence to the decomp presented alongside it
    // (ADR-0016 §6).
    let source = Judgement::publish(
        Authority::AuditChecker,
        context,
        source_prop_id,
        Outcome::Audited,
        Support::Settlement(&decomp),
    )
    .expect("AuditChecker/Audited/Settlement(ReplayVerified) is a legal route");

    let budget = Budget::new(100, 100);
    let result = elaborate_decomposition(&source, &decomp, budget);

    match result {
        ElaborationResult::Proven { judgement, edge } => {
            assert_eq!(judgement.outcome, Outcome::Proven);
            assert_eq!(judgement.outcome.authority(), Authority::ProofKernel);
            assert_eq!(judgement.context, source.context);
            assert_eq!(edge.kind, EdgeKind::ElaborationBoundary);
            assert_eq!(edge.target, source.id().digest());
        }
        ElaborationResult::NotElaborated(verdict) => {
            panic!("Expected Proven, got NotElaborated({verdict:?})");
        }
        ElaborationResult::Refused(err) => {
            panic!("Expected Proven, got Refused({err:?})");
        }
    }
}

#[test]
fn decomposition_n3_well_formed_produces_proven() {
    let context = ContextId::root();
    let source_prop_id = PropositionId::from_canon(b"audited_decomp_prop_n3");

    let g1 = GeneratorId::named("g1@1");
    let g2 = GeneratorId::named("g2@1");
    let g3 = GeneratorId::named("g3@1");
    let x0 = ConfigId::from_canon(b"x0");
    let x1 = ConfigId::from_canon(b"x1");
    let x2 = ConfigId::from_canon(b"x2");
    let x3 = ConfigId::from_canon(b"x3");

    let decomp = verified_chain(vec![g1, g2, g3], vec![x0, x1, x2, x3]);

    let source = Judgement::publish(
        Authority::AuditChecker,
        context,
        source_prop_id,
        Outcome::Audited,
        Support::Settlement(&decomp),
    )
    .expect("AuditChecker/Audited/Settlement(ReplayVerified) is a legal route");

    let budget = Budget::new(100, 100);
    let result = elaborate_decomposition(&source, &decomp, budget);

    match result {
        ElaborationResult::Proven { judgement, edge } => {
            assert_eq!(judgement.outcome, Outcome::Proven);
            assert_eq!(judgement.outcome.authority(), Authority::ProofKernel);
            assert_eq!(judgement.context, source.context);
            assert_eq!(edge.kind, EdgeKind::ElaborationBoundary);
            assert_eq!(edge.target, source.id().digest());
        }
        ElaborationResult::NotElaborated(verdict) => {
            panic!("Expected Proven, got NotElaborated({verdict:?})");
        }
        ElaborationResult::Refused(err) => {
            panic!("Expected Proven, got Refused({err:?})");
        }
    }
}

#[test]
fn decomposition_n1_single_generator_produces_proven() {
    let context = ContextId::root();
    let source_prop_id = PropositionId::from_canon(b"audited_decomp_prop_n1");

    let g1 = GeneratorId::named("g1@1");
    let x0 = ConfigId::from_canon(b"x0");
    let x1 = ConfigId::from_canon(b"x1");

    let decomp = verified_chain(vec![g1], vec![x0, x1]);

    let source = Judgement::publish(
        Authority::AuditChecker,
        context,
        source_prop_id,
        Outcome::Audited,
        Support::Settlement(&decomp),
    )
    .expect("AuditChecker/Audited/Settlement(ReplayVerified) is a legal route");

    let budget = Budget::new(100, 100);
    let result = elaborate_decomposition(&source, &decomp, budget);

    match result {
        ElaborationResult::Proven { judgement, edge } => {
            assert_eq!(judgement.outcome, Outcome::Proven);
            assert_eq!(judgement.outcome.authority(), Authority::ProofKernel);
            assert_eq!(judgement.context, source.context);
            assert_eq!(edge.kind, EdgeKind::ElaborationBoundary);
            assert_eq!(edge.target, source.id().digest());
        }
        ElaborationResult::NotElaborated(verdict) => {
            panic!("Expected Proven, got NotElaborated({verdict:?})");
        }
        ElaborationResult::Refused(err) => {
            panic!("Expected Proven, got Refused({err:?})");
        }
    }
}

#[test]
fn broken_chain_steps_do_not_connect_yields_not_elaborated() {
    let context = ContextId::root();
    let source_prop_id = PropositionId::from_canon(b"audited_decomp_prop_broken");
    let derivation = tree_derivation("broken-chain-steps");
    let source = Judgement::publish(
        Authority::AuditChecker,
        context,
        source_prop_id,
        Outcome::Audited,
        Support::Tree(&derivation),
    )
    .expect("AuditChecker/Audited/Tree(StructureVerified) is a legal route");
    let audited_source = AuditedSource::verify(&source, Support::Tree(&derivation))
        .expect("source binds to its own tree derivation support");

    let g1 = ObjectTerm::Const(PropositionId(GeneratorId::named("g1@1").digest()));
    let g2 = ObjectTerm::Const(PropositionId(GeneratorId::named("g2@1").digest()));
    let x0 = ObjectTerm::Const(PropositionId(ConfigId::from_canon(b"x0").digest()));
    let x1 = ObjectTerm::Const(PropositionId(ConfigId::from_canon(b"x1").digest()));
    let x1_mismatch =
        ObjectTerm::Const(PropositionId(ConfigId::from_canon(b"x1_mismatch").digest()));
    let x2 = ObjectTerm::Const(PropositionId(ConfigId::from_canon(b"x2").digest()));

    // H1: Realizes(g1, x0, x1)
    // H2: Realizes(g2, x1_mismatch, x2) -- g2's source (x1_mismatch) != g1's target (x1)
    let h1 = Prop::Realizes(g1.clone(), x0.clone(), x1.clone());
    let h2 = Prop::Realizes(g2.clone(), x1_mismatch, x2.clone());

    let k_term = ObjectTerm::Compose(Box::new(g2), Box::new(g1));
    let goal_prop = Prop::Realizes(k_term, x0, x2);
    let implication_prop = Prop::Impl(
        Box::new(h1),
        Box::new(Prop::Impl(Box::new(h2), Box::new(goal_prop))),
    );

    // Proof term trying to compose h1 and h2
    let body = TermKind::RealizesComp {
        left: Box::new(TermKind::Hyp(Var::Index(1))),
        right: Box::new(TermKind::Hyp(Var::Index(0))),
    };
    let term_kind = TermKind::Lam {
        var_name: Some("h1".to_string()),
        body: Box::new(TermKind::Lam {
            var_name: Some("h2".to_string()),
            body: Box::new(body),
        }),
    };
    let term = ExplicitTerm::new(context, term_kind);

    let budget = Budget::new(100, 100);
    let result = elaborate_and_publish(&audited_source, &implication_prop, &term, budget);

    match result {
        ElaborationResult::NotElaborated(verdict) => {
            assert!(
                matches!(verdict, Verdict::Rejected(_)),
                "Expected Verdict::Rejected due to middle-match failure, got {verdict:?}"
            );
        }
        ElaborationResult::Proven { .. } => {
            panic!("Broken chain steps do not connect, but yielded Proven!");
        }
        ElaborationResult::Refused(err) => {
            panic!("Broken chain steps do not connect, but yielded Refused({err:?})!");
        }
    }
}

/// Earn a `ReplayVerified` chain the honest way (ADR-0019 D7): build the
/// registry, supply a semantics that realizes exactly this chain's links, and
/// run the **real** checked transition. No test constructs a verified
/// artifact by assertion any more — the stamp constructor is gone.
///
/// This fixture semantics accepts precisely the chain it is handed, which is
/// what a fixture should do: the independent negatives (a padded chain, a
/// corrupted intermediate config, an unregistered generator) live beside
/// `verify_replay` itself in `brix-semantic`.
fn verified_chain(generators: Vec<GeneratorId>, configs: Vec<ConfigId>) -> Decomposition {
    struct ExactChain {
        links: Vec<(GeneratorId, ConfigId, ConfigId)>,
    }
    impl GeneratorSemantics for ExactChain {
        fn realizes(&self, g: &GeneratorId, src: &ConfigId, dst: &ConfigId) -> bool {
            self.links
                .iter()
                .any(|(a, b, c)| a == g && b == src && c == dst)
        }
    }

    let mut registry = GeneratorRegistry::new();
    for g in &generators {
        registry.insert(*g);
    }
    let links = generators
        .iter()
        .enumerate()
        .map(|(i, g)| (*g, configs[i], configs[i + 1]))
        .collect();

    Decomposition::recorded(generators, configs)
        .expect("well-formed fixture chain")
        .verify_replay(&registry, &ExactChain { links })
        .expect("an honest fixture chain earns the tag")
}
