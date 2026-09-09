//! Tests for external input contract, decoding, limits, lowering, and identity (ADR-0031).

use brix_lower::finite_decision::{
    finite_decision_audit_environment_from_plan,
    finite_decision_audit_environment_from_plan_with_inputs, finite_decision_program_id,
    lower_finite_decision_plan, run_finite_decision_plan, run_finite_decision_plan_with_inputs,
    BoundInput, FiniteDecisionBuildError, FiniteDecisionLowerError, FiniteDecisionPlan,
    FiniteDecisionRuntime, FiniteDecisionUnknownReason, FINITE_DECISION_PROFILE,
};
use brix_lower::input::{
    canonicalize_input_shards, decode_input_shard, decode_input_shard_from_file, input_context_id,
    load_input_snapshot_from_paths, validate_against_declarations, validate_completeness,
    InputDecodeError, InputError, InputLimits, InputScalarValue, InputSnapshot,
    InputValidationError, MAX_INPUT_NAME_BYTES,
};
use brix_lower::l3_v2::L3ValueV2;
use brix_lower::{L3ValueType, Outcome};
use brix_syntax::parse;

fn plan(source: &str) -> FiniteDecisionPlan {
    let module = parse(source).expect("fixture parses");
    lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).expect("fixture lowers")
}

fn snapshot_from_json(json: &str) -> InputSnapshot {
    let limits = InputLimits::default();
    let shard = decode_input_shard(json.as_bytes(), &limits).expect("shard decodes");
    canonicalize_input_shards(vec![shard], &limits).expect("snapshot canonicalizes")
}

// ---------------------------------------------------------------------------
// 1. Valid Scalar Decoding (ADR-0031 ⟨D-TYPES⟩, ⟨D-SCHEMA⟩)
// ---------------------------------------------------------------------------

#[test]
fn test_decode_valid_scalars() {
    let json = r#"{
        "schema": "brix.input@1",
        "values": {
            "max_attempts": {"type": "int", "value": "100"},
            "min_val": {"type": "int", "value": "-9223372036854775808"},
            "max_val": {"type": "int", "value": "9223372036854775807"},
            "is_active": {"type": "bool", "value": true},
            "is_dry_run": {"type": "bool", "value": false},
            "greeting": {"type": "string", "value": "hello world"}
        }
    }"#;
    let limits = InputLimits::default();
    let shard = decode_input_shard(json.as_bytes(), &limits).expect("valid shard should decode");

    assert_eq!(shard.schema(), "brix.input@1");
    assert_eq!(shard.values().len(), 6);

    assert_eq!(shard.get("max_attempts"), Some(&InputScalarValue::Int(100)));
    assert_eq!(shard.get("min_val"), Some(&InputScalarValue::Int(i64::MIN)));
    assert_eq!(shard.get("max_val"), Some(&InputScalarValue::Int(i64::MAX)));
    assert_eq!(shard.get("is_active"), Some(&InputScalarValue::Bool(true)));
    assert_eq!(
        shard.get("is_dry_run"),
        Some(&InputScalarValue::Bool(false))
    );
    assert_eq!(
        shard.get("greeting"),
        Some(&InputScalarValue::Str("hello world".into()))
    );
}

// ---------------------------------------------------------------------------
// 2. Duplicate Key Rejection (ADR-0031 ⟨D-NODUPKEYS⟩)
// ---------------------------------------------------------------------------

#[test]
fn test_reject_duplicate_key_in_envelope() {
    let json = r#"{
        "schema": "brix.input@1",
        "schema": "brix.input@1",
        "values": {}
    }"#;
    let limits = InputLimits::default();
    let err = decode_input_shard(json.as_bytes(), &limits)
        .expect_err("must reject duplicate envelope key");
    assert!(
        matches!(err, InputDecodeError::DuplicateKey { ref key, .. } if key == "schema"),
        "expected DuplicateKey(\"schema\"), got {:?}",
        err
    );
}

#[test]
fn test_reject_duplicate_key_in_values() {
    let json = r#"{
        "schema": "brix.input@1",
        "values": {
            "port": {"type": "int", "value": "80"},
            "port": {"type": "int", "value": "443"}
        }
    }"#;
    let limits = InputLimits::default();
    let err =
        decode_input_shard(json.as_bytes(), &limits).expect_err("must reject duplicate values key");
    assert!(
        matches!(err, InputDecodeError::DuplicateKey { ref key, .. } if key == "port"),
        "expected DuplicateKey(\"port\"), got {:?}",
        err
    );
}

#[test]
fn test_reject_duplicate_key_in_tagged_value() {
    let json = r#"{
        "schema": "brix.input@1",
        "values": {
            "flag": {
                "type": "bool",
                "type": "bool",
                "value": true
            }
        }
    }"#;
    let limits = InputLimits::default();
    let err = decode_input_shard(json.as_bytes(), &limits)
        .expect_err("must reject duplicate tagged value key");
    assert!(
        matches!(err, InputDecodeError::DuplicateKey { ref key, .. } if key == "type"),
        "expected DuplicateKey(\"type\"), got {:?}",
        err
    );
}

// ---------------------------------------------------------------------------
// 3. Strict Schema and Format Validation (ADR-0031 ⟨D-SCHEMA⟩)
// ---------------------------------------------------------------------------

#[test]
fn test_reject_unsupported_or_missing_schema() {
    let limits = InputLimits::default();

    let missing_schema = r#"{"values": {}}"#;
    let err = decode_input_shard(missing_schema.as_bytes(), &limits).unwrap_err();
    assert!(matches!(err, InputDecodeError::MissingField("schema")));

    let wrong_schema = r#"{"schema": "brix.input@2", "values": {}}"#;
    let err = decode_input_shard(wrong_schema.as_bytes(), &limits).unwrap_err();
    assert!(
        matches!(err, InputDecodeError::InvalidSchema { ref found, .. } if found == "brix.input@2")
    );
}

#[test]
fn test_reject_missing_values() {
    let limits = InputLimits::default();
    let no_values = r#"{"schema": "brix.input@1"}"#;
    let err = decode_input_shard(no_values.as_bytes(), &limits).unwrap_err();
    assert!(matches!(err, InputDecodeError::MissingField("values")));
}

#[test]
fn test_reject_unexpected_envelope_fields() {
    let limits = InputLimits::default();
    let extra = r#"{"schema": "brix.input@1", "values": {}, "comment": "test"}"#;
    let err = decode_input_shard(extra.as_bytes(), &limits).unwrap_err();
    assert!(matches!(err, InputDecodeError::UnknownField { ref field, .. } if field == "comment"));
}

#[test]
fn test_reject_unexpected_tagged_value_fields() {
    let limits = InputLimits::default();
    let extra = r#"{
        "schema": "brix.input@1",
        "values": {
            "count": {"type": "int", "value": "5", "unit": "items"}
        }
    }"#;
    let err = decode_input_shard(extra.as_bytes(), &limits).unwrap_err();
    assert!(matches!(err, InputDecodeError::UnknownField { ref field, .. } if field == "unit"));
}

#[test]
fn test_reject_unsupported_value_type_or_mismatched_payload() {
    let limits = InputLimits::default();

    // Float type is forbidden in wave 1 (ADR-0031 ⟨D-TYPES⟩)
    let float_val = r#"{
        "schema": "brix.input@1",
        "values": {
            "ratio": {"type": "float", "value": "1.5"}
        }
    }"#;
    let err = decode_input_shard(float_val.as_bytes(), &limits).unwrap_err();
    assert!(matches!(err, InputDecodeError::InvalidType { ref found, .. } if found == "float"));

    // Bool type with string value
    let bool_as_str = r#"{
        "schema": "brix.input@1",
        "values": {
            "flag": {"type": "bool", "value": "true"}
        }
    }"#;
    let err = decode_input_shard(bool_as_str.as_bytes(), &limits).unwrap_err();
    assert!(matches!(err, InputDecodeError::InvalidType { .. }));

    // Int type with boolean value
    let int_as_bool = r#"{
        "schema": "brix.input@1",
        "values": {
            "num": {"type": "int", "value": true}
        }
    }"#;
    let err = decode_input_shard(int_as_bool.as_bytes(), &limits).unwrap_err();
    assert!(matches!(err, InputDecodeError::InvalidType { .. }));

    // Int string with overflow
    let int_overflow = r#"{
        "schema": "brix.input@1",
        "values": {
            "num": {"type": "int", "value": "99999999999999999999999999999999999"}
        }
    }"#;
    let err = decode_input_shard(int_overflow.as_bytes(), &limits).unwrap_err();
    assert!(matches!(err, InputDecodeError::IntegerOverflow { .. }));

    // Int string with decimal
    let int_decimal = r#"{
        "schema": "brix.input@1",
        "values": {
            "num": {"type": "int", "value": "12.34"}
        }
    }"#;
    let err = decode_input_shard(int_decimal.as_bytes(), &limits).unwrap_err();
    assert!(matches!(err, InputDecodeError::InvalidIntegerFormat { .. }));
}

