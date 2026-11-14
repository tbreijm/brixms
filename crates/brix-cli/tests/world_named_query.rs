//! End-to-end coverage for host-scoped named world queries through the built CLI.

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("brix-world-named-query-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn brix_bin() -> PathBuf {
    std::env::var_os("CARGO_BIN_EXE_brix")
        .or_else(|| std::env::var_os("NEXTEST_BIN_EXE_brix"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_brix")))
}

// Keep the policy file beside the world, with only tenant changed between hosts.
fn policy(scratch: &Scratch, tenant: &str) -> PathBuf {
    let path = scratch.path().join(format!("host-{tenant}.json"));
    let value = json!({
        "schema": "brix.host-query-policy@1",
        "profile": {
            "profile_id": "brix.host-scope.partition@1",
            "partition_fields": {"main::catalogue": "tenant"},
            "global_relations": []
        },
        "namespace": "catalogue",
        "tenant": tenant,
        "principal": "test-host",
        "valid_time_start": 0,
        "valid_time_end": 100,
        "validity_start": 0,
        "validity_end": 100,
        "step_budget": 10000,
        "depth_budget": 20,
        "limits": {"bytes": 65536, "assumptions": 256, "depth": 128, "artifact_bytes": 65536},
        "query_limits": {
            "page_rows": 8,
            "max_input_rows": 1000,
            "max_input_bytes": 1000000,
            "max_eval_work": 10000,
            "max_eval_matches": 10000,
            "max_output_rows": 100
        }
    });
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    path
}

fn upsert(row: &str, id: &str, tenant: &str, kind: &str) -> Value {
    json!({
        "op": "upsert", "relation": "main::catalogue", "key": row,
        "tuple": {"row": row, "id": id, "tenant": tenant, "kind": kind}
    })
}

fn write_batch(path: &Path, base: u64, key: &str, operations: Vec<Value>) {
    fs::write(
        path,
        serde_json::to_vec(&json!({
            "schema": "brix.world.batch@1",
            "expected_base_revision": base,
            "idempotency_key": key,
            "operations": operations
        }))
        .unwrap(),
    )
    .unwrap();
}

fn assert_cli_ok(output: std::process::Output) -> Value {
    assert!(
        output.status.success(),
        "status: {}; stdout: {}; stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "invalid JSON stdout ({e}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn cli_query(
    world: &Path,
    policy: &Path,
    args: &str,
    rev: Option<u64>,
    limit: Option<usize>,
    cursor: Option<&str>,
) -> std::process::Output {
    let mut command = Command::new(brix_bin());
    command
        .args(["world", "query-named", "--json"])
        .arg(world)
        .arg("catalogue_for")
        .arg("--host-scope")
        .arg(policy)
        .arg("--args")
        .arg(args);
    if let Some(rev) = rev {
        command.args(["--rev", &rev.to_string()]);
    }
    if let Some(limit) = limit {
        command.args(["--limit", &limit.to_string()]);
    }
    if let Some(cursor) = cursor {
        command.args(["--cursor", cursor]);
    }
    command.output().unwrap()
}

fn result(output: std::process::Output) -> Value {
    let value: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "invalid JSON stdout ({e}): {}; stderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    });
    assert_eq!(value["command"], "world.query-named", "{value}");
    value
}

fn files(root: &Path, base: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            files(&entry.path(), base, out);
        } else {
            out.insert(
                entry.path().strip_prefix(base).unwrap().to_path_buf(),
                fs::read(entry.path()).unwrap(),
            );
        }
    }
}

#[test]
fn cli_named_query_scopes_typed_arguments_cursors_and_read_only_files() {
    let scratch = Scratch::new();
    let world = scratch.path().join("world");
    let program = scratch.path().join("main.brix");
    let batch1 = scratch.path().join("batch1.json");
    let batch2 = scratch.path().join("batch2.json");
    fs::write(
        &program,
        r#"
rel input catalogue: { row: Str, id: Str, tenant: Str, kind: Str } key row
query catalogue_for(kind: Str) = select { id: c.id, kind: c.kind }
  from c in catalogue where c.kind == kind
"#,
    )
    .unwrap();
    assert_cli_ok(
        Command::new(brix_bin())
            .args(["world", "init", "--json"])
            .arg(&world)
            .arg(&program)
            .output()
            .unwrap(),
    );
    write_batch(
        &batch1,
        0,
        "named-query-seed",
        vec![
            upsert("a-1", "a-book-1", "tenant-a", "book"),
            upsert("a-2", "a-book-2", "tenant-a", "book"),
            upsert("b-1", "b-book-1", "tenant-b", "book"),
            upsert("b-2", "b-game", "tenant-b", "game"),
        ],
    );
    write_batch(
        &batch2,
        1,
        "named-query-head-advance",
        vec![upsert("a-3", "a-book-3", "tenant-a", "book")],
    );
    assert_eq!(
        assert_cli_ok(
            Command::new(brix_bin())
                .args(["world", "batch", "--json"])
                .arg(&world)
                .arg(&batch1)
                .output()
                .unwrap(),
        )["revision"],
        1
    );
    let host_a = policy(&scratch, "tenant-a");
    let host_b = policy(&scratch, "tenant-b");

    // Every invocation opens the persisted program/world in a fresh CLI process.
    let page = result(cli_query(
        &world,
        &host_a,
        r#"{"kind":"book"}"#,
        Some(1),
        Some(1),
        None,
    ));
    assert_eq!(page["ok"], true, "{page}");
    assert_eq!(page["revision"], 1);
    assert_eq!(page["count"], 1);
    assert_eq!(page["rows"][0]["kind"], "book");
    assert_eq!(page["rows"][0]["id"], "a-book-1");
    assert_eq!(page["has_more"], true);
    let cursor = page["next_cursor"].as_str().unwrap().to_owned();

    let next = result(cli_query(
        &world,
        &host_a,
        r#"{"kind":"book"}"#,
        Some(1),
        Some(1),
        Some(&cursor),
    ));
    assert_eq!(next["ok"], true, "{next}");
    assert_eq!(next["revision"], 1);
    assert_eq!(next["rows"][0]["id"], "a-book-2");
    let tenant_a_all = result(cli_query(
        &world,
        &host_a,
        r#"{"kind":"book"}"#,
        Some(1),
        Some(8),
        None,
    ));
    assert_eq!(tenant_a_all["count"], 2);
    assert_eq!(
        tenant_a_all["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["a-book-1", "a-book-2"]
    );
    let tenant_b_all = result(cli_query(
        &world,
        &host_b,
        r#"{"kind":"book"}"#,
        Some(1),
        Some(8),
        None,
    ));
    assert_eq!(tenant_b_all["count"], 1);
    assert_eq!(tenant_b_all["rows"][0]["id"], "b-book-1");

    let applied = assert_cli_ok(
        Command::new(brix_bin())
            .args(["world", "batch", "--json"])
            .arg(&world)
            .arg(&batch2)
            .output()
            .unwrap(),
    );
    assert_eq!(applied["revision"], 2);
    let mut before_reads = BTreeMap::new();
    files(&world, &world, &mut before_reads);

    let tenant_leak = result(cli_query(
        &world,
        &host_b,
        r#"{"kind":"book"}"#,
        Some(1),
        Some(1),
        Some(&cursor),
    ));
    assert_eq!(tenant_leak["ok"], false, "{tenant_leak}");
    let wrong_revision = result(cli_query(
        &world,
        &host_a,
        r#"{"kind":"book"}"#,
        Some(2),
        Some(1),
        Some(&cursor),
    ));
    assert_eq!(wrong_revision["ok"], false, "{wrong_revision}");
    let wrong_arguments = result(cli_query(
        &world,
        &host_a,
        r#"{"kind":"game"}"#,
        Some(1),
        Some(1),
        Some(&cursor),
    ));
    assert_eq!(wrong_arguments["ok"], false, "{wrong_arguments}");

    let wrong_type = result(cli_query(
        &world,
        &host_a,
        r#"{"kind":42}"#,
        Some(1),
        None,
        None,
    ));
    assert_eq!(wrong_type["ok"], false, "{wrong_type}");
    let duplicate_keys = result(cli_query(
        &world,
        &host_a,
        r#"{"kind":"book","kind":"game"}"#,
        Some(1),
        None,
        None,
    ));
    assert_eq!(duplicate_keys["ok"], false, "{duplicate_keys}");
    assert!(duplicate_keys["error"]
        .as_str()
        .unwrap()
        .contains("duplicate"));

    let new_head = result(cli_query(
        &world,
        &host_a,
        r#"{"kind":"book"}"#,
        None,
        Some(8),
        None,
    ));
    assert_eq!(new_head["ok"], true, "{new_head}");
    assert_eq!(new_head["revision"], 2);
    assert_eq!(new_head["count"], 3);

    let mut after = BTreeMap::new();
    files(&world, &world, &mut after);
    assert_eq!(
        before_reads, after,
        "named query reads must not change world files"
    );
}

#[test]
fn stdio_named_query_requires_trusted_host_and_accepts_host_policy_channel() {
    let scratch = Scratch::new();
    let world = scratch.path().join("world");
    let program = scratch.path().join("main.brix");
    let batch = scratch.path().join("batch.json");
    fs::write(
        &program,
        r#"
rel input catalogue: { row: Str, id: Str, tenant: Str, kind: Str } key row
query catalogue_for(kind: Str) = select { id: c.id, kind: c.kind }
  from c in catalogue where c.kind == kind
"#,
    )
    .unwrap();
    assert_cli_ok(
        Command::new(brix_bin())
            .args(["world", "init", "--json"])
            .arg(&world)
            .arg(&program)
            .output()
            .unwrap(),
    );
    write_batch(
        &batch,
        0,
        "stdio-named-query-seed",
        vec![upsert("a-1", "a-book", "tenant-a", "book")],
    );
    assert_cli_ok(
        Command::new(brix_bin())
            .args(["world", "batch", "--json"])
            .arg(&world)
            .arg(&batch)
            .output()
            .unwrap(),
    );
    let host_a = policy(&scratch, "tenant-a");
    let policy_value: Value = serde_json::from_slice(&fs::read(host_a).unwrap()).unwrap();
    let params = json!({
        "dir": world,
        "name": "catalogue_for",
        "args": {"kind": "book"},
        "host_policy": policy_value,
        "rev": 1,
        "limit": 4
    });
    let request = json!({"id": 17, "method": "world.query-named", "params": params});

    let untrusted = serve_call(false, &request);
    assert_eq!(untrusted["ok"], false, "{untrusted}");
    assert_eq!(untrusted["error"]["code"], "host-channel-required");

    let trusted = serve_call(true, &request);
    assert_eq!(trusted["ok"], true, "{trusted}");
    assert_eq!(trusted["exit_code"], 0);
    assert_eq!(trusted["result"]["ok"], true, "{trusted}");
    assert_eq!(trusted["result"]["rows"][0]["id"], "a-book");
    assert_eq!(trusted["result"]["revision"], 1);

    // Preserve duplicate keys in the outer request line instead of roundtriping
    // through serde_json::Value, which cannot represent them.
    let duplicate_line = format!(
        r#"{{"id":18,"method":"world.query-named","params":{{"dir":{},"name":"catalogue_for","args":{{"kind":"book","kind":"game"}},"host_policy":{},"rev":1}}}}"#,
        serde_json::to_string(world.to_str().unwrap()).unwrap(),
        serde_json::to_string(&policy_value).unwrap()
    );
    let duplicate = serve_call_raw(true, &duplicate_line);
    assert_eq!(duplicate["ok"], false, "{duplicate}");
    assert_eq!(duplicate["error"]["code"], "malformed-request");
    assert!(duplicate["error"]["message"]
        .as_str()
        .unwrap()
        .contains("duplicate"));
}

fn serve_call(trusted_host: bool, request: &Value) -> Value {
    serve_call_raw(trusted_host, &serde_json::to_string(request).unwrap())
}

fn serve_call_raw(trusted_host: bool, request: &str) -> Value {
    let mut command = Command::new(brix_bin());
    command.args(["serve", "--stdio"]);
    if trusted_host {
        command.arg("--trusted-host");
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(child.stdin.as_mut().unwrap(), "{request}").unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "status: {}; stderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "invalid stdio JSON ({e}): {}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

#[cfg(unix)]
struct ReadOnlyWorld {
    saved: Vec<(PathBuf, u32, bool)>,
}

#[cfg(unix)]
impl ReadOnlyWorld {
    fn new(root: &Path) -> std::io::Result<Self> {
        use std::os::unix::fs::PermissionsExt;

        fn collect(path: &Path, saved: &mut Vec<(PathBuf, u32, bool)>) -> std::io::Result<()> {
            let metadata = fs::metadata(path)?;
            let is_dir = metadata.is_dir();
            saved.push((path.to_path_buf(), metadata.permissions().mode(), is_dir));
            if is_dir {
                for entry in fs::read_dir(path)? {
                    collect(&entry?.path(), saved)?;
                }
            }
            Ok(())
        }

        fn restore(saved: &[(PathBuf, u32, bool)]) {
            use std::os::unix::fs::PermissionsExt;
            // Restore parent directories first so every descendant remains reachable.
            for entry in saved.iter() {
                if entry.2 {
                    let _ = fs::set_permissions(&entry.0, fs::Permissions::from_mode(entry.1));
                }
            }
            for entry in saved.iter() {
                if !entry.2 {
                    let _ = fs::set_permissions(&entry.0, fs::Permissions::from_mode(entry.1));
                }
            }
        }

        let mut saved = Vec::new();
        collect(root, &mut saved)?;
        // Files first, then directories from deepest to the world root.
        for (path, _, is_dir) in &saved {
            if !*is_dir {
                if let Err(error) = fs::set_permissions(path, fs::Permissions::from_mode(0o444)) {
                    restore(&saved);
                    return Err(error);
                }
            }
        }
        for (path, _, is_dir) in saved.iter().rev() {
            if *is_dir {
                if let Err(error) = fs::set_permissions(path, fs::Permissions::from_mode(0o555)) {
                    restore(&saved);
                    return Err(error);
                }
            }
        }
        Ok(Self { saved })
    }
}

#[cfg(unix)]
impl Drop for ReadOnlyWorld {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        // Restore parent directories first, then file modes, before Scratch removes the tree.
        for entry in &self.saved {
            if entry.2 {
                let _ = fs::set_permissions(&entry.0, fs::Permissions::from_mode(entry.1));
            }
        }
        for entry in &self.saved {
            if !entry.2 {
                let _ = fs::set_permissions(&entry.0, fs::Permissions::from_mode(entry.1));
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn cli_named_query_succeeds_with_read_only_world_permissions() {
    let scratch = Scratch::new();
    let world = scratch.path().join("world");
    let program = scratch.path().join("main.brix");
    let batch = scratch.path().join("batch.json");
    fs::write(
        &program,
        r#"
rel input catalogue: { row: Str, id: Str, tenant: Str, kind: Str } key row
query catalogue_for(kind: Str) = select { id: c.id, kind: c.kind }
  from c in catalogue where c.kind == kind
"#,
    )
    .unwrap();
    assert_cli_ok(
        Command::new(brix_bin())
            .args(["world", "init", "--json"])
            .arg(&world)
            .arg(&program)
            .output()
            .unwrap(),
    );
    write_batch(
        &batch,
        0,
        "readonly-query-seed",
        vec![upsert("a-1", "a-book", "tenant-a", "book")],
    );
    assert_cli_ok(
        Command::new(brix_bin())
            .args(["world", "batch", "--json"])
            .arg(&world)
            .arg(&batch)
            .output()
            .unwrap(),
    );
    let host_a = policy(&scratch, "tenant-a");

    // Writer sessions leave this lock file behind. Remove it after writes so
    // the read-only query proves it does not create/acquire one itself.
    let lock = world.join(".lock");
    let _ = fs::remove_file(&lock);
    assert!(!lock.exists());
    let mut before = BTreeMap::new();
    files(&world, &world, &mut before);

    let _readonly = ReadOnlyWorld::new(&world).unwrap();
    let queried = result(cli_query(
        &world,
        &host_a,
        r#"{"kind":"book"}"#,
        Some(1),
        Some(8),
        None,
    ));
    assert_eq!(queried["ok"], true, "{queried}");
    assert_eq!(queried["revision"], 1);
    assert_eq!(queried["rows"][0]["id"], "a-book");

    let mut after = BTreeMap::new();
    files(&world, &world, &mut after);
    assert_eq!(before, after, "read-only query must not modify world files");
    assert!(
        !lock.exists(),
        "read-only query must not create a writer lock"
    );
}
