//! Conformance and hostile-input acceptance tests for the Context Contract (Issue #59).
//!
//! Covers representative context contract behavior:
//! - Legacy root compatibility and frozen root digest
//! - Legacy extension identity compatibility
//! - Supported core profile round-trip and independent digest reconstruction
//! - Dimension sensitivity (revisions, profiles, modes, limits)
//! - Duplicate local assumption rejection
//! - Simulation vs Production isolation
//! - Strict bounded hostile input rejection (truncation, trailing bytes, limits)
//! - Parent recursion depth limit and unresolved reference rejection

use brix_canon::{CanonWriter, Digest};
use brix_semantic::context_contract::{
    decode_context_envelope_v1, MAX_CONTEXT_ASSUMPTIONS, MAX_CONTEXT_BYTES, MAX_CONTEXT_DEPTH,
};
use brix_semantic::{
    authenticate_context_envelope_v1, context_id_of_payload_v1, encode_context_envelope_v1,
    validate_context_v1, validate_context_with_parent_resolver, validate_root_context_v1,
    ContextAssumptionV1, ContextBudget, ContextId, ContextPayloadV1, ContextValidationError,
    ExecutionMode, MaterializedCoreContextV1, CONTEXT_FORMAT_V1, CONTEXT_MARKER,
    CONTEXT_PROFILE_CORE_V1,
};

const FROZEN_ROOT_HEX: &str = "a7d1f9a56c727ac00ad5dd6dd97d4af1e943df9f605efcc265248c2c7b355c5c";

fn canon_payload(s: &str) -> Vec<u8> {
    let mut w = CanonWriter::new();
    w.write_str(s);
    w.finish()
}

fn consume_after_validation<F>(
    bytes: &[u8],
    expected_id: &ContextId,
    expected_mode: ExecutionMode,
    parents: &[(ContextId, brix_semantic::ValidatedContextV1)],
    consumer: F,
) -> Result<(), ContextValidationError>
where
    F: FnOnce(&brix_semantic::ValidatedContextV1),
{
    let validated = validate_context_with_parent_resolver(
        bytes,
        expected_id,
        &ContextBudget::strict(),
        |id| {
            parents
                .iter()
                .find(|(parent_id, _)| parent_id == id)
                .map(|(_, h)| h.clone())
        },
    )?;
    validated.require_mode(expected_mode)?;
    consumer(&validated);
    Ok(())
}

#[test]
fn test_legacy_root_compatibility_and_frozen_digest() {
    let payload = ContextPayloadV1::LegacyRoot;
    let bytes = encode_context_envelope_v1(&payload);

    // ContextId of legacy root payload matches ContextId::root() exactly
    let id = ContextId::from_canon(&bytes);
    assert_eq!(id, ContextId::root());
    assert_eq!(id.to_hex(), FROZEN_ROOT_HEX);

    // Validation succeeds against ContextId::root()
    let validated = validate_context_v1(&bytes, &ContextId::root(), &ContextBudget::strict())
        .expect("root context must validate");
    assert_eq!(validated.id(), ContextId::root());
    assert!(validated.is_root());
    assert_eq!(validated.declared_mode(), Some(ExecutionMode::Production));
    assert!(validated.assumptions().is_empty());

    let val_root = validate_root_context_v1(&bytes, &ContextId::root(), &ContextBudget::strict())
        .expect("root context must validate as full context");
    assert!(!val_root.is_simulation());
    assert_eq!(val_root.execution_mode(), ExecutionMode::Production);
    assert!(val_root.require_mode(ExecutionMode::Production).is_ok());
}

#[test]
fn test_legacy_extend_compatibility() {
    let parent = ContextId::root();
    let assumption = b"x : Int";
    let expected_child = parent.extend(assumption);

    let payload = ContextPayloadV1::LegacyExtend {
        parent,
        assumption: assumption.to_vec(),
    };
    let bytes = encode_context_envelope_v1(&payload);

    let id = ContextId::from_canon(&bytes);
    assert_eq!(id, expected_child);
    assert_ne!(id, parent);

    // 1. Envelope authentication succeeds without parent resolution
    let auth = authenticate_context_envelope_v1(&bytes, &expected_child, &ContextBudget::strict())
        .expect("legacy extend envelope must authenticate");
    assert_eq!(auth.id(), expected_child);
    assert_eq!(auth.parent(), Some(parent));
    assert_eq!(auth.declared_mode(), None); // Mode is inherited, unknown until parent resolved

    // 2. Full chain validation with parent resolver
    let root_bytes = encode_context_envelope_v1(&ContextPayloadV1::LegacyRoot);
    let root_val =
        validate_root_context_v1(&root_bytes, &parent, &ContextBudget::strict()).unwrap();
    let validated = validate_context_with_parent_resolver(
        &bytes,
        &expected_child,
        &ContextBudget::strict(),
        |pid| {
            if *pid == parent {
                Some(root_val.clone())
            } else {
                None
            }
        },
    )
    .expect("legacy extend must validate with parent");
    assert_eq!(validated.id(), expected_child);
    assert_eq!(validated.parent(), Some(parent));
    assert_eq!(validated.execution_mode(), ExecutionMode::Production);
}