// ---------------------------------------------------------------------------
// 4. Multi-Shard Canonicalization and Duplicate Key Detection (ADR-0031 ⟨D-SHARDS⟩)
// ---------------------------------------------------------------------------

#[test]
fn test_canonicalize_disjoint_shards() {
    let limits = InputLimits::default();
    let s1 = decode_input_shard(
        br#"{"schema":"brix.input@1","values":{"a":{"type":"int","value":"1"}}}"#,
        &limits,
    )
    .unwrap();
    let s2 = decode_input_shard(
        br#"{"schema":"brix.input@1","values":{"b":{"type":"bool","value":true}}}"#,
        &limits,
    )
    .unwrap();

    let snapshot =
        canonicalize_input_shards(vec![s1, s2], &limits).expect("disjoint shards canonicalize");
    assert_eq!(snapshot.len(), 2);
    assert_eq!(snapshot.shard_count(), 2);
    assert_eq!(snapshot.get("a"), Some(&InputScalarValue::Int(1)));
    assert_eq!(snapshot.get("b"), Some(&InputScalarValue::Bool(true)));
}

#[test]
fn test_reject_duplicate_key_across_shards() {
    let limits = InputLimits::default();
    let s1 = decode_input_shard(
        br#"{"schema":"brix.input@1","values":{"a":{"type":"int","value":"1"}}}"#,
        &limits,
    )
    .unwrap();
    let s2 = decode_input_shard(
        br#"{"schema":"brix.input@1","values":{"a":{"type":"int","value":"2"}}}"#,
        &limits,
    )
    .unwrap();

    let err = canonicalize_input_shards(vec![s1, s2], &limits)
        .expect_err("colliding shards must be rejected");
    assert!(
        matches!(err, InputError::DuplicateAcrossShards { ref name } if name == "a"),
        "expected DuplicateAcrossShards(\"a\"), got {:?}",
        err
    );
}

// ---------------------------------------------------------------------------
// 5. Hostile Boundaries and Limits (ADR-0031 ⟨D-BOUNDS⟩)
// ---------------------------------------------------------------------------

#[test]
fn test_enforce_shard_limits() {
    let custom_limits = InputLimits {
        max_file_bytes: 500,
        max_files: 2,
        max_aggregate_bytes: 1000,
        max_input_count: 2,
        max_name_bytes: 10,
        max_string_value_bytes: 50,
        max_depth: 4,
    };

    // 1. Shard exceeds max_file_bytes
    let big_shard = format!(
        r#"{{"schema":"brix.input@1","values":{{"x":{{"type":"string","value":"{}"}}}}}}"#,
        "a".repeat(600)
    );
    let err = decode_input_shard(big_shard.as_bytes(), &custom_limits).unwrap_err();
    assert!(matches!(err, InputDecodeError::FileTooLarge { .. }));

    // 2. Key too long (> 10 bytes, but <= 50 bytes)
    let long_key = r#"{"schema":"brix.input@1","values":{"super_long_key_name_exceeding_ten":{"type":"int","value":"1"}}}"#;
    let err = decode_input_shard(long_key.as_bytes(), &custom_limits).unwrap_err();
    assert!(
        matches!(err, InputDecodeError::NameTooLong { .. }),
        "expected NameTooLong, got {:?}",
        err
    );

    // 3. String value too long (> 50 bytes)
    let long_str = format!(
        r#"{{"schema":"brix.input@1","values":{{"s":{{"type":"string","value":"{}"}}}}}}"#,
        "a".repeat(60)
    );
    let err = decode_input_shard(long_str.as_bytes(), &custom_limits).unwrap_err();
    assert!(matches!(err, InputDecodeError::StringValueTooLong { .. }));

    // 4. Exceed max inputs per shard (> 2 inputs)
    let three_inputs = r#"{
        "schema": "brix.input@1",
        "values": {
            "a": {"type": "int", "value": "1"},
            "b": {"type": "int", "value": "2"},
            "c": {"type": "int", "value": "3"}
        }
    }"#;
    let err = decode_input_shard(three_inputs.as_bytes(), &custom_limits).unwrap_err();
    assert!(matches!(err, InputDecodeError::LimitExceeded(_)));

    // 5. Exceed max shards in canonicalization (> 2 files)
    let s1 = decode_input_shard(
        br#"{"schema":"brix.input@1","values":{"a":{"type":"int","value":"1"}}}"#,
        &custom_limits,
    )
    .unwrap();
    let s2 = decode_input_shard(
        br#"{"schema":"brix.input@1","values":{"b":{"type":"int","value":"2"}}}"#,
        &custom_limits,
    )
    .unwrap();
    let s3 = decode_input_shard(
        br#"{"schema":"brix.input@1","values":{"c":{"type":"int","value":"3"}}}"#,
        &custom_limits,
    )
    .unwrap();

    let err = canonicalize_input_shards(vec![s1, s2, s3], &custom_limits).unwrap_err();
    assert!(matches!(err, InputError::TooManyFiles { .. }));
}

#[test]
fn test_enforce_depth_limit() {
    let custom_limits = InputLimits {
        max_depth: 2,
        ..InputLimits::default()
    };
    let valid_shard = r#"{"schema":"brix.input@1","values":{"a":{"type":"int","value":"1"}}}"#;
    let err = decode_input_shard(valid_shard.as_bytes(), &custom_limits).unwrap_err();
    assert!(matches!(
        err,
        InputDecodeError::LimitExceeded("JSON nesting depth limit")
    ));
}

// ---------------------------------------------------------------------------
// 6. Order-Independent Snapshot Identity (ADR-0031 ⟨D-IDENTITY⟩)
// ---------------------------------------------------------------------------

#[test]
fn test_order_independent_snapshot_identity() {
    let limits = InputLimits::default();

    // Shard 1 has (beta, alpha) in JSON order
    let s1 = decode_input_shard(
        br#"{
            "schema": "brix.input@1",
            "values": {
                "beta": {"type": "bool", "value": true},
                "alpha": {"type": "int", "value": "42"}
            }
        }"#,
        &limits,
    )
    .unwrap();

    // Shard 2 has gamma
    let s2 = decode_input_shard(
        br#"{
            "schema": "brix.input@1",
            "values": {
                "gamma": {"type": "string", "value": "test"}
            }
        }"#,
        &limits,
    )
    .unwrap();

    // Canonicalize in order [s1, s2]
    let snap_1_2 = canonicalize_input_shards(vec![s1.clone(), s2.clone()], &limits).unwrap();

    // Canonicalize in order [s2, s1]
    let snap_2_1 = canonicalize_input_shards(vec![s2, s1], &limits).unwrap();

    assert_eq!(
        snap_1_2.id(),
        snap_2_1.id(),
        "shard order must not affect snapshot id"
    );
    assert_eq!(
        snap_1_2.preimage(),
        snap_2_1.preimage(),
        "shard order must not affect preimage"
    );
}

#[test]
fn test_json_key_permutation_within_shard_identity() {
    let limits = InputLimits::default();

    let s1 = decode_input_shard(
        br#"{"schema":"brix.input@1","values":{"x":{"type":"int","value":"1"},"y":{"type":"int","value":"2"}}}"#,
        &limits,
    ).unwrap();

    let s2 = decode_input_shard(
        br#"{"schema":"brix.input@1","values":{"y":{"type":"int","value":"2"},"x":{"type":"int","value":"1"}}}"#,
        &limits,
    ).unwrap();

    let snap1 = canonicalize_input_shards(vec![s1], &limits).unwrap();
    let snap2 = canonicalize_input_shards(vec![s2], &limits).unwrap();

    assert_eq!(
        snap1.id(),
        snap2.id(),
        "json key permutation must not affect snapshot id"
    );
    assert_eq!(snap1.preimage(), snap2.preimage());
}

// ---------------------------------------------------------------------------
// 7. Value-Sensitive Snapshot and Context Identity (ADR-0031 ⟨D-IDENTITY⟩)
// ---------------------------------------------------------------------------

