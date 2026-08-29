//! Deeply nested expressions must be *checked*, not crash the process.
//!
//! **What this guards.** The recursive `infer_tree` allocated a native frame
//! per expression level and, on a default 2 MiB thread, died between depth 24
//! and 32 with `fatal runtime error: stack overflow, aborting` — a process
//! abort, not a `TypeError`. The parser accepts nesting to 128
//! (`Limits::max_nesting_depth`), so a source the front end considered legal
//! could kill the compiler rather than being checked or rejected. #319's arm
//! splitting bought a factor of two and left the shape of the problem intact.
//!
//! These run on ordinary test threads with **no stack workaround**, which is
//! the whole claim. A `with_deep_stack`-style helper here would test that a
//! 32 MiB stack is bigger than a 2 MiB one.
//!
//! **Two ceilings remain, and the lower one is not here.**
//!
//! Around depth ~1500 the derived `Clone`/`Drop` glue on the `Box`-based `Expr`
//! and `TyTree` runs out of stack — cloning a subexpression into an endpoint
//! atom recurses once per level. That is a property of the data
//! representation, not of the traversal; it costs a few bytes per level instead
//! of tens of kilobytes, and it sits an order of magnitude above anything the
//! parser will produce. Removing it means interning the expression into an
//! arena, a separate change with its own justification.
//!
//! The one that actually binds is `brix_kernel::acceptance`, which
//! `elaborate_tree` runs over the proof term this derivation becomes. On the
//! same 2 MiB stack it aborts between expression depth 8 and 12 in **debug**,
//! and between 128 and 256 in release — so it is mostly the debug frame-size
//! pathology #319 named, but release also clears the parser's 128 by less than
//! a factor of two. `check_module` therefore aborts in debug on input these
//! tests pass, and `brix-lower/tests/packaged_brix.rs` still needs its
//! large-stack helper.
//!
//! These tests stop at `audited_type_check_tree` for that reason, not by
//! oversight: extending them through elaboration would only re-measure the
//! kernel's limit. See `Type_Realization_Contract.md` §5.5.

use soc_regimes::type_realization::*;

/// Comfortably past the parser's own nesting limit of 128.
const DEEP: usize = 512;

fn spine(d: usize) -> Expr {
    let mut e = Expr::Lit(0);
    for _ in 0..d {
        e = Expr::Arith(ArithOp::Add, Box::new(e), Box::new(Expr::Lit(1)));
    }
    e
}

#[test]
fn a_deep_arithmetic_spine_checks() {
    let (ty, _, _) = infer_tree(&spine(DEEP), &TyCtx::new(), Infer::new()).expect("must check");
    assert_eq!(ty, Ty::Con("Int"));
}

/// Right-nested rather than left-nested: the order children are issued in is
/// the part of the traversal most likely to be got wrong, and the two spines
/// exercise it in opposite directions.
#[test]
fn a_deep_right_nested_spine_checks() {
    let mut e = Expr::Lit(0);
    for _ in 0..DEEP {
        e = Expr::Arith(ArithOp::Add, Box::new(Expr::Lit(1)), Box::new(e));
    }
    let (ty, _, _) = infer_tree(&e, &TyCtx::new(), Infer::new()).expect("must check");
    assert_eq!(ty, Ty::Con("Int"));
}

/// Deep *binder* nesting, so the context extends once per level.
///
/// Each level's endpoints name a distinct scope, so this is also the depth
/// test for the `(Γ, e)` threading ADR-0028 introduced.
#[test]
fn deeply_nested_binders_check() {
    let mut e = Expr::Lit(0);
    for i in 0..DEEP {
        e = Expr::LamAnn(format!("x{i}"), Ty::Con("Int"), Box::new(e));
    }
    let (ty, _, _) = infer_tree(&e, &TyCtx::new(), Infer::new()).expect("must check");
    assert!(matches!(ty, Ty::Fn(..)));
}

/// The full audited path, not just inference: materialization, the endpoint
/// check, and the tree audit all walk the derivation too.
#[test]
fn a_deep_expression_survives_the_whole_audited_path() {
    let (judgement, derivation) =
        audited_type_check_tree(&spine(DEEP), &TyCtx::new()).expect("must check");
    assert_eq!(judgement.context, TyCtx::new().context());
    assert!(derivation.tree().leaves().len() > DEEP);
}