#[test]
fn test_core_context_independent_primitive_reproduction() {
    let core = MaterializedCoreContextV1 {
        parent: Some(ContextId::root()),
        program_revision: Some(Digest::from_bytes([42u8; 32])),
        world_revision: None,
        assumptions: vec![ContextAssumptionV1::new("binding", canon_payload("Type"))],
        regime_profile: "brix.kernel.profile@1.2".to_string(),
        execution_mode: ExecutionMode::Production,
        step_budget: 500,
        depth_budget: 20,
    };
    let payload = ContextPayloadV1::Core(core);
    let encoder_bytes = encode_context_envelope_v1(&payload);

    // Independent reconstruction via primitive writes matching Spec §2.2:
    let mut w = CanonWriter::new();
    w.write_bytes(b"brix.context.envelope");
    w.write_uint(1);
    w.write_str("brix.context.core@1");
    // Pinned kind ordinal 1 (Spec §2.2)
    w.write_uint(1);
    // parent Some(ContextId::root())
    w.write_enum(1, |w| {
        w.write_bytes(ContextId::root().digest().as_bytes());
    });
    // program_revision Some([42u8; 32])
    w.write_enum(1, |w| {
        w.write_bytes(&[42u8; 32]);
    });
    // world_revision None
    w.write_enum(0, |_| {});
    // assumptions: count 1
    w.write_uint(1);
    w.write_str("binding");
    w.write_bytes(&canon_payload("Type"));
    // regime_profile
    w.write_str("brix.kernel.profile@1.2");
    // execution_mode
    w.write_uint(0); // Production
                     // budgets
    w.write_uint(500);
    w.write_uint(20);
    let primitive_bytes = w.finish();

    assert_eq!(encoder_bytes, primitive_bytes);
    let independent_id = ContextId::from_canon(&primitive_bytes);
    assert_eq!(context_id_of_payload_v1(&payload), independent_id);
}

#[test]
fn test_core_context_encode_decode_round_trip() {
    let core = MaterializedCoreContextV1 {
        parent: Some(ContextId::root()),
        program_revision: Some(Digest::from_bytes([1u8; 32])),
        world_revision: Some(Digest::from_bytes([2u8; 32])),
        assumptions: vec![
            ContextAssumptionV1::new("x", canon_payload("Int")),
            ContextAssumptionV1::new("y", canon_payload("Bool")),
        ],
        regime_profile: "brix.kernel.profile@1.2".to_string(),
        execution_mode: ExecutionMode::Production,
        step_budget: 10_000,
        depth_budget: 100,
    };
    let payload = ContextPayloadV1::Core(core.clone());
    let bytes = encode_context_envelope_v1(&payload);
    let expected_id = context_id_of_payload_v1(&payload);

    // Independent reconstruction of ID directly from canonical bytes
    assert_eq!(ContextId::from_canon(&bytes), expected_id);

    let root_bytes = encode_context_envelope_v1(&ContextPayloadV1::LegacyRoot);
    let root_val =
        validate_root_context_v1(&root_bytes, &ContextId::root(), &ContextBudget::strict())
            .unwrap();

    let validated = validate_context_with_parent_resolver(
        &bytes,
        &expected_id,
        &ContextBudget::strict(),
        |pid| {
            if *pid == ContextId::root() {
                Some(root_val.clone())
            } else {
                None
            }
        },
    )
    .expect("core context must validate with parent");
    assert_eq!(validated.id(), expected_id);
    assert_eq!(validated.profile(), CONTEXT_PROFILE_CORE_V1);
    assert_eq!(validated.assumptions().len(), 2);
    assert_eq!(validated.assumptions()[0].name, "x");
    assert_eq!(validated.assumptions()[1].name, "y");
    assert_eq!(validated.parent(), Some(ContextId::root()));
}

#[test]
fn test_distinct_identities_per_dimension() {
    let base = MaterializedCoreContextV1 {
        parent: None,
        program_revision: None,
        world_revision: None,
        assumptions: vec![ContextAssumptionV1::new("a", b"1".to_vec())],
        regime_profile: "brix.type@1".to_string(),
        execution_mode: ExecutionMode::Production,
        step_budget: 5_000,
        depth_budget: 50,
    };
    let base_id = context_id_of_payload_v1(&ContextPayloadV1::Core(base.clone()));

    // 1. Changing parent
    let mut v_parent = base.clone();
    v_parent.parent = Some(ContextId::root());
    assert_ne!(
        base_id,
        context_id_of_payload_v1(&ContextPayloadV1::Core(v_parent))
    );

    // 2. Changing program revision
    let mut v_prog = base.clone();
    v_prog.program_revision = Some(Digest::from_bytes([9u8; 32]));
    assert_ne!(
        base_id,
        context_id_of_payload_v1(&ContextPayloadV1::Core(v_prog))
    );

    // 3. Changing world revision
    let mut v_world = base.clone();
    v_world.world_revision = Some(Digest::from_bytes([8u8; 32]));
    assert_ne!(
        base_id,
        context_id_of_payload_v1(&ContextPayloadV1::Core(v_world))
    );

    // 4. Changing assumptions
    let mut v_assump = base.clone();
    v_assump.assumptions = vec![ContextAssumptionV1::new("b", b"1".to_vec())];
    assert_ne!(
        base_id,
        context_id_of_payload_v1(&ContextPayloadV1::Core(v_assump))
    );

    // 5. Changing regime profile
    let mut v_regime = base.clone();
    v_regime.regime_profile = "brix.kernel.profile@1.2".to_string();
    assert_ne!(
        base_id,
        context_id_of_payload_v1(&ContextPayloadV1::Core(v_regime))
    );

    // 6. Changing execution mode
    let mut v_mode = base.clone();
    v_mode.execution_mode = ExecutionMode::Simulation;
    assert_ne!(
        base_id,
        context_id_of_payload_v1(&ContextPayloadV1::Core(v_mode))
    );

    // 7. Changing budget
    let mut v_budget = base.clone();
    v_budget.step_budget = 99_999;
    assert_ne!(
        base_id,
        context_id_of_payload_v1(&ContextPayloadV1::Core(v_budget))
    );
}