#[test]
fn test_value_sensitive_snapshot_identity() {
    let limits = InputLimits::default();

    let snap_a = canonicalize_input_shards(
        vec![decode_input_shard(
            br#"{"schema":"brix.input@1","values":{"x":{"type":"int","value":"1"}}}"#,
            &limits,
        )
        .unwrap()],
        &limits,
    )
    .unwrap();

    let snap_b = canonicalize_input_shards(
        vec![decode_input_shard(
            br#"{"schema":"brix.input@1","values":{"x":{"type":"int","value":"2"}}}"#,
            &limits,
        )
        .unwrap()],
        &limits,
    )
    .unwrap();

    let snap_c = canonicalize_input_shards(
        vec![decode_input_shard(
            br#"{"schema":"brix.input@1","values":{"x":{"type":"int","value":"1"},"y":{"type":"int","value":"1"}}}"#,
            &limits,
        ).unwrap()],
        &limits,
    ).unwrap();

    assert_ne!(
        snap_a.id(),
        snap_b.id(),
        "different values must produce different snapshot IDs"
    );
    assert_ne!(
        snap_a.id(),
        snap_c.id(),
        "different keys must produce different snapshot IDs"
    );

    // Check context identity folding
    let p = plan("config D = A propose a() priority 1 when true = A commit c from (a)");
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let cid_none = input_context_id(runtime.program, runtime.initial_world, runtime.policy, None);
    let cid_a = input_context_id(
        runtime.program,
        runtime.initial_world,
        runtime.policy,
        Some(&snap_a),
    );
    let cid_b = input_context_id(
        runtime.program,
        runtime.initial_world,
        runtime.policy,
        Some(&snap_b),
    );

    assert_ne!(
        cid_none, cid_a,
        "context id with inputs must differ from context id without inputs"
    );
    assert_ne!(
        cid_a, cid_b,
        "different input snapshots must produce different context IDs"
    );
}

// ---------------------------------------------------------------------------
// 8. Lowering with External Inputs (ADR-0031 ⟨D-GRAMMAR⟩, ⟨D-CHECK⟩)
// ---------------------------------------------------------------------------

#[test]
fn test_plan_lowering_with_inputs_and_expression_scope() {
    let source = r#"
config Decision = Expedite | Ship | Hold

input stock: Int
input threshold: Int
input urgent: Bool

rule base_stock() = stock + 1

propose expedite(base_stock) priority 5 when urgent = Expedite
propose ship(base_stock) priority 10 when stock >= threshold = Ship
propose hold() priority 100 when true = Hold

commit shipping_decision from (expedite, ship, hold)
"#;
    let p = plan(source);
    assert_eq!(p.inputs.len(), 3);
    assert_eq!(p.inputs[0].name, "stock");
    assert_eq!(p.inputs[0].ty, L3ValueType::Int);
    assert_eq!(p.inputs[1].name, "threshold");
    assert_eq!(p.inputs[1].ty, L3ValueType::Int);
    assert_eq!(p.inputs[2].name, "urgent");
    assert_eq!(p.inputs[2].ty, L3ValueType::Bool);

    assert!(p.find_input("stock").is_some());
    assert!(p.find_input("threshold").is_some());
    assert!(p.find_input("urgent").is_some());
    assert!(p.find_input("unknown").is_none());
}

#[test]
fn test_lowering_duplicate_input_name_fails() {
    let source = r#"
input threshold: Int
input threshold: Int
config D = A
propose a() priority 1 when true = A
commit c from (a)
"#;
    let module = parse(source).unwrap();
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(
        matches!(err, FiniteDecisionLowerError::DuplicateInputName(ref n) if n == "threshold"),
        "expected DuplicateInputName for \"threshold\", got {:?}",
        err
    );
}

#[test]
fn test_lowering_input_colliding_with_rule_fails() {
    let source = r#"
input threshold: Int
rule threshold() = 10
config D = A
propose a() priority 1 when true = A
commit c from (a)
"#;
    let module = parse(source).unwrap();
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(
        matches!(err, FiniteDecisionLowerError::DuplicateItemName(ref n) if n == "threshold"),
        "expected DuplicateItemName(\"threshold\"), got {:?}",
        err
    );
}

#[test]
fn test_lowering_unsupported_input_type_fails() {
    let source = r#"
input ratio: Float
config D = A
propose a() priority 1 when true = A
commit c from (a)
"#;
    let module = parse(source).unwrap();
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(
        matches!(err, FiniteDecisionLowerError::UnsupportedInputType { ref name, ref ty } if name == "ratio" && ty == "Float"),
        "expected UnsupportedInputType, got {:?}",
        err
    );
}

// ---------------------------------------------------------------------------
// 9. Declaration-Sensitive Program Representation (ADR-0031 ⟨D-IDENTITY⟩)
// ---------------------------------------------------------------------------

#[test]
fn test_declaration_sensitive_program_id() {
    let base = r#"
config D = A
propose a() priority 1 when true = A
commit c from (a)
"#;
    let with_int = r#"
input x: Int
config D = A
propose a() priority 1 when true = A
commit c from (a)
"#;
    let with_bool = r#"
input x: Bool
config D = A
propose a() priority 1 when true = A
commit c from (a)
"#;
    let with_y = r#"
input y: Int
config D = A
propose a() priority 1 when true = A
commit c from (a)
"#;

    let id_base = finite_decision_program_id(&plan(base));
    let id_int = finite_decision_program_id(&plan(with_int));
    let id_bool = finite_decision_program_id(&plan(with_bool));
    let id_y = finite_decision_program_id(&plan(with_y));

    assert_ne!(
        id_base, id_int,
        "adding an input declaration must change ProgramId"
    );
    assert_ne!(id_int, id_bool, "changing input type must change ProgramId");
    assert_ne!(id_int, id_y, "changing input name must change ProgramId");
}

// ---------------------------------------------------------------------------
// 10. Separable Validation vs Completeness (ADR-0031 ⟨D-CHECK⟩)
// ---------------------------------------------------------------------------

#[test]
fn test_separable_validation_vs_completeness() {
    let source = r#"
input stock: Int
input urgent: Bool
config D = A
propose a() priority 1 when true = A
commit c from (a)
"#;
    let p = plan(source);
    let limits = InputLimits::default();

    // 1. Partial snapshot (stock only):
    // validate_against_declarations passes (partial artifact check)
    // validate_completeness fails (missing urgent)
    let partial_shard = decode_input_shard(
        br#"{"schema":"brix.input@1","values":{"stock":{"type":"int","value":"10"}}}"#,
        &limits,
    )
    .unwrap();
    let partial_snap = canonicalize_input_shards(vec![partial_shard], &limits).unwrap();

    assert!(validate_against_declarations(&partial_snap, &p).is_ok());
    let comp_err = validate_completeness(&partial_snap, &p).unwrap_err();
    assert!(
        matches!(comp_err, InputValidationError::MissingInput { ref name, ref declared } if name == "urgent" && declared == &L3ValueType::Bool),
        "expected MissingInput(\"urgent\"), got {:?}",
        comp_err
    );

    // 2. Complete valid snapshot: both pass
    let complete_shard = decode_input_shard(
        br#"{"schema":"brix.input@1","values":{"stock":{"type":"int","value":"10"},"urgent":{"type":"bool","value":true}}}"#,
        &limits,
    ).unwrap();
    let complete_snap = canonicalize_input_shards(vec![complete_shard], &limits).unwrap();

    assert!(validate_against_declarations(&complete_snap, &p).is_ok());
    assert!(validate_completeness(&complete_snap, &p).is_ok());

    // 3. Undeclared input: validate_against_declarations fails
    let extra_shard = decode_input_shard(
        br#"{"schema":"brix.input@1","values":{"stock":{"type":"int","value":"10"},"extra":{"type":"int","value":"1"}}}"#,
        &limits,
    ).unwrap();
    let extra_snap = canonicalize_input_shards(vec![extra_shard], &limits).unwrap();

    let decl_err = validate_against_declarations(&extra_snap, &p).unwrap_err();
    assert!(
        matches!(decl_err, InputValidationError::UndeclaredInput { ref name } if name == "extra"),
        "expected UndeclaredInput(\"extra\"), got {:?}",
        decl_err
    );

    // 4. Type mismatch: stock provided as Bool instead of Int
    let mismatch_shard = decode_input_shard(
        br#"{"schema":"brix.input@1","values":{"stock":{"type":"bool","value":true}}}"#,
        &limits,
    )
    .unwrap();
    let mismatch_snap = canonicalize_input_shards(vec![mismatch_shard], &limits).unwrap();

    let type_err = validate_against_declarations(&mismatch_snap, &p).unwrap_err();
    assert!(
        matches!(type_err, InputValidationError::TypeMismatch { ref name, ref declared, ref supplied }
            if name == "stock" && declared == &L3ValueType::Int && supplied == &L3ValueType::Bool),
        "expected TypeMismatch for stock, got {:?}",
        type_err
    );
}

// ---------------------------------------------------------------------------
// 11. Backward Compatibility Regression (ADR-0031 ⟨D-COMPAT⟩)
// ---------------------------------------------------------------------------

