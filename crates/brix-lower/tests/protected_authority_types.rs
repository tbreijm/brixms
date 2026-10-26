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

#[test]
fn review_default_registry_must_not_trust_a_fixture_uri() {
    let source = r#"
config DocumentedStep = { digest: Str, status: Str, uri: Str }
config Decision = Ship | Hold
input raw: Str
rule accepted() = match validate<DocumentedStep>(raw) {
  Satisfied(_) => true
  Violated(_) => false
  Unknown(_) => false
}
propose ship priority 10 when accepted = Ship
propose hold otherwise = Hold
commit d from (ship, hold)
"#;
    let input =
        r#"{"schema":"brix.input@5","values":{"raw":{"type":"string","value":"doc://sec-9"}}}"#;
    let stop = run_program_with_input(source, input).expect("runs");
    assert!(
        matches!(stop, FiniteDecisionStop::Selected(ref d) if d.candidate == "hold"),
        "no registry was supplied; a test fixture URI must not authorize: {stop:?}"
    );
}

#[test]
fn review_forged_structured_runtime_claim_cannot_use_default_registry() {
    let source = r#"
config RuntimeClaim = { candidate: Str, decision_name: Str, status: Str }
config RuntimeOutcome = { candidate: Str, decision_name: Str, status: Str }
config Decision = Ship | Hold
input raw: RuntimeClaim
rule accepted() = match validate<RuntimeOutcome>(raw) {
  Satisfied(_) => true
  Violated(_) => false
  Unknown(_) => false
}
propose ship priority 10 when accepted = Ship
propose hold otherwise = Hold
commit d from (ship, hold)
"#;
    let input = r#"{"schema":"brix.input@5","values":{"raw":{"type":"record","nominal":"RuntimeClaim","fields":[
      {"name":"candidate","value":{"type":"string","value":"ship"}},
      {"name":"decision_name","value":{"type":"string","value":"attacker::dispatch"}},
      {"name":"status","value":{"type":"string","value":"verified"}}
    ]}}}"#;
    let stop = run_program_with_input(source, input).expect("runs");
    assert!(
        matches!(stop, FiniteDecisionStop::Selected(ref d) if d.candidate == "hold"),
        "no verified runtime receipt was supplied: {stop:?}"
    );
}

// ---------------------------------------------------------------------------
// 1. Direct JSON input decoding rejection
// ---------------------------------------------------------------------------

#[test]
fn review_public_constructor_cannot_mint_verified_runtime_evidence() {
    use brix_canon::{Digest, Domain};
    use brix_lower::evidence::{
        Authority, Receipt, VerifiedDecisionClaim, VerifiedDecisionEvidence,
    };
    use brix_lower::l3_v2::DocumentaryRegistry;
    let digest = Digest::of(Domain::Value, b"no world or replay exists");
    let claim = VerifiedDecisionClaim {
        world: digest,
        program: digest,
        revision: 1,
        revision_digest: digest,
        decide: "attacker::dispatch".into(),
        entity: "q1".into(),
        candidate: "ship".into(),
        value_digest: digest,
        caller: Some("attacker".into()),
        grants: vec!["admin:all".into()],
        release: "forged-release".into(),
    };
    let receipt = Receipt::issue(
        "decision:attacker::dispatch:q1:ship",
        "q1",
        digest.to_hex(),
        Authority::RuntimeCheck,
        vec!["admin:all".into()],
        None,
    );
    let forged = VerifiedDecisionEvidence::__issue_from_world_verifier(claim, receipt);
    assert!(
        DocumentaryRegistry::new()
            .admit_runtime_evidence(forged)
            .is_err(),
        "an external caller performed no verification and must not mint an accepted handle"
    );
}

