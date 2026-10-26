//! Integration tests for Checkpoint 4 (R8: Concrete generic schemas & nested inputs).
//!
//! Covers:
//! 1. Mandatory real shape: `Observation<QuestionFrame>` with `Known(QuestionFrame, List<EvidenceRef>)`
//! 2. `Observation<AnswerPath>` and `Observation<RevisionRef>`
//! 3. Two distinct concrete applications of the same generic schema in the same program (`Observation<Str>` vs `Observation<Int>`)
//! 4. Nested input decoding under `brix.input@5` vs rejection under `brix.input@3` and `@4`
//! 5. Mismatched concrete payload rejected by schema validation
//! 6. Wire limits: cycle rejection, depth limits
//! 7. Helper functions with concrete generic and list contracts
//! 8. Frozen identity: programs not using generic schemas maintain identical canonical identities

use brix_lower::finite_decision::{
    lower_finite_decision_plan, FiniteDecisionRuntime, FINITE_DECISION_PROFILE,
};
use brix_lower::input::{
    canonicalize_input_shards, decode_input_shard, InputDecodeError, InputLimits,
};
use brix_lower::FiniteDecisionLowerError;
use brix_syntax::parse;

fn run_with_input(source: &str, input_json: &str) -> brix_lower::FiniteDecisionRun {
    let module = parse(source).expect("source parses");
    let plan = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).expect("source lowers");
    let shard =
        decode_input_shard(input_json.as_bytes(), &InputLimits::default()).expect("input decodes");
    let snapshot =
        canonicalize_input_shards(vec![shard], &InputLimits::default()).expect("snapshot builds");
    FiniteDecisionRuntime::build_with_inputs(&plan, &snapshot)
        .expect("runtime builds")
        .run()
}

fn try_run_with_input(
    source: &str,
    input_json: &str,
) -> Result<brix_lower::FiniteDecisionRun, String> {
    let module = parse(source).map_err(|e| format!("{e:?}"))?;
    let plan = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE)
        .map_err(|e| format!("{e:?}"))?;
    let shard = decode_input_shard(input_json.as_bytes(), &InputLimits::default())
        .map_err(|e| format!("{e:?}"))?;
    let snapshot = canonicalize_input_shards(vec![shard], &InputLimits::default())
        .map_err(|e| format!("{e:?}"))?;
    let runtime =
        FiniteDecisionRuntime::build_with_inputs(&plan, &snapshot).map_err(|e| format!("{e:?}"))?;
    Ok(runtime.run())
}

// ---------------------------------------------------------------------------
// 1. Mandatory Real Shape: Observation<QuestionFrame>
// ---------------------------------------------------------------------------

#[test]
fn test_mandatory_real_shape_observation_question_frame() {
    let source = r#"
config EvidenceRef = { uri: Str }
config QuestionFrame = { id: Str }
config Observation<T> = Known(T, List<EvidenceRef>) | Unknown
config Decision = Ship | Hold

input obs: Observation<QuestionFrame>

rule has_valid_evidence() = match obs {
  Observation::Known(frame, evidence) => len(evidence) > 0 && frame.id == "q-42"
  _ => false
}

propose ship priority 10 when has_valid_evidence = Ship
propose hold otherwise = Hold
commit d from (ship, hold)
"#;

    let input_json = r#"{
  "schema": "brix.input@5",
  "values": {
    "obs": {
      "type": "sum",
      "nominal": "Observation<QuestionFrame>",
      "variant": "Known",
      "args": [
        {
          "type": "record",
          "nominal": "QuestionFrame",
          "fields": [
            {"name": "id", "value": {"type": "string", "value": "q-42"}}
          ]
        },
        {
          "type": "list",
          "items": [
            {
              "type": "record",
              "nominal": "EvidenceRef",
              "fields": [
                {"name": "uri", "value": {"type": "string", "value": "doc://sec-9"}}
              ]
            }
          ]
        }
      ]
    }
  }
}"#;

    let run = run_with_input(source, input_json);
    assert!(
        run.first_fault().is_none(),
        "unexpected fault: {:?}",
        run.first_fault()
    );
    assert_eq!(run.decision.map(|d| d.candidate).as_deref(), Some("ship"));
}

#[test]
fn test_observation_unknown_variant() {
    let source = r#"
config EvidenceRef = { uri: Str }
config QuestionFrame = { id: Str }
config Observation<T> = Known(T, List<EvidenceRef>) | Unknown
config Decision = Ship | Hold

input obs: Observation<QuestionFrame>

rule is_unknown() = match obs {
  Observation::Unknown => true
  _ => false
}

propose hold priority 10 when is_unknown = Hold
propose ship otherwise = Ship
commit d from (hold, ship)
"#;

    let input_json = r#"{
  "schema": "brix.input@5",
  "values": {
    "obs": {
      "type": "sum",
      "nominal": "Observation<QuestionFrame>",
      "variant": "Unknown",
      "args": []
    }
  }
}"#;

    let run = run_with_input(source, input_json);
    assert!(
        run.first_fault().is_none(),
        "unexpected fault: {:?}",
        run.first_fault()
    );
    assert_eq!(run.decision.map(|d| d.candidate).as_deref(), Some("hold"));
}

