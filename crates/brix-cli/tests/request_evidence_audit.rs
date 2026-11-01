//! End-to-end portable request-context audit/replay coverage.

use brix_canon::Digest;
use brix_lower::finite_decision::{
    finite_decision_program_id, lower_finite_decision_plan, FINITE_DECISION_PROFILE,
};
use brix_lower::l3_v2::{DocumentaryDisposition, DocumentaryEntry, DocumentarySnapshot};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const PROGRAM: &str = r#"
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

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "brix-request-audit-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn program_id() -> String {
    let module = brix_syntax::parse(PROGRAM).unwrap();
    let plan = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap();
    finite_decision_program_id(&plan).digest().to_hex()
}

fn evidence() -> (String, String) {
    let snapshot = DocumentarySnapshot::new().with_entry(DocumentaryEntry {
        evidence_id: "doc://audit".into(),
        content_digest: Digest::from_bytes([0; 32]),
        evidence_kind: "policy-clause".into(),
        authority_scope: "shipping-release".into(),
        disposition: DocumentaryDisposition::Satisfied,
    });
    let digest = snapshot.digest().to_hex();
    let contents = json!({
        "schema": "brix.evidence.env@1",
        "documentary": {
            "expected_digest": digest,
            "entries": [{
                "evidence_id": "doc://audit",
                "content_digest": "00".repeat(32),
                "evidence_kind": "policy-clause",
                "authority_scope": "shipping-release",
                "disposition": "Satisfied"
            }]
        }
    })
    .to_string();
    (digest, contents)
}

fn shard() -> String {
    json!({
        "schema": "brix.input@5",
        "values": {"ev": {
            "type": "record",
            "nominal": "EvidenceRef",
            "fields": [
                {"name": "kind", "value": {"type": "string", "value": "policy-clause"}},
                {"name": "scope", "value": {"type": "string", "value": "shipping-release"}},
                {"name": "uri", "value": {"type": "string", "value": "doc://audit"}}
            ]
        }}
    })
    .to_string()
}

fn policy(program: &str, digest: &str, tenant: &str) -> Value {
    json!({
        "schema": "brix.request-evidence-policy@1",
        "expected_program": program,
        "runtime_claim_ids": [],
        "documentary_digest": digest,
        "verifier": "brix.world.decision-evidence-replay@1",
        "profile": "brix.l3.finite-decision@1",
        "namespace": "shipping",
        "tenant": tenant,
        "principal": "host",
        "limits": {
            "max_input_files": 16,
            "max_input_file_bytes": 1_048_576,
            "max_input_aggregate_bytes": 4_194_304,
            "max_runtime_claims": 64,
            "max_evidence_artifact_bytes": 16_777_216,
            "max_evidence_aggregate_bytes": 67_108_864,
            "max_evidence_bundle_bytes": 8_388_608,
            "max_evidence_proof_nodes": 512,
            "max_evidence_node_bytes": 65_536,
            "max_replay_work": 500_000,
            "context_bytes": 65_536,
            "context_artifact_bytes": 65_536,
            "context_depth": 128
        }
    })
}

fn run_cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_brix"))
        .args(args)
        .env_remove("BRIX_EXPECT_EVIDENCE_DIGEST")
        .env_remove("BRIX_EXPECT_RUNTIME_CLAIMS")
        .output()
        .unwrap()
}

