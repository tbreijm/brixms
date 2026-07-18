//! Reproducibility & backend parity (issue #41, G3 in-repo portion).
//!
//! The G3 release gate is *two independent physical machines* producing
//! byte-identical generated artifacts and identical canonical results. That
//! two-machine comparison and the published-toolchain evidence are a
//! release/CI-infra concern (they belong to #46 Developer Day); this suite is
//! the strongest single-machine proxy plus the correctness guarantees that gate
//! must be able to assume:
//!
//! 1. **Deterministic emit** — the full generated workspace (every file:
//!    `Cargo.toml`, `src/generated.rs`, `src/main.rs`, `src/native_program.rs`)
//!    is byte-identical across independent assembles of the same source. The
//!    hermetic build-input contract (`brixc::CacheInputs`) is what guarantees
//!    this off one machine to the next; its input-sensitivity is unit-tested in
//!    `brixc::cache`.
//! 2. **Cache integrity / fail-closed** — a corrupted or incomplete cache entry
//!    is rebuilt, never trusted as a successful artifact (spec §26.8).
//! 3. **Deterministic perf budgets** — deterministic size metrics of the
//!    generated artifacts are held under committed budgets so a regression is a
//!    CI signal; wall-clock is reported, never asserted.
//! 4. **Backend parity** — two clean builds produce identical on-disk sources
//!    and identical settled canonical dumps (the `#[ignore]`d subprocess test).
//!
//! The heavy subprocess tests shell out to `cargo build`/`cargo run` (same cost
//! as `build_run_smoke.rs`); `two_clean_builds_are_byte_identical_on_disk` runs
//! two full builds and is `#[ignore]`d, while `cache_integrity_is_fail_closed`
//! runs one build and stays in the default gate.

use std::collections::BTreeMap;
use std::io::Write;
use std::process::{Command, Stdio};

use brix_ast::parse_file;
use brixc::pipeline::PhaseAssign;
use brixc::AstPhase;
use camino::{Utf8Path, Utf8PathBuf};

const FLAGSHIP: &str =
    include_str!("../../brix-ast/tests/fixtures/spec/0001-part-i-the-flagship-program.brix");

/// Assemble the flagship's full generated workspace at the library level (no
/// `cargo`). `runtime_path` is a fixed string here — the two-machine gate must
/// vendor/relativize it for true cross-machine byte-identity, a documented
/// limitation out of Wave 1 scope; within one machine it is constant, so this
/// is a faithful determinism probe of everything brixc emits.
fn flagship_workspace() -> BTreeMap<Utf8PathBuf, String> {
    let (file, diags) = parse_file(FLAGSHIP);
    assert!(!diags.has_errors(), "flagship must parse cleanly");
    let lowered = brixc::lower_file(&file, &diags);
    assert!(!lowered.has_errors(), "flagship must lower cleanly");
    let phased = AstPhase
        .assign_phases(lowered)
        .expect("flagship must be well-stratified");
    let (relations, rules) = brixc::emit::project_phased(&phased);
    let native_program = brixc::emit::project_program(&phased);
    brixc::emit::assemble_workspace_with_runtime(
        "repro.pkg",
        &relations,
        &rules,
        "../brix-rt",
        &native_program,
    )
}

// ---------------------------------------------------------------------
// 1. Deterministic emit.
// ---------------------------------------------------------------------

#[test]
fn flagship_workspace_emit_is_byte_identical_across_independent_runs() {
    let a = flagship_workspace();
    let b = flagship_workspace();
    assert_eq!(
        a.keys().collect::<Vec<_>>(),
        b.keys().collect::<Vec<_>>(),
        "generated file set must be identical across independent assembles"
    );
    for (path, contents) in &a {
        assert_eq!(
            Some(contents),
            b.get(path),
            "generated file `{path}` differs across independent assembles"
        );
    }
    // Sanity: this really is a whole workspace, not an empty map.
    for expected in ["Cargo.toml", "src/generated.rs", "src/main.rs"] {
        assert!(
            a.contains_key(&Utf8PathBuf::from(expected)),
            "expected generated file `{expected}` in the workspace, got {:?}",
            a.keys().collect::<Vec<_>>()
        );
    }
}

