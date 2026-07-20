//! Issue #42: multi-file packages, CLI-level. A package directory with
//! `src/world.brix` plus sibling `src/<name>.brix` submodules is one package:
//! `brix check`/`brix fmt` see every file, cross-module calls resolve, and a
//! diagnostic in a submodule renders against that submodule's own text —
//! never garbled against the entry's.

use std::fs;

use camino::Utf8PathBuf;

fn tmp_pkg(name: &str) -> Utf8PathBuf {
    let root = Utf8PathBuf::from(format!(
        "{}/brix-multi-file-{name}-{}",
        std::env::temp_dir().display(),
        std::process::id()
    ));
    fs::remove_dir_all(&root).ok();
    fs::create_dir_all(root.join("src")).unwrap();
    root
}

const MANIFEST: &str = "[package]\nname = \"pkg.mathtest\"\nversion = \"0.1.0\"\n";

const WORLD: &str = "package pkg.mathtest @ 0.1.0\n\
module MathTest\n";

const ORDER: &str = "fn min(a: Int, b: Int) -> Int = if a < b then a else b\n\
fn max(a: Int, b: Int) -> Int = if a > b then a else b\n\
fn clamp(x: Int, lo: Int, hi: Int) -> Int = min(max(x, lo), hi)\n";

const INTERP: &str = "fn mix(a: Int, b: Int, t: Int) -> Int = clamp(t, a, b)\n";

#[test]
fn check_sees_every_local_file_and_resolves_cross_module_calls() {
    let root = tmp_pkg("check-ok");
    fs::write(root.join("brix.toml"), MANIFEST).unwrap();
    fs::write(root.join("src/world.brix"), WORLD).unwrap();
    fs::write(root.join("src/order.brix"), ORDER).unwrap();
    fs::write(root.join("src/interp.brix"), INTERP).unwrap();

    let located = brix_cli::package::locate(root.as_str()).expect("locate");
    assert_eq!(located.submodules.len(), 2, "world.brix is excluded from submodules");

    let outcome = brix_cli::build::check(root.as_str());
    assert!(
        outcome.is_ok(),
        "expected a clean multi-file check: {}",
        outcome.err().map(|e| e.to_string()).unwrap_or_default()
    );

    fs::remove_dir_all(&root).ok();
}

#[test]
fn a_submodule_error_renders_against_the_submodule_not_the_entry() {
    let root = tmp_pkg("check-err");
    fs::write(root.join("brix.toml"), MANIFEST).unwrap();
    fs::write(root.join("src/world.brix"), WORLD).unwrap();
    // `min`/`max` return `Int`, but `clamp` claims `Bool` — a real type error,
    // located entirely inside `order.brix`.
    fs::write(
        root.join("src/order.brix"),
        "fn min(a: Int, b: Int) -> Int = if a < b then a else b\n\
         fn max(a: Int, b: Int) -> Int = if a > b then a else b\n\
         fn clamp(x: Int, lo: Int, hi: Int) -> Bool = min(max(x, lo), hi)\n",
    )
    .unwrap();

    let err = match brix_cli::build::check(root.as_str()) {
        Err(e) => e,
        Ok(_) => panic!("type error must fail check"),
    };
    let rendered = err.to_string();
    assert!(
        rendered.contains("src/order.brix") || rendered.contains("BRX-IR-0005"),
        "expected the submodule's own path or a type-error code in the rendering, got: {rendered}"
    );

    fs::remove_dir_all(&root).ok();
}

#[test]
fn duplicate_bare_export_across_two_submodules_fails_closed() {
    let root = tmp_pkg("check-dup");
    fs::write(root.join("brix.toml"), MANIFEST).unwrap();
    fs::write(root.join("src/world.brix"), WORLD).unwrap();
    fs::write(root.join("src/order.brix"), ORDER).unwrap();
    fs::write(
        root.join("src/other.brix"),
        "fn clamp(x: Int) -> Int = x\n",
    )
    .unwrap();

    let err = match brix_cli::build::check(root.as_str()) {
        Err(e) => e,
        Ok(_) => panic!("duplicate export must fail"),
    };
    assert!(
        err.to_string().contains("BRX-PKG-0002") || format!("{err:?}").contains("BRX-PKG-0002"),
        "expected the duplicate-export diagnostic, got: {err}"
    );

    fs::remove_dir_all(&root).ok();
}