#[test]
fn test_alpha2_shipping_regression_and_zero_inputs() {
    let source = include_str!("../../../examples/shipping.brix");
    let mut module = parse(source).expect("examples/shipping.brix must parse");
    // Strip CLI surface directive `show` as done by prepare_finite_decision_module
    module
        .items
        .retain(|i| !matches!(i, brix_syntax::ast::Item::Show(_)));

    let p = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE)
        .expect("examples/shipping.brix must lower");

    assert!(p.inputs.is_empty(), "examples/shipping.brix has no inputs");

    let runtime = FiniteDecisionRuntime::build(&p).expect("shipping runtime builds");

    // ProgramId must match runtime.program
    let prog_id = finite_decision_program_id(&p);
    assert_eq!(runtime.program, prog_id);

    // ContextId without inputs must equal runtime.context
    let cid_none = input_context_id(runtime.program, runtime.initial_world, runtime.policy, None);
    let cid_empty = input_context_id(
        runtime.program,
        runtime.initial_world,
        runtime.policy,
        Some(&InputSnapshot::empty()),
    );

    assert_eq!(
        cid_none, runtime.context,
        "input_context_id(..., None) must match runtime.context"
    );
    assert_eq!(
        cid_empty, runtime.context,
        "input_context_id(..., Some(empty)) must match runtime.context"
    );
}

// ---------------------------------------------------------------------------
// 9. Contract Drift & Rejection Tests (ADR-0031 ⟨D-TYPES⟩, ⟨D-SCHEMA⟩)
// ---------------------------------------------------------------------------

#[test]
fn test_reject_native_int_str_alias_and_malformed_numbers() {
    let limits = InputLimits::default();

    // 1. Native JSON int payload must be rejected (decimal string ONLY)
    let native_int = r#"{"schema":"brix.input@1","values":{"n":{"type":"int","value":42}}}"#;
    let err = decode_input_shard(native_int.as_bytes(), &limits).unwrap_err();
    assert!(
        matches!(err, InputDecodeError::InvalidType { .. }),
        "native int must be rejected: {err:?}"
    );

    // 2. "str" alias must be rejected (only "string" allowed)
    let str_alias = r#"{"schema":"brix.input@1","values":{"s":{"type":"str","value":"hello"}}}"#;
    let err = decode_input_shard(str_alias.as_bytes(), &limits).unwrap_err();
    assert!(
        matches!(err, InputDecodeError::InvalidType { ref found, .. } if found == "str"),
        "str alias must be rejected: {err:?}"
    );

    // 3. Leading-zero decimal string must be rejected
    let leading_zero_str =
        r#"{"schema":"brix.input@1","values":{"n":{"type":"int","value":"0123"}}}"#;
    let err = decode_input_shard(leading_zero_str.as_bytes(), &limits).unwrap_err();
    assert!(
        matches!(err, InputDecodeError::InvalidIntegerFormat { .. }),
        "leading zero decimal string must be rejected: {err:?}"
    );

    // 4. Negative zero decimal string "-0" must be rejected
    let neg_zero_str = r#"{"schema":"brix.input@1","values":{"n":{"type":"int","value":"-0"}}}"#;
    let err = decode_input_shard(neg_zero_str.as_bytes(), &limits).unwrap_err();
    assert!(
        matches!(err, InputDecodeError::InvalidIntegerFormat { .. }),
        "negative zero decimal string must be rejected: {err:?}"
    );

    // 5. Negative leading zero decimal string "-0123" must be rejected
    let neg_leading_zero_str =
        r#"{"schema":"brix.input@1","values":{"n":{"type":"int","value":"-0123"}}}"#;
    let err = decode_input_shard(neg_leading_zero_str.as_bytes(), &limits).unwrap_err();
    assert!(
        matches!(err, InputDecodeError::InvalidIntegerFormat { .. }),
        "negative leading zero decimal string must be rejected: {err:?}"
    );

    // 6. Native JSON number with leading zero "0123" must fail JSON syntax
    let native_leading_zero =
        r#"{"schema":"brix.input@1","values":{"n":{"type":"int","value":0123}}}"#;
    let err = decode_input_shard(native_leading_zero.as_bytes(), &limits).unwrap_err();
    assert!(
        matches!(err, InputDecodeError::SyntaxError { .. }),
        "native leading zero must fail syntax: {err:?}"
    );

    // 7. Native malformed JSON number "1." must fail JSON syntax
    let native_malformed = r#"{"schema":"brix.input@1","values":{"n":{"type":"int","value":1.}}}"#;
    let err = decode_input_shard(native_malformed.as_bytes(), &limits).unwrap_err();
    assert!(
        matches!(err, InputDecodeError::SyntaxError { .. }),
        "native malformed number must fail syntax: {err:?}"
    );

    // 8. Null values must be rejected
    for (t, val) in [("int", "null"), ("string", "null"), ("bool", "null")] {
        let null_json = format!(
            r#"{{"schema":"brix.input@1","values":{{"x":{{"type":"{t}","value":{val}}}}}}}"#
        );
        let err = decode_input_shard(null_json.as_bytes(), &limits).unwrap_err();
        assert!(
            matches!(err, InputDecodeError::InvalidType { .. }),
            "null for {t} must be rejected: {err:?}"
        );
    }

    // 9. Array values must be rejected
    let array_val = r#"{"schema":"brix.input@1","values":{"x":{"type":"int","value":[1, 2]}}}"#;
    let err = decode_input_shard(array_val.as_bytes(), &limits).unwrap_err();
    assert!(
        matches!(err, InputDecodeError::InvalidType { .. }),
        "array for int must be rejected: {err:?}"
    );

    let str_array_val =
        r#"{"schema":"brix.input@1","values":{"x":{"type":"string","value":["hello"]}}}"#;
    let err = decode_input_shard(str_array_val.as_bytes(), &limits).unwrap_err();
    assert!(
        matches!(err, InputDecodeError::InvalidType { .. }),
        "array for string must be rejected: {err:?}"
    );

    // 10. Float values must be rejected
    let native_float = r#"{"schema":"brix.input@1","values":{"x":{"type":"int","value":1.5}}}"#;
    let err = decode_input_shard(native_float.as_bytes(), &limits).unwrap_err();
    assert!(
        matches!(err, InputDecodeError::InvalidType { .. }),
        "native float for int must be rejected: {err:?}"
    );

    let str_float = r#"{"schema":"brix.input@1","values":{"x":{"type":"int","value":"1.5"}}}"#;
    let err = decode_input_shard(str_float.as_bytes(), &limits).unwrap_err();
    assert!(
        matches!(err, InputDecodeError::InvalidIntegerFormat { .. }),
        "decimal string float for int must be rejected: {err:?}"
    );

    // 11. Strict bool vs int
    let int_with_true = r#"{"schema":"brix.input@1","values":{"x":{"type":"int","value":true}}}"#;
    let err = decode_input_shard(int_with_true.as_bytes(), &limits).unwrap_err();
    assert!(matches!(err, InputDecodeError::InvalidType { .. }));

    let int_with_false = r#"{"schema":"brix.input@1","values":{"x":{"type":"int","value":false}}}"#;
    let err = decode_input_shard(int_with_false.as_bytes(), &limits).unwrap_err();
    assert!(matches!(err, InputDecodeError::InvalidType { .. }));

    let bool_with_native_int =
        r#"{"schema":"brix.input@1","values":{"x":{"type":"bool","value":1}}}"#;
    let err = decode_input_shard(bool_with_native_int.as_bytes(), &limits).unwrap_err();
    assert!(matches!(err, InputDecodeError::InvalidType { .. }));

    let bool_with_str_one =
        r#"{"schema":"brix.input@1","values":{"x":{"type":"bool","value":"1"}}}"#;
    let err = decode_input_shard(bool_with_str_one.as_bytes(), &limits).unwrap_err();
    assert!(matches!(err, InputDecodeError::InvalidType { .. }));

    let bool_with_str_true =
        r#"{"schema":"brix.input@1","values":{"x":{"type":"bool","value":"true"}}}"#;
    let err = decode_input_shard(bool_with_str_true.as_bytes(), &limits).unwrap_err();
    assert!(matches!(err, InputDecodeError::InvalidType { .. }));

    // Valid single "0" and positive/negative ints
    let valid_zero = r#"{"schema":"brix.input@1","values":{"zero":{"type":"int","value":"0"}}}"#;
    let s = decode_input_shard(valid_zero.as_bytes(), &limits).expect("single zero is valid");
    assert_eq!(s.get("zero"), Some(&InputScalarValue::Int(0)));
}

// ---------------------------------------------------------------------------
// 10. JSON Correctness: UTF-8 and Surrogate Pairs (ADR-0031)
// ---------------------------------------------------------------------------