#[test]
fn review_caller_built_trie_is_not_independent_world_replay() {
    use brix_canon::{CanonWriter, Digest, Domain};
    use brix_lower::evidence::{
        verify_runtime_decision_proof, Authority, DecisionProofKey, DecisionProofTuple,
        DecisionReplayProof, Receipt, VerifiedDecisionClaim,
    };
    use soc_core::store::{NodeStore, StorageError, TrieMap};
    struct Store(std::collections::BTreeMap<Digest, Vec<u8>>);
    impl NodeStore for Store {
        fn get_node(&self, digest: &Digest) -> Result<Option<Vec<u8>>, StorageError> {
            Ok(self.0.get(digest).cloned())
        }
        fn put_node(&mut self, digest: Digest, bytes: Vec<u8>) {
            self.0.insert(digest, bytes);
        }
    }
    let invented = Digest::of(Domain::Value, b"invented world/program/revision");
    let bytes = b"invented result, not replayed";
    let claim = VerifiedDecisionClaim {
        world: invented,
        program: invented,
        revision: 99,
        revision_digest: invented,
        decide: "attacker::plan".into(),
        entity: "q1".into(),
        candidate: "approve".into(),
        value_digest: Digest::of(Domain::Value, bytes),
        caller: Some("attacker".into()),
        grants: vec!["admin:all".into()],
        release: "invented".into(),
    };
    let receipt = Receipt::issue(
        "invented receipt",
        "q1",
        invented.to_hex(),
        Authority::RuntimeCheck,
        claim.grants.clone(),
        None,
    );
    let mut key = CanonWriter::new();
    key.write_tag("brix.world.decision-key@1");
    key.write_str(&claim.decide);
    key.write_str(&claim.entity);
    let mut tuple = CanonWriter::new();
    tuple.write_tag("brix.world.decision@1");
    tuple.write_str(&claim.candidate); // Deliberately no settlement value or revision history.
    let trie = TrieMap::<DecisionProofKey, DecisionProofTuple>::new().insert(
        DecisionProofKey(key.finish()),
        DecisionProofTuple(tuple.finish()),
    );
    let mut store = Store(std::collections::BTreeMap::new());
    trie.persist_to_store(&mut store);
    let proof = DecisionReplayProof {
        decision_root: trie.root_digest(),
        proof_nodes: &store.0,
        replayed_value_bytes: bytes,
    };
    assert!(
        verify_runtime_decision_proof(&claim, &receipt, &proof).is_err(),
        "a caller-chosen root and receipt must not issue verified runtime authority"
    );
}

#[test]
fn review_public_witness_constant_cannot_authorize_handle_issuance() {
    use brix_canon::{Digest, Domain};
    use brix_lower::evidence::{
        Authority, Receipt, VerifiedDecisionClaim, VerifiedDecisionEvidence, WORLD_VERIFIER_WITNESS,
    };
    use brix_lower::l3_v2::DocumentaryRegistry;
    let digest = Digest::of(Domain::Value, b"no verified world exists");
    let claim = VerifiedDecisionClaim {
        world: digest,
        program: digest,
        revision: 1,
        revision_digest: digest,
        decide: "attacker::plan".into(),
        entity: "q1".into(),
        candidate: "approve".into(),
        value_digest: digest,
        caller: Some("attacker".into()),
        grants: vec!["admin:all".into()],
        release: "invented".into(),
    };
    let receipt = Receipt::issue(
        "invented receipt",
        "q1",
        digest.to_hex(),
        Authority::RuntimeCheck,
        claim.grants.clone(),
        None,
    );
    let result = VerifiedDecisionEvidence::__issue_authenticated_for_world_verifier(
        claim,
        receipt,
        &WORLD_VERIFIER_WITNESS,
    );
    if let Ok(handle) = result {
        assert!(
            DocumentaryRegistry::new()
                .admit_runtime_evidence(handle)
                .is_err(),
            "a publicly exported constant must not grant verifier authority to external callers"
        );
    }
}