#[test]
fn test_duplicate_assumption_name_rejection() {
    let core = MaterializedCoreContextV1 {
        parent: None,
        program_revision: None,
        world_revision: None,
        assumptions: vec![
            ContextAssumptionV1::new("x", canon_payload("Int")),
            ContextAssumptionV1::new("x", canon_payload("Str")), // duplicate "x"
        ],
        regime_profile: "brix.type@1".to_string(),
        execution_mode: ExecutionMode::Production,
        step_budget: 1000,
        depth_budget: 10,
    };
    let bytes = encode_context_envelope_v1(&ContextPayloadV1::Core(core));
    let expected_id = ContextId::from_canon(&bytes);

    let err = validate_context_v1(&bytes, &expected_id, &ContextBudget::strict()).unwrap_err();
    assert_eq!(
        err,
        ContextValidationError::DuplicateAssumption("x".to_string())
    );
}

#[test]
fn test_identity_mismatch_fails_closed() {
    let core = MaterializedCoreContextV1 {
        parent: None,
        program_revision: None,
        world_revision: None,
        assumptions: vec![ContextAssumptionV1::new("z", b"Unit".to_vec())],
        regime_profile: "brix.test@1".to_string(),
        execution_mode: ExecutionMode::Production,
        step_budget: 1000,
        depth_budget: 10,
    };
    let bytes = encode_context_envelope_v1(&ContextPayloadV1::Core(core));
    let honest_id = ContextId::from_canon(&bytes);
    let hostile_id = ContextId::root(); // Claiming it is root

    let err = validate_context_v1(&bytes, &hostile_id, &ContextBudget::strict()).unwrap_err();
    assert_eq!(
        err,
        ContextValidationError::IdentityMismatch {
            claimed: honest_id,
            expected: hostile_id,
        }
    );
}

#[test]
fn test_hostile_trailing_bytes_rejection() {
    let payload = ContextPayloadV1::LegacyRoot;
    let mut bytes = encode_context_envelope_v1(&payload);
    bytes.push(0x42); // append junk

    let expected_id = ContextId::from_canon(&bytes);
    let err = validate_context_v1(&bytes, &expected_id, &ContextBudget::strict()).unwrap_err();
    assert_eq!(err, ContextValidationError::TrailingBytes);
}

#[test]
fn test_hostile_oversized_payload_rejection() {
    let budget = ContextBudget {
        max_bytes: 50,
        max_assumptions: 10,
        max_depth: 5,
    };
    let core = MaterializedCoreContextV1 {
        parent: None,
        program_revision: None,
        world_revision: None,
        assumptions: vec![ContextAssumptionV1::new(
            "long_variable_name_here",
            vec![0u8; 100],
        )],
        regime_profile: "brix.kernel.profile@1.2".to_string(),
        execution_mode: ExecutionMode::Production,
        step_budget: 1000,
        depth_budget: 10,
    };
    let bytes = encode_context_envelope_v1(&ContextPayloadV1::Core(core));
    let id = ContextId::from_canon(&bytes);

    let err = validate_context_v1(&bytes, &id, &budget).unwrap_err();
    assert!(matches!(
        err,
        ContextValidationError::PayloadTooLarge { .. }
    ));
}

#[test]
fn test_hostile_assumption_limits() {
    // 1. Empty name
    let mut w = CanonWriter::new();
    w.write_bytes(CONTEXT_MARKER);
    w.write_uint(CONTEXT_FORMAT_V1);
    w.write_str(CONTEXT_PROFILE_CORE_V1);
    w.write_uint(1); // kind 1: MaterializedCoreV1
    w.write_enum(0, |_| {}); // parent None
    w.write_enum(0, |_| {}); // program_revision None
    w.write_enum(0, |_| {}); // world_revision None
    w.write_uint(1); // 1 assumption
    w.write_str(""); // empty name!
    w.write_bytes(b"data");
    w.write_str("brix.test@1");
    w.write_uint(0); // Production
    w.write_uint(100);
    w.write_uint(10);
    let bytes = w.finish();
    let id = ContextId::from_canon(&bytes);
    let err = validate_context_v1(&bytes, &id, &ContextBudget::strict()).unwrap_err();
    assert_eq!(err, ContextValidationError::InvalidAssumptionName);

    // 2. Zero budget
    let mut w = CanonWriter::new();
    w.write_bytes(CONTEXT_MARKER);
    w.write_uint(CONTEXT_FORMAT_V1);
    w.write_str(CONTEXT_PROFILE_CORE_V1);
    w.write_uint(1); // kind 1: MaterializedCoreV1
    w.write_enum(0, |_| {}); // parent None
    w.write_enum(0, |_| {}); // program_revision None
    w.write_enum(0, |_| {}); // world_revision None
    w.write_uint(0); // 0 assumptions
    w.write_str("brix.test@1");
    w.write_uint(0); // Production
    w.write_uint(0); // zero step budget!
    w.write_uint(10);
    let bytes = w.finish();
    let id = ContextId::from_canon(&bytes);
    let err = validate_context_v1(&bytes, &id, &ContextBudget::strict()).unwrap_err();
    assert_eq!(err, ContextValidationError::InvalidBudget);
}

#[test]
fn test_simulation_vs_production_parent_boundary() {
    let sim_parent = MaterializedCoreContextV1 {
        parent: None,
        program_revision: None,
        world_revision: None,
        assumptions: vec![],
        regime_profile: "brix.test@1".to_string(),
        execution_mode: ExecutionMode::Simulation,
        step_budget: 100,
        depth_budget: 10,
    };
    let sim_payload = ContextPayloadV1::Core(sim_parent);
    let sim_bytes = encode_context_envelope_v1(&sim_payload);
    let sim_id = ContextId::from_canon(&sim_bytes);
    let validated_sim_parent =
        validate_root_context_v1(&sim_bytes, &sim_id, &ContextBudget::strict()).unwrap();

    // Child claiming Production while inheriting from Simulation parent
    let prod_child = MaterializedCoreContextV1 {
        parent: Some(sim_id),
        program_revision: None,
        world_revision: None,
        assumptions: vec![],
        regime_profile: "brix.test@1".to_string(),
        execution_mode: ExecutionMode::Production,
        step_budget: 100,
        depth_budget: 10,
    };
    let child_payload = ContextPayloadV1::Core(prod_child);
    let child_bytes = encode_context_envelope_v1(&child_payload);
    let child_id = ContextId::from_canon(&child_bytes);

    let err = validate_context_with_parent_resolver(
        &child_bytes,
        &child_id,
        &ContextBudget::strict(),
        |pid| {
            if *pid == sim_id {
                Some(validated_sim_parent.clone())
            } else {
                None
            }
        },
    )
    .unwrap_err();

    assert_eq!(
        err,
        ContextValidationError::ModeMismatch {
            claimed: ExecutionMode::Production,
            expected: ExecutionMode::Simulation,
        }
    );
}

