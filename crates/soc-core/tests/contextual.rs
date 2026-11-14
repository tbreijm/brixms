use brix_canon::{Digest, Domain};
use brix_semantic::*;
use soc_core::contextual::{
    check_quiescence_in_context, NoExternalAssumptions, ResolutionContextV2,
};
use soc_core::saturate::*;
use soc_core::{AdmAll, Candidate, ExecConfig, History, Interner};
use std::collections::BTreeSet;
fn dimensions(parent: Option<ContextId>) -> ContextDimensionsV2 {
    ContextDimensionsV2 {
        parent,
        program: None,
        world: None,
        namespace: "n".into(),
        tenant: "t".into(),
        principal: "p".into(),
        capability_view: "c".into(),
        scenario_mode: ScenarioModeV2::Production,
        branch: None,
        valid_time_start: 1,
        valid_time_end: 9,
        transaction_time: None,
        correction_time: None,
        simulation_time: None,
        observation_profile: "obs@1".into(),
        redaction_profile: "redact@1".into(),
        kernel_profile: "kernel@1".into(),
        regime_profile: "regime@1".into(),
        axiom_profile: "axiom@1".into(),
        validity_start: 1,
        validity_end: 9,
        step_budget: 100,
        depth_budget: 10,
    }
}

struct Store(Vec<u8>);
impl ContextResolverV2 for Store {
    fn context_bytes(&self, _: &ContextId) -> Option<Vec<u8>> {
        None
    }
    fn artifact_bytes(&self, _: &ArtifactRefV2) -> Option<Vec<u8>> {
        Some(self.0.clone())
    }
}

struct EmptyRegime;
impl soc_core::WitnessProvider for EmptyRegime {
    fn candidates(&self, _: &ExecConfig) -> Vec<Candidate> {
        vec![]
    }
}
impl soc_core::SettlementWitnessProvider for EmptyRegime {
    fn try_decompose(
        &self,
        _: &ExecConfig,
        _: &Candidate,
    ) -> Result<Decomposition, soc_core::CommitError> {
        panic!("an empty frontier has no candidate to decompose")
    }
}

#[test]
fn materialized_discovery_and_quiescence_bind_exact_context_and_observation() {
    let program = b"presentation-v1".to_vec();
    let program_id = Digest::of(Domain::Value, &program);
    let store = Store(program);
    let profile = GeneratorPartitionProfile::all_realizing(BTreeSet::new());
    let mut d = dimensions(None);
    d.program = Some(ArtifactRefV2 {
        kind: "soc.presentation@1".into(),
        digest: program_id,
    });
    d.regime_profile = SATURATION_PROFILE_V1.into();
    d.axiom_profile = "none".into();
    d.kernel_profile = "none".into();
    d.observation_profile = profile.id().0.to_hex();
    let model = ContextModelV2::new(d.clone(), vec![]).unwrap();
    let bytes = encode_context_model_v2(&model);
    let request = ResolutionContextV2::validate(
        &bytes,
        model.id(),
        &d,
        ModelLimits::default(),
        &store,
        &NoExternalAssumptions,
    )
    .unwrap();
    let mut interner = Interner::new();
    let world = interner.intern(Digest::of(Domain::Value, b"world"));
    let policy = interner.intern(Digest::of(Domain::Value, b"policy"));
    let exec = ExecConfig::new(world, policy, History::empty().digest());
    let regimes: &[&dyn soc_core::SettlementWitnessProvider] = &[&EmptyRegime];
    let mut presentation = PresentationV1 {
        id: PresentationIdV1(program_id),
        regimes,
        regime_set: Digest::of(Domain::Value, b"empty-frontier-regime"),
        adm: &AdmAll,
        adm_id: Digest::of(Domain::Value, b"adm"),
        profile: &profile,
        interner: &interner,
        context: model.id(),
        assumptions: DeclaredAssumptions::all(),
    };
    let (step, consumed, _) = sat_step(
        &presentation,
        &exec,
        0,
        &mut |_: &Candidate, phase| soc_core::Key::new(phase, 0, Digest::of(Domain::Value, b"key")),
        SaturationBudget::uniform(10),
    );
    let SaturatedStep::Quiescent(certificate) = step else {
        panic!("empty frontier must quiesce")
    };
    assert!(check_quiescence_in_context(
        request.checked(),
        &certificate,
        &presentation,
        &exec,
        &consumed
    )
    .unwrap()
    .is_verified());
    presentation.regimes = &[];
    assert_eq!(
        check_quiescence_in_context(
            request.checked(),
            &certificate,
            &presentation,
            &exec,
            &consumed
        )
        .unwrap(),
        CertificateCheck::Unknown(CertificateCheckError::EnumerationNotScanned)
    );
    presentation.regimes = regimes;
    presentation.context = ContextId::root();
    assert_eq!(
        check_quiescence_in_context(
            request.checked(),
            &certificate,
            &presentation,
            &exec,
            &consumed
        ),
        Err(ModelError::BindingMismatch)
    );
    presentation.context = model.id();
    presentation.id = PresentationIdV1(Digest::of(Domain::Value, b"changed"));
    assert_eq!(
        check_quiescence_in_context(
            request.checked(),
            &certificate,
            &presentation,
            &exec,
            &consumed
        ),
        Err(ModelError::BindingMismatch)
    );
    let mut wrong = d.clone();
    wrong.tenant = "other".into();
    assert!(ResolutionContextV2::validate(
        &bytes,
        model.id(),
        &wrong,
        ModelLimits::default(),
        &store,
        &NoExternalAssumptions
    )
    .is_err());
    let unsupported = ContextModelV2::new(
        d,
        vec![TypedAssumptionV2 {
            name: "external".into(),
            schema: "unchecked".into(),
            canonical_value: vec![1],
        }],
    )
    .unwrap();
    assert!(ResolutionContextV2::validate(
        &encode_context_model_v2(&unsupported),
        unsupported.id(),
        unsupported.dimensions(),
        ModelLimits::default(),
        &store,
        &NoExternalAssumptions
    )
    .is_err());
}

