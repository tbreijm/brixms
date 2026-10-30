use std::collections::{BTreeMap, BTreeSet};
use std::mem::ManuallyDrop;

use brix_canon::Canonical;
use brix_kernel::{
    acceptance, acceptance_in_context, native_verifier, Budget, ContextAcceptanceError,
    ExplicitTerm, KernelAssumptionValidator, ObjectTerm, Prop, TermKind, Var, Verdict,
    CONTEXT_VERIFIER_NAME_V1, KERNEL_PROFILE_V1, KERNEL_PROP_SCHEMA_V1, NO_AXIOMS_PROFILE_V1,
    PROOF_REGIME_PROFILE_V1,
};
use brix_semantic::{
    encode_context_model_v2, ContextDimensionsV2, ContextId, ContextModelV2, ContextResolverV2,
    ModelError, ModelLimits, PropositionId, ScenarioModeV2, TypedAssumptionV2,
};

#[derive(Default)]
struct Store {
    contexts: BTreeMap<ContextId, Vec<u8>>,
}
impl ContextResolverV2 for Store {
    fn context_bytes(&self, id: &ContextId) -> Option<Vec<u8>> {
        self.contexts.get(id).cloned()
    }
    fn artifact_bytes(&self, _: &brix_semantic::ArtifactRefV2) -> Option<Vec<u8>> {
        None
    }
}

fn dims() -> ContextDimensionsV2 {
    ContextDimensionsV2 {
        parent: None,
        program: None,
        world: None,
        namespace: "n".into(),
        tenant: "t".into(),
        principal: "p".into(),
        capability_view: "all".into(),
        scenario_mode: ScenarioModeV2::Production,
        branch: None,
        valid_time_start: 1,
        valid_time_end: 2,
        transaction_time: None,
        correction_time: None,
        simulation_time: None,
        observation_profile: "obs".into(),
        redaction_profile: "red".into(),
        kernel_profile: KERNEL_PROFILE_V1.into(),
        regime_profile: PROOF_REGIME_PROFILE_V1.into(),
        axiom_profile: NO_AXIOMS_PROFILE_V1.into(),
        validity_start: 1,
        validity_end: 2,
        step_budget: 100,
        depth_budget: 100,
    }
}

fn assumption(name: &str, prop: &Prop) -> TypedAssumptionV2 {
    TypedAssumptionV2 {
        name: name.into(),
        schema: KERNEL_PROP_SCHEMA_V1.into(),
        canonical_value: prop.canon_bytes(),
    }
}

fn model(d: ContextDimensionsV2, assumptions: Vec<TypedAssumptionV2>) -> ContextModelV2 {
    ContextModelV2::new(d, assumptions).unwrap()
}

fn call(
    model: &ContextModelV2,
    resolver: &Store,
    prop: &Prop,
    term: &ExplicitTerm,
    limits: ModelLimits,
) -> Result<brix_kernel::ContextAcceptance, ContextAcceptanceError> {
    let bytes = encode_context_model_v2(model);
    acceptance_in_context(
        &bytes,
        model.id(),
        model.dimensions(),
        model.dimensions().scenario_mode,
        limits,
        resolver,
        prop,
        term,
        Budget::new(100, 100),
    )
}

#[test]
fn named_hypothesis_is_accepted_only_after_context_assumption_validation() {
    let a = Prop::Atom(PropositionId::from_canon(b"A"));
    let m = model(dims(), vec![assumption("h", &a)]);
    let term = ExplicitTerm::new(m.id(), TermKind::Hyp(Var::Named("h".into())));
    let ctx = call(&m, &Store::default(), &a, &term, ModelLimits::default()).unwrap();
    let Verdict::Accepted(cert) = ctx.verdict() else {
        panic!("expected accepted context proof: {:?}", ctx.verdict())
    };
    assert_eq!(
        cert.verifier,
        brix_semantic::VerifierId::named(CONTEXT_VERIFIER_NAME_V1)
    );
    assert_ne!(cert.verifier, native_verifier());

    assert!(matches!(
        acceptance(&m.id(), &a, &term, Budget::new(100, 100)),
        Verdict::Rejected(_)
    ));
}

