//! `brixc::selfhost::native_typecheck` — Track A slice C.
//!
//! Drives the entry point from REAL `.brix` source through `lower_file` (so
//! `meta`/spans are populated, unlike the synthetic `FrontendSource`
//! fixtures `brix-conformance`'s parity harness builds by hand) and checks
//! it against `reflect::analyze` run over that same lowered source, so a
//! disagreement between the two would fail loudly rather than silently.

use brix_ast::parse_file;
use brix_ir::reflect::{analyze, ConflictKind};
use brixc::lower_file;
use brixc::selfhost::native_typecheck;

fn lower(src: &str) -> brixc::Lowered {
    let (file, parse_diags) = parse_file(src);
    assert!(
        !parse_diags.has_errors(),
        "fixture must parse cleanly:\n{}",
        parse_diags.render(src, "t")
    );
    lower_file(&file, &parse_diags)
}

/// A role-literal type mismatch: `Input.label` is declared `String`, but the
/// rule body binds it to the integer literal `5`. Mirrors
/// `brix_conformance::typecorpus::NATIVE_ROLE_LIT_MISMATCH_FIXTURE` (the
/// parity harness's own fixture for this exact shape) — reproduced here
/// rather than imported since `brixc` does not (and must not) depend on
/// `brix-conformance`.
const ROLE_LIT_MISMATCH_SRC: &str = r#"
package t @ 1.0.0

rel Input { count: Int; label: String } key(count)
rel Output { count: Int } key(count)

derive Copy: Output(count: count) from {
    Input(count: count, label: 5)
}
"#;

#[test]
fn role_literal_mismatch_yields_one_nat_mismatch_diagnostic_with_a_real_span() {
    let lowered = lower(ROLE_LIT_MISMATCH_SRC);

    // Cross-check: reflect's own report agrees there is exactly one
    // Mismatch conflict for this program (proving native and reflect agree,
    // not just that native fired on *something*).
    let report = analyze(&lowered.source, &lowered.resolver);
    let reflect_mismatches: Vec<_> = report
        .conflicts
        .iter()
        .filter(|c| matches!(c.kind, ConflictKind::Mismatch { .. }))
        .collect();
    assert_eq!(
        reflect_mismatches.len(),
        1,
        "reflect::analyze should report exactly one Mismatch conflict: {:#?}",
        report.conflicts
    );

    let diags = native_typecheck(&lowered);
    assert_eq!(
        diags.len(),
        1,
        "native_typecheck should report exactly one diagnostic: {:#?}",
        diags
    );
    let diag = &diags[0];
    assert_eq!(diag.code, "BRX-NAT-0001");
    assert_ne!(
        diag.span.start, diag.span.end,
        "the diagnostic span must be non-empty, proving span-mapping through \
         the expr origin worked: {:#?}",
        diag
    );
    assert!(
        diag.message.contains("type mismatch"),
        "unexpected message: {}",
        diag.message
    );
}

/// A clean, well-typed program (the same shape `lower_units.rs`'s stable-
/// origin fixture uses) — `native_typecheck` must report zero diagnostics,
/// proving no false positives.
const CLEAN_SRC: &str = r#"
package t @ 1.0.0

rel Input { x: Int } key(x)
rel Output { y: Int } key(y)

derive R: Output(y: y) from {
    Input(x);
    let y = x + 1
}
"#;

#[test]
fn well_typed_program_yields_zero_native_diagnostics() {
    let lowered = lower(CLEAN_SRC);
    assert!(
        !lowered.has_errors(),
        "fixture must lower cleanly: {:#?}",
        lowered.diags
    );

    let diags = native_typecheck(&lowered);
    assert!(
        diags.is_empty(),
        "expected zero native diagnostics on a clean program, got: {:#?}",
        diags
    );
}

/// An arity mismatch: `f` takes one `Int` parameter, but the call site
/// passes zero arguments. Same fixture shape as `lower_units.rs`'s
/// `call_arity_is_one_targeted_error`.
const ARITY_MISMATCH_SRC: &str = r#"
package t @ 1.0.0

fn f(x: Int) -> Int = x

rel Input { x: Int } key(x)
rel Output { x: Int } key(x)

derive R: Output(x: y) from {
    Input(x);
    let y = f()
}
"#;

#[test]
fn call_arity_mismatch_yields_a_nat_arity_diagnostic() {
    let lowered = lower(ARITY_MISMATCH_SRC);

    let report = analyze(&lowered.source, &lowered.resolver);
    let reflect_arities: Vec<_> = report
        .conflicts
        .iter()
        .filter(|c| matches!(c.kind, ConflictKind::Arity { .. }))
        .collect();
    assert_eq!(
        reflect_arities.len(),
        1,
        "reflect::analyze should report exactly one Arity conflict: {:#?}",
        report.conflicts
    );

    let diags = native_typecheck(&lowered);
    let arity_diags: Vec<_> = diags.iter().filter(|d| d.code == "BRX-NAT-0004").collect();
    assert_eq!(
        arity_diags.len(),
        1,
        "expected exactly one BRX-NAT-0004 diagnostic: {:#?}",
        diags
    );
    assert!(
        arity_diags[0].message.contains("arity mismatch"),
        "unexpected message: {}",
        arity_diags[0].message
    );
}