#[test]
fn test_depth_limit_and_unresolved_parent() {
    let budget = ContextBudget {
        max_bytes: 65536,
        max_assumptions: 256,
        max_depth: 2,
    };

    let p0 = ContextId::root();
    let p1 = p0.extend(b"step1");
    let p2 = p1.extend(b"step2");
    let p3 = p2.extend(b"step3");

    let child = MaterializedCoreContextV1 {
        parent: Some(p3),
        program_revision: None,
        world_revision: None,
        assumptions: vec![],
        regime_profile: "brix.kernel.profile@1.2".to_string(),
        execution_mode: ExecutionMode::Production,
        step_budget: 100,
        depth_budget: 10,
    };
    let bytes = encode_context_envelope_v1(&ContextPayloadV1::Core(child));
    let id = ContextId::from_canon(&bytes);

    // 1. Unresolved parent
    let err = validate_context_with_parent_resolver(&bytes, &id, &budget, |_| None).unwrap_err();
    assert_eq!(err, ContextValidationError::UnresolvedParent(p3));

    // 2. Depth limit exceeded
    let (id_p0, val_p0) = {
        let payload = ContextPayloadV1::Core(MaterializedCoreContextV1 {
            parent: None,
            program_revision: None,
            world_revision: None,
            assumptions: vec![],
            regime_profile: "brix.kernel.profile@1.2".to_string(),
            execution_mode: ExecutionMode::Production,
            step_budget: 100,
            depth_budget: 10,
        });
        let b = encode_context_envelope_v1(&payload);
        let id = ContextId::from_canon(&b);
        let val = validate_root_context_v1(&b, &id, &ContextBudget::strict()).unwrap();
        (id, val)
    };

    let (id_p1, val_p1) = {
        let payload = ContextPayloadV1::Core(MaterializedCoreContextV1 {
            parent: Some(id_p0),
            program_revision: None,
            world_revision: None,
            assumptions: vec![],
            regime_profile: "brix.kernel.profile@1.2".to_string(),
            execution_mode: ExecutionMode::Production,
            step_budget: 100,
            depth_budget: 10,
        });
        let b = encode_context_envelope_v1(&payload);
        let id = ContextId::from_canon(&b);
        let val = validate_context_with_parent_resolver(&b, &id, &ContextBudget::strict(), |pid| {
            if *pid == id_p0 {
                Some(val_p0.clone())
            } else {
                None
            }
        })
        .unwrap();
        (id, val)
    };

    let (id_p2, val_p2) = {
        let payload = ContextPayloadV1::Core(MaterializedCoreContextV1 {
            parent: Some(id_p1),
            program_revision: None,
            world_revision: None,
            assumptions: vec![],
            regime_profile: "brix.kernel.profile@1.2".to_string(),
            execution_mode: ExecutionMode::Production,
            step_budget: 100,
            depth_budget: 10,
        });
        let b = encode_context_envelope_v1(&payload);
        let id = ContextId::from_canon(&b);
        let val = validate_context_with_parent_resolver(&b, &id, &ContextBudget::strict(), |pid| {
            if *pid == id_p1 {
                Some(val_p1.clone())
            } else if *pid == id_p0 {
                Some(val_p0.clone())
            } else {
                None
            }
        })
        .unwrap();
        (id, val)
    };

    let (id_p3, val_p3) = {
        let payload = ContextPayloadV1::Core(MaterializedCoreContextV1 {
            parent: Some(id_p2),
            program_revision: None,
            world_revision: None,
            assumptions: vec![],
            regime_profile: "brix.kernel.profile@1.2".to_string(),
            execution_mode: ExecutionMode::Production,
            step_budget: 100,
            depth_budget: 10,
        });
        let b = encode_context_envelope_v1(&payload);
        let id = ContextId::from_canon(&b);
        let val = validate_context_with_parent_resolver(&b, &id, &ContextBudget::strict(), |pid| {
            if *pid == id_p2 {
                Some(val_p2.clone())
            } else if *pid == id_p1 {
                Some(val_p1.clone())
            } else if *pid == id_p0 {
                Some(val_p0.clone())
            } else {
                None
            }
        })
        .unwrap();
        (id, val)
    };

    let deep_child = MaterializedCoreContextV1 {
        parent: Some(id_p3),
        program_revision: None,
        world_revision: None,
        assumptions: vec![],
        regime_profile: "brix.kernel.profile@1.2".to_string(),
        execution_mode: ExecutionMode::Production,
        step_budget: 100,
        depth_budget: 10,
    };
    let deep_bytes = encode_context_envelope_v1(&ContextPayloadV1::Core(deep_child));
    let deep_id = ContextId::from_canon(&deep_bytes);

    let err_depth = validate_context_with_parent_resolver(&deep_bytes, &deep_id, &budget, |pid| {
        if *pid == id_p3 {
            Some(val_p3.clone())
        } else if *pid == id_p2 {
            Some(val_p2.clone())
        } else if *pid == id_p1 {
            Some(val_p1.clone())
        } else if *pid == id_p0 {
            Some(val_p0.clone())
        } else {
            None
        }
    })
    .unwrap_err();

    assert_eq!(
        err_depth,
        ContextValidationError::DepthLimitExceeded { depth: 3, max: 2 }
    );
}