#[test]
fn identity_binding_schema_profile_and_budget_are_checked_before_proof_work() {
    let a = Prop::Atom(PropositionId::from_canon(b"A"));
    let mut d = dims();
    let m = model(d.clone(), vec![assumption("h", &a)]);
    let term = ExplicitTerm::new(m.id(), TermKind::Hyp(Var::Named("h".into())));
    let bytes = encode_context_model_v2(&m);

    assert_eq!(
        acceptance_in_context(
            &bytes,
            ContextId::from_canon(b"wrong"),
            m.dimensions(),
            ScenarioModeV2::Production,
            ModelLimits::default(),
            &Store::default(),
            &a,
            &term,
            Budget::new(100, 100)
        ),
        Err(ContextAcceptanceError::Model(ModelError::IdentityMismatch))
    );
    let mut bound = d.clone();
    bound.tenant = "other".into();
    assert_eq!(
        acceptance_in_context(
            &bytes,
            m.id(),
            &bound,
            ScenarioModeV2::Production,
            ModelLimits::default(),
            &Store::default(),
            &a,
            &term,
            Budget::new(100, 100)
        ),
        Err(ContextAcceptanceError::Model(ModelError::BindingMismatch))
    );
    assert_eq!(
        acceptance_in_context(
            &bytes,
            m.id(),
            &d,
            ScenarioModeV2::Simulation,
            ModelLimits::default(),
            &Store::default(),
            &a,
            &term,
            Budget::new(100, 100)
        ),
        Err(ContextAcceptanceError::ModeMismatch)
    );
    d.kernel_profile = "unknown".into();
    let bad_profile = model(d, vec![assumption("h", &a)]);
    assert_eq!(
        call(
            &bad_profile,
            &Store::default(),
            &a,
            &term,
            ModelLimits::default()
        ),
        Err(ContextAcceptanceError::UnsupportedKernelProfile)
    );
    let mut d = dims();
    d.regime_profile = "other@1".into();
    let bad_regime = model(d, vec![assumption("h", &a)]);
    assert_eq!(
        call(
            &bad_regime,
            &Store::default(),
            &a,
            &term,
            ModelLimits::default()
        ),
        Err(ContextAcceptanceError::UnsupportedRegimeProfile)
    );
    let mut d = dims();
    d.axiom_profile = "axioms@1".into();
    let bad_axioms = model(d, vec![assumption("h", &a)]);
    assert_eq!(
        call(
            &bad_axioms,
            &Store::default(),
            &a,
            &term,
            ModelLimits::default()
        ),
        Err(ContextAcceptanceError::UnsupportedAxiomProfile)
    );
    assert!(matches!(
        call(
            &m,
            &Store::default(),
            &a,
            &term,
            ModelLimits {
                bytes: 1,
                ..ModelLimits::default()
            }
        ),
        Err(ContextAcceptanceError::Model(ModelError::TooLarge))
    ));
}

#[test]
fn malformed_assumption_and_wrong_schema_fail_closed() {
    let a = Prop::Atom(PropositionId::from_canon(b"A"));
    let malformed = TypedAssumptionV2 {
        name: "h".into(),
        schema: KERNEL_PROP_SCHEMA_V1.into(),
        canonical_value: vec![255],
    };
    let m = model(dims(), vec![malformed]);
    let term = ExplicitTerm::new(m.id(), TermKind::Hyp(Var::Named("h".into())));
    assert!(matches!(
        call(&m, &Store::default(), &a, &term, ModelLimits::default()),
        Err(ContextAcceptanceError::Model(
            ModelError::AssumptionRejected(_)
        ))
    ));
    let mut wrong = assumption("h", &a);
    wrong.schema = "some.other.schema@1".into();
    let m = model(dims(), vec![wrong]);
    let term = ExplicitTerm::new(m.id(), TermKind::Hyp(Var::Named("h".into())));
    assert!(matches!(
        call(&m, &Store::default(), &a, &term, ModelLimits::default()),
        Err(ContextAcceptanceError::Model(
            ModelError::AssumptionRejected(_)
        ))
    ));
}

#[test]
fn context_chain_is_resolved_and_child_proof_does_not_escape_to_parent() {
    let a = Prop::Atom(PropositionId::from_canon(b"A"));
    let parent = model(dims(), vec![assumption("parent_h", &a)]);
    let parent_bytes = encode_context_model_v2(&parent);
    let mut d = dims();
    d.parent = Some(parent.id());
    let child = model(d, vec![]);
    let child_term = ExplicitTerm::new(child.id(), TermKind::Hyp(Var::Named("parent_h".into())));
    let mut store = Store::default();
    store.contexts.insert(parent.id(), parent_bytes);
    let accepted = call(&child, &store, &a, &child_term, ModelLimits::default()).unwrap();
    assert!(matches!(accepted.verdict(), Verdict::Accepted(_)));
    let parent_term = ExplicitTerm::new(parent.id(), TermKind::Hyp(Var::Named("parent_h".into())));
    let escaped = call(&child, &store, &a, &parent_term, ModelLimits::default()).unwrap();
    assert!(matches!(escaped.verdict(), Verdict::ContextMismatch { .. }));

    let mut missing_dims = dims();
    missing_dims.parent = Some(ContextId::from_canon(b"missing"));
    let missing = model(missing_dims, vec![]);
    let term = ExplicitTerm::new(missing.id(), TermKind::Hyp(Var::Named("x".into())));
    assert!(matches!(
        call(
            &missing,
            &Store::default(),
            &a,
            &term,
            ModelLimits::default()
        ),
        Err(ContextAcceptanceError::Model(ModelError::MissingParent(_)))
    ));
}