// ---------------------------------------------------------------------
// 3. Deterministic perf budgets.
// ---------------------------------------------------------------------

/// Committed upper bounds on deterministic size metrics of the flagship's
/// generated artifacts. These are *budgets* (regression alarms), not exact
/// freezes: they should change only when the flagship or codegen genuinely
/// grows, and a change that blows a budget is a reviewable CI failure. Tune
/// upward deliberately, never to paper over an unexpected blow-up.
const BUDGET_FILE_COUNT: usize = 8;
const BUDGET_TOTAL_SOURCE_BYTES: usize = 48_000;

#[test]
fn flagship_generated_artifact_size_is_within_budget() {
    let ws = flagship_workspace();
    let file_count = ws.len();
    let total_bytes: usize = ws.values().map(|c| c.len()).sum();

    // Deterministic measurements, reported (this line lands in CI logs / the
    // step summary); wall-clock is deliberately NOT measured here — it is not a
    // semantic quantity and must never gate.
    eprintln!(
        "repro perf (deterministic): flagship generated files={file_count} \
         total_source_bytes={total_bytes} (budgets: files<={BUDGET_FILE_COUNT}, \
         bytes<={BUDGET_TOTAL_SOURCE_BYTES})"
    );

    assert!(
        file_count <= BUDGET_FILE_COUNT,
        "generated file count {file_count} exceeds budget {BUDGET_FILE_COUNT} — \
         a regression, or bump the budget deliberately"
    );
    assert!(
        total_bytes <= BUDGET_TOTAL_SOURCE_BYTES,
        "generated source size {total_bytes}B exceeds budget {BUDGET_TOTAL_SOURCE_BYTES}B — \
         a regression, or bump the budget deliberately"
    );
}

// ---------------------------------------------------------------------
// 2. Cache integrity / fail-closed (one real build).
// ---------------------------------------------------------------------

fn brix(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_brix"))
        .args(args)
        .output()
        .expect("brix binary must be spawnable")
}

