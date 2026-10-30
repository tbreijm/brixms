use std::collections::{BTreeMap, BTreeSet};

use brix_canon::{CanonWriter, Digest};
use brix_semantic::{
    decode_context_model_v2, encode_context_envelope_v1, encode_context_model_v2,
    validate_context_model_v2, ArtifactRefV2, AssumptionValidatorV2, ContextDimensionsV2,
    ContextId, ContextModelV2, ContextPayloadV1, ContextResolverV2, ModelError, ModelLimits,
    ScenarioModeV2, TypedAssumptionV2,
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
        kernel_profile: "kernel@1".into(),
        regime_profile: "regime@1".into(),
        axiom_profile: "axiom@1".into(),
        validity_start: 1,
        validity_end: 9,
        step_budget: 100,
        depth_budget: 10,
    }
}

#[test]
fn semantic_validator_and_expected_id_are_required() {
    struct Reject;
    impl AssumptionValidatorV2 for Reject {
        fn validate(&self, _: &str, _: &[u8], _: &mut usize) -> bool {
            false
        }
    }
    let m = ContextModelV2::new(dimensions(None), vec![assumption("x", b"opaque")]).unwrap();
    let bytes = encode_context_model_v2(&m);
    assert_eq!(
        validate_context_model_v2(
            &bytes,
            ContextId::from_canon(b"wrong"),
            m.dimensions(),
            ModelLimits::default(),
            &Resolver::default(),
            &Accept
        )
        .unwrap_err(),
        ModelError::IdentityMismatch
    );
    assert_eq!(
        validate_context_model_v2(
            &bytes,
            m.id(),
            m.dimensions(),
            ModelLimits::default(),
            &Resolver::default(),
            &Reject
        )
        .unwrap_err(),
        ModelError::AssumptionRejected("x".into())
    );
}
fn assumption(name: &str, value: &[u8]) -> TypedAssumptionV2 {
    TypedAssumptionV2 {
        name: name.into(),
        schema: "test.bytes@1".into(),
        canonical_value: value.to_vec(),
    }
}

#[derive(Default)]
struct Resolver {
    contexts: BTreeMap<ContextId, Vec<u8>>,
    artifacts: BTreeMap<ArtifactRefV2, Vec<u8>>,
}
impl ContextResolverV2 for Resolver {
    fn context_bytes(&self, id: &ContextId) -> Option<Vec<u8>> {
        self.contexts.get(id).cloned()
    }
    fn artifact_bytes(&self, a: &ArtifactRefV2) -> Option<Vec<u8>> {
        self.artifacts.get(a).cloned()
    }
}
struct Accept;
impl AssumptionValidatorV2 for Accept {
    fn validate(&self, _schema: &str, value: &[u8], remaining: &mut usize) -> bool {
        if value.len() > *remaining {
            return false;
        };
        *remaining -= value.len();
        !value.is_empty()
    }
}

#[test]
fn canonical_roundtrip_and_hostile_bytes_fail_closed() {
    let m = ContextModelV2::new(dimensions(None), vec![assumption("x", b"opaque")]).unwrap();
    let bytes = encode_context_model_v2(&m);
    assert_eq!(
        decode_context_model_v2(&bytes, ModelLimits::default()).unwrap(),
        m
    );
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert_eq!(
        decode_context_model_v2(&trailing, ModelLimits::default()).unwrap_err(),
        ModelError::Malformed
    );
    assert_eq!(
        decode_context_model_v2(
            &bytes,
            ModelLimits {
                bytes: 1,
                ..ModelLimits::default()
            }
        )
        .unwrap_err(),
        ModelError::TooLarge
    );
}