#[test]
fn discovery_rejects_a_different_world_policy_or_history_before_provider_call() {
    use soc_core::contextual::execution_context_preimage;
    use std::cell::Cell;
    struct Provider(Cell<usize>);
    impl soc_core::WitnessProvider for Provider {
        fn candidates(&self, _: &ExecConfig) -> Vec<Candidate> {
            self.0.set(self.0.get() + 1);
            vec![]
        }
    }
    let mut interner = Interner::new();
    let world = interner.intern(Digest::of(Domain::Value, b"world"));
    let policy = interner.intern(Digest::of(Domain::Value, b"policy"));
    let other = interner.intern(Digest::of(Domain::Value, b"other"));
    let exec = ExecConfig::new(world, policy, History::empty().digest());
    let raw = execution_context_preimage(&exec, &interner).unwrap();
    let mut d = dimensions(None);
    d.world = Some(ArtifactRefV2 {
        kind: "soc.exec@1".into(),
        digest: Digest::of(Domain::Value, &raw),
    });
    let model = ContextModelV2::new(d.clone(), vec![]).unwrap();
    let request = ResolutionContextV2::validate(
        &encode_context_model_v2(&model),
        model.id(),
        &d,
        ModelLimits::default(),
        &Store(raw.clone()),
        &NoExternalAssumptions,
    )
    .unwrap();
    let provider = Provider(Cell::new(0));
    let result = request
        .candidates(&[&provider], &AdmAll, &exec, &interner)
        .unwrap();
    assert_eq!(result.context(), model.id());
    // The goal is supplied by the caller and retained verbatim. Providers only
    // see ExecConfig, so this test does not claim goal-to-candidate proof or
    // exhaustive goal resolution.
    let query = soc_core::resolution::ResolutionQuery::new(
        model.id(),
        PropositionId::from_canon(b"catalogue-goal"),
        Digest::of(Domain::Value, b"policy"),
    );
    let report = request
        .resolution_report(
            query,
            result.clone(),
            None,
            None,
            soc_core::resolution::ResolutionUnknown::IncompleteEnumeration,
        )
        .unwrap();
    assert_eq!(report.query(), &query);
    assert_eq!(
        report.completeness(),
        soc_core::resolution::CompletenessScope::Incomplete
    );
    assert!(report.verified_evidence_digest().is_none());
    assert!(matches!(
        report.outcome(),
        soc_core::resolution::ResolutionOutcome::Unknown(
            soc_core::resolution::ResolutionUnknown::IncompleteEnumeration
        )
    ));
    let wrong_query = soc_core::resolution::ResolutionQuery::new(
        ContextId::root().extend(b"wrong-context"),
        query.goal,
        query.policy,
    );
    assert!(matches!(
        request.resolution_report(
            wrong_query,
            result,
            None,
            None,
            soc_core::resolution::ResolutionUnknown::IncompleteEnumeration,
        ),
        Err(ModelError::BindingMismatch)
    ));
    assert_eq!(provider.0.get(), 1);

    // A candidate set produced by a separately admitted context cannot be
    // rebound to this request, even when its world and policy bindings match.
    let mut other_dimensions = d.clone();
    other_dimensions.tenant = "other-tenant".into();
    let other_model = ContextModelV2::new(other_dimensions.clone(), vec![]).unwrap();
    let other_request = ResolutionContextV2::validate(
        &encode_context_model_v2(&other_model),
        other_model.id(),
        &other_dimensions,
        ModelLimits::default(),
        &Store(raw.clone()),
        &NoExternalAssumptions,
    )
    .unwrap();
    let foreign_discovery = other_request
        .candidates(&[&provider], &AdmAll, &exec, &interner)
        .unwrap();
    assert!(matches!(
        request.resolution_report(
            query,
            foreign_discovery,
            None,
            None,
            soc_core::resolution::ResolutionUnknown::IncompleteEnumeration,
        ),
        Err(ModelError::BindingMismatch)
    ));
    assert_eq!(provider.0.get(), 2);

    let wrong_policy_query = soc_core::resolution::ResolutionQuery::new(
        model.id(),
        query.goal,
        Digest::of(Domain::Value, b"other-policy"),
    );
    assert!(matches!(
        request.resolution_report(
            wrong_policy_query,
            request
                .candidates(&[&provider], &AdmAll, &exec, &interner)
                .unwrap(),
            None,
            None,
            soc_core::resolution::ResolutionUnknown::IncompleteEnumeration,
        ),
        Err(ModelError::BindingMismatch)
    ));
    assert_eq!(provider.0.get(), 3);
    for wrong in [
        ExecConfig::new(other, policy, exec.history),
        ExecConfig::new(world, other, exec.history),
        ExecConfig::new(world, policy, Digest::of(Domain::Value, b"changed-history")),
    ] {
        assert!(matches!(
            request.candidates(&[&provider], &AdmAll, &wrong, &interner),
            Err(ModelError::BindingMismatch)
        ));
    }
    assert_eq!(provider.0.get(), 3);
}