#[test]
fn test_json_unicode_escapes_and_utf8_correctness() {
    let limits = InputLimits::default();

    // 1. Raw non-ASCII UTF-8
    let raw_utf8 = r#"{"schema":"brix.input@1","values":{"msg":{"type":"string","value":"こんにちは世界 🦀 🚀"}}}"#;
    let shard =
        decode_input_shard(raw_utf8.as_bytes(), &limits).expect("valid raw UTF-8 should decode");
    assert_eq!(
        shard.get("msg"),
        Some(&InputScalarValue::Str("こんにちは世界 🦀 🚀".to_string()))
    );

    // 2. Escaped BMP
    let escaped_bmp = r#"{"schema":"brix.input@1","values":{"greeting":{"type":"string","value":"\u0048\u0065\u006c\u006c\u006f\u3042"}}}"#;
    let shard =
        decode_input_shard(escaped_bmp.as_bytes(), &limits).expect("escaped BMP should decode");
    assert_eq!(
        shard.get("greeting"),
        Some(&InputScalarValue::Str("Helloあ".to_string()))
    );

    // 3. Valid escaped surrogate pair (emoji 😀 = U+1F600 = \uD83D\uDE00, 🌈 = U+1F308 = \uD83C\uDF08)
    let surrogate_emoji = r#"{"schema":"brix.input@1","values":{"icons":{"type":"string","value":"\uD83D\uDE00\uD83C\uDF08"}}}"#;
    let shard = decode_input_shard(surrogate_emoji.as_bytes(), &limits)
        .expect("valid surrogate pairs should decode");
    assert_eq!(
        shard.get("icons"),
        Some(&InputScalarValue::Str("😀🌈".to_string()))
    );

    // 4. Lone high surrogate
    let lone_high =
        r#"{"schema":"brix.input@1","values":{"bad":{"type":"string","value":"\uD83D"}}}"#;
    let err = decode_input_shard(lone_high.as_bytes(), &limits).unwrap_err();
    assert!(
        matches!(err, InputDecodeError::SyntaxError { .. }),
        "lone high surrogate must be rejected: {err:?}"
    );

    // 5. Lone high surrogate followed by ASCII text
    let lone_high_text =
        r#"{"schema":"brix.input@1","values":{"bad":{"type":"string","value":"\uD83Dfoo"}}}"#;
    let err = decode_input_shard(lone_high_text.as_bytes(), &limits).unwrap_err();
    assert!(
        matches!(err, InputDecodeError::SyntaxError { .. }),
        "lone high surrogate followed by text must be rejected: {err:?}"
    );

    // 6. Lone low surrogate
    let lone_low =
        r#"{"schema":"brix.input@1","values":{"bad":{"type":"string","value":"\uDE00"}}}"#;
    let err = decode_input_shard(lone_low.as_bytes(), &limits).unwrap_err();
    assert!(
        matches!(err, InputDecodeError::SyntaxError { .. }),
        "lone low surrogate must be rejected: {err:?}"
    );

    // 7. Malformed surrogate pair (high + high)
    let high_high =
        r#"{"schema":"brix.input@1","values":{"bad":{"type":"string","value":"\uD83D\uD83D"}}}"#;
    let err = decode_input_shard(high_high.as_bytes(), &limits).unwrap_err();
    assert!(
        matches!(err, InputDecodeError::SyntaxError { .. }),
        "high+high surrogate must be rejected: {err:?}"
    );

    // 8. Reversed surrogate pair (low + high)
    let low_high =
        r#"{"schema":"brix.input@1","values":{"bad":{"type":"string","value":"\uDE00\uD83D"}}}"#;
    let err = decode_input_shard(low_high.as_bytes(), &limits).unwrap_err();
    assert!(
        matches!(err, InputDecodeError::SyntaxError { .. }),
        "low+high surrogate must be rejected: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// 11. Bounded Reads and TOCTOU Hardening (ADR-0031 ⟨D-BOUNDS⟩)
// ---------------------------------------------------------------------------

struct TempFileGuard {
    path: std::path::PathBuf,
}

impl TempFileGuard {
    fn new(name: &str, content: &[u8]) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut path = std::env::temp_dir();
        path.push(format!(
            "brix_test_{}_{}_{}.json",
            std::process::id(),
            id,
            name
        ));
        std::fs::write(&path, content).expect("write temp file");
        Self { path }
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[test]
fn test_bounded_reads_and_toctou_prevention() {
    let valid_base = r#"{"schema":"brix.input@1","values":{"x":{"type":"int","value":"1"}}}"#;
    let exact_len = valid_base.len();

    let custom_limits = InputLimits {
        max_file_bytes: exact_len,
        ..Default::default()
    };

    // 1. Exact boundary: file length == max_file_bytes (must succeed)
    let exact_file = TempFileGuard::new("exact_bound", valid_base.as_bytes());
    let shard = decode_input_shard_from_file(exact_file.path(), &custom_limits)
        .expect("exact boundary file must decode successfully");
    assert_eq!(shard.get("x"), Some(&InputScalarValue::Int(1)));

    // 2. Over boundary (+1 byte): file length == max_file_bytes + 1 (must fail FileTooLarge)
    let mut over_bytes = valid_base.as_bytes().to_vec();
    over_bytes.push(b' '); // 1 extra whitespace byte
    let over_file = TempFileGuard::new("over_bound", &over_bytes);
    let err = decode_input_shard_from_file(over_file.path(), &custom_limits).unwrap_err();
    assert!(
        matches!(err, InputDecodeError::FileTooLarge { limit, found } if limit == exact_len && found == (exact_len + 1) as u64),
        "over-boundary file must return FileTooLarge: {err:?}"
    );

    // 3. Non-regular path: directory path must fail closed with NotARegularFile
    let temp_dir = std::env::temp_dir();
    let err = decode_input_shard_from_file(&temp_dir, &custom_limits).unwrap_err();
    assert!(
        matches!(err, InputDecodeError::NotARegularFile(_)),
        "directory path must fail with NotARegularFile: {err:?}"
    );

    // 4. Non-existent file path: must return structured IoError carrying actual safe path and message
    let non_existent = std::path::PathBuf::from("does_not_exist_file_12345.json");
    let err = decode_input_shard_from_file(&non_existent, &custom_limits).unwrap_err();
    match err {
        InputDecodeError::IoError { path, message } => {
            assert_eq!(path, non_existent.display().to_string());
            assert!(!message.is_empty(), "message must not be empty");
        }
        other => panic!("expected structured IoError, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 12. Whole-Set Preflight and Loading API (ADR-0031 ⟨D-SHARDS⟩, ⟨D-BOUNDS⟩)
// ---------------------------------------------------------------------------

#[test]
fn test_whole_set_preflight_and_limits() {
    let limits = InputLimits {
        max_files: 3,
        max_aggregate_bytes: 200,
        ..Default::default()
    };

    // 1. Too many files: 4 paths when max_files is 3 (enforced before opening)
    let dummy_paths = vec![
        std::path::PathBuf::from("a.json"),
        std::path::PathBuf::from("b.json"),
        std::path::PathBuf::from("c.json"),
        std::path::PathBuf::from("d.json"),
    ];
    let err = load_input_snapshot_from_paths(&dummy_paths, &limits).unwrap_err();
    assert!(
        matches!(err, InputError::TooManyFiles { limit: 3, found: 4 }),
        "too many files must be rejected: {err:?}"
    );

    // 2. Aggregate metadata limit exceeded
    let s1_json = r#"{"schema":"brix.input@1","values":{"a":{"type":"int","value":"1"}}}"#;
    let s2_json = r#"{"schema":"brix.input@1","values":{"b":{"type":"int","value":"2"}}}"#;
    // s1 is ~68 bytes, s2 is ~68 bytes. Set max_aggregate_bytes to 100 bytes:
    let tight_aggregate_limits = InputLimits {
        max_files: 3,
        max_aggregate_bytes: 100,
        ..Default::default()
    };
    let f1 = TempFileGuard::new("agg1", s1_json.as_bytes());
    let f2 = TempFileGuard::new("agg2", s2_json.as_bytes());

    let err = load_input_snapshot_from_paths(&[f1.path(), f2.path()], &tight_aggregate_limits)
        .unwrap_err();
    assert!(
        matches!(err, InputError::AggregateBytesExceeded { .. }),
        "aggregate size exceeded must be rejected: {err:?}"
    );

    // 3. Successful whole-set loading of disjoint shards
    let ok_limits = InputLimits {
        max_files: 3,
        max_aggregate_bytes: 500,
        ..Default::default()
    };
    let snapshot = load_input_snapshot_from_paths(&[f1.path(), f2.path()], &ok_limits)
        .expect("valid multi-shard input set must load");
    assert_eq!(snapshot.len(), 2);
    assert_eq!(snapshot.shard_count(), 2);
    assert_eq!(snapshot.get("a"), Some(&InputScalarValue::Int(1)));
    assert_eq!(snapshot.get("b"), Some(&InputScalarValue::Int(2)));

    // 4. Preflight IO error carries actual safe path
    let missing_path = std::path::PathBuf::from("nonexistent_preflight_shard_9999.json");
    let err = load_input_snapshot_from_paths(&[&missing_path], &ok_limits).unwrap_err();
    match err {
        InputError::Decode(InputDecodeError::IoError { path, message }) => {
            assert_eq!(path, missing_path.display().to_string());
            assert!(!message.is_empty());
        }
        other => panic!("expected structured preflight IoError, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 13. Name Bounds: Source Lowering and Artifact Decoding (ADR-0031 ⟨D-BOUNDS⟩)
// ---------------------------------------------------------------------------

#[test]
fn test_name_bounds_source_lowering_and_artifact_decoding() {
    let limits = InputLimits::default();
    assert_eq!(limits.max_name_bytes, MAX_INPUT_NAME_BYTES);

    // 1. Source lowering: exact boundary (64 bytes)
    let name_64 = "a".repeat(64);
    let src_64 = format!(
        "input {name_64}: Int\nconfig D = A\npropose a() priority 1 when true = A\ncommit c from (a)\n"
    );
    let module_64 = parse(&src_64).expect("64-byte name parses");
    let plan_64 = lower_finite_decision_plan(&module_64, FINITE_DECISION_PROFILE)
        .expect("64-byte name lowers successfully");
    assert!(plan_64.find_input(&name_64).is_some());

    // 2. Source lowering: one-over boundary (65 bytes)
    let name_65 = "a".repeat(65);
    let src_65 = format!(
        "input {name_65}: Int\nconfig D = A\npropose a() priority 1 when true = A\ncommit c from (a)\n"
    );
    let module_65 = parse(&src_65).expect("65-byte name parses");
    let err_65 = lower_finite_decision_plan(&module_65, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(
        matches!(
            err_65,
            FiniteDecisionLowerError::InputNameTooLong { limit: 64 }
        ),
        "65-byte input name must fail lowering with InputNameTooLong: {err_65:?}"
    );

    // 3. Artifact decoding: exact boundary (64 bytes)
    let artifact_64 = format!(
        r#"{{"schema":"brix.input@1","values":{{"{name_64}":{{"type":"int","value":"1"}}}}}}"#
    );
    let shard_64 = decode_input_shard(artifact_64.as_bytes(), &limits)
        .expect("64-byte name in artifact must decode");
    assert!(shard_64.get(&name_64).is_some());

    // 4. Artifact decoding: one-over boundary (65 bytes)
    let artifact_65 = format!(
        r#"{{"schema":"brix.input@1","values":{{"{name_65}":{{"type":"int","value":"1"}}}}}}"#
    );
    let err_artifact = decode_input_shard(artifact_65.as_bytes(), &limits).unwrap_err();
    assert!(
        matches!(err_artifact, InputDecodeError::NameTooLong { limit: 64 }),
        "65-byte name in artifact must fail with NameTooLong: {err_artifact:?}"
    );
}

// ---------------------------------------------------------------------------
// 14. Wave 2 Runtime Semantics: Fallible Construction and Fail-Closed Validation
// ---------------------------------------------------------------------------

#[test]
fn test_runtime_construction_fails_closed_on_invalid_or_missing_inputs() {
    let source = r#"
input threshold: Int
config Flag = Off
rule limit() = threshold + 5
propose opt(limit) priority 1 when limit > 10 = Off
commit pick from (opt)
"#;
    let p = plan(source);

    // 1. Calling FiniteDecisionRuntime::build(&p) on input-declaring plan fails closed
    let build_err = match FiniteDecisionRuntime::build(&p) {
        Err(e) => e,
        Ok(_) => panic!("expected build to fail on missing input"),
    };
    match build_err {
        FiniteDecisionBuildError::InputValidation(InputValidationError::MissingInput {
            ref name,
            ..
        }) => {
            assert_eq!(name, "threshold");
        }
        other => panic!("expected MissingInput, got {other:?}"),
    }

    // 2. Public helper run_finite_decision_plan(&p) fails closed with the same typed error
    let run_err = match run_finite_decision_plan(&p) {
        Err(e) => e,
        Ok(_) => panic!("expected run to fail on missing input"),
    };
    assert!(matches!(
        run_err,
        FiniteDecisionBuildError::InputValidation(InputValidationError::MissingInput { .. })
    ));

    // 3. Public helper finite_decision_audit_environment_from_plan(&p) fails closed
    let audit_err = match finite_decision_audit_environment_from_plan(&p) {
        Err(e) => e,
        Ok(_) => panic!("expected audit env to fail on missing input"),
    };
    assert!(matches!(
        audit_err,
        FiniteDecisionBuildError::InputValidation(InputValidationError::MissingInput { .. })
    ));

    // 4. Calling build_with_inputs with extra input fails closed
    let extra_json = r#"{
        "schema": "brix.input@1",
        "values": {
            "threshold": {"type": "int", "value": "10"},
            "extra_key": {"type": "string", "value": "unexpected"}
        }
    }"#;
    let extra_snap = snapshot_from_json(extra_json);
    let extra_err = match FiniteDecisionRuntime::build_with_inputs(&p, &extra_snap) {
        Err(e) => e,
        Ok(_) => panic!("expected build to fail on undeclared input"),
    };
    match extra_err {
        FiniteDecisionBuildError::InputValidation(InputValidationError::UndeclaredInput {
            ref name,
        }) => {
            assert_eq!(name, "extra_key");
        }
        other => panic!("expected UndeclaredInput, got {other:?}"),
    }

    // 5. Calling build_with_inputs with type mismatch fails closed
    let mismatch_json = r#"{
        "schema": "brix.input@1",
        "values": {
            "threshold": {"type": "bool", "value": true}
        }
    }"#;
    let mismatch_snap = snapshot_from_json(mismatch_json);
    let mismatch_err = match FiniteDecisionRuntime::build_with_inputs(&p, &mismatch_snap) {
        Err(e) => e,
        Ok(_) => panic!("expected build to fail on type mismatch"),
    };
    match mismatch_err {
        FiniteDecisionBuildError::InputValidation(InputValidationError::TypeMismatch {
            ref name,
            ref declared,
            ref supplied,
        }) => {
            assert_eq!(name, "threshold");
            assert_eq!(declared, &L3ValueType::Int);
            assert_eq!(supplied, &L3ValueType::Bool);
        }
        other => panic!("expected TypeMismatch, got {other:?}"),
    }

    // 6. Calling build_with_inputs with valid snapshot succeeds
    let valid_json = r#"{
        "schema": "brix.input@1",
        "values": {
            "threshold": {"type": "int", "value": "10"}
        }
    }"#;
    let valid_snap = snapshot_from_json(valid_json);
    let runtime = FiniteDecisionRuntime::build_with_inputs(&p, &valid_snap)
        .expect("runtime builds with valid inputs");
    let run = runtime.run();
    assert!(run.is_selected());
    assert_eq!(
        run.decision.as_ref().map(|d| d.candidate.as_str()),
        Some("opt")
    );
}

// ---------------------------------------------------------------------------
// 15. Runtime Evaluates Int, Bool, and Str Inputs Across Expressions and Shows
// ---------------------------------------------------------------------------

#[test]
fn test_runtime_evaluates_all_scalar_input_types_across_expressions_and_shows() {
    let source = r#"
input count: Int
input flag: Bool
input label: Str

let double_count = count * 2

rule greeting() = label

propose opt_active(greeting) priority 1 when flag == true = greeting
propose opt_fallback(greeting) priority 2 when flag == false = "default"

commit pick from (opt_active, opt_fallback)

show double_count
show count
show greeting
show flag
"#;
    let p = plan(source);

    // Run A: flag=true, count=5, label="welcome" -> opt_active selected
    let json_a = r#"{
        "schema": "brix.input@1",
        "values": {
            "count": {"type": "int", "value": "5"},
            "flag": {"type": "bool", "value": true},
            "label": {"type": "string", "value": "welcome"}
        }
    }"#;
    let snap_a = snapshot_from_json(json_a);
    let runtime_a =
        FiniteDecisionRuntime::build_with_inputs(&p, &snap_a).expect("runtime builds with inputs");
    let run_a = runtime_a.run();

    assert!(run_a.is_selected());
    assert_eq!(
        run_a.decision.as_ref().map(|d| d.candidate.as_str()),
        Some("opt_active")
    );
    assert_eq!(
        run_a.decision.as_ref().unwrap().value,
        L3ValueV2::Str("welcome".to_string())
    );

    // Verify evaluate_shows
    let shows_a = runtime_a
        .evaluate_shows(&run_a)
        .expect("evaluate_shows succeeds");
    assert_eq!(shows_a.len(), 4);
    assert_eq!(shows_a[0], L3ValueV2::Int(10)); // double_count (let reading input)
    assert_eq!(shows_a[1], L3ValueV2::Int(5)); // count (direct input)
    assert_eq!(shows_a[2], L3ValueV2::Str("welcome".to_string())); // greeting (rule reading input)
    assert_eq!(shows_a[3], L3ValueV2::Bool(true)); // flag (bool input)

    // Run B: flag=false, count=5, label="welcome" -> opt_fallback selected
    let json_b = r#"{
        "schema": "brix.input@1",
        "values": {
            "count": {"type": "int", "value": "5"},
            "flag": {"type": "bool", "value": false},
            "label": {"type": "string", "value": "welcome"}
        }
    }"#;
    let snap_b = snapshot_from_json(json_b);
    let runtime_b =
        FiniteDecisionRuntime::build_with_inputs(&p, &snap_b).expect("runtime builds with inputs");
    let run_b = runtime_b.run();

    assert!(run_b.is_selected());
    assert_eq!(
        run_b.decision.as_ref().map(|d| d.candidate.as_str()),
        Some("opt_fallback")
    );
    assert_eq!(
        run_b.decision.as_ref().unwrap().value,
        L3ValueV2::Str("default".to_string())
    );

    let shows_b = runtime_b
        .evaluate_shows(&run_b)
        .expect("evaluate_shows succeeds");
    assert_eq!(shows_b.len(), 4);
    assert_eq!(shows_b[0], L3ValueV2::Int(10));
    assert_eq!(shows_b[1], L3ValueV2::Int(5));
    assert_eq!(shows_b[2], L3ValueV2::Str("welcome".to_string()));
    assert_eq!(shows_b[3], L3ValueV2::Bool(false));

    // Cross-runtime / cross-snapshot verification:
    // runtime_a must reject run_b (context and bound inputs mismatch)
    let cross_err_a_b = runtime_a.evaluate_shows(&run_b).unwrap_err();
    assert!(
        matches!(
            cross_err_a_b,
            FiniteDecisionUnknownReason::InvariantViolation { .. }
        ),
        "expected InvariantViolation when runtime_a evaluates run_b, got: {cross_err_a_b:?}"
    );

    // runtime_b must reject run_a
    let cross_err_b_a = runtime_b.evaluate_shows(&run_a).unwrap_err();
    assert!(
        matches!(
            cross_err_b_a,
            FiniteDecisionUnknownReason::InvariantViolation { .. }
        ),
        "expected InvariantViolation when runtime_b evaluates run_a, got: {cross_err_b_a:?}"
    );

    // Mutated / tampered run inputs rejection:
    let mut tampered_run = run_a.clone();
    tampered_run.inputs[0].value = L3ValueV2::Int(999);
    let tamper_err = runtime_a.evaluate_shows(&tampered_run).unwrap_err();
    assert!(
        matches!(
            tamper_err,
            FiniteDecisionUnknownReason::InvariantViolation { .. }
        ),
        "expected InvariantViolation on tampered inputs, got: {tamper_err:?}"
    );

    // Mutated / tampered run facts rejection (Item 5 regression test):
    let mut tampered_facts_run = run_a.clone();
    tampered_facts_run.facts[0].value = L3ValueV2::Int(999);
    let tamper_facts_err = runtime_a.evaluate_shows(&tampered_facts_run).unwrap_err();
    assert!(
        matches!(
            tamper_facts_err,
            FiniteDecisionUnknownReason::InvariantViolation { .. }
        ),
        "expected InvariantViolation on tampered facts, got: {tamper_facts_err:?}"
    );

    // Cross-program verification: run from different program rejected
    let diff_prog =
        plan("rule r() = 1\npropose p(r) priority 1 when true = 1\ncommit pick from (p)\n");
    let rt_diff = FiniteDecisionRuntime::build(&diff_prog).unwrap();
    let run_diff = rt_diff.run();
    let prog_err = runtime_a.evaluate_shows(&run_diff).unwrap_err();
    assert!(
        matches!(
            prog_err,
            FiniteDecisionUnknownReason::InvariantViolation { .. }
        ),
        "expected InvariantViolation on cross-program run, got: {prog_err:?}"
    );
}

// ---------------------------------------------------------------------------
// 16. Bound Inputs Epistemic Grade (@Derived) and Strict Fact Separation
// ---------------------------------------------------------------------------

#[test]
fn test_bound_input_records_epistemic_grade_and_fact_separation() {
    let source = r#"
input alpha: Int
input beta: Str

rule computed() = alpha + 10

propose opt(computed) priority 1 when true = beta

commit pick from (opt)
"#;
    let p = plan(source);
    let json = r#"{
        "schema": "brix.input@1",
        "values": {
            "alpha": {"type": "int", "value": "42"},
            "beta": {"type": "string", "value": "hello"}
        }
    }"#;
    let snap = snapshot_from_json(json);
    let runtime = FiniteDecisionRuntime::build_with_inputs(&p, &snap).expect("builds");
    let run = runtime.run();

    assert!(run.is_selected());

    // 1. run.inputs contains both inputs with Outcome::Derived
    assert_eq!(run.inputs.len(), 2);
    let in_alpha: &BoundInput = run.input("alpha").expect("alpha present");
    assert_eq!(in_alpha.ordinal, 0);
    assert_eq!(in_alpha.name, "alpha");
    assert_eq!(in_alpha.ty, L3ValueType::Int);
    assert_eq!(in_alpha.value, L3ValueV2::Int(42));
    assert_eq!(in_alpha.grade, Outcome::Derived);

    let in_beta = run.input("beta").expect("beta present");
    assert_eq!(in_beta.ordinal, 1);
    assert_eq!(in_beta.name, "beta");
    assert_eq!(in_beta.ty, L3ValueType::Str);
    assert_eq!(in_beta.value, L3ValueV2::Str("hello".to_string()));
    assert_eq!(in_beta.grade, Outcome::Derived);

    assert!(run.input("nonexistent").is_none());

    // 2. Strict fact separation: run.facts contains ONLY rule-derived facts, NOT inputs
    assert!(run.facts.iter().any(|f| f.rule == "computed"));
    assert!(
        !run.facts.iter().any(|f| f.rule == "alpha"),
        "inputs must never be in run.facts"
    );
    assert!(
        !run.facts.iter().any(|f| f.rule == "beta"),
        "inputs must never be in run.facts"
    );
}

// ---------------------------------------------------------------------------
// 17. Input Value Change Shifts Decision & Changes Context, Preserving Program ID
// ---------------------------------------------------------------------------

#[test]
fn test_input_value_change_shifts_decision_and_preserves_program_id() {
    let source = r#"
input score: Int

rule rating() = score

propose pass(rating) priority 1 when rating >= 50 = "APPROVED"
propose fail(rating) priority 2 when rating < 50 = "REJECTED"

commit pick from (pass, fail)
"#;
    let p = plan(source);
    let prog_id = finite_decision_program_id(&p);

    let snap_pass = snapshot_from_json(
        r#"{
        "schema": "brix.input@1",
        "values": { "score": {"type": "int", "value": "75"} }
    }"#,
    );
    let snap_fail = snapshot_from_json(
        r#"{
        "schema": "brix.input@1",
        "values": { "score": {"type": "int", "value": "20"} }
    }"#,
    );

    let runtime_pass = FiniteDecisionRuntime::build_with_inputs(&p, &snap_pass).unwrap();
    let runtime_fail = FiniteDecisionRuntime::build_with_inputs(&p, &snap_fail).unwrap();

    // Program IDs are identical (specification invariance)
    assert_eq!(runtime_pass.program, prog_id);
    assert_eq!(runtime_fail.program, prog_id);
    assert_eq!(runtime_pass.program, runtime_fail.program);

    // Context IDs differ because input values differ
    assert_ne!(runtime_pass.context, runtime_fail.context);

    let run_pass = runtime_pass.run();
    let run_fail = runtime_fail.run();

    // Runs preserve runtime context
    assert_eq!(run_pass.context, runtime_pass.context);
    assert_eq!(run_fail.context, runtime_fail.context);

    // Decision shifts deterministically
    assert_eq!(
        run_pass.decision.as_ref().map(|d| d.candidate.as_str()),
        Some("pass")
    );
    assert_eq!(
        run_fail.decision.as_ref().map(|d| d.candidate.as_str()),
        Some("fail")
    );
}

// ---------------------------------------------------------------------------
// 18. Shard and Key Permutation Order Independence for Runtime Execution
// ---------------------------------------------------------------------------

#[test]
fn test_shard_and_key_permutation_order_independence_in_runtime() {
    let source = r#"
input x: Int
input y: Int

rule sum() = x + y

propose p(sum) priority 1 when sum > 0 = sum

commit pick from (p)
"#;
    let p = plan(source);

    // Permutation 1: single shard with x then y
    let snap_1 = snapshot_from_json(
        r#"{
        "schema": "brix.input@1",
        "values": {
            "x": {"type": "int", "value": "10"},
            "y": {"type": "int", "value": "20"}
        }
    }"#,
    );

    // Permutation 2: single shard with y then x
    let snap_2 = snapshot_from_json(
        r#"{
        "schema": "brix.input@1",
        "values": {
            "y": {"type": "int", "value": "20"},
            "x": {"type": "int", "value": "10"}
        }
    }"#,
    );

    // Permutation 3: two disjoint shards [x], [y]
    let limits = InputLimits::default();
    let s_x = decode_input_shard(
        r#"{"schema":"brix.input@1","values":{"x":{"type":"int","value":"10"}}}"#.as_bytes(),
        &limits,
    )
    .unwrap();
    let s_y = decode_input_shard(
        r#"{"schema":"brix.input@1","values":{"y":{"type":"int","value":"20"}}}"#.as_bytes(),
        &limits,
    )
    .unwrap();
    let snap_3 = canonicalize_input_shards(vec![s_x.clone(), s_y.clone()], &limits).unwrap();

    // Permutation 4: two disjoint shards in reverse order [y], [x]
    let snap_4 = canonicalize_input_shards(vec![s_y, s_x], &limits).unwrap();

    let rt1 = FiniteDecisionRuntime::build_with_inputs(&p, &snap_1).unwrap();
    let rt2 = FiniteDecisionRuntime::build_with_inputs(&p, &snap_2).unwrap();
    let rt3 = FiniteDecisionRuntime::build_with_inputs(&p, &snap_3).unwrap();
    let rt4 = FiniteDecisionRuntime::build_with_inputs(&p, &snap_4).unwrap();

    // All four must yield identical runtime ContextId
    assert_eq!(rt1.context, rt2.context);
    assert_eq!(rt1.context, rt3.context);
    assert_eq!(rt1.context, rt4.context);

    // All four runs produce identical decision, journal, and bound inputs
    let r1 = rt1.run();
    let r2 = rt2.run();
    let r3 = rt3.run();
    let r4 = rt4.run();

    assert_eq!(r1.context, r2.context);
    assert_eq!(r1.decision, r2.decision);
    assert_eq!(r1.journal.step_digests(), r2.journal.step_digests());
    assert_eq!(r1.inputs, r2.inputs);

    assert_eq!(r1.context, r3.context);
    assert_eq!(r1.decision, r3.decision);
    assert_eq!(r1.journal.step_digests(), r3.journal.step_digests());
    assert_eq!(r1.inputs, r3.inputs);

    assert_eq!(r1.context, r4.context);
    assert_eq!(r1.decision, r4.decision);
    assert_eq!(r1.journal.step_digests(), r4.journal.step_digests());
    assert_eq!(r1.inputs, r4.inputs);
}

