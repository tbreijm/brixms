//! Versioned collection semantics across typed CLI output and portable audits.
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "brix-collect-profile-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn bin() -> Command {
    let binary = std::env::var_os("CARGO_BIN_EXE_brix")
        .or_else(|| std::env::var_os("NEXTEST_BIN_EXE_brix"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_brix")));
    Command::new(binary)
}
fn success(output: Output) -> Value {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/legacy_collect_profile")
}
fn record_field<'a>(record: &'a Value, name: &str) -> &'a Value {
    &record["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["name"] == name)
        .unwrap()["value"]
}
fn items(list: &Value) -> Vec<&str> {
    list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["value"].as_str().unwrap())
        .collect()
}

#[test]
fn independently_pinned_legacy_collect_audit_keeps_its_original_tie_order() {
    // Captured once with the reviewed pre-A9-6 binary, evaluator @1. The
    // winning list is deliberately different from UTF-8 primary-key order.
    let pins: Value =
        serde_json::from_slice(&fs::read(fixture().join("pins.json")).unwrap()).unwrap();
    assert_ne!(pins["legacy_order"], json!(["aa", "b", "ccc", "z"]));
    let frozen_bundle = brix_kb::world::audit::decode_world_audit_bundle_v1(
        &fs::read(fixture().join("legacy_collect_profile.brixworldaudit")).unwrap(),
        &brix_kb::world::audit::WorldAuditDecodeLimits::strict(),
    )
    .unwrap();
    assert_eq!(frozen_bundle.id().to_hex(), pins["bundle_id"]);
    assert_eq!(
        frozen_bundle.exec_profile.crate_version,
        pins["exec_crate_version"].as_str().unwrap(),
        "the frozen alpha.8 profile version is part of this historical fixture"
    );
    assert_eq!(
        frozen_bundle.exec_profile.crate_version, "0.1.0-alpha.8",
        "do not relabel the frozen historical audit when the workspace version changes"
    );
    let result = success(
        bin()
            .args([
                "world",
                "verify",
                "--json",
                "--expect-head",
                pins["expect_head"].as_str().unwrap(),
                "--expect-program",
                pins["expect_program"].as_str().unwrap(),
            ])
            .arg(fixture().join("legacy_collect_profile.brixworldaudit"))
            .output()
            .unwrap(),
    );
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["head_digest"], pins["expect_head"]);
}

#[test]
fn new_collect_profile_has_typed_prefix_output_and_portable_replay() {
    let scratch = Scratch::new();
    let world = scratch.0.join("world");
    let source = scratch.0.join("model.brix");
    fs::write(&source,r#"
rel input record: { id: Str, kind: Str, ordinal: Int } key id
rel derived groups = select { kind:r.kind, ids:collect(r.id,r.ordinal,100), page:collect(r.id,r.ordinal).take(2) } from r in record group by r.kind
decide full for g in groups per kind { propose answer priority 1 when true = g.ids }
decide bounded for g in groups per kind { propose answer priority 1 when true = g.page }
query pages(kind: Str) = select { page: g.page } from g in groups where g.kind == kind
"#).unwrap();
    let init = success(
        bin()
            .args(["world", "init", "--json"])
            .arg(&world)
            .arg(&source)
            .output()
            .unwrap(),
    );
    let program = init["program_digest"].as_str().unwrap();
    let profile: Value =
        serde_json::from_slice(&fs::read(world.join("program.json")).unwrap()).unwrap();
    assert_eq!(profile["exec_profile"]["evaluator"], "brix-world-net@2");
    assert_eq!(
        profile["exec_profile"]["crate_version"],
        env!("CARGO_PKG_VERSION"),
        "new worlds must record the current package version"
    );
    let batch = scratch.0.join("batch.json");
    fs::write(&batch,json!({"schema":"brix.world.batch@1","expected_base_revision":0,"idempotency_key":"load","operations":(["aa","b","ccc","z"].iter().map(|id|json!({"op":"upsert","relation":"model::record","key":id,"tuple":{"id":id,"kind":"k","ordinal":0}})).collect::<Vec<_>>())} ).to_string()).unwrap();
    let receipt = success(
        bin()
            .args(["world", "batch", "--json"])
            .arg(&world)
            .arg(&batch)
            .output()
            .unwrap(),
    );
    let head = receipt["digest"].as_str().unwrap();
    let result = success(
        bin()
            .args(["world", "query", "--typed", "--json"])
            .arg(&world)
            .arg("model::groups")
            .output()
            .unwrap(),
    );
    let row = &result["entries"][0]["tuple"];
    assert_eq!(items(&row["ids"]), vec!["aa", "b", "ccc", "z"]);
    assert_eq!(items(record_field(&row["page"], "items")), vec!["aa", "b"]);
    assert_eq!(record_field(&row["page"], "truncated")["value"], true);
    success(
        bin()
            .args(["world", "open-check", "--verify-store", "--json"])
            .arg(&world)
            .output()
            .unwrap(),
    );
    let bundle = scratch.0.join("audit.bundle");
    success(
        bin()
            .args(["world", "audit", "--json"])
            .arg(&world)
            .arg("--bundle")
            .arg(&bundle)
            .output()
            .unwrap(),
    );
    let audit = brix_kb::world::audit::decode_world_audit_bundle_v1(
        &fs::read(&bundle).unwrap(),
        &brix_kb::world::audit::WorldAuditDecodeLimits::strict(),
    )
    .unwrap();
    assert_eq!(audit.exec_profile.evaluator, "brix-world-net@2");
    assert_eq!(audit.exec_profile.crate_version, env!("CARGO_PKG_VERSION"));
    success(
        bin()
            .args([
                "world",
                "verify",
                "--json",
                "--expect-head",
                head,
                "--expect-program",
                program,
            ])
            .arg(&bundle)
            .output()
            .unwrap(),
    );
    // Certificates must carry the actual producing profile, too.
    let mut session = brix_kb::world::WorldSession::open(&world).unwrap();
    let certificate = session
        .request_decision_evidence(1, "model::bounded", "k", vec![], true)
        .unwrap();
    brix_kb::world::decision_evidence::verify_decision_evidence(&certificate, &certificate.target)
        .unwrap();
}