#[test]
fn admitted_context_feeds_resolution_seam_and_maps_errors() {
    use soc_core::contextual::{resolution_context_error, AdmittedContextValidator};
    use soc_core::resolution::{ContextValidationError as E, ContextValidator};

    let d = dimensions(None);
    let model = ContextModelV2::new(d.clone(), vec![]).unwrap();
    let bytes = encode_context_model_v2(&model);
    let store = Store(vec![]);
    let request = ResolutionContextV2::validate(
        &bytes,
        model.id(),
        &d,
        ModelLimits::default(),
        &store,
        &NoExternalAssumptions,
    )
    .unwrap();
    let v = AdmittedContextValidator::new(&request);
    assert_eq!(v.validate_context(model.id(), model.id()), Ok(()));
    let other = ContextId::root().extend(b"other");
    assert_eq!(v.validate_context(other, model.id()), Err(E::Mismatched));
    assert_eq!(v.validate_context(model.id(), other), Err(E::Mismatched));

    // Wrong expected identity is rejected by #59 and surfaces as Mismatched.
    let err = ResolutionContextV2::validate(
        &bytes,
        other,
        &d,
        ModelLimits::default(),
        &store,
        &NoExternalAssumptions,
    )
    .err()
    .expect("identity mismatch must fail");
    assert_eq!(resolution_context_error(&err), E::Mismatched);
    assert_eq!(resolution_context_error(&ModelError::TooDeep), E::Exhausted);
    assert_eq!(
        resolution_context_error(&ModelError::MissingParent(other)),
        E::Unavailable
    );
    assert_eq!(
        resolution_context_error(&ModelError::Malformed),
        E::Malformed
    );
}
