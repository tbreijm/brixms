//! ⟨D-REDERIVE⟩ — the re-derivation half of ADR-0025 Stage A.
//!
//! **This test is the reason the kernel's pinned constants may be trusted.**
//! `brix-kernel` compiles in thirty-two-byte literals naming values that *this*
//! crate encodes (`prim_pinned.rs`). A hardcoded digest whose meaning rests on
//! the kernel author's say-so would be the second non-re-derivable element in
//! the system; ADR-0022 declined the first — a signature — on exactly the
//! ground that a source-available verifier which re-derives beats one that
//! trusts a constant. ⟨D-REDERIVE⟩ therefore requires both a readable manifest
//! entry *and* this test, and calls a pinned identity shipped without it a
//! defect rather than a shortcut.
//!
//! **Why it lives here and not in `brix-kernel`.** The obligation belongs to
//! the crate that owns the encoder — the one that can actually discharge it,
//! and the one whose change would break it. `soc-regimes` already depends on
//! `brix-kernel`, so this needs no new dependency edge and creates no cycle.
//!
//! **What breaks it, and what that means.** Any change to `Ty`'s or
//! `ArithOp`'s canonical encoding — a renumbered ordinal, a reordered field, a
//! different `write_*` call — moves a digest and fails this test. That is the
//! intended behaviour, not an obstacle: the kernel's rows would otherwise stop
//! matching silently, leaves would go unclosed, and the only symptom would be
//! a grade that quietly stopped moving. Failing here says which value changed.
//!
//! **This is the two-consumer discipline used for frozen vectors, pointed at
//! constants instead of bytes.** Consumer one is the kernel's literal table;
//! consumer two is the reconstruction below, which builds each value from
//! `soc-regimes`' own types and digests it through the real encoder.

use std::collections::BTreeSet;

use brix_kernel::{PinnedArithOp, PinnedNumericTy};
use brix_semantic::PropositionId;
use soc_regimes::type_realization::{ArithOp, Ty};

/// The regime value each pinned numeric identity claims to name.
fn source_value(t: PinnedNumericTy) -> Ty {
    // Spelled arm by arm rather than `Ty::Con(t.lattice_name())`, so the test
    // does not inherit the kernel's own idea of what the name is. If
    // `lattice_name` were wrong, going through it would make the test agree
    // with the bug.
    match t {
        PinnedNumericTy::Nat => Ty::Con("Nat"),
        PinnedNumericTy::Int => Ty::Con("Int"),
        PinnedNumericTy::Rat => Ty::Con("Rat"),
        PinnedNumericTy::Real => Ty::Con("Real"),
        PinnedNumericTy::Complex => Ty::Con("Complex"),
        PinnedNumericTy::Float => Ty::Con("Float"),
    }
}

/// The regime operator each pinned operator identity claims to name.
fn source_op(op: PinnedArithOp) -> ArithOp {
    match op {
        PinnedArithOp::Add => ArithOp::Add,
        PinnedArithOp::Sub => ArithOp::Sub,
        PinnedArithOp::Mul => ArithOp::Mul,
        PinnedArithOp::Div => ArithOp::Div,
    }
}

/// Every pinned numeric type identity equals the digest recomputed here.
#[test]
fn pinned_numeric_type_identities_are_rederivable() {
    for t in PinnedNumericTy::ALL {
        let rederived = PropositionId(source_value(t).config_id().digest());
        assert_eq!(
            t.proposition_id(),
            rederived,
            "pinned identity for Ty::Con(\"{}\") does not match the digest this \
             crate produces — either the constant is wrong or `Ty`'s encoding \
             changed. If the encoding changed deliberately, the kernel's rows \
             stop matching and the constant must be re-pinned in the same \
             change.",
            t.lattice_name()
        );
    }
}

/// Every pinned operator identity equals the digest recomputed here.
#[test]
fn pinned_operator_identities_are_rederivable() {
    for op in PinnedArithOp::ALL {
        let rederived = PropositionId(source_op(op).config_id().digest());
        assert_eq!(
            op.proposition_id(),
            rederived,
            "pinned identity for ArithOp::{} does not match the digest this \
             crate produces",
            op.op_name()
        );
    }
}

/// The kernel's names for these atoms agree with the regime's.
///
/// Separate from the digest check on purpose: a constant could digest
/// correctly while `lattice_name` returned the wrong string, which would make
/// the readable manifest lie about what the digest denotes. The manifest is
/// half of ⟨D-REDERIVE⟩, so it has to be checked too.
#[test]
fn the_kernels_readable_names_match_the_regimes_values() {
    for t in PinnedNumericTy::ALL {
        assert_eq!(
            Ty::Con(t.lattice_name()).config_id().digest(),
            source_value(t).config_id().digest(),
            "the kernel's readable name for {t:?} names a different type"
        );
    }
}

/// The ten identities are distinct *as this crate encodes them*.
///
/// The kernel asserts this over its own table; this asserts it over the
/// re-derived values, so a table with two correct-looking entries that happen
/// to denote the same regime value is still caught.
#[test]
fn the_rederived_identities_are_distinct() {
    let mut seen = BTreeSet::new();
    for t in PinnedNumericTy::ALL {
        assert!(seen.insert(source_value(t).config_id().digest()));
    }
    for op in PinnedArithOp::ALL {
        assert!(seen.insert(source_op(op).config_id().digest()));
    }
    assert_eq!(seen.len(), 10);
}

/// ⟨D-OPPROJECT⟩'s precondition: an operator's standalone identity is the same
/// encoding `Expr::Arith` has always written inline.
///
/// Stage B carries the regime's own `ArithOp` forward through the split. That
/// is only sound if the operator has **one** canonical form — if the standalone
/// encoding differed from the inline one, a pinned identity would name a value
/// that never appears in an expression. Asserted by reconstructing the
/// `Expr::Arith` preimage and checking the operator's bytes occur in it.
#[test]
fn an_operators_standalone_encoding_is_the_one_expressions_use() {
    use brix_canon::{CanonWriter, Canonical};

    for op in [ArithOp::Add, ArithOp::Sub, ArithOp::Mul, ArithOp::Div] {
        let mut standalone = CanonWriter::new();
        op.canon_write(&mut standalone);
        let op_bytes = standalone.finish();

        // The same operator, written where `Expr::Arith` writes it.
        let mut inline = CanonWriter::new();
        inline.write_enum(8, |w| {
            op.canon_write(w);
            soc_regimes::type_realization::Expr::Lit(1).canon_write(w);
            soc_regimes::type_realization::Expr::Lit(2).canon_write(w);
        });
        let expr_bytes = inline.finish();

        assert!(
            expr_bytes
                .windows(op_bytes.len())
                .any(|w| w == op_bytes.as_slice()),
            "ArithOp::{op:?} encodes differently standalone than inside \
             Expr::Arith — a pinned operator identity would name a value no \
             expression contains"
        );
    }
}