#[test]
fn review_documented_step_requires_documentary_kind_not_just_matching_labels() {
    use brix_canon::{Digest, Domain};
    use brix_lower::l3_v2::{
        DocumentaryDisposition, DocumentaryEntry, DocumentaryRegistry, DocumentarySnapshot,
    };
    let snapshot = DocumentarySnapshot::new().with_entry(DocumentaryEntry {
        evidence_id: "doc://runtime-entry".into(),
        content_digest: Digest::of(Domain::Value, b"runtime outcome"),
        evidence_kind: "runtime-outcome".into(),
        authority_scope: "runtime".into(),
        disposition: DocumentaryDisposition::Satisfied,
    });
    let pin = snapshot.digest();
    let registry = std::sync::Arc::new(DocumentaryRegistry::from_snapshot(snapshot, pin).unwrap());
    let source = r#"
config EvidenceRef = { uri: Str, kind: Str, scope: Str }
config DocumentedStep = { digest: Str, status: Str, uri: Str }
config Decision = Ship | Hold
input ev: EvidenceRef
rule accepted() = match validate<DocumentedStep>(ev) {
 Satisfied(_) => true
 Violated(_) => false
 Unknown(_) => false
}
propose ship priority 10 when accepted = Ship
propose hold otherwise = Hold
commit d from (ship, hold)
"#;
    let input = r#"{"schema":"brix.input@5","values":{"ev":{"type":"record","nominal":"EvidenceRef","fields":[
      {"name":"kind","value":{"type":"string","value":"runtime-outcome"}},
      {"name":"scope","value":{"type":"string","value":"runtime"}},
      {"name":"uri","value":{"type":"string","value":"doc://runtime-entry"}}
    ]}}}"#;
    let stop = run_program_with_input_and_registry(source, input, registry).expect("runs");
    assert!(
        matches!(stop, FiniteDecisionStop::Selected(ref d) if d.candidate == "hold"),
        "runtime evidence must not become DocumentedStep: {stop:?}"
    );
}

#[test]
fn review_non_ascii_digest_is_refused_without_panicking() {
    let source = r#"
config RuntimeClaim = { decision_name: Str, entity: Str, world: Str, revision: Int, candidate: Str, result: Str, caller: Str, release: Str, grants: List<Str> }
config RuntimeOutcome = { candidate: Str, decision_name: Str, status: Str }
config Decision = Ship | Hold
input raw: RuntimeClaim
rule accepted() = match validate<RuntimeOutcome>(raw) {
 Satisfied(_) => true
 Violated(_) => false
 Unknown(_) => false
}
propose ship priority 10 when accepted = Ship
propose hold otherwise = Hold
commit d from (ship, hold)
"#;
    let bad_hex = format!("€{}", "a".repeat(61));
    assert_eq!(bad_hex.len(), 64);
    let input = format!(
        r#"{{"schema":"brix.input@5","values":{{"raw":{{"type":"record","nominal":"RuntimeClaim","fields":[
      {{"name":"caller","value":{{"type":"string","value":"caller"}}}},
      {{"name":"candidate","value":{{"type":"string","value":"ship"}}}},
      {{"name":"decision_name","value":{{"type":"string","value":"ops::dispatch"}}}},
      {{"name":"entity","value":{{"type":"string","value":"q1"}}}},
      {{"name":"grants","value":{{"type":"list","items":[]}}}},
      {{"name":"release","value":{{"type":"string","value":"release"}}}},
      {{"name":"result","value":{{"type":"string","value":"{}"}}}},
      {{"name":"revision","value":{{"type":"int","value":"1"}}}},
      {{"name":"world","value":{{"type":"string","value":"{bad_hex}"}}}}
    ]}}}}}}"#,
        "0".repeat(64)
    );
    let outcome = std::panic::catch_unwind(|| run_program_with_input(source, &input));
    assert!(
        outcome.is_ok(),
        "a malformed external digest must not panic the evaluator"
    );
    let stop = outcome.unwrap().expect("runs with refusal");
    assert!(matches!(stop, FiniteDecisionStop::Selected(ref d) if d.candidate == "hold"));
}

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

