//! Issue #42 Slice 1: a locked multi-package graph builds and runs through the
//! **public `brix` CLI**, offline, from a package-local registry — plus the
//! deterministic-failure negatives (a missing dependency, a tampered store).

use std::collections::BTreeMap;
use std::io::Write;
use std::process::{Command, Stdio};

use brixpkg::{Manifest, Registry};
use camino::Utf8PathBuf;

const LIB_SRC: &str = "package lib @ 1.0.0\n\
pub rel Widget { id: Int; n: Int } key(id)\n\
pub fn scale(x: Int) -> Int = x + x\n";

const APP_SRC: &str = "package app @ 0.1.0\n\
use lib.{Widget, scale}\n\
rel Out { id: Int; v: Int } key(id)\n\
derive R: Out(id: i, v: y) from { Widget(id: i, n: x); let y = scale(x) }\n";

const APP_TOML: &str =
    "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\nlib = \"^1.0.0\"\n";

fn tmp_dir(tag: &str) -> Utf8PathBuf {
    let mut p =
        Utf8PathBuf::from_path_buf(std::env::temp_dir()).expect("system temp dir must be UTF-8");
    p.push(format!(
        "brix-graph-cli-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    p
}

/// Lay out an `app` package that depends on `lib`, with `lib` published into
/// the package-local registry. Returns the app root and its entry source path.
/// If `publish_lib` is false, the registry is created empty (missing-dep case).
fn scaffold_app(tag: &str, publish_lib: bool) -> (Utf8PathBuf, Utf8PathBuf) {
    let root = tmp_dir(tag);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("brix.toml"), APP_TOML).unwrap();
    let source_path = root.join("src").join("world.brix");
    std::fs::write(&source_path, APP_SRC).unwrap();

    let registry = Registry::open(root.join(".brix").join("registry")).expect("registry opens");
    if publish_lib {
        let lib_manifest =
            Manifest::parse("[package]\nname = \"lib\"\nversion = \"1.0.0\"\n").unwrap();
        let mut lib_files: BTreeMap<Utf8PathBuf, Vec<u8>> = BTreeMap::new();
        lib_files.insert(
            Utf8PathBuf::from("src/world.brix"),
            LIB_SRC.as_bytes().to_vec(),
        );
        registry
            .publish(&lib_manifest, &lib_files, None)
            .expect("publish lib");
    }
    (root, source_path)
}

fn brix(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_brix"))
        .args(args)
        .output()
        .expect("brix binary must be spawnable")
}

#[test]
#[ignore = "shells out to `cargo build` on a generated multi-package workspace \
            twice (cold-compiles brix-rt) + runs it; slow — same convention as \
            acceptance_corpus/reproducibility's heavy subprocess builds. The \
            fast in-process cross-package proof lives in tests/graph.rs. Run \
            with `--ignored`."]
fn locked_multi_package_graph_builds_and_runs_through_the_cli() {
    let (root, source) = scaffold_app("ok", true);

    // Build: resolves `lib` from the local registry, hydrates, compiles the
    // merged graph.
    let build = brix(&["build", source.as_str()]);
    assert!(
        build.status.success(),
        "multi-package build failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr),
    );

    // Run + drive a transaction: assert a `lib.Widget`, expect the rule to fire
    // `scale` (compiled from lib's source) and settle — a nonzero, well-formed
    // canonical dump line proves cross-package execution through the CLI.
    let binary = std::fs::read_dir(root.join(".brix-cache"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let binary = Utf8PathBuf::from_path_buf(binary)
        .unwrap()
        .join("target")
        .join("debug")
        .join(format!("app{}", std::env::consts::EXE_SUFFIX));
    let mut child = Command::new(&binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"assert lib.Widget id=int:5,n=int:3\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "run: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.starts_with("1 "),
        "expected a settled revision dump, got: {stdout}"
    );

    // Determinism: a second build with an unchanged graph is a cache hit
    // (the real lockfile digest is part of the cache key).
    let rebuild = brix(&["build", source.as_str()]);
    assert!(rebuild.status.success());
    assert!(
        String::from_utf8_lossy(&rebuild.stderr).contains("cache hit"),
        "warm rebuild of the graph must hit the cache: {}",
        String::from_utf8_lossy(&rebuild.stderr)
    );

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn missing_dependency_fails_closed() {
    // Registry created but `lib` never published — resolution has no solution.
    let (root, source) = scaffold_app("missing", false);
    let out = brix(&["build", source.as_str()]);
    assert!(
        !out.status.success(),
        "a build whose dependency isn't in the registry must fail"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("resolving dependencies") || stderr.contains("dependency"),
        "expected a dependency-resolution error, got: {stderr}"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn cyclic_dependency_fails_closed() {
    // `app` depends on `a`; `a` depends on `b`; `b` depends back on `a` — a
    // genuine package-dependency cycle (issue #42 Slice 2). `resolve`
    // terminates and produces a lockfile (pubgrub is fine with a cyclic
    // *version* graph); `hydrate`'s `check_acyclic` is what rejects it,
    // before any compilation, so this needs no `cargo build` leg.
    let root = tmp_dir("cyclic");
    std::fs::create_dir_all(root.join("src")).unwrap();
    let app_toml =
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\na = \"^1.0.0\"\n";
    std::fs::write(root.join("brix.toml"), app_toml).unwrap();
    let source_path = root.join("src").join("world.brix");
    std::fs::write(&source_path, "package app @ 0.1.0\n").unwrap();

    let registry = Registry::open(root.join(".brix").join("registry")).expect("registry opens");
    let a_manifest = Manifest::parse(
        "[package]\nname = \"a\"\nversion = \"1.0.0\"\n[dependencies]\nb = \"^1.0.0\"\n",
    )
    .unwrap();
    let mut a_files: BTreeMap<Utf8PathBuf, Vec<u8>> = BTreeMap::new();
    a_files.insert(
        Utf8PathBuf::from("src/world.brix"),
        b"package a @ 1.0.0\n".to_vec(),
    );
    registry
        .publish(&a_manifest, &a_files, None)
        .expect("publish a");

    let b_manifest = Manifest::parse(
        "[package]\nname = \"b\"\nversion = \"1.0.0\"\n[dependencies]\na = \"^1.0.0\"\n",
    )
    .unwrap();
    let mut b_files: BTreeMap<Utf8PathBuf, Vec<u8>> = BTreeMap::new();
    b_files.insert(
        Utf8PathBuf::from("src/world.brix"),
        b"package b @ 1.0.0\n".to_vec(),
    );
    registry
        .publish(&b_manifest, &b_files, None)
        .expect("publish b");

    let out = brix(&["build", source_path.as_str()]);
    assert!(
        !out.status.success(),
        "a cyclic dependency graph must fail the build"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("cycle"),
        "expected a cycle error, got: {stderr}"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn tampered_dependency_fails_closed() {
    let (root, source) = scaffold_app("tampered", true);
    // Corrupt the store blob so the fetched tree no longer matches its locked
    // content digest — hydration must reject it.
    let store = root.join(".brix").join("registry").join("store");
    let blob = std::fs::read_dir(&store)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::write(
        &blob,
        brixpkg::digest::pack(&{
            let mut other: BTreeMap<Utf8PathBuf, Vec<u8>> = BTreeMap::new();
            other.insert(
                Utf8PathBuf::from("src/world.brix"),
                b"package lib @ 1.0.0\nrel Mutated { x: Int } key(x)\n".to_vec(),
            );
            other
        }),
    )
    .unwrap();

    let out = brix(&["build", source.as_str()]);
    assert!(
        !out.status.success(),
        "a tampered dependency store must fail the build"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("tampered") || stderr.contains("digest mismatch"),
        "expected a tamper/digest error, got: {stderr}"
    );
    std::fs::remove_dir_all(&root).ok();
}

// --- Issue #42 Slice 4: multi-file packages ---------------------------

/// A dependency-free package split across two `src/**/*.brix` files that both
/// declare `rel Widget` is a duplicate export in the flat namespace. The build
/// must fail deterministically at lowering (before any `cargo`), so this needs
/// no build leg. Uses a *directory* operand so multi-file discovery runs.
#[test]
fn duplicate_decl_across_root_files_fails_closed() {
    let root = tmp_dir("dupdecl");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("brix.toml"),
        "[package]\nname = \"dup\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    // Entry module + a second module that redeclares the same nominal name.
    std::fs::write(
        root.join("src").join("world.brix"),
        "package dup @ 0.1.0\nmodule World\nrel Widget { id: Int } key(id)\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src").join("other.brix"),
        "package dup @ 0.1.0\nmodule Other\nrel Widget { id: Int } key(id)\n",
    )
    .unwrap();

    let out = brix(&["build", root.as_str()]);
    assert!(
        !out.status.success(),
        "a package with a duplicate nominal decl across files must fail"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("BRX-LOW-0015") || stderr.contains("duplicate declaration"),
        "expected a duplicate-declaration error, got: {stderr}"
    );
    std::fs::remove_dir_all(&root).ok();
}
