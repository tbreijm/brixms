use brix_canon::{CanonWriter, Digest, Domain};
use brix_elaborate::{elaborate_decomposition, ElaborationResult};
use brix_kernel::Budget;
use brix_semantic::{
    compose_chain, Authority, ConfigId, ContextId, Decomposition, EdgeKind, Evidence, GeneratorId,
    GeneratorRegistry, Outcome, Realizes,
};
use soc_core::{
    audit_step, commit_tick, AdmAll, AuditResult, AuditedStep, Candidate, CommitError, ExecConfig,
    GeneratorSemantics, Handle, History, Interner, Key, Regime, SettlementRegime,
};

/// A multi-generator (n=2) settlement regime fixture.
struct MultiGenFixtureRegime {
    regime_handle: Handle,
    witness_handle: Handle,
    successor_handle: Handle,
    generators: Vec<GeneratorId>,
    configs: Vec<ConfigId>,
}

impl Regime for MultiGenFixtureRegime {
    fn candidates(&self, _e: &ExecConfig) -> Vec<Candidate> {
        vec![Candidate {
            regime: self.regime_handle,
            witness: self.witness_handle,
            successor: self.successor_handle,
        }]
    }
}

impl SettlementRegime for MultiGenFixtureRegime {
    fn try_decompose(&self, _e: &ExecConfig, _c: &Candidate) -> Result<Decomposition, CommitError> {
        Ok(
            Decomposition::recorded(self.generators.clone(), self.configs.clone())
                .expect("valid recorded decomposition"),
        )
    }
}

/// A generator semantics fixture that accepts any step transition.
struct AlwaysRealizesSemantics;

impl GeneratorSemantics for AlwaysRealizesSemantics {
    fn realizes(&self, _g: &GeneratorId, _src: &ConfigId, _dst: &ConfigId) -> bool {
        true
    }
}

fn candidate_tiebreak(c: &Candidate) -> Digest {
    let mut w = CanonWriter::new();
    w.write_uint(c.witness.raw() as u64);
    w.write_uint(c.successor.raw() as u64);
    w.digest(Domain::Value)
}

