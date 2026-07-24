//! Track A slice D: `brix check --native-typecheck` wires the self-hosted
//! `brix.type` checker into `brix_cli::build::check` as an ADVISORY pass —
//! it may populate `CheckOutcome::native_report` with `BRX-NAT-*` findings,
//! but it never turns a clean check into an `Err` (nor a failing one into an
//! `Ok`). `infer`/phase-assignment remain the sole error floor.
//!
//! Divergence note (see the two tests below and the module doc on
//! `native_typecheck_mechanism_populates_native_report_when_native_disagrees_with_a_silenced_lowered`):
//! every real `.brix` shape probed while building this slice (role-literal
//! mismatch, call arity, and an `Estimate<Int>` -> `Int` epistemic erasure)
//! was flagged by `infer` (and hence fails lowering, `has_errors() == true`)
//! whenever it was flagged by the native checker. This is expected: Track A
//! slices 4-7 built the native checker's 7 `*Conflict` extents specifically
//! to reproduce `infer`'s own decisions (12/12 parity), and native's checks
//! are a strict subset of `infer`'s 10 `TypeError` variants (no native
//! counterpart exists for `Dimension`, `NoMatchingOverload`, or
//! `AmbiguousOverload`). No infer-clean-but-native-flagged real program was
//! found — a useful signal for slice E's differential: on the current
//! corpus, native and infer agree everywhere native has an opinion at all.

use std::path::PathBuf;

use camino::Utf8PathBuf;