#[test]
fn fmt_all_covers_the_entry_and_every_submodule() {
    let root = tmp_pkg("fmt-all");
    fs::write(root.join("brix.toml"), MANIFEST).unwrap();
    fs::write(root.join("src/world.brix"), WORLD).unwrap();
    fs::write(root.join("src/order.brix"), ORDER).unwrap();
    fs::write(root.join("src/interp.brix"), INTERP).unwrap();

    let outcomes = brix_cli::build::format_all(root.as_str()).expect("format_all");
    assert_eq!(outcomes.len(), 3, "entry + 2 submodules");
    assert!(outcomes[0].source_path.ends_with("src/world.brix"));
    let paths: Vec<String> = outcomes
        .iter()
        .map(|o| o.source_path.to_string())
        .collect();
    assert!(paths.iter().any(|p| p.ends_with("src/order.brix")));
    assert!(paths.iter().any(|p| p.ends_with("src/interp.brix")));

    fs::remove_dir_all(&root).ok();
}

#[test]
fn test_selects_and_executes_a_scenario_declared_in_a_submodule() {
    let root = tmp_pkg("test-submodule-scenario");
    fs::write(root.join("brix.toml"), MANIFEST).unwrap();
    fs::write(root.join("src/world.brix"), WORLD).unwrap();
    fs::write(root.join("src/order.brix"), ORDER).unwrap();
    // The scenario lives entirely inside a submodule — `brix test` must
    // discover and run it exactly as it would from `world.brix` (issue #42:
    // a submodule's tests are not second-class).
    fs::write(
        root.join("src/interp.brix"),
        "fn mix(a: Int, b: Int, t: Int) -> Int = clamp(t, a, b)\n\
         scenario ClampsIntoRange {\n  seed 1\n  assert at end { true }\n}\n",
    )
    .unwrap();

    let outcome = brix_cli::test::run(root.as_str(), &["ClampsIntoRange".into()])
        .expect("scenario declared in a submodule must be selectable and executable");
    assert_eq!(outcome.passed, 1);
    assert_eq!(outcome.selected, vec!["ClampsIntoRange".to_string()]);

    fs::remove_dir_all(&root).ok();
}

#[test]
fn quality_standard_fails_when_a_submodule_is_not_canonically_formatted() {
    let root = tmp_pkg("quality-submodule-fmt");
    fs::write(root.join("brix.toml"), MANIFEST).unwrap();
    fs::write(root.join("src/world.brix"), WORLD).unwrap();
    fs::write(root.join("src/order.brix"), ORDER).unwrap();
    // Extra blank lines: parses and lowers fine, but is not the canonical
    // formatter's own rendering — the `source.canonical_format` rule must
    // catch this in a submodule exactly like it always has in `world.brix`.
    fs::write(
        root.join("src/interp.brix"),
        "fn mix(a: Int, b: Int, t: Int) -> Int = clamp(t, a, b)\n\n\n",
    )
    .unwrap();

    let err = match brix_cli::quality::evaluate(
        root.as_str(),
        brix_cli::quality::QualityProfile::Standard,
    ) {
        Err(e) => e,
        Ok(_) => panic!("a non-canonical submodule must fail the standard quality gate"),
    };
    let rendered = err.render(brix_diag::DiagnosticFormat::Json);
    assert!(
        rendered.contains("source.canonical_format") && rendered.contains("\"status\":\"failed\""),
        "expected the canonical-format rule to fail, got: {rendered}"
    );

    fs::remove_dir_all(&root).ok();
}

#[test]
fn reordering_submodule_files_on_disk_does_not_change_check_outcome() {
    let a = tmp_pkg("reorder-a");
    fs::write(a.join("brix.toml"), MANIFEST).unwrap();
    fs::write(a.join("src/world.brix"), WORLD).unwrap();
    fs::write(a.join("src/order.brix"), ORDER).unwrap();
    fs::write(a.join("src/interp.brix"), INTERP).unwrap();

    let b = tmp_pkg("reorder-b");
    fs::write(b.join("brix.toml"), MANIFEST).unwrap();
    fs::write(b.join("src/world.brix"), WORLD).unwrap();
    // Same files, written in the opposite order.
    fs::write(b.join("src/interp.brix"), INTERP).unwrap();
    fs::write(b.join("src/order.brix"), ORDER).unwrap();

    assert!(brix_cli::build::check(a.as_str()).is_ok());
    assert!(brix_cli::build::check(b.as_str()).is_ok());

    fs::remove_dir_all(&a).ok();
    fs::remove_dir_all(&b).ok();
}