fn write_policy(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

#[test]
fn host_audit_wrapper_replays_after_inputs_are_removed_and_rejects_wrong_or_forged_context() {
    let dir = TestDir::new();
    let source = dir.file("policy.brix");
    let inputs = dir.file("input.json");
    let evidence_path = dir.file("evidence.json");
    let bundle = dir.file("audit.bundle.json");
    let policy_path = dir.file("request-policy.json");
    let wrong_policy_path = dir.file("wrong-request-policy.json");
    fs::write(&source, PROGRAM).unwrap();
    fs::write(&inputs, shard()).unwrap();
    let (digest, evidence_contents) = evidence();
    fs::write(&evidence_path, evidence_contents).unwrap();
    let program = program_id();
    let correct_policy = policy(&program, &digest, "atlas");
    let wrong_policy = policy(&program, &digest, "other-tenant");
    write_policy(&policy_path, &correct_policy);
    write_policy(&wrong_policy_path, &wrong_policy);

    let audit = run_cli(&[
        "--request-context",
        policy_path.to_str().unwrap(),
        "audit",
        source.to_str().unwrap(),
        "--bundle",
        bundle.to_str().unwrap(),
        "--input",
        inputs.to_str().unwrap(),
        "--input",
        evidence_path.to_str().unwrap(),
        "--json",
    ]);
    assert!(
        audit.status.success(),
        "audit failed: {}",
        String::from_utf8_lossy(&audit.stdout)
    );
    let wrapper_bytes = fs::read(&bundle).unwrap();
    let wrapper: Value = serde_json::from_slice(&wrapper_bytes).unwrap();
    assert_eq!(wrapper["schema"], "brix.audit-request-evidence@1");
    assert_eq!(wrapper["inputs"].as_array().unwrap().len(), 2);

    // The portable wrapper is self-contained: verification does not reopen
    // either original input/evidence path.
    fs::remove_file(&inputs).unwrap();
    fs::remove_file(&evidence_path).unwrap();
    let verify = run_cli(&[
        "--request-context",
        policy_path.to_str().unwrap(),
        "verify",
        "--expect-program",
        &program,
        source.to_str().unwrap(),
        bundle.to_str().unwrap(),
        "--json",
    ]);
    assert!(
        verify.status.success(),
        "portable replay failed: {}",
        String::from_utf8_lossy(&verify.stdout)
    );
    let result: Value = serde_json::from_slice(&verify.stdout).unwrap();
    assert_eq!(result["status"], "audit-bundle-verified");
    assert!(result["request_context"].is_string());

    let wrong = run_cli(&[
        "--request-context",
        wrong_policy_path.to_str().unwrap(),
        "verify",
        "--expect-program",
        &program,
        source.to_str().unwrap(),
        bundle.to_str().unwrap(),
        "--json",
    ]);
    assert!(!wrong.status.success());
    let wrong_result: Value = serde_json::from_slice(&wrong.stdout).unwrap();
    assert!(wrong_result["diagnostics"][0]
        .as_str()
        .unwrap()
        .contains("request context does not match"));

    let forged = dir.file("forged.bundle.json");
    let mut changed = wrapper_bytes;
    let position = changed.iter().position(|byte| *byte == b'0').unwrap();
    changed[position] = b'1';
    fs::write(&forged, changed).unwrap();
    let forged_result = run_cli(&[
        "--request-context",
        policy_path.to_str().unwrap(),
        "verify",
        "--expect-program",
        &program,
        source.to_str().unwrap(),
        forged.to_str().unwrap(),
        "--json",
    ]);
    assert!(!forged_result.status.success());
}

#[test]
fn host_verify_rejects_unbound_v1_and_oversized_wrapper_before_replay() {
    let dir = TestDir::new();
    let source = dir.file("simple.brix");
    let raw_bundle = dir.file("raw-v1.bundle");
    fs::write(
        &source,
        "config Decision = Yes | No\npropose yes otherwise = Yes\ncommit d from (yes)\n",
    )
    .unwrap();
    let program = {
        let module = brix_syntax::parse(&fs::read_to_string(&source).unwrap()).unwrap();
        let plan = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap();
        finite_decision_program_id(&plan).digest().to_hex()
    };
    let env = run_cli(&[
        "audit",
        source.to_str().unwrap(),
        "--bundle",
        raw_bundle.to_str().unwrap(),
        "--json",
    ]);
    assert!(
        env.status.success(),
        "evidence-free baseline audit failed: {}",
        String::from_utf8_lossy(&env.stdout)
    );
    let host_policy = policy(&program, &"00".repeat(32), "atlas");
    let host_policy_path = dir.file("host-policy.json");
    write_policy(&host_policy_path, &host_policy);
    let rejected = run_cli(&[
        "--request-context",
        host_policy_path.to_str().unwrap(),
        "verify",
        "--expect-program",
        &program,
        source.to_str().unwrap(),
        raw_bundle.to_str().unwrap(),
        "--json",
    ]);
    assert!(!rejected.status.success());

    let oversized = dir.file("oversized.bundle");
    let file = fs::File::create(&oversized).unwrap();
    // The bounded wrapper ceiling is below 300 MiB; `set_len` creates a sparse
    // fixture without writing that many bytes to disk.
    file.set_len(300 * 1024 * 1024).unwrap();
    let capped = run_cli(&[
        "verify",
        "--expect-program",
        &program,
        source.to_str().unwrap(),
        oversized.to_str().unwrap(),
        "--json",
    ]);
    assert!(!capped.status.success());
    assert!(String::from_utf8_lossy(&capped.stdout).contains("exceeds maximum allowed size"));
}
