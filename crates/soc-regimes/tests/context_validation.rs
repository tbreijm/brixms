use std::collections::BTreeMap;

use brix_canon::{CanonWriter, Canonical};
use brix_semantic::{
    encode_context_model_v2, ContextDimensionsV2, ContextId, ContextModelV2, ContextResolverV2,
    ModelError, ModelLimits, ScenarioModeV2, TypedAssumptionV2,
};
use soc_regimes::{
    context_validation::{
        audited_type_check_tree_in_context_v2, decode_closed_ty, encode_closed_ty,
        infer_tree_in_context_v2, typed_assumption, ContextTypeError, TyDecodeError,
        TyDecodeLimits, TYPE_ASSUMPTION_SCHEMA_V1,
    },
    Expr, Ty,
};

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
        kernel_profile: brix_kernel::KERNEL_PROFILE_V1.into(),
        regime_profile: "brix.type@1".into(),
        axiom_profile: "none".into(),
        validity_start: 1,
        validity_end: 9,
        step_budget: 100,
        depth_budget: 10,
    }
}

#[derive(Default)]
struct Resolver(BTreeMap<ContextId, Vec<u8>>);
impl ContextResolverV2 for Resolver {
    fn context_bytes(&self, id: &ContextId) -> Option<Vec<u8>> {
        self.0.get(id).cloned()
    }
    fn artifact_bytes(&self, _reference: &brix_semantic::ArtifactRefV2) -> Option<Vec<u8>> {
        None
    }
}

#[test]
fn arithmetic_bridge_closure_retains_the_materialized_context() {
    let d = dimensions(None);
    let model =
        ContextModelV2::new(d.clone(), vec![typed_assumption("x", &Ty::Con("Int"))]).unwrap();
    let expr = Expr::Arith(
        soc_regimes::type_realization::ArithOp::Add,
        Box::new(Expr::Var("x".into())),
        Box::new(Expr::Lit(2)),
    );
    let (source, tree) = audited_type_check_tree_in_context_v2(
        &expr,
        &encode_context_model_v2(&model),
        model.id(),
        &d,
        ModelLimits::default(),
        &Resolver::default(),
    )
    .unwrap();
    let budget = brix_kernel::Budget::new(10_000, 256);
    let brix_elaborate::TreeElaboration::Proven { proof, .. } =
        brix_elaborate::elaborate_tree_certified(&source, &tree, budget)
    else {
        panic!("arithmetic must elaborate in the admitted context")
    };
    assert_eq!(*proof.context(), model.id());
    assert_eq!(proof.closed_occurrences().count(), 2);
    let grounds = soc_regimes::type_realization::typing_grounds(
        brix_semantic::Outcome::Proven,
        &tree,
        &proof,
    )
    .unwrap();
    assert!(grounds.obligations.is_empty());
    // Structural grounds remain antecedents; this is not an unconditional theorem.
    assert!(proof.remaining_occurrences().count() > 0);
    proof.recheck(&tree, &model.id(), budget).unwrap();
    assert!(proof.recheck(&tree, &ContextId::root(), budget).is_err());
}

#[test]
fn checked_context_adapter_preserves_identity_and_shadowing_for_real_inference() {
    let parent = ContextModelV2::new(
        dimensions(None),
        vec![typed_assumption("x", &Ty::Con("Int"))],
    )
    .unwrap();
    let parent_id = parent.id();
    let mut resolver = Resolver::default();
    resolver
        .0
        .insert(parent_id, encode_context_model_v2(&parent));

    let child_dimensions = dimensions(Some(parent_id));
    let child = ContextModelV2::new(
        child_dimensions.clone(),
        vec![
            typed_assumption("x", &Ty::Con("Str")),
            typed_assumption("child_only", &Ty::Con("Int")),
        ],
    )
    .unwrap();
    let child_bytes = encode_context_model_v2(&child);
    let expr = Expr::Var("x".into());
    let (inferred, _, _) = infer_tree_in_context_v2(
        &expr,
        &child_bytes,
        child.id(),
        &child_dimensions,
        ModelLimits::default(),
        &resolver,
        soc_regimes::type_realization::Infer::new(),
    )
    .unwrap();
    assert_eq!(inferred, Ty::Con("Str"));
    let (judgement, _) = audited_type_check_tree_in_context_v2(
        &expr,
        &child_bytes,
        child.id(),
        &child_dimensions,
        ModelLimits::default(),
        &resolver,
    )
    .unwrap();
    assert_eq!(judgement.context, child.id());

    let child_expr = Expr::Var("child_only".into());
    assert!(matches!(
        infer_tree_in_context_v2(
            &child_expr,
            &encode_context_model_v2(&parent),
            parent_id,
            parent.dimensions(),
            ModelLimits::default(),
            &resolver,
            soc_regimes::type_realization::Infer::new()
        ),
        Err(ContextTypeError::Type(_))
    ));
}