#[test]
fn test_invalid_assumption_identifier_characters() {
    let invalid_names = [
        "1starts_with_digit",
        "has space",
        "has\nnewline",
        "has\0null",
        "has-dash",
        "@special",
    ];

    for bad_name in invalid_names {
        let core = MaterializedCoreContextV1 {
            parent: None,
            program_revision: None,
            world_revision: None,
            assumptions: vec![ContextAssumptionV1::new(bad_name, b"val".to_vec())],
            regime_profile: "brix.type@1".to_string(),
            execution_mode: ExecutionMode::Production,
            step_budget: 1000,
            depth_budget: 10,
        };
        let bytes = encode_context_envelope_v1(&ContextPayloadV1::Core(core));
        let id = ContextId::from_canon(&bytes);

        let err = validate_context_v1(&bytes, &id, &ContextBudget::strict()).unwrap_err();
        assert_eq!(
            err,
            ContextValidationError::InvalidAssumptionName,
            "failed to reject bad name: {bad_name}"
        );
    }
}

#[test]
fn test_budget_overflow_rejected() {
    // 1. Step budget too large
    let core1 = MaterializedCoreContextV1 {
        parent: None,
        program_revision: None,
        world_revision: None,
        assumptions: vec![],
        regime_profile: "brix.type@1".to_string(),
        execution_mode: ExecutionMode::Production,
        step_budget: 1_000_000_001, // exceeds MAX_ALLOWED_STEP_BUDGET
        depth_budget: 10,
    };
    let b1 = encode_context_envelope_v1(&ContextPayloadV1::Core(core1));
    let id1 = ContextId::from_canon(&b1);
    let err1 = validate_context_v1(&b1, &id1, &ContextBudget::strict()).unwrap_err();
    assert_eq!(err1, ContextValidationError::InvalidBudget);

    // 2. Depth budget too large
    let core2 = MaterializedCoreContextV1 {
        parent: None,
        program_revision: None,
        world_revision: None,
        assumptions: vec![],
        regime_profile: "brix.type@1".to_string(),
        execution_mode: ExecutionMode::Production,
        step_budget: 1000,
        depth_budget: 100_001, // exceeds MAX_ALLOWED_DEPTH_BUDGET
    };
    let b2 = encode_context_envelope_v1(&ContextPayloadV1::Core(core2));
    let id2 = ContextId::from_canon(&b2);
    let err2 = validate_context_v1(&b2, &id2, &ContextBudget::strict()).unwrap_err();
    assert_eq!(err2, ContextValidationError::InvalidBudget);
}

#[test]
fn test_require_mode_enforcement() {
    let sim_core = MaterializedCoreContextV1 {
        parent: None,
        program_revision: None,
        world_revision: None,
        assumptions: vec![],
        regime_profile: "brix.test@1".to_string(),
        execution_mode: ExecutionMode::Simulation,
        step_budget: 100,
        depth_budget: 10,
    };
    let bytes = encode_context_envelope_v1(&ContextPayloadV1::Core(sim_core));
    let id = ContextId::from_canon(&bytes);
    let val_auth = validate_context_v1(&bytes, &id, &ContextBudget::strict()).unwrap();
    assert_eq!(val_auth.declared_mode(), Some(ExecutionMode::Simulation));

    let validated = validate_root_context_v1(&bytes, &id, &ContextBudget::strict()).unwrap();

    assert!(validated.is_simulation());
    assert_eq!(validated.execution_mode(), ExecutionMode::Simulation);

    // Enforcing production fails closed
    let err = validated
        .require_mode(ExecutionMode::Production)
        .unwrap_err();
    assert_eq!(
        err,
        ContextValidationError::ModeMismatch {
            claimed: ExecutionMode::Simulation,
            expected: ExecutionMode::Production,
        }
    );

    // Enforcing simulation succeeds
    assert!(validated.require_mode(ExecutionMode::Simulation).is_ok());
}

// --- Regression & Adversarial Probes from Review ---

#[test]
fn test_wrong_parent_handle_is_rejected() {
    let sim_core = MaterializedCoreContextV1 {
        parent: None,
        program_revision: None,
        world_revision: None,
        assumptions: vec![],
        regime_profile: "brix.test@1".into(),
        execution_mode: ExecutionMode::Simulation,
        step_budget: 100,
        depth_budget: 10,
    };
    let sim_bytes = encode_context_envelope_v1(&ContextPayloadV1::Core(sim_core));
    let sim_id = ContextId::from_canon(&sim_bytes);
    let _sim_handle =
        validate_root_context_v1(&sim_bytes, &sim_id, &ContextBudget::strict()).unwrap();

    let root_bytes = encode_context_envelope_v1(&ContextPayloadV1::LegacyRoot);
    let root_id = ContextId::root();
    let root_handle =
        validate_root_context_v1(&root_bytes, &root_id, &ContextBudget::strict()).unwrap();

    let child_core = MaterializedCoreContextV1 {
        parent: Some(sim_id),
        program_revision: None,
        world_revision: None,
        assumptions: vec![],
        regime_profile: "brix.test@1".into(),
        execution_mode: ExecutionMode::Production,
        step_budget: 100,
        depth_budget: 10,
    };
    let child_bytes = encode_context_envelope_v1(&ContextPayloadV1::Core(child_core));
    let child_id = ContextId::from_canon(&child_bytes);

    // Returning root_handle when sim_id was requested must fail with IdentityMismatch
    let err = validate_context_with_parent_resolver(
        &child_bytes,
        &child_id,
        &ContextBudget::strict(),
        |requested| {
            assert_eq!(*requested, sim_id);
            assert_ne!(root_handle.id(), *requested);
            Some(root_handle.clone())
        },
    )
    .unwrap_err();

    assert_eq!(
        err,
        ContextValidationError::IdentityMismatch {
            claimed: root_handle.id(),
            expected: sim_id,
        }
    );
}