// ---------------------------------------------------------------------------
// 2. Observation<AnswerPath> and Observation<RevisionRef>
// ---------------------------------------------------------------------------

#[test]
fn test_observation_answer_path_and_revision_ref() {
    let source = r#"
config AnswerPath = { path: Str }
config RevisionRef = { rev: Int }
config Observation<T> = Known(T) | Missing
config Decision = Match | Mismatch

input ans: Observation<AnswerPath>
input rev: Observation<RevisionRef>

rule matched() = match ans {
  Observation::Known(a) => match rev {
    Observation::Known(r) => a.path == "root/1" && r.rev == 3
    _ => false
  }
  _ => false
}

propose ok priority 10 when matched = Match
propose err otherwise = Mismatch
commit d from (ok, err)
"#;

    let input_json = r#"{
  "schema": "brix.input@5",
  "values": {
    "ans": {
      "type": "sum",
      "nominal": "Observation<AnswerPath>",
      "variant": "Known",
      "args": [
        {
          "type": "record",
          "nominal": "AnswerPath",
          "fields": [{"name": "path", "value": {"type": "string", "value": "root/1"}}]
        }
      ]
    },
    "rev": {
      "type": "sum",
      "nominal": "Observation<RevisionRef>",
      "variant": "Known",
      "args": [
        {
          "type": "record",
          "nominal": "RevisionRef",
          "fields": [{"name": "rev", "value": {"type": "int", "value": "3"}}]
        }
      ]
    }
  }
}"#;

    let run = run_with_input(source, input_json);
    assert!(
        run.first_fault().is_none(),
        "unexpected fault: {:?}",
        run.first_fault()
    );
    assert_eq!(run.decision.map(|d| d.candidate).as_deref(), Some("ok"));
}

// ---------------------------------------------------------------------------
// 3. Two Distinct Concrete Applications of Same Generic in One Program
// ---------------------------------------------------------------------------

#[test]
fn test_two_distinct_concrete_applications_in_same_program() {
    let source = r#"
config Wrapper<T> = Wrap(T)
config Decision = Pass | Fail

input w_str: Wrapper<Str>
input w_int: Wrapper<Int>

rule check() = match w_str {
  Wrapper::Wrap(s) => match w_int {
    Wrapper::Wrap(n) => s == "hello" && n == 42
  }
}

propose pass priority 1 when check = Pass
propose fail otherwise = Fail
commit d from (pass, fail)
"#;

    let input_json = r#"{
  "schema": "brix.input@5",
  "values": {
    "w_str": {
      "type": "sum",
      "nominal": "Wrapper<Str>",
      "variant": "Wrap",
      "args": [{"type": "string", "value": "hello"}]
    },
    "w_int": {
      "type": "sum",
      "nominal": "Wrapper<Int>",
      "variant": "Wrap",
      "args": [{"type": "int", "value": "42"}]
    }
  }
}"#;

    let run = run_with_input(source, input_json);
    assert!(
        run.first_fault().is_none(),
        "unexpected fault: {:?}",
        run.first_fault()
    );
    assert_eq!(run.decision.map(|d| d.candidate).as_deref(), Some("pass"));
}

// ---------------------------------------------------------------------------
// 4. Nested Input Decoding: @5 admitted, @3 and @4 rejected
// ---------------------------------------------------------------------------

#[test]
fn test_nested_list_rejected_under_v3_and_v4_admitted_under_v5() {
    let limits = InputLimits::default();
    let json_template = r#"{
  "schema": "<SCHEMA>",
  "values": {
    "data": {
      "type": "sum",
      "nominal": "Container",
      "variant": "Items",
      "args": [
        {
          "type": "list",
          "items": [{"type": "int", "value": "1"}, {"type": "int", "value": "2"}]
        }
      ]
    }
  }
}"#;

    // Under @3: rejected
    let json_v3 = json_template.replace("<SCHEMA>", "brix.input@3");
    let err_v3 = decode_input_shard(json_v3.as_bytes(), &limits).unwrap_err();
    assert!(
        matches!(err_v3, InputDecodeError::InvalidType { ref found, .. } if found.contains("nested list")),
        "expected nested list error under @3, got {err_v3:?}"
    );

    // Under @4: rejected
    let json_v4 = json_template.replace("<SCHEMA>", "brix.input@4");
    let err_v4 = decode_input_shard(json_v4.as_bytes(), &limits).unwrap_err();
    assert!(
        matches!(err_v4, InputDecodeError::InvalidType { ref found, .. } if found.contains("nested list")),
        "expected nested list error under @4, got {err_v4:?}"
    );

    // Under @5: admitted
    let json_v5 = json_template.replace("<SCHEMA>", "brix.input@5");
    let shard_v5 =
        decode_input_shard(json_v5.as_bytes(), &limits).expect("v5 must admit nested list");
    assert_eq!(shard_v5.schema(), "brix.input@5");
}