// ---------------------------------------------------------------------------
// 19. Audit Environment Context Derivation & Convenience Rejection
// ---------------------------------------------------------------------------

#[test]
fn test_audit_environment_with_inputs_and_convenience_rejection() {
    let source = r#"
input limit: Int

rule val() = limit * 2

propose opt(val) priority 1 when val > 0 = val

commit pick from (opt)
"#;
    let p = plan(source);
    let snap = snapshot_from_json(
        r#"{
        "schema": "brix.input@1",
        "values": {
            "limit": {"type": "int", "value": "5"}
        }
    }"#,
    );

    // 1. Input-aware audit helper matches runtime context
    let runtime = FiniteDecisionRuntime::build_with_inputs(&p, &snap).unwrap();
    let (audit_cid, _registry, _semantics) =
        finite_decision_audit_environment_from_plan_with_inputs(&p, &snap)
            .expect("audit environment builds with inputs");
    assert_eq!(audit_cid, runtime.context);

    let run = runtime.run();
    assert_eq!(audit_cid, run.context);

    // 2. Convenience run_finite_decision_plan_with_inputs helper produces matching run
    let convenience_run =
        run_finite_decision_plan_with_inputs(&p, &snap).expect("convenience run succeeds");
    assert_eq!(convenience_run.context, runtime.context);
    assert_eq!(convenience_run.decision, run.decision);
    assert_eq!(convenience_run.inputs, run.inputs);

    // 3. No-input audit helper fails closed on input-declaring plan
    let err = match finite_decision_audit_environment_from_plan(&p) {
        Err(e) => e,
        Ok(_) => panic!("expected audit env helper to fail"),
    };
    assert!(matches!(
        err,
        FiniteDecisionBuildError::InputValidation(InputValidationError::MissingInput { .. })
    ));
}