#[test]
fn test_parent_checks_cannot_be_skipped_with_same_type() {
    let missing = ContextId::from_canon(b"unavailable parent");
    let child_core = MaterializedCoreContextV1 {
        parent: Some(missing),
        program_revision: None,
        world_revision: None,
        assumptions: vec![],
        regime_profile: "brix.test@1".into(),
        execution_mode: ExecutionMode::Production,
        step_budget: 100,
        depth_budget: 10,
    };
    let child_bytes = encode_context_envelope_v1(&ContextPayloadV1::Core(child_core));
    let child_id = ContextId::from_canon(&child_bytes);

    // 1. Envelope authenticates but has no require_mode method
    let auth = validate_context_v1(&child_bytes, &child_id, &ContextBudget::strict()).unwrap();
    assert_eq!(auth.id(), child_id);
    assert_eq!(auth.declared_mode(), Some(ExecutionMode::Production));

    // 2. Full validation without resolver fails closed with UnresolvedParent
    let err = validate_context_with_parent_resolver(
        &child_bytes,
        &child_id,
        &ContextBudget::strict(),
        |_| None,
    )
    .unwrap_err();
    assert_eq!(err, ContextValidationError::UnresolvedParent(missing));
}

#[test]
fn test_legacy_extension_of_simulation_inherits_simulation_mode() {
    let sim_core = MaterializedCoreContextV1 {
        parent: None,
        program_revision: None,
        world_revision: None,
        assumptions: vec![],
        regime_profile: "brix.test@1".into(),
        execution_mode: ExecutionMode::Simulation,
        step_budget: 100,
        depth_budget: 10,
    };
    let sim_bytes = encode_context_envelope_v1(&ContextPayloadV1::Core(sim_core));
    let sim_id = ContextId::from_canon(&sim_bytes);
    let sim_handle =
        validate_root_context_v1(&sim_bytes, &sim_id, &ContextBudget::strict()).unwrap();

    let ext_bytes = encode_context_envelope_v1(&ContextPayloadV1::LegacyExtend {
        parent: sim_id,
        assumption: b"x : Int".to_vec(),
    });
    let ext_id = ContextId::from_canon(&ext_bytes);

    // Authenticating envelope succeeds but has no declared mode
    let auth = validate_context_v1(&ext_bytes, &ext_id, &ContextBudget::strict()).unwrap();
    assert_eq!(auth.declared_mode(), None);

    // Validating without resolver fails closed
    assert_eq!(
        validate_context_with_parent_resolver(
            &ext_bytes,
            &ext_id,
            &ContextBudget::strict(),
            |_| None
        )
        .unwrap_err(),
        ContextValidationError::UnresolvedParent(sim_id)
    );

    // Validating with resolver inherits Simulation mode
    let validated = validate_context_with_parent_resolver(
        &ext_bytes,
        &ext_id,
        &ContextBudget::strict(),
        |pid| {
            if *pid == sim_id {
                Some(sim_handle.clone())
            } else {
                None
            }
        },
    )
    .unwrap();

    assert!(validated.is_simulation());
    assert_eq!(validated.execution_mode(), ExecutionMode::Simulation);

    // Requiring production fails closed
    assert_eq!(
        validated
            .require_mode(ExecutionMode::Production)
            .unwrap_err(),
        ContextValidationError::ModeMismatch {
            claimed: ExecutionMode::Simulation,
            expected: ExecutionMode::Production,
        }
    );
}

#[test]
fn test_opaque_noncanonical_assumption_and_unknown_regime_are_rejected() {
    // A canonically framed arbitrary string passes structural validation. This layer does
    // not establish that the bytes encode a well-typed assumption.
    let opaque = MaterializedCoreContextV1 {
        parent: None,
        program_revision: None,
        world_revision: None,
        assumptions: vec![ContextAssumptionV1::new(
            "x",
            canon_payload("not type checked"),
        )],
        regime_profile: "brix.test@1".into(),
        execution_mode: ExecutionMode::Production,
        step_budget: 100,
        depth_budget: 10,
    };
    let opaque_bytes = encode_context_envelope_v1(&ContextPayloadV1::Core(opaque));
    let opaque_id = ContextId::from_canon(&opaque_bytes);
    assert!(validate_context_v1(&opaque_bytes, &opaque_id, &ContextBudget::strict()).is_ok());

    // 1. Malformed non-canonical assumption bytes
    let bad_assump = MaterializedCoreContextV1 {
        parent: None,
        program_revision: None,
        world_revision: None,
        assumptions: vec![ContextAssumptionV1::new("x", vec![255, 255, 255])],
        regime_profile: "brix.test@1".into(),
        execution_mode: ExecutionMode::Production,
        step_budget: 100,
        depth_budget: 10,
    };
    let (bytes1, id1) = {
        let b = encode_context_envelope_v1(&ContextPayloadV1::Core(bad_assump));
        let id = ContextId::from_canon(&b);
        (b, id)
    };
    assert_eq!(
        validate_context_v1(&bytes1, &id1, &ContextBudget::strict()).unwrap_err(),
        ContextValidationError::InvalidAssumptionPayload
    );

    // 2. Unconsumed trailing bytes in assumption payload also rejected
    let bad_trailing = MaterializedCoreContextV1 {
        parent: None,
        program_revision: None,
        world_revision: None,
        assumptions: vec![ContextAssumptionV1::new("x", vec![1, b'a', 255, 255])],
        regime_profile: "brix.test@1".into(),
        execution_mode: ExecutionMode::Production,
        step_budget: 100,
        depth_budget: 10,
    };
    let (b_trail, id_trail) = {
        let b = encode_context_envelope_v1(&ContextPayloadV1::Core(bad_trailing));
        let id = ContextId::from_canon(&b);
        (b, id)
    };
    assert_eq!(
        validate_context_v1(&b_trail, &id_trail, &ContextBudget::strict()).unwrap_err(),
        ContextValidationError::InvalidAssumptionPayload
    );

    // 3. Malformed regime profile with NUL and emojis
    let bad_regime = MaterializedCoreContextV1 {
        parent: None,
        program_revision: None,
        world_revision: None,
        assumptions: vec![],
        regime_profile: "unsupported\0regime\u{1f600}".into(),
        execution_mode: ExecutionMode::Production,
        step_budget: 100,
        depth_budget: 10,
    };
    let (bytes2, id2) = {
        let b = encode_context_envelope_v1(&ContextPayloadV1::Core(bad_regime));
        let id = ContextId::from_canon(&b);
        (b, id)
    };
    assert!(matches!(
        validate_context_v1(&bytes2, &id2, &ContextBudget::strict()).unwrap_err(),
        ContextValidationError::MalformedRegimeProfile(_)
    ));

    // 4. Unknown ASCII regime profile rejected by allowlist
    let unknown_regime = MaterializedCoreContextV1 {
        parent: None,
        program_revision: None,
        world_revision: None,
        assumptions: vec![],
        regime_profile: "brix.unknown@1".into(),
        execution_mode: ExecutionMode::Production,
        step_budget: 100,
        depth_budget: 10,
    };
    let (bytes3, id3) = {
        let b = encode_context_envelope_v1(&ContextPayloadV1::Core(unknown_regime));
        let id = ContextId::from_canon(&b);
        (b, id)
    };
    assert_eq!(
        validate_context_v1(&bytes3, &id3, &ContextBudget::strict()).unwrap_err(),
        ContextValidationError::UnsupportedRegimeProfile("brix.unknown@1".into())
    );
}