#[test]
fn checked_adapter_rejects_bad_schema_payload_and_revision_binding() {
    let d = dimensions(None);
    let bad_schema = TypedAssumptionV2 {
        name: "x".into(),
        schema: "other@1".into(),
        canonical_value: encode_closed_ty(&Ty::Con("Int")),
    };
    let model = ContextModelV2::new(d.clone(), vec![bad_schema]).unwrap();
    assert_eq!(
        infer_tree_in_context_v2(
            &Expr::Var("x".into()),
            &encode_context_model_v2(&model),
            model.id(),
            &d,
            ModelLimits::default(),
            &Resolver::default(),
            soc_regimes::type_realization::Infer::new()
        )
        .unwrap_err(),
        ContextTypeError::Context(ModelError::AssumptionRejected("x".into()))
    );

    let bad_payload = TypedAssumptionV2 {
        name: "x".into(),
        schema: TYPE_ASSUMPTION_SCHEMA_V1.into(),
        canonical_value: vec![0xff],
    };
    let model = ContextModelV2::new(d.clone(), vec![bad_payload]).unwrap();
    assert!(matches!(
        infer_tree_in_context_v2(
            &Expr::Var("x".into()),
            &encode_context_model_v2(&model),
            model.id(),
            &d,
            ModelLimits::default(),
            &Resolver::default(),
            soc_regimes::type_realization::Infer::new()
        ),
        Err(ContextTypeError::Context(ModelError::AssumptionRejected(_)))
    ));

    let model = ContextModelV2::new(d.clone(), vec![]).unwrap();
    let mut changed = d.clone();
    changed.regime_profile = "brix.type@2".into();
    assert!(matches!(
        infer_tree_in_context_v2(
            &Expr::Lit(1),
            &encode_context_model_v2(&model),
            model.id(),
            &changed,
            ModelLimits::default(),
            &Resolver::default(),
            soc_regimes::type_realization::Infer::new()
        ),
        Err(ContextTypeError::UnsupportedProfile)
    ));
}

fn bytes(ty: &Ty) -> Vec<u8> {
    let mut writer = CanonWriter::new();
    ty.canon_write(&mut writer);
    writer.finish()
}

#[test]
fn decodes_closed_canonical_types_without_changing_identity() {
    let ty = Ty::Rec(
        "List".into(),
        Box::new(Ty::Sum(
            "List".into(),
            vec![
                (
                    "Cons".into(),
                    vec![Ty::Con("Int"), Ty::RecVar("List".into())],
                ),
                ("Nil".into(), vec![]),
            ],
        )),
    );
    let decoded = decode_closed_ty(&bytes(&ty), TyDecodeLimits::default()).unwrap();
    assert_eq!(decoded, ty);
    assert_eq!(decoded.config_id(), ty.config_id());
}

#[test]
fn public_decoder_limits_are_clamped_even_when_callers_request_unbounded_values() {
    let mut ty = Ty::Con("Complex");
    for _ in 0..129 {
        ty = Ty::Fn(Box::new(Ty::Con("Int")), Box::new(ty));
    }
    assert_eq!(
        decode_closed_ty(
            &bytes(&ty),
            TyDecodeLimits {
                max_bytes: usize::MAX,
                max_nodes: usize::MAX,
                max_depth: usize::MAX,
                max_members: usize::MAX,
            },
        ),
        Err(TyDecodeError::LimitExceeded)
    );
}

#[test]
fn rejects_open_unknown_noncanonical_and_over_budget_types() {
    let mut writer = CanonWriter::new();
    writer.write_enum(2, |w| w.write_uint(0));
    assert_eq!(
        decode_closed_ty(&writer.finish(), TyDecodeLimits::default()),
        Err(TyDecodeError::OpenType)
    );

    let mut writer = CanonWriter::new();
    writer.write_enum(0, |w| w.write_str("Mystery"));
    assert!(matches!(
        decode_closed_ty(&writer.finish(), TyDecodeLimits::default()),
        Err(TyDecodeError::UnknownConstant(_))
    ));

    let mut writer = CanonWriter::new();
    writer.write_enum(3, |w| {
        w.write_uint(2);
        w.write_str("z");
        Ty::Con("Int").canon_write(w);
        w.write_str("a");
        Ty::Con("Str").canon_write(w);
    });
    assert_eq!(
        decode_closed_ty(&writer.finish(), TyDecodeLimits::default()),
        Err(TyDecodeError::UnsortedRecord)
    );

    let payload = bytes(&Ty::Fn(Box::new(Ty::Con("Int")), Box::new(Ty::Con("Str"))));
    let limited = TyDecodeLimits {
        max_depth: 0,
        ..TyDecodeLimits::default()
    };
    assert_eq!(
        decode_closed_ty(&payload, limited),
        Err(TyDecodeError::LimitExceeded)
    );
}

