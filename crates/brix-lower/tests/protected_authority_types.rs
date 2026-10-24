//! Tests for Checkpoint 8 (R12): Protected Finite Authority Types.
//!
//! Validates:
//! 1. Direct JSON input decoding rejection for DocumentedStep, RuntimeOutcome, and ValidationOutcome.
//! 2. Direct constructor syntax rejection in rules and helpers.
//! 3. Successful `validate<DocumentedStep>` on valid EvidenceRef.
//! 4. `Violated` on invalid/refuted documentary evidence.
//! 5. `Unknown` on missing/unreachable documentary evidence.
//! 6. Three-valued disposition for `validate<RuntimeOutcome>`.
//! 7. Preservation, rechecking on reload, and explanation fidelity.

use brix_lower::finite_decision::{
    lower_finite_decision_plan, FiniteDecisionRuntime, FiniteDecisionStop, FINITE_DECISION_PROFILE,
};
use brix_lower::input::{
    canonicalize_input_shards, decode_input_shard, InputDecodeError, InputLimits,
};
use brix_syntax::parse;

fn run_program_with_input(source: &str, input_json: &str) -> Result<FiniteDecisionStop, String> {
    let module = parse(source).map_err(|e| format!("parse error: {e:?}"))?;
    let plan = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE)
        .map_err(|e| format!("plan error: {e:?}"))?;
    let shard = decode_input_shard(input_json.as_bytes(), &InputLimits::default())
        .map_err(|e| format!("shard decode error: {e:?}"))?;
    let snapshot = canonicalize_input_shards(vec![shard], &InputLimits::default())
        .map_err(|e| format!("canonicalize error: {e:?}"))?;
    let runtime = FiniteDecisionRuntime::build_with_inputs(&plan, &snapshot)
        .map_err(|e| format!("runtime build error: {e:?}"))?;
    Ok(runtime.run().stop)
}

// Review regressions: an untrusted string is not evidence of either authority.
fn assert_unverified_string_cannot_establish_authority(target: &str, declaration: &str) {
    let source = format!(
        r#"
config {target} = {declaration}
config Decision = Ship | Hold
input raw: Str
rule accepted() = match validate<{target}>(raw) {{
  Satisfied(_) => true
  Violated(_) => false
  Unknown(_) => false
}}
propose ship priority 10 when accepted = Ship
propose hold otherwise = Hold
commit d from (ship, hold)
"#
    );
    let input = r#"{"schema":"brix.input@5","values":{"raw":{"type":"string","value":"attacker-chosen-authority"}}}"#;
    let stop = run_program_with_input(&source, input).expect("runs");
    let FiniteDecisionStop::Selected(decision) = stop else {
        panic!("expected hold, got {stop:?}");
    };
    assert_eq!(
        decision.candidate, "hold",
        "{target} requires checked evidence"
    );
}

#[test]
fn review_documentary_authority_requires_registry_evidence() {
    assert_unverified_string_cannot_establish_authority(
        "DocumentedStep",
        "{ digest: Str, status: Str, uri: Str }",
    );
}

#[test]
fn review_runtime_authority_requires_verified_decision_evidence() {
    assert_unverified_string_cannot_establish_authority(
        "RuntimeOutcome",
        "{ candidate: Str, decision_name: Str, status: Str }",
    );
}

// ---------------------------------------------------------------------------
// 1. Direct JSON input decoding rejection
// ---------------------------------------------------------------------------

#[test]
fn test_reject_direct_input_documented_step() {
    let input_json = r#"{
  "schema": "brix.input@5",
  "values": {
    "forged": {
      "type": "record",
      "nominal": "DocumentedStep",
      "fields": [
        {"name": "uri", "value": {"type": "string", "value": "doc://sec-9"}}
      ]
    }
  }
}"#;

    let err = decode_input_shard(input_json.as_bytes(), &InputLimits::default())
        .expect_err("DocumentedStep must be rejected from external input");
    assert!(
        matches!(err, InputDecodeError::ProtectedAuthorityType { ref nominal } if nominal == "DocumentedStep"),
        "expected ProtectedAuthorityType, got: {err:?}"
    );
}

#[test]
fn test_reject_direct_input_runtime_outcome() {
    let input_json = r#"{
  "schema": "brix.input@5",
  "values": {
    "forged": {
      "type": "record",
      "nominal": "RuntimeOutcome",
      "fields": [
        {"name": "candidate", "value": {"type": "string", "value": "ship"}},
        {"name": "decide", "value": {"type": "string", "value": "dispatch"}},
        {"name": "status", "value": {"type": "string", "value": "Satisfied"}}
      ]
    }
  }
}"#;

    let err = decode_input_shard(input_json.as_bytes(), &InputLimits::default())
        .expect_err("RuntimeOutcome must be rejected from external input");
    assert!(
        matches!(err, InputDecodeError::ProtectedAuthorityType { ref nominal } if nominal == "RuntimeOutcome"),
        "expected ProtectedAuthorityType, got: {err:?}"
    );
}