// ---------------------------------------------------------------------------
// 20. Backward Compatibility on Zero-Input Plans
// ---------------------------------------------------------------------------

#[test]
fn test_zero_input_plan_full_backward_compatibility() {
    let source = r#"
config Mode = Fast

rule speed() = 100

propose opt(speed) priority 1 when speed > 50 = Fast

commit pick from (opt)
"#;
    let p = plan(source);

    // 1. build(&p) succeeds
    let rt_legacy = FiniteDecisionRuntime::build(&p).expect("legacy build succeeds");

    // 2. build_with_inputs(&p, empty) succeeds
    let rt_explicit = FiniteDecisionRuntime::build_with_inputs(&p, &InputSnapshot::empty())
        .expect("explicit empty build succeeds");

    // Both contexts match
    assert_eq!(rt_legacy.context, rt_explicit.context);

    // 3. Audit environment succeeds
    let (audit_cid, _reg, _sem) = finite_decision_audit_environment_from_plan(&p)
        .expect("audit env succeeds for zero-input plan");
    assert_eq!(audit_cid, rt_legacy.context);

    // 4. Run produces empty inputs, facts populated, decision selected
    let run = rt_legacy.run();
    assert!(run.is_selected());
    assert_eq!(
        run.decision.as_ref().map(|d| d.candidate.as_str()),
        Some("opt")
    );
    assert!(run.inputs.is_empty());
    assert!(run.facts.iter().any(|f| f.rule == "speed"));
    assert_eq!(run.context, rt_legacy.context);
}

