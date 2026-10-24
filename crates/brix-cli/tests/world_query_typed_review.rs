//! Typed world projection must follow declared field types, not string contents.
use serde_json::{json, Value};
use std::{fs, path::Path, process::Command};

fn run(args: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_brix"))
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn typed_query_preserves_numeric_and_boolean_looking_strings() {
    let root = std::env::temp_dir().join(format!(
        "brix-typed-query-review-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let program = root.join("fixture.brix");
    fs::write(
        &program,
        "rel input rows: { id: Str, label: Str, qty: Int } key id\n",
    )
    .unwrap();
    let world = root.join("world");
    let p = |p: &Path| p.to_str().unwrap().to_owned();
    run(&["world", "init", &p(&world), &p(&program), "--json"]);
    let batch = root.join("batch.json");
    fs::write(
        &batch,
        json!({
            "schema":"brix.world.batch@1", "expected_base_revision":0, "idempotency_key":"seed",
            "operations":[{"op":"upsert", "relation":"fixture::rows", "key":"001",
                "tuple":{"id":"001", "label":"true", "qty":7}}]
        })
        .to_string(),
    )
    .unwrap();
    run(&["world", "batch", &p(&world), &p(&batch), "--json"]);
    let result = run(&[
        "world",
        "query",
        &p(&world),
        "fixture::rows",
        "--typed",
        "--json",
    ]);
    fs::remove_dir_all(&root).unwrap();
    let tuple = &result["entries"][0]["tuple"];
    assert_eq!(tuple["qty"], json!(7));
    assert_eq!(
        tuple["id"],
        json!("001"),
        "Str field must not be parsed as an integer"
    );
    assert_eq!(
        tuple["label"],
        json!("true"),
        "Str field must not be parsed as a boolean"
    );
}