fn build_fixture_documentary_registry() -> std::sync::Arc<brix_lower::l3_v2::DocumentaryRegistry> {
    use brix_canon::{Digest, Domain};
    use brix_lower::l3_v2::{
        DocumentaryDisposition, DocumentaryEntry, DocumentaryRegistry, DocumentarySnapshot,
    };

    let snapshot = DocumentarySnapshot::new()
        .with_entry(DocumentaryEntry {
            evidence_id: "doc://sec-9".to_string(),
            content_digest: Digest::of(Domain::Claim, b"doc://sec-9:content-v1"),
            evidence_kind: "policy-clause".to_string(),
            authority_scope: "shipping-release".to_string(),
            disposition: DocumentaryDisposition::Satisfied,
        })
        .with_entry(DocumentaryEntry {
            evidence_id: "doc://refuted/sec-9".to_string(),
            content_digest: Digest::of(Domain::Claim, b"doc://refuted/sec-9:content-v1"),
            evidence_kind: "policy-clause".to_string(),
            authority_scope: "shipping-release".to_string(),
            disposition: DocumentaryDisposition::Violated(
                "documentary evidence refuted or invalid: doc://refuted/sec-9".to_string(),
            ),
        });
    let expected_digest = snapshot.digest();
    std::sync::Arc::new(
        DocumentaryRegistry::from_snapshot(snapshot, expected_digest)
            .expect("verified documentary snapshot"),
    )
}

fn run_program_with_input_and_registry(
    source: &str,
    input_json: &str,
    registry: std::sync::Arc<brix_lower::l3_v2::DocumentaryRegistry>,
) -> Result<FiniteDecisionStop, String> {
    let module = parse(source).map_err(|e| format!("parse error: {e:?}"))?;
    let plan = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE)
        .map_err(|e| format!("plan error: {e:?}"))?;
    let shard = decode_input_shard(input_json.as_bytes(), &InputLimits::default())
        .map_err(|e| format!("shard decode error: {e:?}"))?;
    let snapshot = canonicalize_input_shards(vec![shard], &InputLimits::default())
        .map_err(|e| format!("canonicalize error: {e:?}"))?;
    let runtime = FiniteDecisionRuntime::build_with_inputs(&plan, &snapshot)
        .map_err(|e| format!("runtime build error: {e:?}"))?
        .with_documentary_registry(registry);
    Ok(runtime.run().stop)
}

#[test]
fn test_validate_documented_step_satisfied() {
    use brix_canon::{Digest, Domain};
    use brix_lower::l3_v2::{DocumentaryRegistry, DocumentarySnapshot};

    // Snapshot digest mismatch must be rejected at admission
    let wrong_digest = Digest::of(Domain::Snapshot, b"wrong-digest");
    assert!(
        DocumentaryRegistry::from_snapshot(DocumentarySnapshot::new(), wrong_digest).is_err(),
        "mismatched documentary snapshot digest must be rejected"
    );

    let source = r#"
config EvidenceRef = { uri: Str, kind: Str, scope: Str }
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
        {"name": "kind", "value": {"type": "string", "value": "policy-clause"}},
        {"name": "scope", "value": {"type": "string", "value": "shipping-release"}},
        {"name": "uri", "value": {"type": "string", "value": "doc://sec-9"}}
      ]
    }
  }
}"#;

    let registry = build_fixture_documentary_registry();
    let stop = run_program_with_input_and_registry(source, input_json, registry)
        .expect("runs successfully");
    let FiniteDecisionStop::Selected(ref decision) = stop else {
        panic!("expected Selected, got: {stop:?}");
    };
    assert_eq!(decision.candidate, "ship");
}

// ---------------------------------------------------------------------------
// 4. `Violated` on invalid/refuted documentary evidence or kind/scope mismatch
// ---------------------------------------------------------------------------