#[test]
fn test_b3_end_to_end_audited_decomposition_to_proven() {
    // 1. Drive soc-core to produce a REAL committed + audited step
    let mut interner = Interner::new();
    let world_handle = interner.intern(Digest::of(Domain::Value, b"world_0"));
    let policy_handle = interner.intern(Digest::of(Domain::Value, b"policy_0"));
    let regime_handle = interner.intern(Digest::of(Domain::Value, b"regime_0"));
    let witness_handle = interner.intern(Digest::of(Domain::Value, b"witness_0"));
    let successor_handle = interner.intern(Digest::of(Domain::Value, b"world_1"));

    let exec_config = ExecConfig::new(world_handle, policy_handle, History::empty().digest());
    let context = ContextId::root();

    let g1 = GeneratorId::named("b3.step@1");
    let g2 = GeneratorId::named("b3.step@2");

    let x0 = ConfigId(interner.resolve(exec_config.world));
    let x1 = ConfigId::from_canon(b"b3.config@1");
    let x2 = ConfigId(interner.resolve(successor_handle));

    let fixture_regime = MultiGenFixtureRegime {
        regime_handle,
        witness_handle,
        successor_handle,
        generators: vec![g1, g2],
        configs: vec![x0, x1, x2],
    };

    let regimes: Vec<&dyn SettlementRegime> = vec![&fixture_regime];

    // Call commit_tick to get the committed step (Outcome::Derived)
    let (committed, step_opt, _cost) = commit_tick(
        &regimes,
        &AdmAll,
        &interner,
        &exec_config,
        context,
        0,
        &mut |c, phase| Key::new(phase, 0, candidate_tiebreak(c)),
    );

    assert!(matches!(committed, soc_core::Committed::Step { .. }));
    let committed_step = step_opt.expect("commit_tick should return a committed step");

    // Populate GeneratorRegistry for audit_step
    let mut registry = GeneratorRegistry::new();
    registry.insert(g1);
    registry.insert(g2);

    // Audit the committed step to get Audited judgement + replay-verified Decomposition
    let audit_res = audit_step(
        &committed_step,
        context,
        &registry,
        &AlwaysRealizesSemantics,
    );
    let AuditedStep {
        audited: audited_judgement,
        verified: decomposition,
        ..
    } = match audit_res {
        AuditResult::Audited(step_box) => *step_box,
        AuditResult::Unknown(reason) => panic!("Audit failed unexpectedly: {reason}"),
    };

    assert_eq!(audited_judgement.outcome, Outcome::Audited);

    // 2. Feed real Audited judgement + replay-verified Decomposition into elaborate_decomposition
    let budget = Budget::new(1000, 1000);
    let elaboration_res = elaborate_decomposition(&audited_judgement, &decomposition, budget);

    // 3. ASSERT:
    // - result is ElaborationResult::Proven { judgement, edge }
    // - judgement.outcome == Outcome::Proven
    // - Outcome::Proven.authority() == Authority::ProofKernel
    // - evidence is a KernelCertificate
    // - edge is EdgeKind::ElaborationBoundary FROM Proven judgement TO audited judgement's id
    // - committed witness in the audited proposition == compose_chain(&decomposition.generators).unwrap()
    match elaboration_res {
        ElaborationResult::Proven { judgement, edge } => {
            assert_eq!(judgement.outcome, Outcome::Proven);
            assert_eq!(judgement.outcome.authority(), Authority::ProofKernel);
            assert_eq!(judgement.context, audited_judgement.context);

            // Verify evidence is a KernelCertificate
            // EvidenceId matches Evidence::KernelCertificate
            let expected_verifier = brix_kernel::native_verifier();
            // Check that judgement.evidence matches a KernelCertificate by building expected evidence
            // Or verifying that it matches Evidence::KernelCertificate with expected_verifier
            let witness_chain = compose_chain(&decomposition.generators).unwrap();

            // Assert committed witness in the audited proposition == compose_chain(&decomposition.generators).unwrap()
            let expected_audited_prop = Realizes::new(witness_chain, x0, x2).proposition_id();
            assert_eq!(audited_judgement.proposition, expected_audited_prop);
            assert_eq!(committed_step.witness, witness_chain);

            // Edge assertion: EdgeKind::ElaborationBoundary FROM Proven judgement TO audited judgement's id
            assert_eq!(edge.kind, EdgeKind::ElaborationBoundary);
            assert_eq!(edge.target, audited_judgement.id().digest());

            // Build kernel certificate id payload to assert evidence identity matches exact expected KernelCertificate
            let h1 = brix_kernel::Prop::Realizes(
                brix_kernel::ObjectTerm::Const(brix_semantic::PropositionId(g1.digest())),
                brix_kernel::ObjectTerm::Const(brix_semantic::PropositionId(x0.digest())),
                brix_kernel::ObjectTerm::Const(brix_semantic::PropositionId(x1.digest())),
            );
            let h2 = brix_kernel::Prop::Realizes(
                brix_kernel::ObjectTerm::Const(brix_semantic::PropositionId(g2.digest())),
                brix_kernel::ObjectTerm::Const(brix_semantic::PropositionId(x1.digest())),
                brix_kernel::ObjectTerm::Const(brix_semantic::PropositionId(x2.digest())),
            );
            let k_term = brix_kernel::ObjectTerm::Compose(
                Box::new(brix_kernel::ObjectTerm::Const(
                    brix_semantic::PropositionId(g2.digest()),
                )),
                Box::new(brix_kernel::ObjectTerm::Const(
                    brix_semantic::PropositionId(g1.digest()),
                )),
            );
            let goal_prop = brix_kernel::Prop::Realizes(
                k_term,
                brix_kernel::ObjectTerm::Const(brix_semantic::PropositionId(x0.digest())),
                brix_kernel::ObjectTerm::Const(brix_semantic::PropositionId(x2.digest())),
            );
            let implication_prop = brix_kernel::Prop::Impl(
                Box::new(h1),
                Box::new(brix_kernel::Prop::Impl(Box::new(h2), Box::new(goal_prop))),
            );

            let body = brix_kernel::TermKind::RealizesComp {
                left: Box::new(brix_kernel::TermKind::Hyp(brix_kernel::Var::Index(1))),
                right: Box::new(brix_kernel::TermKind::Hyp(brix_kernel::Var::Index(0))),
            };
            let term_kind = brix_kernel::TermKind::Lam {
                var_name: Some("h1".to_string()),
                body: Box::new(brix_kernel::TermKind::Lam {
                    var_name: Some("h2".to_string()),
                    body: Box::new(body),
                }),
            };
            let explicit_term = brix_kernel::ExplicitTerm::new(context, term_kind);

            let cert_id = brix_kernel::certificate_id_v1(&brix_kernel::CertificateMaterialV1::new(
                &context,
                &implication_prop,
                &explicit_term,
            ));
            let expected_evidence = Evidence::KernelCertificate {
                verifier: expected_verifier,
                certificate: cert_id,
            };

            assert_eq!(judgement.evidence, expected_evidence.id());
            assert_eq!(judgement.proposition, implication_prop.proposition_id());
        }
        ElaborationResult::NotElaborated(verdict) => {
            panic!("Expected Proven, got NotElaborated({verdict:?})");
        }
        ElaborationResult::Refused(err) => {
            panic!("Expected Proven, got Refused({err:?})");
        }
    }
}
