use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run_with_environment(environment: Value) -> Output {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = Scratch(std::env::temp_dir().join(format!(
        "brix-env-review-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )));
    fs::create_dir_all(&dir.0).unwrap();
    let source = dir.0.join("main.brix");
    fs::write(
        &source,
        r#"
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
"#,
    )
    .unwrap();
    let input = dir.0.join("input.json");
    fs::write(
        &input,
        json!({"schema":"brix.input@5","values":{"ev":{
        "type":"record","nominal":"EvidenceRef","fields":[
            {"name":"kind","value":{"type":"string","value":"policy-clause"}},
            {"name":"scope","value":{"type":"string","value":"shipping-release"}},
            {"name":"uri","value":{"type":"string","value":"doc://attacker"}}
        ]}}})
        .to_string(),
    )
    .unwrap();
    let env = dir.0.join("environment.json");
    fs::write(&env, environment.to_string()).unwrap();
    Command::new(env!("CARGO_BIN_EXE_brix"))
        .arg("run")
        .arg(source)
        .arg("--input")
        .arg(input)
        .arg("--input")
        .arg(env)
        .arg("--json")
        .output()
        .unwrap()
}

#[test]
fn review_cli_environment_requires_an_independent_trust_pin() {
    let output = run_with_environment(json!({"schema":"brix.evidence.env@1","documentary":{
        "entries":[{"evidence_id":"doc://attacker","content_digest":"00".repeat(32),
            "evidence_kind":"policy-clause","authority_scope":"shipping-release","disposition":"Satisfied"}]
    }}));
    assert_ne!(output.status.code(), Some(101), "CLI must not panic");
    let body: Value = serde_json::from_slice(&output.stdout).expect("structured CLI result");
    assert!(
        body["decision"]["candidate"] != "ship",
        "an untrusted --input file authorized its own evidence without an independent pin: {body}"
    );
}

#[test]
fn review_cli_environment_rejects_short_digest_without_panicking() {
    let output = run_with_environment(json!({"schema":"brix.evidence.env@1","documentary":{
        "expected_digest":"x", "entries":[]
    }}));
    assert_eq!(
        output.status.code(),
        Some(1),
        "malformed evidence must produce a structured refusal, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _: Value = serde_json::from_slice(&output.stdout).expect("structured refusal");
}

#[test]
fn review_cli_environment_cannot_supply_its_own_trust_pin() {
    use brix_canon::Digest;
    use brix_lower::l3_v2::{DocumentaryDisposition, DocumentaryEntry, DocumentarySnapshot};
    let snapshot = DocumentarySnapshot::new().with_entry(DocumentaryEntry {
        evidence_id: "doc://attacker".into(),
        content_digest: Digest::from_bytes([0; 32]),
        evidence_kind: "policy-clause".into(),
        authority_scope: "shipping-release".into(),
        disposition: DocumentaryDisposition::Satisfied,
    });
    let output = run_with_environment(json!({"schema":"brix.evidence.env@1","documentary":{
        "expected_digest": snapshot.digest().to_hex(),
        "entries":[{"evidence_id":"doc://attacker","content_digest":"00".repeat(32),
            "evidence_kind":"policy-clause","authority_scope":"shipping-release","disposition":"Satisfied"}]
    }}));
    assert_ne!(output.status.code(), Some(101), "CLI must not panic");
    let body: Value = serde_json::from_slice(&output.stdout).expect("structured CLI result");
    assert!(
        body["decision"]["candidate"] != "ship",
        "a snapshot and matching digest from the same untrusted input must not authorize: {body}"
    );
}

#[test]
fn runtime_certificate_replays_in_fresh_run_and_audit_processes() {
    use brix_kb::world::{TupleRecord, WorldBatch, WorldBatchOp, WorldKey, WorldSession};
    use std::collections::BTreeMap;
    let dir =
        Scratch(std::env::temp_dir().join(format!("brix-runtime-env-{}", std::process::id())));
    fs::create_dir_all(&dir.0).unwrap();
    let sources = BTreeMap::from([(
        "root".into(),
        r#"
config Plan = Done
config CertifiedAnswer = { path: Plan, caller: Str, release_id: Str, grants: List<Str> }
rel input question: { id: Str } key id
decide plan for q in question per id {
    propose answer priority 1 when true = CertifiedAnswer {
        path: Plan::Done, caller: "atlas", release_id: "catalogue-7", grants: ["read:atlas"]
    }
}
"#
        .into(),
    )]);
    let mut world =
        WorldSession::from_program_with_sources(dir.0.join("world"), "root", &sources).unwrap();
    let mut row = TupleRecord::new();
    row.set_str("id", "q1");
    world
        .apply_batch(WorldBatch::new(
            0,
            "seed",
            vec![WorldBatchOp::Upsert {
                relation: "root::question".into(),
                key: WorldKey::from_str("q1"),
                tuple: row.to_tuple(),
            }],
        ))
        .unwrap();
    let evidence = world
        .request_decision_evidence(1, "root::plan", "q1", vec!["read:atlas".into()], true)
        .unwrap();
    let target = &evidence.target;
    let pin = target.id().to_hex();
    let artifact: Value = serde_json::from_slice(&evidence.encode().unwrap()).unwrap();
    drop(world);
    // No live world can be consulted by any of the subprocesses below.
    fs::remove_dir_all(dir.0.join("world")).unwrap();
    fs::write(
        dir.0.join("consumer.brix"),
        r#"
config Claim = {
 decision_name: Str, entity: Str, world: Str, program: Str, revision: Int,
 revision_digest: Str, candidate: Str, result: Str, caller: Str, release: Str, grants: List<Str>
}
config RuntimeOutcome = { candidate: Str, decision_name: Str, status: Str }
config Decision = Ship | Hold
input claim: Claim
rule accepted() = match validate<RuntimeOutcome>(claim) {
 Satisfied(_) => true
 _ => false
}
propose ship priority 10 when accepted = Ship
propose hold otherwise = Hold
commit d from (ship, hold)
"#,
    )
    .unwrap();
    let text = |s: &str| json!({"type":"string", "value":s});
    let mut fields = vec![
        ("decision_name", text(&target.decide)),
        ("entity", text(&target.entity)),
        ("world", text(&target.world.to_hex())),
        ("program", text(&target.program.to_hex())),
        (
            "revision",
            json!({"type":"int", "value":target.revision.to_string()}),
        ),
        ("revision_digest", text(&target.revision_digest.to_hex())),
        ("candidate", text(&target.candidate)),
        ("result", text(&target.value_digest.to_hex())),
        ("caller", text(target.caller.as_ref().unwrap())),
        ("release", text(&target.release)),
        (
            "grants",
            json!({"type":"list","items":[text("read:atlas")]}),
        ),
    ];
    fields.sort_by_key(|(name, _)| *name);
    let fields: Vec<Value> = fields
        .into_iter()
        .map(|(name, value)| json!({"name":name,"value":value}))
        .collect();
    fs::write(
        dir.0.join("input.json"),
        json!({"schema":"brix.input@5","values":{
            "claim":{"type":"record","nominal":"Claim","fields":fields}
        }})
        .to_string(),
    )
    .unwrap();
    let write_env = |artifact: Value| {
        fs::write(
            dir.0.join("environment.json"),
            json!({"schema":"brix.evidence.env@1","runtime":[artifact]}).to_string(),
        )
        .unwrap()
    };
    write_env(artifact.clone());
    let invoke = |args: &[&str], pinned: bool| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_brix"));
        cmd.current_dir(&dir.0)
            .args(args)
            .args([
                "--json",
                "--input",
                "input.json",
                "--input",
                "environment.json",
            ])
            .env_remove("BRIX_EXPECT_EVIDENCE_DIGEST")
            .env_remove("BRIX_EXPECT_RUNTIME_CLAIMS");
        if pinned {
            cmd.env("BRIX_EXPECT_RUNTIME_CLAIMS", &pin);
        }
        let out = cmd.output().unwrap();
        assert_ne!(
            out.status.code(),
            Some(101),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let body: Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|_| {
            panic!(
                "stdout={} stderr={}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )
        });
        (out.status.success(), body)
    };
    let (ok, body) = invoke(&["run", "consumer.brix"], false);
    assert!(!ok, "unpinned evidence authorized itself: {body}");
    let (ok, body) = invoke(&["run", "consumer.brix"], true);
    assert!(ok, "{body}");
    assert_eq!(body["decision"]["candidate"], "ship", "{body}");
    let (ok, audit) = invoke(
        &["audit", "consumer.brix", "--bundle", "consumer.bundle"],
        true,
    );
    assert!(ok, "{audit}");
    let args = [
        "verify",
        "--expect-program",
        audit["program"].as_str().unwrap(),
        "consumer.brix",
        "consumer.bundle",
    ];
    let (ok, body) = invoke(&args, true);
    assert!(ok, "{body}");
    assert_eq!(body["status"], "audit-bundle-verified");
    let mut missing_replay = artifact.clone();
    missing_replay["bundle"] = Value::Null;
    write_env(missing_replay);
    let (ok, body) = invoke(&args, true);
    assert!(
        !ok,
        "audit trusted retained claim instead of replaying: {body}"
    );
    let mut changed_target = artifact;
    changed_target["target"]["entity"] = json!("q2");
    write_env(changed_target);
    let (ok, body) = invoke(&["run", "consumer.brix"], true);
    assert!(!ok, "changed target bypassed independent pin: {body}");
}
