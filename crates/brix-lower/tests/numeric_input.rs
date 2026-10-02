//! Versioned numeric transport, exact parsing, and canonical snapshot identity.
use brix_lower::input::{
    canonicalize_input_shards, decode_input_shard, InputLimits, InputSnapshot, InputValue,
};
use brix_lower::{lower_finite_decision_plan, FiniteDecisionRuntime, FINITE_DECISION_PROFILE};

fn snapshot(json: &str) -> InputSnapshot {
    let limits = InputLimits::default();
    let shard = decode_input_shard(json.as_bytes(), &limits).unwrap();
    canonicalize_input_shards(vec![shard], &limits).unwrap()
}

fn document(version: u8, value: &str) -> String {
    format!(r#"{{"schema":"brix.input@{version}","values":{{"x":{value}}}}}"#)
}

#[test]
fn numeric_strings_preserve_decimal_precision_and_runtime_roundtrip() {
    for (tag, raw) in [
        ("f64", "1.2345678901234567"),
        ("decimal", "9007199254740993.123456789012345678"),
        ("decimal", "-170141183460469231731687303715884105728"),
    ] {
        let json = document(4, &format!(r#"{{"type":"{tag}","value":"{raw}"}}"#));
        let values = snapshot(&json);
        let value = values.get("x").unwrap();
        assert_eq!(
            InputValue::from_l3_value(&value.to_l3_value()).as_ref(),
            Some(value)
        );
        match value {
            InputValue::F64(n) => assert_eq!(n.to_string(), raw),
            InputValue::Decimal(n) => assert_eq!(brix_canon::decimal_format(*n), raw),
            other => panic!("unexpected value {other:?}"),
        }
    }
}

#[test]
fn old_schemas_reject_numeric_values_recursively_even_when_schema_is_last() {
    for version in 1..=3 {
        for numeric in [
            r#"{"type":"f64","value":"1.25"}"#,
            r#"{"type":"decimal","value":"0.1"}"#,
        ] {
            for value in [
                numeric.to_owned(),
                format!(r#"{{"type":"sum","nominal":"N","variant":"V","args":[{numeric}]}}"#),
                format!(
                    r#"{{"type":"record","nominal":"N","fields":[{{"name":"a","value":{numeric}}}]}}"#
                ),
                format!(r#"{{"type":"list","items":[{numeric}]}}"#),
            ] {
                let json =
                    format!(r#"{{"values":{{"x":{value}}},"schema":"brix.input@{version}"}}"#);
                assert!(
                    decode_input_shard(json.as_bytes(), &InputLimits::default()).is_err(),
                    "{json}"
                );
                assert!(
                    decode_input_shard(document(4, &value).as_bytes(), &InputLimits::default())
                        .is_ok(),
                    "{value}"
                );
            }
        }
    }
}

#[test]
fn malformed_nonfinite_and_nonstring_numeric_payloads_are_rejected() {
    for (tag, raw) in [
        ("f64", "NaN"),
        ("f64", "inf"),
        ("f64", "-inf"),
        ("f64", "1e999"),
        ("f64", ""),
        ("f64", " 1.0"),
        ("decimal", "NaN"),
        ("decimal", "0.0000000000000000001"),
        ("decimal", "170141183460469231731687303715884105728"),
    ] {
        let json = document(4, &format!(r#"{{"type":"{tag}","value":"{raw}"}}"#));
        assert!(
            decode_input_shard(json.as_bytes(), &InputLimits::default()).is_err(),
            "{json}"
        );
    }
    for tag in ["f64", "decimal"] {
        for raw in ["1.25", "true", "null"] {
            let json = document(4, &format!(r#"{{"type":"{tag}","value":{raw}}}"#));
            assert!(
                decode_input_shard(json.as_bytes(), &InputLimits::default()).is_err(),
                "{json}"
            );
        }
        let json = document(4, &format!(r#"{{"type":"{tag}","value":"1","value":"2"}}"#));
        assert!(decode_input_shard(json.as_bytes(), &InputLimits::default()).is_err());
        let json = document(4, &format!(r#"{{"type":"{tag}","value":"1","items":[]}}"#));
        assert!(decode_input_shard(json.as_bytes(), &InputLimits::default()).is_err());
    }
}

#[test]
fn numeric_normalization_preserves_identity_and_types_remain_distinct() {
    for (tag, a, b) in [("f64", "-0.0", "0"), ("decimal", "0.1000", "0.1")] {
        let a = snapshot(&document(
            4,
            &format!(r#"{{"type":"{tag}","value":"{a}"}}"#),
        ));
        let b = snapshot(&document(
            4,
            &format!(r#"{{"type":"{tag}","value":"{b}"}}"#),
        ));
        assert_eq!(a.id(), b.id());
        assert_eq!(a.values(), b.values());
    }
    let ids: std::collections::BTreeSet<_> = ["int", "f64", "decimal"]
        .iter()
        .map(|tag| snapshot(&document(4, &format!(r#"{{"type":"{tag}","value":"1"}}"#))).id())
        .collect();
    assert_eq!(ids.len(), 3);
}

#[test]
fn old_values_have_identical_snapshot_ids_under_new_schema() {
    for value in [
        r#"{"type":"int","value":"42"}"#,
        r#"{"type":"bool","value":true}"#,
        r#"{"type":"string","value":"hello"}"#,
    ] {
        let old = snapshot(&document(1, value));
        let new = snapshot(&document(4, value));
        assert_eq!(old.id(), new.id());
    }
}

#[test]
fn numeric_list_inputs_bind_execute_and_reject_wrong_domains() {
    let source = r#"
config Decision = Accepted | Rejected
input measurements: List<F64> max 4
input amounts: List<Decimal> max 4
propose accepted priority 1 when all(measurements, x => x > f64("0")) && all(amounts, x => x > decimal("0")) = Accepted
propose rejected otherwise = Rejected
commit result from (accepted, rejected)
"#;
    let module = brix_syntax::parse(source).unwrap();
    let plan = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap();
    let input = r#"{
        "schema":"brix.input@4",
        "values":{
            "measurements":{"type":"list","items":[
                {"type":"f64","value":"1.25"},
                {"type":"f64","value":"2.5"}
            ]},
            "amounts":{"type":"list","items":[
                {"type":"decimal","value":"0.1"},
                {"type":"decimal","value":"9007199254740993.1"}
            ]}
        }
    }"#;
    let bound_snapshot = snapshot(input);
    let run = FiniteDecisionRuntime::build_with_inputs(&plan, &bound_snapshot)
        .unwrap()
        .run();
    assert!(run.first_fault().is_none(), "{:?}", run.first_fault());
    assert_eq!(
        run.decision.map(|d| d.candidate).as_deref(),
        Some("accepted")
    );

    let wrong_domain = snapshot(&input.replace(
        r#"{"type":"f64","value":"1.25"}"#,
        r#"{"type":"decimal","value":"1.25"}"#,
    ));
    assert!(
        FiniteDecisionRuntime::build_with_inputs(&plan, &wrong_domain).is_err(),
        "a Decimal list must not bind to List<F64>"
    );
}
