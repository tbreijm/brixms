//! Independence harness (#15 north-star: retire `reflect.rs`).
//!
//! The shadow-mode parity harness (`selfhost_parity.rs`) proves the native
//! `brix.type` package derives the same verdicts as `reflect.rs` — but its
//! Ground inputs come *from* `reflect` (via `typefacts::export(&analyze(..))`),
//! so the package is a second observer of reflect, not an independent checker.
//!
//! This harness proves the first step of true independence: for the
//! role-binding fragment, `brixc::selfhost::extract::extract_role` produces the
//! package's Ground inputs by a **syntactic walk of the lowered program alone**
//! — no `reflect::analyze` anywhere in the pipeline — and the package settles
//! to byte-identical `RoleVar`/`RoleLit`/`SchemaRole` (inputs) and
//! `HasType`/`MismatchConflict` (derived verdicts) extents either way.
//!
//! When this holds for every (a)-family fact, and a native HM driver supplies
//! the (b)-family, `reflect.rs` can be deleted. Today it covers the role
//! fragment; later slices widen it to `OpApply`/`WhenCond`/`FieldAccess`.

use std::collections::{BTreeMap, BTreeSet};

use brix_ast::parse_file;
use brix_ir::frontend::{FrontendSource, SchemaResolver};
use brix_ir::reflect::analyze;
use brix_rt::engine::{Extent, Program, Row, Store, Transaction, TransactionOp};
use brixc::pipeline::PhaseAssign;
use brixc::selfhost::{extract, typefacts};
use brixc::{emit, lower_file, AstPhase};

use brix_conformance::typecorpus::{
    constraint_role_mismatch, role_mismatch, NATIVE_ROLE_BINDINGS_FIXTURE,
    NATIVE_ROLE_LIT_MISMATCH_FIXTURE, NATIVE_VAR_THREE_ROLES_FIXTURE,
    NATIVE_VAR_TWO_ROLES_MISMATCH_FIXTURE,
};

const PACKAGE_SRC: &str = include_str!("../../../packages/brix.type/src/world.brix");

/// The relations whose settled extents must agree between the reflect-fed and
/// the reflect-free pipelines: the three role-fragment Ground inputs plus the
/// two derived verdict relations the role rules produce.
const COMPARED: &[&str] = &[
    "RoleVar",
    "RoleLit",
    "SchemaRole",
    "HasType",
    "MismatchConflict",
];

/// Compile `packages/brix.type/src/world.brix` through the real native
/// pipeline — never a hand-built `Program`. (Mirrors `selfhost_parity.rs`'s
/// `compiled_package`; duplicated because integration-test files cannot share
/// helpers.)
fn compiled_package() -> Program {
    let (file, parse_diags) = parse_file(PACKAGE_SRC);
    assert!(
        !parse_diags.has_errors(),
        "brix.type package must parse cleanly: {:#?}",
        parse_diags.iter().collect::<Vec<_>>()
    );
    let lowered = lower_file(&file, &parse_diags);
    assert!(
        !lowered.has_errors(),
        "brix.type package must lower and type-check cleanly: {:#?}",
        lowered.diags
    );
    let phased = AstPhase
        .assign_phases(lowered)
        .expect("brix.type package must be well-stratified (Appendix F)");
    emit::project_program(&phased)
}

/// Settle one Ground transaction against a fresh package instance and return
/// its derived extents.
fn settle(ops: Vec<TransactionOp>, tag: &[u8]) -> BTreeMap<String, Extent> {
    let mut txn = Transaction::new(tag.to_vec());
    txn.ops = ops;
    let mut store = Store::new(compiled_package());
    store
        .commit(&txn)
        .expect("exported facts must commit cleanly (shadow mode never rejects)")
        .extents
        .clone()
}

/// The set of rows in one settled relation (extents are sets keyed by their
/// declared key; row identity is the comparison atom — tokens are
/// content-addressed, so equal facts produce equal rows across both paths).
fn row_set(extents: &BTreeMap<String, Extent>, relation: &str) -> BTreeSet<Row> {
    extents
        .get(relation)
        .map(|extent| extent.values().map(|record| record.row.clone()).collect())
        .unwrap_or_default()
}

/// The core assertion: settling the reflect-free extractor output and the
/// reflect-fed exporter output yields identical extents for every compared
/// relation. Every fact the package consumes (and every verdict it derives)
/// is reproduced without `reflect::analyze`.
fn assert_independent<R: SchemaResolver>(label: &str, source: &FrontendSource, resolver: &R) {
    let reflect_extents = {
        let export = typefacts::export(&analyze(source, resolver));
        settle(export.ops, b"independence-reflect")
    };
    let native_extents = {
        let export = extract::extract_role(source, resolver);
        settle(export.ops, b"independence-extract")
    };

    let mut total = 0usize;
    for &relation in COMPARED {
        let reflect_rows = row_set(&reflect_extents, relation);
        let native_rows = row_set(&native_extents, relation);
        assert_eq!(
            native_rows, reflect_rows,
            "[{label}] reflect-free extractor and reflect-fed exporter must settle to the same \
             {relation} extent"
        );
        total += reflect_rows.len();
    }
    // Non-vacuity: every role fixture declares schema roles and at least one
    // role binding, so a bug that emitted *nothing* would show as empty==empty
    // and pass silently. Require the comparison to have had real rows to agree
    // on (SchemaRole is populated by every fixture).
    assert!(
        !row_set(&reflect_extents, "SchemaRole").is_empty(),
        "[{label}] role fixture must populate SchemaRole — otherwise the extent comparison is \
         vacuous"
    );
    assert!(
        total > 0,
        "[{label}] compared extents must be non-empty (the equivalence is not vacuous)"
    );
}

/// Lower a `.brix` source fixture through the real pipeline and run the
/// independence assertion on its `FrontendSource` + resolver.
fn assert_source_independent(label: &str, src: &str) {
    let (file, parse_diags) = parse_file(src);
    assert!(
        !parse_diags.has_errors(),
        "fixture must parse cleanly: {:#?}",
        parse_diags.iter().collect::<Vec<_>>()
    );
    let lowered = lower_file(&file, &parse_diags);
    assert_independent(label, &lowered.source, &lowered.resolver);
}

#[test]
fn clean_role_bindings_extract_reflect_free() {
    assert_source_independent("role_bindings", NATIVE_ROLE_BINDINGS_FIXTURE);
}

#[test]
fn role_literal_mismatch_extract_reflect_free() {
    assert_source_independent("role_lit_mismatch", NATIVE_ROLE_LIT_MISMATCH_FIXTURE);
}

#[test]
fn var_at_two_roles_extract_reflect_free() {
    assert_source_independent("var_two_roles", NATIVE_VAR_TWO_ROLES_MISMATCH_FIXTURE);
}

#[test]
fn var_at_three_roles_extract_reflect_free() {
    // Exercises the per-(declaration, variable) ordinal counter across three
    // occurrences — the extractor must reach ordinals 0/1/2 in the same order
    // reflect does or the RoleVar extent (and the derived mismatches) diverge.
    assert_source_independent("var_three_roles", NATIVE_VAR_THREE_ROLES_FIXTURE);
}

#[test]
fn builder_rule_role_mismatch_extract_reflect_free() {
    let fixture = role_mismatch();
    assert_independent("role_mismatch", &fixture.source, &fixture.resolver);
}

#[test]
fn builder_constraint_role_mismatch_extract_reflect_free() {
    let fixture = constraint_role_mismatch();
    assert_independent(
        "constraint_role_mismatch",
        &fixture.source,
        &fixture.resolver,
    );
}
