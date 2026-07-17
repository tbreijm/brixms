//! Acceptance test (issue #9): `brix build path/to/world.brix` produces a
//! Rust workspace that compiles; `brix run` executes it; a warm rebuild is
//! a cache hit. Drives the real `brix` binary as a subprocess — this is
//! the only way to exercise `main.rs`'s dispatch and the real `cargo`
//! invocation end to end.

use std::process::Command;

use camino::Utf8PathBuf;

const FIXTURE: &str = "package smoke.build @ 0.1.0\n\
\n\
rel Input { value: I64 } key(value)\n\
rel Output { value: I64 } key(value)\n\
derive R: Output(value: value) from { Input(value) }\n";

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
