//! A world another writer holds is `world-locked` (retryable), not `unknown-error`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

/// The `brix` binary, resolved at run time first: CI runs tests from a
/// nextest archive extracted elsewhere, where the compile-time path is gone.
fn brix() -> Command {
    let bin = std::env::var_os("CARGO_BIN_EXE_brix")
        .or_else(|| std::env::var_os("NEXTEST_BIN_EXE_brix"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_brix")));
    Command::new(bin)
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

#[test]
fn a_locked_world_reports_world_locked() {
    let scratch = std::env::temp_dir().join(format!("brix-cli-locked-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    fs::create_dir_all(&scratch).unwrap();
    let world = scratch.join("world");
    let init = brix()
        .args(["world", "init", "--json"])
        .arg(&world)
        .arg(repo_root().join("examples/world-linked/main.brix"))
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );

    let batch = scratch.join("batch.json");
    fs::write(
        &batch,
        serde_json::to_vec(&json!({
            "schema": "brix.world.batch@1",
            "expected_base_revision": 0,
            "idempotency_key": "locked-1",
            "operations": [
                {"op":"upsert", "relation":"policy::orders", "key":"order-1",
                 "tuple":{"id":"order-1", "customer":"Ada", "sku":"sku-1", "qty":2, "express":"yes"}}
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    let guard = brix_kb::world::session::WorldLockGuard::acquire(&world).unwrap();
    let out = brix()
        .args(["world", "batch", "--json"])
        .arg(&world)
        .arg(&batch)
        .output()
        .unwrap();
    drop(guard);
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(v["status"], "world-locked", "{v}");
    assert!(!out.status.success());

    // Released: the same command succeeds.
    let out = brix()
        .args(["world", "batch", "--json"])
        .arg(&world)
        .arg(&batch)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let _ = fs::remove_dir_all(&scratch);
}