#[test]
fn test_validate_documented_step_violated() {
    let source = r#"
config EvidenceRef = { uri: Str, kind: Str, scope: Str }
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
        {"name": "kind", "value": {"type": "string", "value": "policy-clause"}},
        {"name": "scope", "value": {"type": "string", "value": "shipping-release"}},
        {"name": "uri", "value": {"type": "string", "value": "doc://refuted/sec-9"}}
      ]
    }
  }
}"#;

    let registry = build_fixture_documentary_registry();
    let stop = run_program_with_input_and_registry(source, input_json, registry.clone())
        .expect("runs successfully");
    let FiniteDecisionStop::Selected(ref decision) = stop else {
        panic!("expected Selected, got: {stop:?}");
    };
    assert_eq!(
        decision.candidate, "refuted",
        "refuted documentary evidence must select refuted proposal"
    );

    // Kind or scope mismatch against a satisfied entry must also produce Violated
    let wrong_scope_json = r#"{
  "schema": "brix.input@5",
  "values": {
    "ev": {
      "type": "record",
      "nominal": "EvidenceRef",
      "fields": [
        {"name": "kind", "value": {"type": "string", "value": "policy-clause"}},
        {"name": "scope", "value": {"type": "string", "value": "wrong-scope"}},
        {"name": "uri", "value": {"type": "string", "value": "doc://sec-9"}}
      ]
    }
  }
}"#;
    let stop_wrong_scope =
        run_program_with_input_and_registry(source, wrong_scope_json, registry).expect("runs");
    let FiniteDecisionStop::Selected(ref decision) = stop_wrong_scope else {
        panic!("expected Selected, got: {stop_wrong_scope:?}");
    };
    assert_eq!(
        decision.candidate, "refuted",
        "authority scope mismatch must produce Violated"
    );
}

// ---------------------------------------------------------------------------
// 5. `Unknown` on missing/unreachable documentary evidence
// ---------------------------------------------------------------------------

#[test]
fn test_validate_documented_step_unknown() {
    let source = r#"
config EvidenceRef = { uri: Str, kind: Str, scope: Str }
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
        {"name": "kind", "value": {"type": "string", "value": "policy-clause"}},
        {"name": "scope", "value": {"type": "string", "value": "shipping-release"}},
        {"name": "uri", "value": {"type": "string", "value": "doc://missing/audit-log"}}
      ]
    }
  }
}"#;

    let registry = build_fixture_documentary_registry();
    let stop = run_program_with_input_and_registry(source, input_json, registry)
        .expect("runs successfully");
    let FiniteDecisionStop::Selected(ref decision) = stop else {
        panic!("expected Selected, got: {stop:?}");
    };
    assert_eq!(
        decision.candidate, "needs_review",
        "unreachable documentary evidence must select needs_review proposal"
    );
}

// ---------------------------------------------------------------------------
// 6. Three-valued disposition for `validate<RuntimeOutcome>` with exact target checking
// ---------------------------------------------------------------------------

