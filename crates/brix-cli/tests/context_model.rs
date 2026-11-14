use brix_canon::{Digest, Domain};
use brix_semantic::*;
use serde_json::Value;
use soc_regimes::{context_validation::typed_assumption, Ty};
use std::{path::PathBuf, process::Command};
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
        kernel_profile: brix_kernel::KERNEL_PROFILE_V1.into(),
        regime_profile: "brix.type@1".into(),
        axiom_profile: "none".into(),
        validity_start: 1,
        validity_end: 9,
        step_budget: 100,
        depth_budget: 10,
    }
}

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("brix-context-cli-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn call(args: &[&str]) -> (bool, Value) {
    // Nextest archives are extracted on another runner: the build-time path
    // is not valid there. Prefer the runner's relocated executable path.
    let binary = std::env::var_os("CARGO_BIN_EXE_brix")
        .or_else(|| std::env::var_os("NEXTEST_BIN_EXE_brix"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_brix")));
    let o = Command::new(binary).args(args).output().unwrap();
    (
        o.status.success(),
        serde_json::from_slice(&o.stdout)
            .unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&o.stderr))),
    )
}
#[test]
fn cli_context_admission_uses_independent_pin_and_bounded_artifact_store() {
    let scratch = Scratch::new();
    let path = scratch.0.join("input.context");
    let p = path.to_str().unwrap();
    let artifact = b"canonical-program-preimage";
    let mut dims = dimensions(None);
    dims.program = Some(ArtifactRefV2 {
        kind: "program".into(),
        digest: Digest::of(Domain::Value, artifact),
    });
    let model = ContextModelV2::new(dims, vec![typed_assumption("x", &Ty::Con("Int"))]).unwrap();
    let bytes = encode_context_model_v2(&model);
    std::fs::write(&path, &bytes).unwrap();
    let pin = model.id().to_hex();
    let store = scratch.0.to_str().unwrap();
    let (ok, inspection) = call(&["context", p, "--json"]);
    assert!(ok);
    assert_eq!(inspection["status"], "decoded-only");
    assert_eq!(inspection["context"], pin);
    assert_eq!(inspection["dimensions"]["tenant"], "t");
    assert_eq!(inspection["authority"], "none");
    let (ok, missing) = call(&["context", p, "--expect", &pin, "--store", store, "--json"]);
    assert!(!ok);
    assert!(missing["diagnostics"]
        .to_string()
        .contains("MissingArtifact"));
    let artifact_path = scratch.0.join(format!(
        "{}.artifact",
        model.dimensions().program.as_ref().unwrap().digest.to_hex()
    ));
    std::fs::write(&artifact_path, artifact).unwrap();
    let (ok, validated) = call(&["context", p, "--expect", &pin, "--store", store, "--json"]);
    assert!(ok, "{validated}");
    assert_eq!(validated["status"], "validated");
    assert_eq!(validated["authority"], "none");
    let wrong = ContextId::root().to_hex();
    assert!(!call(&["context", p, "--expect", &wrong, "--store", store, "--json"]).0);
    assert!(
        !call(&[
            "context",
            p,
            "--expect",
            &pin,
            "--store",
            store,
            "--simulation",
            "--json"
        ])
        .0
    );
    assert!(
        !call(&[
            "context",
            p,
            "--expect",
            &pin,
            "--store",
            store,
            "--profile",
            "kernel",
            "--json"
        ])
        .0
    );
    std::fs::write(&artifact_path, b"forged").unwrap();
    let (ok, forged) = call(&["context", p, "--expect", &pin, "--store", store, "--json"]);
    assert!(!ok);
    assert!(forged["diagnostics"]
        .to_string()
        .contains("ArtifactMismatch"));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    std::fs::write(&path, vec![0; 65_537]).unwrap();
    assert!(!call(&["context", p, "--json"]).0);
}