#[test]
fn expected_dimensions_and_artifacts_are_independently_bound() {
    let artifact_bytes = b"program artifact".to_vec();
    let reference = ArtifactRefV2 {
        kind: "program".into(),
        digest: Digest::of(brix_canon::Domain::Value, &artifact_bytes),
    };
    let mut d = dimensions(None);
    d.program = Some(reference.clone());
    let m = ContextModelV2::new(d.clone(), vec![]).unwrap();
    let bytes = encode_context_model_v2(&m);
    let id = m.id();
    assert_eq!(
        validate_context_model_v2(
            &bytes,
            id,
            &d,
            ModelLimits::default(),
            &Resolver::default(),
            &Accept
        )
        .unwrap_err(),
        ModelError::MissingArtifact(reference.clone())
    );
    let mut r = Resolver::default();
    r.artifacts.insert(reference.clone(), artifact_bytes);
    assert!(validate_context_model_v2(&bytes, id, &d, ModelLimits::default(), &r, &Accept).is_ok());
    let mut forged = d.clone();
    forged.namespace = "other".into();
    assert_eq!(
        validate_context_model_v2(&bytes, id, &forged, ModelLimits::default(), &r, &Accept)
            .unwrap_err(),
        ModelError::BindingMismatch
    );
    r.artifacts.insert(reference.clone(), b"forged".to_vec());
    assert_eq!(
        validate_context_model_v2(&bytes, id, &d, ModelLimits::default(), &r, &Accept).unwrap_err(),
        ModelError::ArtifactMismatch(reference)
    );

    // Every authority, world, profile, time, and limit field is independently caller-bound.
    let plain = ContextModelV2::new(dimensions(None), vec![]).unwrap();
    let plain_bytes = encode_context_model_v2(&plain);
    let mut mutations = Vec::new();
    for mutate in 0..14 {
        let mut changed = plain.dimensions().clone();
        match mutate {
            0 => changed.tenant = "other".into(),
            1 => changed.principal = "other".into(),
            2 => changed.capability_view = "other".into(),
            3 => {
                changed.world = Some(ArtifactRefV2 {
                    kind: "world".into(),
                    digest: Digest::from_bytes([3; 32]),
                })
            }
            4 => changed.observation_profile = "other".into(),
            5 => changed.redaction_profile = "other".into(),
            6 => changed.kernel_profile = "other".into(),
            7 => changed.regime_profile = "other".into(),
            8 => changed.axiom_profile = "other".into(),
            9 => changed.valid_time_start += 1,
            10 => changed.transaction_time = Some(2),
            11 => changed.validity_end -= 1,
            12 => changed.step_budget += 1,
            _ => changed.depth_budget += 1,
        }
        mutations.push(changed);
    }
    for expected in mutations {
        assert_eq!(
            validate_context_model_v2(
                &plain_bytes,
                plain.id(),
                &expected,
                ModelLimits::default(),
                &Resolver::default(),
                &Accept
            )
            .unwrap_err(),
            ModelError::BindingMismatch
        );
    }
}