fn tmp_dir(tag: &str) -> Utf8PathBuf {
    let mut p =
        Utf8PathBuf::from_path_buf(std::env::temp_dir()).expect("system temp dir must be UTF-8");
    p.push(format!(
        "brix-repro-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    p
}

const TINY: &str = "package smoke.repro @ 0.1.0\n\
\n\
rel Input { value: I64 } key(value)\n\
rel Output { value: I64 } key(value)\n\
derive R: Output(value: value) from { Input(value) }\n";

/// The single cache entry directory under `.brix-cache/`.
fn cache_entry(root: &Utf8Path) -> Utf8PathBuf {
    let cache_root = root.join(".brix-cache");
    let entry = std::fs::read_dir(&cache_root)
        .unwrap_or_else(|e| panic!("no .brix-cache at {cache_root}: {e}"))
        .next()
        .expect("at least one cache entry")
        .unwrap()
        .path();
    Utf8PathBuf::from_path_buf(entry).unwrap()
}

fn stderr_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Deliverable 2, the headline #41 correctness bullet: neither a *corrupted*
/// generated source nor an *incomplete* cache entry (missing completion marker)
/// may be treated as a successful cache hit — each forces a clean rebuild.
#[test]
fn cache_integrity_is_fail_closed() {
    let root = tmp_dir("cache-integrity");
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("world.brix");
    std::fs::write(&source, TINY).unwrap();

    // Cold build populates the entry and writes the completion marker.
    let cold = brix(&["build", source.as_str()]);
    assert!(cold.status.success(), "cold build: {}", stderr_of(&cold));
    let entry = cache_entry(&root);
    let marker = entry.join(".brix-manifest");
    assert!(marker.exists(), "cold build must write a completion marker");

    // Warm rebuild is a genuine cache hit.
    let warm = brix(&["build", source.as_str()]);
    assert!(warm.status.success());
    assert!(
        stderr_of(&warm).contains("cache hit"),
        "an untouched warm rebuild must be a cache hit: {}",
        stderr_of(&warm)
    );

    // Corrupt a generated source: truncate src/generated.rs. The next build
    // must NOT trust it — no cache hit — and must repair it.
    let generated = entry.join("src/generated.rs");
    let good = std::fs::read_to_string(&generated).unwrap();
    std::fs::write(&generated, "// corrupted\n").unwrap();
    let after_corruption = brix(&["build", source.as_str()]);
    assert!(
        after_corruption.status.success(),
        "rebuild after corruption: {}",
        stderr_of(&after_corruption)
    );
    assert!(
        !stderr_of(&after_corruption).contains("cache hit"),
        "a corrupted cache entry must be rebuilt, not treated as a hit: {}",
        stderr_of(&after_corruption)
    );
    assert_eq!(
        std::fs::read_to_string(&generated).unwrap(),
        good,
        "the rebuild must restore the corrupted generated source"
    );

    // Incomplete entry: remove the completion marker. Even with a valid binary
    // and intact sources present, a missing marker means "build did not finish"
    // and must force a rebuild.
    std::fs::remove_file(&marker).unwrap();
    let after_incomplete = brix(&["build", source.as_str()]);
    assert!(after_incomplete.status.success());
    assert!(
        !stderr_of(&after_incomplete).contains("cache hit"),
        "an entry with no completion marker must be rebuilt: {}",
        stderr_of(&after_incomplete)
    );
    assert!(
        marker.exists(),
        "the rebuild must re-write the completion marker"
    );

    std::fs::remove_dir_all(&root).ok();
}

// ---------------------------------------------------------------------
// 4. Two clean builds byte-identical (two real builds; slow -> ignored).
// ---------------------------------------------------------------------

/// The strongest single-machine proxy for the G3 two-machine gate: two fully
/// independent clean builds (separate package roots, separate caches) of the
/// same source produce byte-identical generated sources AND identical settled
/// canonical dumps. `#[ignore]`d because it shells out to `cargo build` twice
/// (run with `--ignored`).
#[test]
#[ignore = "two full `cargo build`s; slow"]
fn two_clean_builds_are_byte_identical_on_disk() {
    let build_into = |tag: &str| -> Utf8PathBuf {
        let root = tmp_dir(tag);
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("world.brix");
        std::fs::write(&source, TINY).unwrap();
        let out = brix(&["build", source.as_str()]);
        assert!(out.status.success(), "{tag} build: {}", stderr_of(&out));
        root
    };
    let root_a = build_into("clean-a");
    let root_b = build_into("clean-b");
    let entry_a = cache_entry(&root_a);
    let entry_b = cache_entry(&root_b);

    // Independent builds of identical source share a cache key -> same entry dir
    // name -> and byte-identical generated sources.
    assert_eq!(
        entry_a.file_name(),
        entry_b.file_name(),
        "identical source must yield the same cache key on both builds"
    );
    for rel in [
        "Cargo.toml",
        "src/generated.rs",
        "src/main.rs",
        "src/native_program.rs",
    ] {
        let a = std::fs::read_to_string(entry_a.join(rel));
        let b = std::fs::read_to_string(entry_b.join(rel));
        assert_eq!(
            a.ok(),
            b.ok(),
            "generated `{rel}` differs between clean builds"
        );
    }

    // Backend parity: both binaries settle the same transaction to the same
    // canonical dump.
    let dump = |entry: &Utf8Path| -> String {
        let binary = entry
            .join("target")
            .join("debug")
            .join(format!("smoke_repro{}", std::env::consts::EXE_SUFFIX));
        let mut child = Command::new(binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"assert Input value=int:7\n")
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .expect("dump line")
            .to_string()
    };
    assert_eq!(
        dump(&entry_a),
        dump(&entry_b),
        "two clean builds must settle to the same canonical dump"
    );

    std::fs::remove_dir_all(&root_a).ok();
    std::fs::remove_dir_all(&root_b).ok();
}