#[test]
fn test_validate_runtime_outcome_three_valued() {
    use brix_canon::{Digest, Domain};
    use brix_lower::evidence::{
        Authority, Receipt, VerifiedDecisionClaim, VerifiedDecisionEvidence, WORLD_VERIFIER_WITNESS,
    };
    use brix_lower::l3_v2::DocumentaryRegistry;

    let world = Digest::of(Domain::Value, b"world-v1");
    let program = Digest::of(Domain::Value, b"program-v1");
    let rev_digest = Digest::of(Domain::Value, b"rev-1");
    let replayed_value_bytes = b"settled-ship-value";
    let val_digest = Digest::of(Domain::Value, replayed_value_bytes);

    let receipt = Receipt::issue(
        "decision:ops::dispatch:q1@1:ship",
        "q1",
        rev_digest.to_hex(),
        Authority::RuntimeCheck,
        vec!["read:atlas".to_string()],
        Some("atlas-2026.10".to_string()),
    );
    let claim = VerifiedDecisionClaim {
        world,
        program,
        revision: 1,
        revision_digest: rev_digest,
        decide: "ops::dispatch".to_string(),
        entity: "q1".to_string(),
        candidate: "ship".to_string(),
        value_digest: val_digest,
        caller: Some("service:atlas".to_string()),
        release: "atlas-2026.10".to_string(),
        grants: vec!["read:atlas".to_string()],
    };

    let unauthenticated = VerifiedDecisionEvidence::__issue_authenticated_for_world_verifier(
        claim,
        receipt,
        &WORLD_VERIFIER_WITNESS,
    )
    .expect("handle produced");

    let mut reg = DocumentaryRegistry::new();
    assert!(
        reg.admit_runtime_evidence(unauthenticated).is_err(),
        "external caller without independent world replay must be refused admission"
    );
    let registry = std::sync::Arc::new(reg);

    let source = r#"
config RuntimeClaim = {
  decision_name: Str,
  entity: Str,
  world: Str,
  program: Str,
  revision: Int,
  revision_digest: Str,
  candidate: Str,
  result: Str,
  caller: Str,
  release: Str,
  grants: List<Str>,
  status: Str
}
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

    let make_input = |decide: &str,
                      entity: &str,
                      world_hex: &str,
                      prog_hex: &str,
                      revision: i64,
                      rev_digest_hex: &str,
                      candidate: &str,
                      result_hex: &str,
                      caller: &str,
                      release: &str,
                      grant: &str,
                      status: &str| {
        format!(
            r#"{{
  "schema": "brix.input@5",
  "values": {{
    "claim": {{
      "type": "record",
      "nominal": "RuntimeClaim",
      "fields": [
        {{"name": "caller", "value": {{"type": "string", "value": "{caller}"}}}},
        {{"name": "candidate", "value": {{"type": "string", "value": "{candidate}"}}}},
        {{"name": "decision_name", "value": {{"type": "string", "value": "{decide}"}}}},
        {{"name": "entity", "value": {{"type": "string", "value": "{entity}"}}}},
        {{"name": "grants", "value": {{"type": "list", "items": [{{"type": "string", "value": "{grant}"}}]}}}},
        {{"name": "program", "value": {{"type": "string", "value": "{prog_hex}"}}}},
        {{"name": "release", "value": {{"type": "string", "value": "{release}"}}}},
        {{"name": "result", "value": {{"type": "string", "value": "{result_hex}"}}}},
        {{"name": "revision", "value": {{"type": "int", "value": "{revision}"}}}},
        {{"name": "revision_digest", "value": {{"type": "string", "value": "{rev_digest_hex}"}}}},
        {{"name": "status", "value": {{"type": "string", "value": "{status}"}}}},
        {{"name": "world", "value": {{"type": "string", "value": "{world_hex}"}}}}
      ]
    }}
  }}
}}"#
        )
    };

    // An external caller's claim without independent world replay evaluates to Unknown (unresolved)
    let input_unverified = make_input(
        "ops::dispatch",
        "q1",
        &world.to_hex(),
        &program.to_hex(),
        1,
        &rev_digest.to_hex(),
        "ship",
        &val_digest.to_hex(),
        "service:atlas",
        "atlas-2026.10",
        "read:atlas",
        "tampered",
    );
    let stop = run_program_with_input_and_registry(source, &input_unverified, registry.clone())
        .expect("runs");
    assert!(
        matches!(stop, FiniteDecisionStop::Selected(ref d) if d.candidate == "unresolved"),
        "unreplayed runtime claim must evaluate to Unknown: {stop:?}"
    );
}

// ---------------------------------------------------------------------------
// 7. Preservation, rechecking on reload, and explanation fidelity
// ---------------------------------------------------------------------------