#[test]
fn checked_parent_chain_shadowing_projection_and_transport() {
    let root_model = ContextModelV2::new(
        dimensions(None),
        vec![assumption("x", b"old"), assumption("y", b"keep")],
    )
    .unwrap();
    let root_bytes = encode_context_model_v2(&root_model);
    let root_id = root_model.id();
    let mut resolver = Resolver::default();
    resolver.contexts.insert(root_id, root_bytes);
    let root = validate_context_model_v2(
        &encode_context_model_v2(&root_model),
        root_id,
        root_model.dimensions(),
        ModelLimits::default(),
        &resolver,
        &Accept,
    )
    .unwrap();
    let child_model = root
        .extend_typed(
            assumption("x", b"new"),
            ModelLimits::default(),
            &resolver,
            &Accept,
        )
        .unwrap();
    let bindings = child_model.assumptions_in_scope();
    assert_eq!(bindings["x"].canonical_value, b"new");
    assert_eq!(bindings["y"].canonical_value, b"keep");
    assert_ne!(root.id(), child_model.id());
    assert!(root.same_context(&root));
    let leaf = child_model
        .extend_typed(
            assumption("z", b"new"),
            ModelLimits::default(),
            &resolver,
            &Accept,
        )
        .unwrap();
    assert_eq!(leaf.assumptions_in_scope()["x"].canonical_value, b"new");
    let kept = BTreeSet::from(["x".to_string(), "y".to_string()]);
    let projected = leaf
        .restrict(&kept, ModelLimits::default(), &resolver, &Accept)
        .unwrap();
    assert_eq!(projected.model().dimensions(), leaf.model().dimensions());
    let receipt = leaf.transport_to(&projected, &kept).unwrap();
    assert_eq!(receipt.source(), leaf.id());
    assert_eq!(receipt.destination(), projected.id());
    let inherited = child_model
        .transport_to(&leaf, &BTreeSet::from(["x".to_string()]))
        .unwrap();
    assert_eq!(inherited.source(), child_model.id());
    assert_eq!(inherited.destination(), leaf.id());
    assert_eq!(
        root.transport_to(&child_model, &BTreeSet::from(["x".to_string()]))
            .unwrap_err(),
        ModelError::MissingUsedAssumption("x".into())
    );
    assert_eq!(
        leaf.transport_to(&projected, &BTreeSet::from(["z".to_string()]))
            .unwrap_err(),
        ModelError::MissingUsedAssumption("z".into())
    );
}

#[test]
fn parent_dimension_change_and_simulation_to_production_are_rejected() {
    let root_d = dimensions(None);
    let root = ContextModelV2::new(root_d.clone(), vec![]).unwrap();
    let root_id = root.id();
    let mut resolver = Resolver::default();
    resolver
        .contexts
        .insert(root_id, encode_context_model_v2(&root));
    let mut child_d = dimensions(Some(root_id));
    child_d.regime_profile = "changed".into();
    let child = ContextModelV2::new(child_d.clone(), vec![]).unwrap();
    assert_eq!(
        validate_context_model_v2(
            &encode_context_model_v2(&child),
            child.id(),
            &child_d,
            ModelLimits::default(),
            &resolver,
            &Accept
        )
        .unwrap_err(),
        ModelError::ParentDimensionMismatch
    );
    let mut sim_d = dimensions(None);
    sim_d.scenario_mode = ScenarioModeV2::Simulation;
    sim_d.branch = Some(Digest::from_bytes([4; 32]));
    let sim = ContextModelV2::new(sim_d.clone(), vec![]).unwrap();
    let sim_id = sim.id();
    resolver
        .contexts
        .insert(sim_id, encode_context_model_v2(&sim));
    let mut prod_d = sim_d;
    prod_d.parent = Some(sim_id);
    prod_d.scenario_mode = ScenarioModeV2::Production;
    prod_d.branch = None;
    let prod = ContextModelV2::new(prod_d.clone(), vec![]).unwrap();
    assert_eq!(
        validate_context_model_v2(
            &encode_context_model_v2(&prod),
            prod.id(),
            &prod_d,
            ModelLimits::default(),
            &resolver,
            &Accept
        )
        .unwrap_err(),
        ModelError::ModeMismatch
    );
}

#[test]
fn v1_root_encoding_remains_frozen() {
    let bytes = encode_context_envelope_v1(&ContextPayloadV1::LegacyRoot);
    assert_eq!(
        ContextId::from_canon(&bytes).to_hex(),
        "a7d1f9a56c727ac00ad5dd6dd97d4af1e943df9f605efcc265248c2c7b355c5c"
    );
    let mut w = CanonWriter::new();
    w.write_tag("brix.ir.reflect.ScopeId.root");
    assert_eq!(bytes, w.finish());
}