#[test]
fn test_specification_kind_discriminant_accepted() {
    let mut w = CanonWriter::new();
    w.write_bytes(b"brix.context.envelope");
    w.write_uint(1);
    w.write_str("brix.context.core@1");
    w.write_enum(1, |w| {
        // Spec section 2.2 MaterializedCoreV1 kind
        w.write_enum(0, |_| {});
        w.write_enum(0, |_| {});
        w.write_enum(0, |_| {});
        w.write_uint(0);
        w.write_str("brix.test@1");
        w.write_uint(0);
        w.write_uint(100);
        w.write_uint(10);
    });
    let bytes = w.finish();
    let id = ContextId::from_canon(&bytes);
    let val = validate_context_v1(&bytes, &id, &ContextBudget::strict()).unwrap();
    assert_eq!(val.id(), id);
    assert_eq!(val.profile(), "brix.context.core@1");
}

#[test]
fn legacy_extension_checks_zero_depth_before_resolving_parent() {
    let payload = ContextPayloadV1::LegacyExtend {
        parent: ContextId::root(),
        assumption: b"x : Int".to_vec(),
    };
    let bytes = encode_context_envelope_v1(&payload);
    let id = ContextId::from_canon(&bytes);
    let budget = ContextBudget {
        max_bytes: usize::MAX,
        max_assumptions: usize::MAX,
        max_depth: 0,
    };
    let resolver_calls = std::cell::Cell::new(0);

    assert_eq!(
        validate_context_with_parent_resolver(&bytes, &id, &budget, |_| {
            resolver_calls.set(resolver_calls.get() + 1);
            None
        })
        .unwrap_err(),
        ContextValidationError::DepthLimitExceeded { depth: 1, max: 0 }
    );
    assert_eq!(resolver_calls.get(), 0);
}

#[test]
fn exact_depth_boundary_passes_for_core_legacy_and_mixed_chains() {
    for styles in [
        vec![false, false, false], // core
        vec![true, true, true],    // legacy
        vec![false, true, false],  // mixed
    ] {
        let root_bytes = encode_context_envelope_v1(&ContextPayloadV1::LegacyRoot);
        let root =
            validate_root_context_v1(&root_bytes, &ContextId::root(), &ContextBudget::strict())
                .unwrap();
        let mut handles = vec![(root.id(), root)];
        let mut parent_id = ContextId::root();
        let mut leaf_bytes = Vec::new();
        let mut leaf_id = parent_id;

        for (index, legacy) in styles.iter().copied().enumerate() {
            let assumption = format!("edge-{index}").into_bytes();
            let payload = if legacy {
                ContextPayloadV1::LegacyExtend {
                    parent: parent_id,
                    assumption,
                }
            } else {
                ContextPayloadV1::Core(MaterializedCoreContextV1 {
                    parent: Some(parent_id),
                    program_revision: None,
                    world_revision: None,
                    assumptions: vec![],
                    regime_profile: "brix.test@1".into(),
                    execution_mode: ExecutionMode::Production,
                    step_budget: 100,
                    depth_budget: 10,
                })
            };
            leaf_bytes = encode_context_envelope_v1(&payload);
            leaf_id = ContextId::from_canon(&leaf_bytes);
            let handle = validate_context_with_parent_resolver(
                &leaf_bytes,
                &leaf_id,
                &ContextBudget::strict(),
                |id| {
                    handles
                        .iter()
                        .find(|(known, _)| known == id)
                        .map(|(_, h)| h.clone())
                },
            )
            .unwrap();
            parent_id = leaf_id;
            handles.push((leaf_id, handle));
        }

        let exact = ContextBudget {
            max_depth: styles.len(),
            ..ContextBudget::strict()
        };
        assert!(
            validate_context_with_parent_resolver(&leaf_bytes, &leaf_id, &exact, |id| {
                handles
                    .iter()
                    .find(|(known, _)| known == id)
                    .map(|(_, h)| h.clone())
            })
            .is_ok()
        );
        let short = ContextBudget {
            max_depth: styles.len() - 1,
            ..exact
        };
        assert_eq!(
            validate_context_with_parent_resolver(&leaf_bytes, &leaf_id, &short, |id| {
                handles
                    .iter()
                    .find(|(known, _)| known == id)
                    .map(|(_, h)| h.clone())
            })
            .unwrap_err(),
            ContextValidationError::DepthLimitExceeded {
                depth: styles.len(),
                max: styles.len() - 1
            }
        );
    }
}