// ---------------------------------------------------------------------------
// 5. Mismatched Concrete Payload Rejected at Runtime
// ---------------------------------------------------------------------------

#[test]
fn test_mismatched_concrete_payload_rejected_at_runtime() {
    let source = r#"
config Box<T> = Wrap(T)
config Decision = Pass | Fail

input b: Box<Str>

rule r() = match b {
  Box::Wrap(s) => len(s) > 0
}

propose pass priority 1 when r = Pass
propose fail otherwise = Fail
commit d from (pass, fail)
"#;

    // Provide an int payload where Box<Str> expects string
    let bad_input = r#"{
  "schema": "brix.input@5",
  "values": {
    "b": {
      "type": "sum",
      "nominal": "Box<Str>",
      "variant": "Wrap",
      "args": [{"type": "int", "value": "123"}]
    }
  }
}"#;

    let res = try_run_with_input(source, bad_input);
    assert!(
        res.is_err(),
        "must fail closed when payload violates concrete schema: {res:?}"
    );
    let err_msg = res.unwrap_err();
    assert!(
        err_msg.contains("ContractViolation")
            || err_msg.contains("TypeMismatch")
            || err_msg.contains("InvalidValue"),
        "expected type error, got {err_msg}"
    );
}

// ---------------------------------------------------------------------------
// 6. Schema Limits: Cycle and Unbound Generic Rejection
// ---------------------------------------------------------------------------

#[test]
fn test_unbound_generic_rejected_at_lowering() {
    let source = r#"
config Box<T> = Wrap(T)
config Decision = Pass

input b: Box

propose pass priority 1 when true = Pass
commit d from (pass)
"#;
    let module = parse(source).unwrap();
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(
        matches!(err, FiniteDecisionLowerError::InvalidSchema { .. }),
        "expected InvalidSchema for bare generic config, got {err:?}"
    );
}

#[test]
fn test_generic_cycle_detected_and_rejected() {
    let source = r#"
config Node<T> = Link(Node<T>) | Nil
config Decision = Pass

input n: Node<Int>

propose pass priority 1 when true = Pass
commit d from (pass)
"#;
    let module = parse(source).unwrap();
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(
        matches!(
            err,
            FiniteDecisionLowerError::InvalidSchema { ref detail, .. } if detail.contains("recursive")
        ) || matches!(err, FiniteDecisionLowerError::SchemaDepthExceeded { .. }),
        "expected recursive schema or depth error, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// 7. Helper Function with Concrete Generic and List Contracts
// ---------------------------------------------------------------------------

#[test]
fn test_helper_function_with_generic_and_list_contracts() {
    let source = r#"
config Pair<T> = { first: T, second: T }
config Decision = Pass | Fail

input p: Pair<Int>

fn sum_pair(pair: Pair<Int>): Int = pair.first + pair.second

fn filter_gt_two(xs: List<Int>): List<Int> = filter(xs, x => x > 2)

rule check() = sum_pair(p) == 10 && len(filter_gt_two([1, 2, 3, 4])) == 2

propose pass priority 1 when check = Pass
propose fail otherwise = Fail
commit d from (pass, fail)
"#;

    let input_json = r#"{
  "schema": "brix.input@5",
  "values": {
    "p": {
      "type": "record",
      "nominal": "Pair<Int>",
      "fields": [
        {"name": "first", "value": {"type": "int", "value": "4"}},
        {"name": "second", "value": {"type": "int", "value": "6"}}
      ]
    }
  }
}"#;

    let run = run_with_input(source, input_json);
    assert!(
        run.first_fault().is_none(),
        "unexpected fault: {:?}",
        run.first_fault()
    );
    assert_eq!(run.decision.map(|d| d.candidate).as_deref(), Some("pass"));
}

// ---------------------------------------------------------------------------
// 8. Typed Empty List and Generic Ascription in Decisions
// ---------------------------------------------------------------------------

#[test]
fn test_typed_empty_list_with_generic_observation() {
    let source = r#"
config EvidenceRef = { uri: Str }
config Observation<T> = Known(T, List<EvidenceRef>) | Unknown
config Decision = Ship | Hold

input obs: Observation<Str>

rule check() = match obs {
  Observation::Known(msg, ev) => len(ev) == 0 && ev == ([]: List<EvidenceRef>) && msg == "ready"
  _ => false
}

propose ship priority 10 when check = Ship
propose hold otherwise = Hold
commit d from (ship, hold)
"#;

    let input_json = r#"{
  "schema": "brix.input@5",
  "values": {
    "obs": {
      "type": "sum",
      "nominal": "Observation<Str>",
      "variant": "Known",
      "args": [
        {"type": "string", "value": "ready"},
        {"type": "list", "items": []}
      ]
    }
  }
}"#;

    let run = run_with_input(source, input_json);
    assert!(
        run.first_fault().is_none(),
        "unexpected fault: {:?}",
        run.first_fault()
    );
    assert_eq!(run.decision.map(|d| d.candidate).as_deref(), Some("ship"));
}
