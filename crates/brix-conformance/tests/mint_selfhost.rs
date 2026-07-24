//! #15 value-construction primitives — Part E, the `.brix` invocation
//! surface (ATTEMPT). Confirms `packages/brix.type/brix.type.brix`'s
//! `MintOptionOf` spike rule (`derive MintOptionOf: MintedType(orig: t,
//! minted: m) from { TyBytes(ty: t, bytes: b) let raw =
//! brix.ty.mint_unary(14, b) let m = brix.canon.digest(raw) }`) actually
//! runs end-to-end through the real native pipeline — parses, lowers,
//! phase-assigns, and evaluates — and that the `MintedType` row it derives
//! for `Ty::Int` is byte-identical to `Ty::option(Int)`'s own exporter
//! token, exactly like `crates/brix-conformance/tests/engine_mint.rs`
//! proved for the bare Rust builtins.

use brix_ast::parse_file;
use brix_canon::{CanonWriter, Canonical, Domain};
use brix_ir::reflect::analyze;
use brix_ir::types::{IntWidth, Ty};
use brix_rt::engine::{Program, Store, Transaction, Value};
use brixc::pipeline::PhaseAssign;
use brixc::{emit, lower_file, AstPhase};

use brix_conformance::typecorpus::plain_scalar_mismatch;
use brixc::selfhost::typefacts;

const PACKAGE_SRC: &str = include_str!("../../../packages/brix.type/brix.type.brix");

/// Compile `packages/brix.type/brix.type.brix` through the real native
/// pipeline — same anchor `selfhost_parity.rs`'s `compiled_package` uses.
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
        "brix.type package (with the MintOptionOf spike) must lower and \
         type-check cleanly: {:#?}",
        lowered.diags
    );
    let phased = AstPhase
        .assign_phases(lowered)
        .expect("brix.type package (with the MintOptionOf spike) must be well-stratified");
    emit::project_program(&phased)
}

/// The exporter's own tokenization, reproduced via public API (matches
/// `typefacts::digest_hex`'s three-line body exactly — see
/// `engine_mint.rs`'s module doc for why this is provably identical, not a
/// parallel surrogate).
fn exporter_token(ty: &Ty) -> String {
    let mut w = CanonWriter::new();
    ty.canon_write(&mut w);
    w.digest(Domain::Value).to_hex()
}

#[test]
fn mint_option_of_int_rule_derives_the_exporters_own_token() {
    // `plain_scalar_mismatch` exercises a `Fact::UnifyAttempt` with a bare
    // `Ty::Int` operand, so `typefacts::export` walks it through
    // `decompose_ty` and emits a `TyBytes(ty: Int_token, bytes: ...)` Ground
    // fact — exactly what `MintOptionOf`'s body needs to fire.
    let fixture = plain_scalar_mismatch();
    let report = analyze(&fixture.source, &fixture.resolver);

    let export = typefacts::export(&report);
    let mut txn = Transaction::new(b"mint-selfhost-plain-scalar-mismatch".to_vec());
    txn.ops = export.ops;

    let mut store = Store::new(compiled_package());
    let settled = store
        .commit(&txn)
        .expect("exported facts must commit cleanly (shadow mode never rejects)");

    let minted_extent = settled
        .extents
        .get("MintedType")
        .expect("brix.type package must declare a MintedType relation");
    assert!(
        !minted_extent.is_empty(),
        "MintOptionOf must derive at least one MintedType row from the fixture's TyBytes facts"
    );

    let int_ty = Ty::Int(IntWidth::Int);
    let expected_orig = exporter_token(&int_ty);
    let expected_minted = exporter_token(&Ty::option(int_ty));

    let found = minted_extent.values().any(|record| {
        matches!(
            (record.row.get("orig"), record.row.get("minted")),
            (Some(Value::Str(orig)), Some(Value::Str(minted)))
                if *orig == expected_orig && *minted == expected_minted
        )
    });
    assert!(
        found,
        "MintedType must contain (Int_token, Option<Int>_token) — \
         expected orig={expected_orig} minted={expected_minted}, got rows: {:?}",
        minted_extent
            .values()
            .map(|record| record.row.clone())
            .collect::<Vec<_>>()
    );
}
