//! Frozen identity vectors for the encodings added after the published
//! README pins: bounded lists and relations with `brix.input@3` snapshots
//! (ADR-0037, ADR-0040), multiple commit pools (ADR-0039), and per-entity
//! decide blocks (ADR-0043). A change to any of these ids means a canonical
//! encoding changed; that needs an ADR, never an update to make this pass.

use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .to_path_buf()
}

fn run_json(example: &str) -> serde_json::Value {
    let root = repo_root();
    let out = Command::new(env!("CARGO_BIN_EXE_brix"))
        .current_dir(&root)
        .arg("run")
        .arg(format!("examples/{example}.brix"))
        .arg("--input")
        .arg(format!("examples/{example}.json"))
        .arg("--json")
        .output()
        .expect("spawn brix");
    serde_json::from_slice(&out.stdout).expect("brix.cli.result@1 JSON")
}

fn assert_ids(example: &str, program: &str, context: &str, snapshot: &str) {
    let r = run_json(example);
    assert_eq!(r["program"], program, "{example}: program id");
    assert_eq!(r["context"], context, "{example}: context id");
    assert_eq!(
        r["input_snapshot"], snapshot,
        "{example}: input snapshot id"
    );
}

#[test]
fn fulfillment_lists_and_relations() {
    assert_ids(
        "fulfillment",
        "1d762c4a411db951a36cd0bd06b9409d33a1701be5afd78b763a1d85cc83ca97",
        "1257a7307228141463f9645e70e2343ac13fab8e42c2341e0286c628539febf2",
        "dbe14ee5a3e707e8ec29282e60ecaa0c30237d74dc3e9b84a0f958ef80078c6a",
    );
}

#[test]
fn order_desk_multiple_commit_pools() {
    assert_ids(
        "order-desk",
        "4dd13924c243394dc0ea01ba6b884248bb1053c996b6543ffbe19aea7bb56620",
        "42a1893e104cb6610f1f0c812a649bf1f8183b3d330393f8e01b6abe46101fb6",
        "144bfd6848b3a1369564b5d83b7cb6a82491c767cdc3e168ab6df87185425f75",
    );
}

#[test]
fn order_book_per_entity_decisions() {
    assert_ids(
        "order-book",
        "9c9829b83fcafa1d531e258ea756ad59d300a74247cf96a0a2e8f4c98dc9c768",
        "6951f94928b40155e2bd556058e464a2a96ce09304d773e285d317e49990d9dd",
        "c01a0abe53bb511695174065b2ff8df8f2a8a0f629c394bc2f8e7c248d1459b4",
    );
}