#[test]
fn context_proof_certificate_is_not_a_legacy_certificate_and_lambda_discharge_works() {
    let a = Prop::Atom(PropositionId::from_canon(b"A"));
    let id = Prop::Impl(Box::new(a.clone()), Box::new(a));
    let m = model(dims(), vec![]);
    let term = ExplicitTerm::new(
        m.id(),
        TermKind::Lam {
            var_name: Some("x".into()),
            body: Box::new(TermKind::Hyp(Var::Index(0))),
        },
    );
    let contextual = call(&m, &Store::default(), &id, &term, ModelLimits::default()).unwrap();
    let Verdict::Accepted(context_cert) = contextual.verdict() else {
        panic!("expected accepted")
    };
    let Verdict::Accepted(legacy_cert) = acceptance(&m.id(), &id, &term, Budget::new(100, 100))
    else {
        panic!("legacy identity proof should remain accepted")
    };
    assert_ne!(context_cert, &legacy_cert);
    assert_eq!(
        context_cert.verifier,
        brix_semantic::VerifierId::named(CONTEXT_VERIFIER_NAME_V1)
    );
    assert_eq!(legacy_cert.verifier, native_verifier());
}

#[test]
fn free_object_bound_variables_are_rejected_but_closed_existentials_decode() {
    let free = Prop::Eq(ObjectTerm::BoundVar(0), ObjectTerm::BoundVar(0));
    let malformed = TypedAssumptionV2 {
        name: "bad".into(),
        schema: KERNEL_PROP_SCHEMA_V1.into(),
        canonical_value: free.canon_bytes(),
    };
    let m = model(dims(), vec![malformed]);
    let term = ExplicitTerm::new(m.id(), TermKind::Hyp(Var::Named("bad".into())));
    assert!(matches!(
        call(&m, &Store::default(), &free, &term, ModelLimits::default()),
        Err(ContextAcceptanceError::Model(
            ModelError::AssumptionRejected(_)
        ))
    ));
    let closed = Prop::Exists(Box::new(Prop::Eq(
        ObjectTerm::BoundVar(0),
        ObjectTerm::BoundVar(0),
    )));
    let m = model(dims(), vec![assumption("closed", &closed)]);
    let term = ExplicitTerm::new(m.id(), TermKind::Hyp(Var::Named("closed".into())));
    assert!(matches!(
        call(
            &m,
            &Store::default(),
            &closed,
            &term,
            ModelLimits::default()
        )
        .unwrap()
        .verdict(),
        Verdict::Accepted(_)
    ));
}

#[test]
fn zero_budget_refuses_deep_term_before_context_certificate_encoding() {
    let a = Prop::Atom(PropositionId::from_canon(b"A"));
    let m = model(dims(), vec![]);
    let mut kind = TermKind::Hyp(Var::Named("missing".into()));
    for _ in 0..20_000 {
        kind = TermKind::Inl(Box::new(kind));
    }
    // Certificate material must not recursively encode this tree after a
    // zero checker budget has already refused it.
    let term = ManuallyDrop::new(ExplicitTerm::new(m.id(), kind));
    let checked = acceptance_in_context(
        &encode_context_model_v2(&m),
        m.id(),
        m.dimensions(),
        ScenarioModeV2::Production,
        ModelLimits::default(),
        &Store::default(),
        &a,
        &term,
        Budget::new(0, 0),
    )
    .unwrap();
    assert_eq!(
        checked.verdict(),
        &Verdict::ResourceExhausted(brix_kernel::ResourceBudgetReason::StepLimitExceeded)
    );
}

#[test]
fn restriction_rechecks_proofs_in_destination_context() {
    let a = Prop::Atom(PropositionId::from_canon(b"A"));
    let source = model(dims(), vec![assumption("h", &a)]);
    let hyp = ExplicitTerm::new(source.id(), TermKind::Hyp(Var::Named("h".into())));
    let source_acceptance =
        call(&source, &Store::default(), &a, &hyp, ModelLimits::default()).unwrap();
    assert!(matches!(source_acceptance.verdict(), Verdict::Accepted(_)));

    let target = source_acceptance
        .checked_context()
        .restrict(
            &BTreeSet::new(),
            ModelLimits::default(),
            &Store::default(),
            &KernelAssumptionValidator,
        )
        .unwrap();
    let target_bytes = encode_context_model_v2(target.model());
    let target_hyp = ExplicitTerm::new(target.id(), TermKind::Hyp(Var::Named("h".into())));
    let rechecked_hyp = acceptance_in_context(
        &target_bytes,
        target.id(),
        target.model().dimensions(),
        ScenarioModeV2::Production,
        ModelLimits::default(),
        &Store::default(),
        &a,
        &target_hyp,
        Budget::new(100, 100),
    )
    .unwrap();
    assert!(matches!(rechecked_hyp.verdict(), Verdict::Rejected(_)));

    let identity = Prop::Impl(Box::new(a.clone()), Box::new(a.clone()));
    let target_lambda = ExplicitTerm::new(
        target.id(),
        TermKind::Lam {
            var_name: Some("x".into()),
            body: Box::new(TermKind::Hyp(Var::Index(0))),
        },
    );
    let rechecked_lambda = acceptance_in_context(
        &target_bytes,
        target.id(),
        target.model().dimensions(),
        ScenarioModeV2::Production,
        ModelLimits::default(),
        &Store::default(),
        &identity,
        &target_lambda,
        Budget::new(100, 100),
    )
    .unwrap();
    assert!(matches!(rechecked_lambda.verdict(), Verdict::Accepted(_)));
}