#[test]
fn inference_is_preflighted_against_context_steps_and_depth() {
    let mut d = dimensions(None);
    d.step_budget = 2;
    let model = ContextModelV2::new(d.clone(), vec![]).unwrap();
    let expr = Expr::Arith(
        soc_regimes::type_realization::ArithOp::Add,
        Box::new(Expr::Lit(1)),
        Box::new(Expr::Lit(2)),
    );
    assert_eq!(
        infer_tree_in_context_v2(
            &expr,
            &encode_context_model_v2(&model),
            model.id(),
            &d,
            ModelLimits::default(),
            &Resolver::default(),
            soc_regimes::type_realization::Infer::new(),
        )
        .unwrap_err(),
        ContextTypeError::InferenceBudgetExceeded
    );

    d.step_budget = 100;
    d.depth_budget = 1;
    let model = ContextModelV2::new(d.clone(), vec![]).unwrap();
    assert_eq!(
        infer_tree_in_context_v2(
            &expr,
            &encode_context_model_v2(&model),
            model.id(),
            &d,
            ModelLimits::default(),
            &Resolver::default(),
            soc_regimes::type_realization::Infer::new(),
        )
        .unwrap_err(),
        ContextTypeError::InferenceBudgetExceeded
    );
}

#[test]
fn lambda_binder_discharges_and_projection_is_rechecked_under_real_contexts() {
    let d = dimensions(None);
    let source_model = ContextModelV2::new(
        d.clone(),
        vec![
            typed_assumption("external", &Ty::Con("Int")),
            typed_assumption("unused", &Ty::Con("Str")),
        ],
    )
    .unwrap();
    let source_bytes = encode_context_model_v2(&source_model);
    let expr = Expr::LamAnn(
        "arg".into(),
        Ty::Con("Int"),
        Box::new(Expr::Var("arg".into())),
    );
    let source_result = audited_type_check_tree_in_context_v2(
        &expr,
        &source_bytes,
        source_model.id(),
        &d,
        ModelLimits::default(),
        &Resolver::default(),
    )
    .unwrap();
    assert_eq!(source_result.0.context, source_model.id());

    let checked = brix_semantic::validate_context_model_v2(
        &source_bytes,
        source_model.id(),
        &d,
        ModelLimits::default(),
        &Resolver::default(),
        &soc_regimes::ClosedTyAssumptionValidator,
    )
    .unwrap();
    let projected = checked
        .restrict(
            &std::collections::BTreeSet::from(["unused".to_string()]),
            ModelLimits::default(),
            &Resolver::default(),
            &soc_regimes::ClosedTyAssumptionValidator,
        )
        .unwrap();
    checked
        .transport_to(&projected, &std::collections::BTreeSet::new())
        .unwrap();
    let projected_bytes = encode_context_model_v2(projected.model());
    let projected_result = audited_type_check_tree_in_context_v2(
        &expr,
        &projected_bytes,
        projected.id(),
        &d,
        ModelLimits::default(),
        &Resolver::default(),
    )
    .unwrap();
    assert_eq!(projected_result.0.context, projected.id());

    let escaping = Expr::LamAnn(
        "arg".into(),
        Ty::Con("Int"),
        Box::new(Expr::Var("external".into())),
    );
    assert!(matches!(
        audited_type_check_tree_in_context_v2(
            &escaping,
            &projected_bytes,
            projected.id(),
            &d,
            ModelLimits::default(),
            &Resolver::default(),
        ),
        Err(ContextTypeError::Type(_))
    ));
}

#[test]
fn unguarded_recursive_assumptions_are_rejected_before_unfolding() {
    let bad = Ty::Rec("X".into(), Box::new(Ty::RecVar("X".into())));
    assert_eq!(
        decode_closed_ty(&bytes(&bad), TyDecodeLimits::default()),
        Err(TyDecodeError::UnguardedRecursion)
    );
    let nested = Ty::Rec(
        "X".into(),
        Box::new(Ty::Rec("Y".into(), Box::new(Ty::RecVar("X".into())))),
    );
    assert_eq!(
        decode_closed_ty(&bytes(&nested), TyDecodeLimits::default()),
        Err(TyDecodeError::UnguardedRecursion)
    );
}