#[test]
fn test_validate_preservation_and_rechecking_on_reload() {
    use brix_lower::l3_v2::DocumentaryRegistry;

    let source = r#"
config EvidenceRef = { uri: Str, kind: Str, scope: Str }
config DocumentedStep = { digest: Str, status: Str, uri: Str }
config Decision = Ship | Hold

input ev: EvidenceRef

fn check_doc(r: EvidenceRef) -> Bool = match validate<DocumentedStep>(r) {
  Satisfied(_) => true
  _ => false
}

rule verified() = check_doc(ev)

propose ship priority 10 when verified = Ship
propose hold otherwise = Hold
commit d from (ship, hold)
show d
"#;

    let input_valid = r#"{
  "schema": "brix.input@5",
  "values": {
    "ev": {
      "type": "record",
      "nominal": "EvidenceRef",
      "fields": [
        {"name": "kind", "value": {"type": "string", "value": "policy-clause"}},
        {"name": "scope", "value": {"type": "string", "value": "shipping-release"}},
        {"name": "uri", "value": {"type": "string", "value": "doc://sec-9"}}
      ]
    }
  }
}"#;

    let module = parse(source).unwrap();
    let plan = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap();
    let shard = decode_input_shard(input_valid.as_bytes(), &InputLimits::default()).unwrap();
    let snapshot = canonicalize_input_shards(vec![shard], &InputLimits::default()).unwrap();
    let registry = build_fixture_documentary_registry();

    // First run in runtime 1 with verified evidence environment
    let runtime1 = FiniteDecisionRuntime::build_with_inputs(&plan, &snapshot)
        .unwrap()
        .with_documentary_registry(registry.clone());
    let run1 = runtime1.run();
    assert!(matches!(run1.stop, FiniteDecisionStop::Selected(ref d) if d.candidate == "ship"));
    runtime1
        .evaluate_shows(&run1)
        .expect("evaluate_shows succeeds under matching evidence environment");
    runtime1
        .explain_candidate("ship")
        .expect("explain_candidate threads verified evidence environment through helpers");

    // Reload in fresh runtime 2 with reconstructed & rechecked evidence environment
    // from the snapshot directly, without manually re-supplying the registry.
    let reloaded_snapshot = runtime1.snapshot().clone();
    let runtime2 = FiniteDecisionRuntime::build_with_inputs(&plan, &reloaded_snapshot).unwrap();
    let run2 = runtime2.run();
    assert_eq!(
        run1.stop, run2.stop,
        "reloaded runtime must produce identical verified outcome"
    );
    assert_eq!(run1.context, run2.context);

    // Independent audit bundle verification using the reloaded snapshot
    let bundle =
        brix_lower::audit_bundle::produce_finite_decision_audit_input_bundle_v1(&runtime1, &run1)
            .expect("produce audit bundle");
    let program_id = brix_lower::finite_decision_program_id(&plan);
    let audit_limits = soc_core::audit_bundle::AuditDecodeLimits::strict();
    let plan_limits = brix_lower::l3::PlanLimitsV1::generous();
    let report = brix_lower::audit_bundle::check_finite_decision_audit_input_bundle_from_module_with_inputs_v1(
        &module,
        program_id,
        &plan_limits,
        &bundle,
        &audit_limits,
        &reloaded_snapshot,
    )
    .expect("audit bundle independently verifies reloaded execution context");
    assert_eq!(report.context, run1.context);

    // Rechecking run1 against a runtime with a DIFFERENT (empty) evidence environment
    // must fail closed because the evidence environment digest is bound into ContextId!
    let runtime_without_evidence = FiniteDecisionRuntime::build_with_inputs(&plan, &snapshot)
        .unwrap()
        .with_documentary_registry(std::sync::Arc::new(DocumentaryRegistry::new()));
    assert_ne!(
        runtime1.context, runtime_without_evidence.context,
        "evidence snapshot identity must be bound into execution ContextId"
    );
    assert!(
        runtime_without_evidence.evaluate_shows(&run1).is_err(),
        "rechecking under different evidence environment must be refused"
    );
    let stop_without_evidence = runtime_without_evidence.run().stop;
    assert!(
        matches!(stop_without_evidence, FiniteDecisionStop::Selected(ref d) if d.candidate == "hold"),
        "reloaded runtime without verified evidence must refuse prior authorization"
    );
}