#[test]
fn every_nonlogical_dimension_blocks_implicit_transport() {
    let original = ContextModelV2::new(dimensions(None), vec![assumption("x", b"old")]).unwrap();
    let store = Resolver::default();
    let source = validate_context_model_v2(
        &encode_context_model_v2(&original),
        original.id(),
        original.dimensions(),
        ModelLimits::default(),
        &store,
        &Accept,
    )
    .unwrap();
    let mut alternatives = Vec::new();
    macro_rules! change {
        ($field:ident,$value:expr) => {{
            let mut d = dimensions(None);
            d.$field = $value;
            alternatives.push(d);
        }};
    }
    change!(namespace, "other".into());
    change!(tenant, "other".into());
    change!(principal, "other".into());
    change!(capability_view, "other".into());
    change!(observation_profile, "other".into());
    change!(redaction_profile, "other".into());
    change!(kernel_profile, "other".into());
    change!(regime_profile, "other".into());
    change!(axiom_profile, "other".into());
    change!(valid_time_start, 2);
    change!(valid_time_end, 8);
    change!(transaction_time, Some(2));
    change!(correction_time, Some(2));
    change!(simulation_time, Some(2));
    change!(validity_start, 2);
    change!(validity_end, 8);
    change!(step_budget, 99);
    change!(depth_budget, 9);
    let mut sim = dimensions(None);
    sim.scenario_mode = ScenarioModeV2::Simulation;
    sim.branch = Some(Digest::from_bytes([9; 32]));
    alternatives.push(sim);
    for d in alternatives {
        let m = ContextModelV2::new(d, vec![assumption("x", b"old")]).unwrap();
        assert_ne!(m.id(), original.id());
        let dst = validate_context_model_v2(
            &encode_context_model_v2(&m),
            m.id(),
            m.dimensions(),
            ModelLimits::default(),
            &store,
            &Accept,
        )
        .unwrap();
        assert_eq!(
            source.transport_to(&dst, &BTreeSet::new()),
            Err(ModelError::TransportMismatch)
        );
    }
    let changed = ContextModelV2::new(dimensions(None), vec![assumption("x", b"new")]).unwrap();
    let dst = validate_context_model_v2(
        &encode_context_model_v2(&changed),
        changed.id(),
        changed.dimensions(),
        ModelLimits::default(),
        &store,
        &Accept,
    )
    .unwrap();
    assert!(matches!(
        source.transport_to(&dst, &BTreeSet::from(["x".into()])),
        Err(ModelError::MissingUsedAssumption(_))
    ));
}

#[test]
fn projection_preserves_origin_and_cannot_change_used_inherited_binding() {
    let store = Resolver::default();
    let m = ContextModelV2::new(dimensions(None), vec![assumption("x", b"old")]).unwrap();
    let src = validate_context_model_v2(
        &encode_context_model_v2(&m),
        m.id(),
        m.dimensions(),
        ModelLimits::default(),
        &store,
        &Accept,
    )
    .unwrap();
    let child = src
        .extend_typed(
            assumption("y", b"new"),
            ModelLimits::default(),
            &store,
            &Accept,
        )
        .unwrap();
    let (projected, receipt) = child
        .project_assumptions(
            &BTreeSet::from(["x".into()]),
            ModelLimits::default(),
            &store,
            &Accept,
        )
        .unwrap();
    assert_eq!(receipt.source(), child.id());
    assert_eq!(receipt.destination(), projected.id());
    assert_eq!(projected.assumptions_in_scope().len(), 1);
    assert!(projected.assumptions_in_scope().contains_key("x"));
    assert_eq!(src.assumptions_in_scope().len(), 1);
    assert!(child
        .transport_to(&projected, &BTreeSet::from(["y".into()]))
        .is_err());
    let rebound = child
        .rebind_typed(
            assumption("x", b"corrected"),
            ModelLimits::default(),
            &store,
            &Accept,
        )
        .unwrap();
    assert!(child
        .transport_to(&rebound, &BTreeSet::from(["x".into()]))
        .is_err());
    assert_eq!(src.assumptions_in_scope()["x"].canonical_value, b"old");
}

