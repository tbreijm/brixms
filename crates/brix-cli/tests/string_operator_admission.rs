use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn scratch() -> PathBuf {
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("brix-string-admission-{}-{n}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn brix_bin() -> PathBuf {
    std::env::var_os("CARGO_BIN_EXE_brix")
        .or_else(|| std::env::var_os("NEXTEST_BIN_EXE_brix"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_brix")))
}

#[test]
fn check_rejects_unsupported_string_ops_before_runtime_or_world_initialization() {
    let dir = scratch();
    let cases = [
        (
            "finite.brix",
            r#"
config Result = Ok
input value: Str
fn hidden(a, b) = a + b
propose keep() priority 1 when hidden(value, "x") == "x" = Ok
commit result from (keep)
"#,
        ),
        (
            "finite-less.brix",
            r#"
config Result = Ok
input value: Str
propose keep() priority 1 when value < "m" = Ok
commit result from (keep)
"#,
        ),
        (
            "world-add.brix",
            r#"
rel input entries: { id: Str, label: Str } key id
rel derived result = select { value: hidden(e.label, "!") } from e in entries
fn hidden(a, b) = a + b
"#,
        ),
        (
            "world-less.brix",
            r#"
rel input entries: { id: Str, label: Str } key id
rel derived result = select { value: e.label < "m" } from e in entries
"#,
        ),
    ];
    for (name, source) in cases {
        let file = dir.join(name);
        fs::write(&file, source).unwrap();
        let output = Command::new(brix_bin())
            .args(["check", file.to_str().unwrap()])
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            !output.status.success(),
            "check accepted {name}: {stdout}{stderr}"
        );
        assert!(
            format!("{stdout}{stderr}").contains("unsupported for Str operands"),
            "unexpected check error for {name}: {stdout}{stderr}"
        );
    }
    fs::remove_dir_all(dir).unwrap();
}