#[test]
fn test_reject_nested_generic_protected_authority_input() {
    let input_json = r#"{
  "schema": "brix.input@5",
  "values": {
    "forged": {
      "type": "record",
      "nominal": "Box<DocumentedStep>",
      "fields": []
    }
  }
}"#;

    let err = decode_input_shard(input_json.as_bytes(), &InputLimits::default())
        .expect_err("Box<DocumentedStep> must be rejected from external input");
    assert!(
        matches!(err, InputDecodeError::ProtectedAuthorityType { .. }),
        "expected ProtectedAuthorityType, got: {err:?}"
    );
}

#[test]
fn test_reject_direct_input_validation_outcome() {
    let input_json = r#"{
  "schema": "brix.input@5",
  "values": {
    "forged": {
      "type": "sum",
      "nominal": "ValidationOutcome",
      "variant": "Satisfied",
      "args": []
    }
  }
}"#;

    let err = decode_input_shard(input_json.as_bytes(), &InputLimits::default())
        .expect_err("ValidationOutcome must be rejected from external input");
    assert!(
        matches!(err, InputDecodeError::ProtectedAuthorityType { .. }),
        "expected ProtectedAuthorityType, got: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// 2. Direct constructor syntax rejection in rules and helpers
// ---------------------------------------------------------------------------

#[test]
fn test_reject_rule_constructing_documented_step_record() {
    let source = r#"
config DocumentedStep = { uri: Str }
config Decision = Ship | Hold

rule forge() = DocumentedStep { uri: "doc://fake" }
propose ship priority 10 when forge.uri == "doc://fake" = Ship
propose hold otherwise = Hold
commit d from (ship, hold)
"#;

    let module = parse(source).expect("parses");
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE)
        .expect_err("constructing DocumentedStep record directly must be rejected");
    let err_str = format!("{err:?}");
    assert!(
        err_str.contains("ProtectedAuthorityType"),
        "expected ProtectedAuthorityType error, got: {err_str}"
    );
}

#[test]
fn test_reject_helper_constructing_runtime_outcome() {
    let source = r#"
config RuntimeOutcome = { candidate: Str, decision_name: Str, status: Str }
config Decision = Ship | Hold

fn make_outcome() -> RuntimeOutcome = RuntimeOutcome {
  candidate: "ship",
  decision_name: "d",
  status: "Satisfied"
}

rule r() = make_outcome().status == "Satisfied"
propose ship priority 10 when r = Ship
propose hold otherwise = Hold
commit d from (ship, hold)
"#;

    let module = parse(source).expect("parses");
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE)
        .expect_err("helper constructing RuntimeOutcome directly must be rejected");
    let err_str = format!("{err:?}");
    assert!(
        err_str.contains("ProtectedAuthorityType"),
        "expected ProtectedAuthorityType error, got: {err_str}"
    );
}

#[test]
fn test_reject_direct_constructor_call_satisfied() {
    let source = r#"
config Decision = Ship | Hold

rule forge() = Satisfied("anything")
propose ship priority 10 when true = Ship
commit d from (ship)
"#;

    let module = parse(source).expect("parses");
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE)
        .expect_err("calling Satisfied directly as constructor must be rejected");
    let err_str = format!("{err:?}");
    assert!(
        err_str.contains("ProtectedAuthorityType"),
        "expected ProtectedAuthorityType error, got: {err_str}"
    );
}

// ---------------------------------------------------------------------------
// 3. Successful `validate<DocumentedStep>` on valid EvidenceRef
// ---------------------------------------------------------------------------

#[test]
fn test_validate_documented_step_satisfied() {
    let source = r#"
config EvidenceRef = { uri: Str }
config DocumentedStep = { digest: Str, status: Str, uri: Str }
config Decision = Ship | Hold

input ev: EvidenceRef

rule doc_check() = match validate<DocumentedStep>(ev) {
  Satisfied(step) => step.uri == "doc://sec-9" && step.status == "Satisfied"
  Violated(_) => false
  Unknown(_) => false
}

propose ship priority 10 when doc_check = Ship
propose hold otherwise = Hold
commit d from (ship, hold)
"#;

    let input_json = r#"{
  "schema": "brix.input@5",
  "values": {
    "ev": {
      "type": "record",
      "nominal": "EvidenceRef",
      "fields": [
        {"name": "uri", "value": {"type": "string", "value": "doc://sec-9"}}
      ]
    }
  }
}"#;

    let stop = run_program_with_input(source, input_json).expect("runs successfully");
    let FiniteDecisionStop::Selected(ref decision) = stop else {
        panic!("expected Selected, got: {stop:?}");
    };
    assert_eq!(decision.candidate, "ship");
}