#[test]
fn malformed_counts_parent_substitution_and_reference_budget_fail_closed() {
    let base = ContextModelV2::new(dimensions(None), vec![]).unwrap();
    let bytes = encode_context_model_v2(&base);
    let mut count = CanonWriter::new();
    count.write_uint(u64::MAX);
    let mut huge = bytes[..bytes.len() - 1].to_vec();
    huge.extend(count.finish());
    assert!(matches!(
        decode_context_model_v2(&huge, ModelLimits::default()),
        Err(ModelError::TooManyAssumptions) | Err(ModelError::BadLength)
    ));
    let parent_id = ContextId::from_canon(b"missing");
    let child = ContextModelV2::new(dimensions(Some(parent_id)), vec![]).unwrap();
    let mut resolver = Resolver::default();
    resolver.contexts.insert(parent_id, bytes);
    assert_eq!(
        validate_context_model_v2(
            &encode_context_model_v2(&child),
            child.id(),
            child.dimensions(),
            ModelLimits::default(),
            &resolver,
            &Accept
        )
        .unwrap_err(),
        ModelError::IdentityMismatch
    );
    let mut d = dimensions(None);
    let raw = b"artifact".to_vec();
    let reference = ArtifactRefV2 {
        kind: "program".into(),
        digest: Digest::of(brix_canon::Domain::Value, &raw),
    };
    d.program = Some(reference.clone());
    resolver.artifacts.insert(reference, raw);
    let m = ContextModelV2::new(d, vec![]).unwrap();
    assert_eq!(
        validate_context_model_v2(
            &encode_context_model_v2(&m),
            m.id(),
            m.dimensions(),
            ModelLimits {
                artifact_bytes: 1,
                ..ModelLimits::default()
            },
            &resolver,
            &Accept
        )
        .unwrap_err(),
        ModelError::ArtifactBudgetExceeded
    );
}

#[test]
fn independently_pinned_v2_preimage() {
    let model = ContextModelV2::new(dimensions(None), vec![assumption("x", b"opaque")]).unwrap();
    // Independent Python Appendix-G encoder with literal v2 field order.
    let hex="011b627269782e73656d616e7469632e636f6e746578742d6d6f64656c010200000001016e01017401017001016300000101010900000001056f627340310108726564616374403101086b65726e656c40310108726567696d65403101076178696f6d4031010101090164010a0101010178010c746573742e6279746573403101066f7061717565";
    let expected: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    assert_eq!(encode_context_model_v2(&model), expected);
    assert_eq!(model.id(), ContextId::from_canon(&expected));
    assert_eq!(
        decode_context_model_v2(&expected, ModelLimits::default()).unwrap(),
        model
    );
}

#[test]
fn independently_resolved_world_and_program_changes_cannot_transport() {
    let m = ContextModelV2::new(dimensions(None), vec![]).unwrap();
    let mut store = Resolver::default();
    let src = validate_context_model_v2(
        &encode_context_model_v2(&m),
        m.id(),
        m.dimensions(),
        ModelLimits::default(),
        &store,
        &Accept,
    )
    .unwrap();
    for kind in ["program", "world"] {
        let raw = kind.as_bytes().to_vec();
        let reference = ArtifactRefV2 {
            kind: kind.into(),
            digest: Digest::of(brix_canon::Domain::Value, &raw),
        };
        store.artifacts.insert(reference.clone(), raw);
        let mut d = dimensions(None);
        if kind == "program" {
            d.program = Some(reference)
        } else {
            d.world = Some(reference)
        }
        let target = ContextModelV2::new(d, vec![]).unwrap();
        let dst = validate_context_model_v2(
            &encode_context_model_v2(&target),
            target.id(),
            target.dimensions(),
            ModelLimits::default(),
            &store,
            &Accept,
        )
        .unwrap();
        assert_eq!(
            src.transport_to(&dst, &BTreeSet::new()),
            Err(ModelError::TransportMismatch)
        );
    }
}
