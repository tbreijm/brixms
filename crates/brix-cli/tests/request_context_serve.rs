//! Request authority is supplied by the host for each call, never inherited.
use brix_canon::Digest;
use brix_lower::l3_v2::{DocumentaryDisposition, DocumentaryEntry, DocumentarySnapshot};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn brix_bin() -> PathBuf {
    std::env::var_os("CARGO_BIN_EXE_brix")
        .or_else(|| std::env::var_os("NEXTEST_BIN_EXE_brix"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_brix")))
}

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

fn input() -> Value {
    json!({"source":json!({"schema":"brix.input@5","values":{"ev":{
        "type":"record","nominal":"EvidenceRef","fields":[
            {"name":"kind","value":{"type":"string","value":"policy-clause"}},
            {"name":"scope","value":{"type":"string","value":"shipping-release"}},
            {"name":"uri","value":{"type":"string","value":"doc://attacker"}}
        ]}}}).to_string()})
}

fn documentary(satisfied: bool) -> (String, Value) {
    let disposition = if satisfied {
        DocumentaryDisposition::Satisfied
    } else {
        DocumentaryDisposition::Violated("denied".into())
    };
    let snapshot = DocumentarySnapshot::new().with_entry(DocumentaryEntry {
        evidence_id: "doc://attacker".into(),
        content_digest: Digest::from_bytes([0; 32]),
        evidence_kind: "policy-clause".into(),
        authority_scope: "shipping-release".into(),
        disposition,
    });
    let pin = snapshot.digest().to_hex();
    let env = json!({"schema":"brix.evidence.env@1","documentary":{
        "expected_digest":pin,"entries":[{
            "evidence_id":"doc://attacker","content_digest":"00".repeat(32),
            "evidence_kind":"policy-clause","authority_scope":"shipping-release",
            "disposition":if satisfied {"Satisfied"} else {"Violated"},"reason":"denied"
        }]
    }});
    (pin, json!({"source":env.to_string()}))
}

fn policy(program: &str, doc: &str) -> Value {
    json!({"schema":"brix.request-evidence-policy@1","expected_program":program,
    "runtime_claim_ids":[],"documentary_digest":doc,
    "verifier":"brix.world.decision-evidence-replay@1","profile":"brix.l3.finite-decision@1",
    "namespace":"shipping","tenant":"atlas","principal":"host",
    "limits":{
        "max_input_files":16,"max_input_file_bytes":1_048_576,"max_input_aggregate_bytes":4_194_304,
        "max_runtime_claims":64,"max_evidence_artifact_bytes":16_777_216,
        "max_evidence_aggregate_bytes":67_108_864,"max_evidence_bundle_bytes":8_388_608,
        "max_evidence_proof_nodes":512,"max_evidence_node_bytes":65_536,
        "max_replay_work":500_000,"context_bytes":65_536,
        "context_artifact_bytes":65_536,"context_depth":128
    }})
}

fn exchange(input: &mut impl Write, output: &mut impl BufRead, request: Value) -> Value {
    writeln!(input, "{request}").unwrap();
    input.flush().unwrap();
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

#[test]
fn host_requests_isolate_contradictory_evidence_and_default_server_ignores_environment() {
    let (allow_pin, allow_env) = documentary(true);
    let (deny_pin, deny_env) = documentary(false);
    println!("documentary fixture: satisfied={allow_pin} violated={deny_pin}");
    for trusted in [false, true] {
        let mut command = Command::new(brix_bin());
        command.args(["serve", "--stdio"]);
        if trusted {
            command.arg("--trusted-host");
        }
        // Even valid ambient authority must not select authority for a server request.
        command
            .env("BRIX_EXPECT_EVIDENCE_DIGEST", &allow_pin)
            .env("BRIX_EXPECT_RUNTIME_CLAIMS", "malformed-ambient-pin")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut process = command.spawn().unwrap();
        let mut input_pipe = process.stdin.take().unwrap();
        let mut output = BufReader::new(process.stdout.take().unwrap());
        let checked = exchange(
            &mut input_pipe,
            &mut output,
            json!({"id":0,"method":"check","params":{"program":{"source":PROGRAM}}}),
        );
        assert_eq!(checked["result"]["ok"], true, "{checked}");
        let program = checked["result"]["program"].as_str().unwrap();
        let no_context = exchange(
            &mut input_pipe,
            &mut output,
            json!({"id":1,"method":"run","params":{
            "program":{"source":PROGRAM},"inputs":[input(),allow_env.clone()]}}),
        );
        assert_eq!(
            no_context["result"]["ok"], false,
            "ambient trust reached server: {no_context}"
        );
        let unchecked = exchange(
            &mut input_pipe,
            &mut output,
            json!({"id":6,"method":"check","params":{
            "program":{"source":PROGRAM},"inputs":[input(),allow_env.clone()]}}),
        );
        assert_eq!(
            unchecked["result"]["ok"], false,
            "check inherited ambient authority: {unchecked}"
        );
        let allow = policy(program, &allow_pin);
        let deny = policy(program, &deny_pin);
        let mut identities = Vec::new();
        for (id, context, environment, success, candidate) in [
            (2, &allow, &allow_env, true, "ship"),
            (3, &deny, &deny_env, true, "hold"),
            (4, &deny, &allow_env, false, ""),
            (5, &allow, &allow_env, true, "ship"),
        ] {
            let response = exchange(
                &mut input_pipe,
                &mut output,
                json!({"id":id,"method":"run","params":{
                "program":{"source":PROGRAM},"inputs":[input(),environment.clone()],"request_context":context}}),
            );
            if !trusted {
                assert_eq!(
                    response["error"]["code"], "host-channel-required",
                    "{response}"
                );
                continue;
            }
            assert_eq!(response["ok"], true, "{response}");
            assert_eq!(response["result"]["ok"], success, "{response}");
            if success {
                assert_eq!(
                    response["result"]["decision"]["candidate"], candidate,
                    "{response}"
                );
                identities.push(response["result"]["request_context"].clone());
            }
        }
        if trusted {
            assert_ne!(identities[0], identities[1]);
            assert_eq!(identities[0], identities[2]);
        }
        drop(input_pipe);
        assert!(process.wait().unwrap().success());
    }
}

#[test]
fn standalone_check_and_test_keep_explicit_legacy_invocation_pins() {
    struct Scratch(std::path::PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let scratch = Scratch(
        std::env::temp_dir().join(format!("brix-request-legacy-check-{}", std::process::id())),
    );
    std::fs::create_dir(&scratch.0).unwrap();
    std::fs::write(scratch.0.join("main.brix"), PROGRAM).unwrap();
    let input_source = input();
    std::fs::write(
        scratch.0.join("input.json"),
        input_source["source"].as_str().unwrap(),
    )
    .unwrap();
    let (pin, env) = documentary(true);
    std::fs::write(scratch.0.join("env.json"), env["source"].as_str().unwrap()).unwrap();
    std::fs::write(scratch.0.join("suite.json"),json!({"schema":"brix.test@1","program":"main.brix",
        "cases":[{"name":"legacy-pinned","inputs":["input.json","env.json"],"expect":{"status":"selected","decision":"ship"}}]}).to_string()).unwrap();
    for args in [
        vec![
            "check",
            "main.brix",
            "--input",
            "input.json",
            "--input",
            "env.json",
            "--json",
        ],
        vec!["test", "suite.json", "--json"],
    ] {
        let output = Command::new(brix_bin())
            .args(args)
            .current_dir(&scratch.0)
            .env("BRIX_EXPECT_EVIDENCE_DIGEST", &pin)
            .env_remove("BRIX_EXPECT_RUNTIME_CLAIMS")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["ok"], true, "{result}");
    }
}