fn tmp_dir(tag: &str) -> Utf8PathBuf {
    let mut path =
        Utf8PathBuf::from_path_buf(std::env::temp_dir()).expect("temp path must be UTF-8");
    path.push(format!(
        "brix-cli-native-typecheck-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    path
}

const CLEAN_SRC: &str = "package smoke.native @ 0.1.0\n\nrel Input {\n  value: I64\n} key(value)\n";

/// `packages/brix.type`, resolved from this crate's manifest dir — the same
/// helper `brix_type_package.rs` (slice B) uses.
fn brix_type_pkg_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("packages")
        .join("brix.type")
}

/// The flag, off: `native_report` is always `None` — the pre-slice-D
/// behavior, byte for byte.
#[test]
fn native_typecheck_off_never_populates_native_report() {
    let root = tmp_dir("off");
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("world.brix");
    std::fs::write(&source, CLEAN_SRC).unwrap();

    let outcome = brix_cli::build::check(source.as_str(), false)
        .unwrap_or_else(|e| panic!("clean source must check cleanly: {e}"));
    assert!(outcome.native_report.is_none());

    std::fs::remove_dir_all(&root).ok();
}

/// The flag, on, over a clean program: `check` still returns `Ok`, and since
/// the native checker finds nothing to report on a well-typed program,
/// `native_report` stays `None` too (proving zero false positives ride
/// through the real CLI plumbing, not just the `native_typecheck` unit
/// tests in `crates/brixc/tests/native_typecheck.rs`).
#[test]
fn native_typecheck_on_over_clean_source_is_ok_with_no_report() {
    let root = tmp_dir("on-clean");
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("world.brix");
    std::fs::write(&source, CLEAN_SRC).unwrap();

    let outcome = brix_cli::build::check(source.as_str(), true)
        .unwrap_or_else(|e| panic!("clean source must check cleanly with --native-typecheck: {e}"));
    assert!(
        outcome.native_report.is_none(),
        "a well-typed program must yield zero native findings: {:#?}",
        outcome.native_report.map(|r| r.diagnostics)
    );

    std::fs::remove_dir_all(&root).ok();
}

/// The advisory guarantee on a REAL, substantial program: `packages/brix.type`
/// itself (the self-hosted checker's own source, already proven to check
/// cleanly through the real `locate` -> `check` path by slice B's
/// `brix_type_package.rs`). Running the native checker over its own package
/// is the strongest available real-world test of "advisory, never gates":
/// both `check(root, false)` and `check(root, true)` must return `Ok` with
/// an identical `source_path`, and (per the divergence note above) the
/// native pass finds nothing new to report on it either.
#[test]
fn brix_type_package_checks_identically_with_native_typecheck_on_or_off() {
    let root = brix_type_pkg_root();
    let root_str = root.to_str().expect("pkg root must be UTF-8");

    let without_native = brix_cli::build::check(root_str, false)
        .unwrap_or_else(|e| panic!("brix check packages/brix.type failed: {e}"));
    let with_native = brix_cli::build::check(root_str, true)
        .unwrap_or_else(|e| panic!("brix check --native-typecheck packages/brix.type failed: {e}"));

    assert_eq!(without_native.source_path, with_native.source_path);
    assert!(without_native.native_report.is_none());
    assert!(
        with_native.native_report.is_none(),
        "expected zero native findings on brix.type's own (already clean) source: {:#?}",
        with_native.native_report.map(|r| r.diagnostics)
    );
}

/// Mechanism test (fallback per the slice-D brief, since no real
/// infer-clean-but-native-flagged `.brix` program was found — see the
/// module doc): exercises the EXACT call `build::check` makes —
/// `brixc::selfhost::native_typecheck(&lowered)` followed by wrapping any
/// non-empty result in a `DiagnosticReport` — against a `Lowered` whose
/// diagnostics were cleared after the fact to isolate the report-population
/// logic from the (currently redundant, on this fixture) `infer` error that
/// would otherwise short-circuit `check` before native ever ran.
///
/// The fixture is the same role-literal mismatch shape
/// `crates/brixc/tests/native_typecheck.rs` uses for `native_typecheck`
/// itself (`Input.label: String` bound to the integer literal `5`): `infer`
/// DOES flag it for real (`BRX-IR-0005`), which is exactly the divergence
/// finding above — so this test's `lowered.diags.clear()` step is a
/// deliberate simulation of "if lowering had stayed clean," not a claim
/// that it does on this input.
#[test]
fn native_typecheck_mechanism_populates_native_report_when_native_disagrees_with_a_silenced_lowered(
) {
    use brix_ast::parse_file;
    use brix_diag::Diagnostics;

    const ROLE_LIT_MISMATCH_SRC: &str = r#"
package t @ 1.0.0

rel Input { count: Int; label: String } key(count)
rel Output { count: Int } key(count)

derive Copy: Output(count: count) from {
    Input(count: count, label: 5)
}
"#;

    let (file, parse_diags) = parse_file(ROLE_LIT_MISMATCH_SRC);
    assert!(!parse_diags.has_errors(), "fixture must parse cleanly");
    let mut lowered = brixc::lower_file(&file, &parse_diags);
    assert!(
        lowered.has_errors(),
        "documents the divergence finding: infer DOES flag this fixture for real"
    );

    // Simulate "lowering stayed clean" to isolate the native_report-population
    // logic `build::check` runs (see doc comment above for why this is a
    // deliberate simulation, not a claim about real behavior on this input).
    lowered.diags.clear();
    assert!(!lowered.has_errors());

    let diagnostics = brixc::selfhost::native_typecheck(&lowered);
    assert!(
        !diagnostics.is_empty(),
        "expected the native checker to independently flag the same mismatch"
    );
    assert_eq!(diagnostics[0].code, "BRX-NAT-0001");

    // Exactly the `(!diagnostics.is_empty()).then(...)` construction
    // `build::check` uses.
    let native_report = (!diagnostics.is_empty()).then(|| brix_cli::build::DiagnosticReport {
        source: ROLE_LIT_MISMATCH_SRC.to_string(),
        path: "t".to_string(),
        diagnostics: Diagnostics::from_items(diagnostics),
    });
    assert!(native_report.is_some());
    let rendered = native_report
        .unwrap()
        .render(brix_diag::DiagnosticFormat::Human);
    assert!(rendered.contains("BRX-NAT-0001"), "{rendered}");
}
