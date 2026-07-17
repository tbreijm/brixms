//! Acceptance test (issue #9): `brix build path/to/world.brix` produces a
//! Rust workspace that compiles; `brix run` executes it; a warm rebuild is
//! a cache hit. Drives the real `brix` binary as a subprocess — this is
//! the only way to exercise `main.rs`'s dispatch and the real `cargo`
//! invocation end to end.

use std::io::Write;
use std::process::{Command, Stdio};

use camino::Utf8PathBuf;

const FIXTURE: &str = "package smoke.build @ 0.1.0\n\
\n\
rel Input { value: I64 } key(value)\n\
rel Output { value: I64 } key(value)\n\
derive R: Output(value: value) from { Input(value) }\n";

const UNSUPPORTED_FIXTURE: &str = "package smoke.unsupported @ 0.1.0\n\
rel Input { value: I64 } key(value)\n\
rel Other { value: I64 } key(value)\n\
rel Output { value: I64 } key(value)\n\
derive Join: Output(value: value) from { Input(value); Other(value) }\n";

fn tmp_dir(tag: &str) -> Utf8PathBuf {
    let mut p =
        Utf8PathBuf::from_path_buf(std::env::temp_dir()).expect("system temp dir must be UTF-8");
    p.push(format!(
        "brix-cli-smoke-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    p
}

fn brix(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_brix"))
        .args(args)
        .output()
        .expect("brix binary must be spawnable")
}

#[test]
fn build_then_run_then_cache_hit() {
    let root = tmp_dir("build-run");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("world.brix");
    std::fs::write(&source_path, FIXTURE).unwrap();

    // 1. `brix build` on a bare file, no brix.toml alongside it — exercises
    //    the PackageDecl-synthesis path directly.
    let build_out = brix(&["build", source_path.as_str()]);
    assert!(
        build_out.status.success(),
        "brix build failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&build_out.stdout),
        String::from_utf8_lossy(&build_out.stderr),
    );

    let cache_root = root.join(".brix-cache");
    let hex_dir = std::fs::read_dir(&cache_root)
        .unwrap_or_else(|e| panic!("no .brix-cache dir at {cache_root}: {e}"))
        .next()
        .expect("at least one cache entry")
        .unwrap()
        .path();
    let hex_dir = Utf8PathBuf::from_path_buf(hex_dir).unwrap();
    for f in ["Cargo.toml", "src/generated.rs", "src/main.rs"] {
        assert!(hex_dir.join(f).exists(), "missing generated file: {f}");
    }

    // 2. `brix run` on the same path — builds (cache hit this time) and
    //    executes the harness binary, whose fixed marker line must appear.
    let run_out = brix(&["run", source_path.as_str()]);
    assert!(
        run_out.status.success(),
        "brix run failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&run_out.stdout),
        String::from_utf8_lossy(&run_out.stderr),
    );
    assert!(String::from_utf8_lossy(&run_out.stdout).contains("brix: generated workspace OK"));
    assert!(String::from_utf8_lossy(&run_out.stderr).contains("cache hit"));

    // 3. A second `brix build` is the concrete, assertable form of "a warm
    //    rebuild is a cache hit."
    let rebuild_out = brix(&["build", source_path.as_str()]);
    assert!(rebuild_out.status.success());
    assert!(String::from_utf8_lossy(&rebuild_out.stderr).contains("cache hit"));

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn diagnostic_formats_and_exit_codes_are_public_contract() {
    let root = tmp_dir("diagnostics");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("broken.brix");
    std::fs::write(&source_path, "package broken @ 0.1.0\nrel Input {").unwrap();

    let json = brix(&["build", source_path.as_str(), "--diagnostic-format", "json"]);
    assert_eq!(json.status.code(), Some(1));
    let json_output = String::from_utf8_lossy(&json.stdout);
    assert!(
        json_output.starts_with("{\"diagnostics\":"),
        "{json_output}"
    );
    assert!(json_output.contains("BRX-AST-"), "{json_output}");

    let sarif = brix(&["build", source_path.as_str(), "--diagnostic-format=sarif"]);
    assert_eq!(sarif.status.code(), Some(1));
    let sarif_output = String::from_utf8_lossy(&sarif.stdout);
    assert!(
        sarif_output.contains("\"version\":\"2.1.0\""),
        "{sarif_output}"
    );
    assert!(sarif_output.contains("BRX-AST-"), "{sarif_output}");

    let usage = brix(&["build"]);
    assert_eq!(usage.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&usage.stderr).contains("expected a source file"));

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn build_fails_closed_when_a_rule_has_no_runtime_lowering() {
    let root = tmp_dir("unsupported-runtime-rule");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("world.brix");
    std::fs::write(&source_path, UNSUPPORTED_FIXTURE).unwrap();

    let output = brix(&["build", source_path.as_str(), "--diagnostic-format", "json"]);
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("BRX5001"), "{stdout}");
    assert!(stdout.contains("Join"), "{stdout}");

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn generated_binary_consumes_a_transaction_stream_and_emits_canon_dump() {
    let root = tmp_dir("runtime-stream");
    std::fs::create_dir_all(&root).unwrap();
    let source_path = root.join("world.brix");
    std::fs::write(&source_path, FIXTURE).unwrap();

    let build_out = brix(&["build", source_path.as_str()]);
    assert!(build_out.status.success());
    let cache_entry = std::fs::read_dir(root.join(".brix-cache"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let binary = Utf8PathBuf::from_path_buf(cache_entry)
        .unwrap()
        .join("target")
        .join("debug")
        .join(format!("smoke_build{}", std::env::consts::EXE_SUFFIX));

    let mut child = Command::new(binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"+ Input 68656c6c6f\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("1 "), "{stdout}");
    assert!(stdout.contains("brix: generated workspace OK"), "{stdout}");

    std::fs::remove_dir_all(&root).ok();
}