#[test]
fn loose_budgets_cannot_exceed_hard_caps() {
    let loose = ContextBudget {
        max_bytes: usize::MAX,
        max_assumptions: usize::MAX,
        max_depth: usize::MAX,
    };
    let oversized = vec![0; MAX_CONTEXT_BYTES + 1];
    assert_eq!(
        authenticate_context_envelope_v1(&oversized, &ContextId::from_canon(&oversized), &loose)
            .unwrap_err(),
        ContextValidationError::PayloadTooLarge {
            length: MAX_CONTEXT_BYTES + 1,
            max: MAX_CONTEXT_BYTES
        }
    );
    assert_eq!(
        decode_context_envelope_v1(&oversized, &loose).unwrap_err(),
        ContextValidationError::PayloadTooLarge {
            length: MAX_CONTEXT_BYTES + 1,
            max: MAX_CONTEXT_BYTES
        }
    );

    let many = MaterializedCoreContextV1 {
        parent: None,
        program_revision: None,
        world_revision: None,
        assumptions: (0..=MAX_CONTEXT_ASSUMPTIONS)
            .map(|i| ContextAssumptionV1::new(format!("v{i}"), canon_payload("Int")))
            .collect(),
        regime_profile: "brix.test@1".into(),
        execution_mode: ExecutionMode::Production,
        step_budget: 100,
        depth_budget: 10,
    };
    let many_bytes = encode_context_envelope_v1(&ContextPayloadV1::Core(many));
    let many_id = ContextId::from_canon(&many_bytes);
    assert_eq!(
        authenticate_context_envelope_v1(&many_bytes, &many_id, &loose).unwrap_err(),
        ContextValidationError::ExcessiveAssumptions {
            count: MAX_CONTEXT_ASSUMPTIONS + 1,
            max: MAX_CONTEXT_ASSUMPTIONS
        }
    );

    let root_bytes = encode_context_envelope_v1(&ContextPayloadV1::LegacyRoot);
    let root = validate_root_context_v1(&root_bytes, &ContextId::root(), &ContextBudget::strict())
        .unwrap();
    let mut handles = vec![root.clone()];
    let mut previous = root;
    let mut leaf_bytes = Vec::new();
    let mut leaf_id = ContextId::root();
    for i in 0..=MAX_CONTEXT_DEPTH {
        let payload = ContextPayloadV1::Core(MaterializedCoreContextV1 {
            parent: Some(previous.id()),
            program_revision: None,
            world_revision: None,
            assumptions: vec![],
            regime_profile: "brix.test@1".into(),
            execution_mode: ExecutionMode::Production,
            step_budget: 100,
            depth_budget: 10,
        });
        leaf_bytes = encode_context_envelope_v1(&payload);
        leaf_id = ContextId::from_canon(&leaf_bytes);
        if i < MAX_CONTEXT_DEPTH {
            previous = validate_context_with_parent_resolver(&leaf_bytes, &leaf_id, &loose, |id| {
                handles.iter().find(|handle| handle.id() == *id).cloned()
            })
            .unwrap();
            handles.push(previous.clone());
        }
    }
    assert_eq!(
        validate_context_with_parent_resolver(&leaf_bytes, &leaf_id, &loose, |id| {
            handles.iter().find(|handle| handle.id() == *id).cloned()
        })
        .unwrap_err(),
        ContextValidationError::DepthLimitExceeded {
            depth: MAX_CONTEXT_DEPTH + 1,
            max: MAX_CONTEXT_DEPTH
        }
    );
}

#[test]
fn fixture_consumer_requires_identity_parent_profile_and_mode() {
    // Test-only adapter fixture; this is not a production consumer integration.
    let root_bytes = encode_context_envelope_v1(&ContextPayloadV1::LegacyRoot);
    let root = validate_root_context_v1(&root_bytes, &ContextId::root(), &ContextBudget::strict())
        .unwrap();
    let called = std::cell::Cell::new(false);
    let ok = consume_after_validation(
        &root_bytes,
        &root.id(),
        ExecutionMode::Production,
        &[],
        |_| called.set(true),
    );
    assert!(ok.is_ok());
    assert!(called.get());

    called.set(false);
    assert!(consume_after_validation(
        &root_bytes,
        &ContextId::from_canon(b"wrong"),
        ExecutionMode::Production,
        &[],
        |_| called.set(true)
    )
    .is_err());
    assert!(!called.get());

    let child_payload = ContextPayloadV1::LegacyExtend {
        parent: root.id(),
        assumption: b"a".to_vec(),
    };
    let child_bytes = encode_context_envelope_v1(&child_payload);
    let child_id = ContextId::from_canon(&child_bytes);
    assert!(consume_after_validation(
        &child_bytes,
        &child_id,
        ExecutionMode::Production,
        &[],
        |_| called.set(true)
    )
    .is_err());
    assert!(!called.get());

    let mut unknown = CanonWriter::new();
    unknown.write_bytes(CONTEXT_MARKER);
    unknown.write_uint(CONTEXT_FORMAT_V1);
    unknown.write_str("brix.context.unknown@1");
    let unknown_bytes = unknown.finish();
    let unknown_id = ContextId::from_canon(&unknown_bytes);
    assert!(consume_after_validation(
        &unknown_bytes,
        &unknown_id,
        ExecutionMode::Production,
        &[],
        |_| called.set(true)
    )
    .is_err());
    assert!(!called.get());

    assert!(consume_after_validation(
        &root_bytes,
        &root.id(),
        ExecutionMode::Simulation,
        &[],
        |_| called.set(true)
    )
    .is_err());
    assert!(!called.get());
}