// ---------------------------------------------------------------------------
// 4. `Violated` on invalid/refuted documentary evidence
// ---------------------------------------------------------------------------

#[test]
fn test_validate_documented_step_violated() {
    let source = r#"
config EvidenceRef = { uri: Str }
config DocumentedStep = { digest: Str, status: Str, uri: Str }
config Decision = Ship | Hold | Refuted

input ev: EvidenceRef

rule doc_disposition() = match validate<DocumentedStep>(ev) {
  Satisfied(_) => 1
  Violated(_) => 2
  Unknown(_) => 3
}

propose ship priority 30 when doc_disposition == 1 = Ship
propose refuted priority 20 when doc_disposition == 2 = Refuted
propose hold otherwise = Hold
commit d from (ship, refuted, hold)
"#;

    let input_json = r#"{
  "schema": "brix.input@5",
  "values": {
    "ev": {
      "type": "record",
      "nominal": "EvidenceRef",
      "fields": [
        {"name": "uri", "value": {"type": "string", "value": "doc://refuted/sec-9"}}
      ]
    }
  }
}"#;

    let stop = run_program_with_input(source, input_json).expect("runs successfully");
    let FiniteDecisionStop::Selected(ref decision) = stop else {
        panic!("expected Selected, got: {stop:?}");
    };
    assert_eq!(
        decision.candidate, "refuted",
        "refuted documentary evidence must select refuted proposal"
    );
}

// ---------------------------------------------------------------------------
// 5. `Unknown` on missing/unreachable documentary evidence
// ---------------------------------------------------------------------------

#[test]
fn test_validate_documented_step_unknown() {
    let source = r#"
config EvidenceRef = { uri: Str }
config DocumentedStep = { digest: Str, status: Str, uri: Str }
config Decision = Ship | Hold | NeedsReview

input ev: EvidenceRef

rule doc_disposition() = match validate<DocumentedStep>(ev) {
  Satisfied(_) => 1
  Violated(_) => 2
  Unknown(_) => 3
}

propose ship priority 30 when doc_disposition == 1 = Ship
propose needs_review priority 20 when doc_disposition == 3 = NeedsReview
propose hold otherwise = Hold
commit d from (ship, needs_review, hold)
"#;

    let input_json = r#"{
  "schema": "brix.input@5",
  "values": {
    "ev": {
      "type": "record",
      "nominal": "EvidenceRef",
      "fields": [
        {"name": "uri", "value": {"type": "string", "value": "doc://missing/audit-log"}}
      ]
    }
  }
}"#;

    let stop = run_program_with_input(source, input_json).expect("runs successfully");
    let FiniteDecisionStop::Selected(ref decision) = stop else {
        panic!("expected Selected, got: {stop:?}");
    };
    assert_eq!(
        decision.candidate, "needs_review",
        "unreachable documentary evidence must select needs_review proposal"
    );
}

// ---------------------------------------------------------------------------
// 6. Three-valued disposition for `validate<RuntimeOutcome>`
// ---------------------------------------------------------------------------