// ---------------------------------------------------------------------------
// 21. Runtime Unknown Fault Preserves Context and Bound Inputs
// ---------------------------------------------------------------------------

#[test]
fn test_runtime_fault_on_input_arithmetic_overflow_preserves_context_and_inputs() {
    let source = r#"
input big: Int

rule overflow() = big + 1

propose opt(overflow) priority 1 when true = 1

commit pick from (opt)
"#;
    let p = plan(source);
    let snap = snapshot_from_json(
        r#"{
        "schema": "brix.input@1",
        "values": {
            "big": {"type": "int", "value": "9223372036854775807"}
        }
    }"#,
    );
    let rt = FiniteDecisionRuntime::build_with_inputs(&p, &snap).unwrap();
    let run = rt.run();

    assert!(run.is_unknown());
    assert_eq!(run.context, rt.context);
    assert_eq!(run.inputs.len(), 1);
    assert_eq!(run.inputs[0].name, "big");
    assert_eq!(run.inputs[0].value, L3ValueV2::Int(i64::MAX));
    assert_eq!(run.inputs[0].grade, Outcome::Derived);
}

#[test]
fn test_build_with_inputs_panic_free_on_hand_built_plan_with_missing_input() {
    let source = "rule r() = 1\npropose p(r) priority 1 when true = 1\ncommit pick from (p)\n";
    let mut p = plan(source);
    // Inject hand-built undeclared input requirement directly into plan
    p.inputs
        .push(brix_lower::finite_decision::FiniteDecisionInput {
            ordinal: 0,
            name: "missing_scalar".to_string(),
            ty: L3ValueType::Int,
        });
    let snap = InputSnapshot::empty();
    let err = match FiniteDecisionRuntime::build_with_inputs(&p, &snap) {
        Err(e) => e,
        Ok(_) => panic!("expected build to fail on hand-built plan with missing input"),
    };
    assert!(matches!(
        err,
        FiniteDecisionBuildError::InputValidation(InputValidationError::MissingInput {
            ref name,
            declared: L3ValueType::Int,
        }) if name == "missing_scalar"
    ));
}