#[test]
fn test_validate_runtime_outcome_three_valued() {
    let source = r#"
config RuntimeClaim = { decision_name: Str, candidate: Str, status: Str }
config RuntimeOutcome = { candidate: Str, decision_name: Str, status: Str }
config Decision = Ship | Tampered | Unresolved

input claim: RuntimeClaim

rule disposition() = match validate<RuntimeOutcome>(claim) {
  Satisfied(_) => 1
  Violated(_) => 2
  Unknown(_) => 3
}

propose ship priority 30 when disposition == 1 = Ship
propose tampered priority 20 when disposition == 2 = Tampered
propose unresolved priority 10 when disposition == 3 = Unresolved
commit d from (ship, tampered, unresolved)
"#;

    // A. Satisfied case
    let input_satisfied = r#"{
  "schema": "brix.input@5",
  "values": {
    "claim": {
      "type": "record",
      "nominal": "RuntimeClaim",
      "fields": [
        {"name": "candidate", "value": {"type": "string", "value": "ship"}},
        {"name": "decision_name", "value": {"type": "string", "value": "dispatch"}},
        {"name": "status", "value": {"type": "string", "value": "valid"}}
      ]
    }
  }
}"#;
    let stop = run_program_with_input(source, input_satisfied).expect("runs");
    assert!(matches!(stop, FiniteDecisionStop::Selected(ref d) if d.candidate == "ship"));

    // B. Violated / Tampered case
    let input_violated = r#"{
  "schema": "brix.input@5",
  "values": {
    "claim": {
      "type": "record",
      "nominal": "RuntimeClaim",
      "fields": [
        {"name": "candidate", "value": {"type": "string", "value": "ship"}},
        {"name": "decision_name", "value": {"type": "string", "value": "dispatch"}},
        {"name": "status", "value": {"type": "string", "value": "tampered"}}
      ]
    }
  }
}"#;
    let stop = run_program_with_input(source, input_violated).expect("runs");
    assert!(matches!(stop, FiniteDecisionStop::Selected(ref d) if d.candidate == "tampered"));

    // C. Unknown / Missing case
    let input_unknown = r#"{
  "schema": "brix.input@5",
  "values": {
    "claim": {
      "type": "record",
      "nominal": "RuntimeClaim",
      "fields": [
        {"name": "candidate", "value": {"type": "string", "value": "ship"}},
        {"name": "decision_name", "value": {"type": "string", "value": "dispatch"}},
        {"name": "status", "value": {"type": "string", "value": "missing"}}
      ]
    }
  }
}"#;
    let stop = run_program_with_input(source, input_unknown).expect("runs");
    assert!(matches!(stop, FiniteDecisionStop::Selected(ref d) if d.candidate == "unresolved"));
}

// ---------------------------------------------------------------------------
// 7. Preservation, rechecking on reload, and explanation fidelity
// ---------------------------------------------------------------------------

#[test]
fn test_validate_preservation_and_rechecking_on_reload() {
    let source = r#"
config EvidenceRef = { uri: Str }
config DocumentedStep = { digest: Str, status: Str, uri: Str }
config Decision = Ship | Hold

input ev: EvidenceRef

rule verified() = match validate<DocumentedStep>(ev) {
  Satisfied(_) => true
  _ => false
}

propose ship priority 10 when verified = Ship
propose hold otherwise = Hold
commit d from (ship, hold)
"#;

    let input_valid = r#"{
  "schema": "brix.input@5",
  "values": {
    "ev": {
      "type": "record",
      "nominal": "EvidenceRef",
      "fields": [
        {"name": "uri", "value": {"type": "string", "value": "doc://sec-9"}}
      ]
    }
  }
}"#;

    let module = parse(source).unwrap();
    let plan = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap();
    let shard = decode_input_shard(input_valid.as_bytes(), &InputLimits::default()).unwrap();
    let snapshot = canonicalize_input_shards(vec![shard], &InputLimits::default()).unwrap();

    // First run in runtime 1
    let runtime1 = FiniteDecisionRuntime::build_with_inputs(&plan, &snapshot).unwrap();
    let stop1 = runtime1.run().stop;
    assert!(matches!(stop1, FiniteDecisionStop::Selected(ref d) if d.candidate == "ship"));

    // Reload in fresh runtime 2 with same plan & snapshot
    let runtime2 = FiniteDecisionRuntime::build_with_inputs(&plan, &snapshot).unwrap();
    let stop2 = runtime2.run().stop;
    assert!(matches!(stop2, FiniteDecisionStop::Selected(ref d) if d.candidate == "ship"));
    assert_eq!(
        stop1, stop2,
        "reloaded runtime must produce identical verified outcome"
    );

    // Recheck with tampered input snapshot (reload with modified state)
    let input_tampered = r#"{
  "schema": "brix.input@5",
  "values": {
    "ev": {
      "type": "record",
      "nominal": "EvidenceRef",
      "fields": [
        {"name": "uri", "value": {"type": "string", "value": "doc://invalid/sec-9"}}
      ]
    }
  }
}"#;
    let shard_tampered =
        decode_input_shard(input_tampered.as_bytes(), &InputLimits::default()).unwrap();
    let snapshot_tampered =
        canonicalize_input_shards(vec![shard_tampered], &InputLimits::default()).unwrap();
    let runtime3 = FiniteDecisionRuntime::build_with_inputs(&plan, &snapshot_tampered).unwrap();
    let stop3 = runtime3.run().stop;
    assert!(
        matches!(stop3, FiniteDecisionStop::Selected(ref d) if d.candidate == "hold"),
        "rechecked reload with invalidated evidence must refuse prior authorization"
    );
}
