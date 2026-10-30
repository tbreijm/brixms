//! ADR-0015 §5 Stage D through the public lowering path: `let x = 1 + 2`
//! produces a kernel-accepted proof containing `PrimRealizes` for the real
//! `g_arith` occurrence, retained on [`CheckResult`], independently
//! re-checkable, and graded honestly.

use brix_elaborate::{LeafStatus, TreeProofError};
use brix_kernel::{
    acceptance, typing_arith_input_v1, typing_arith_v3, Budget, ExplicitTerm, JudgmentKind,
    TermKind, Verdict,
};
use brix_lower::{check_module, CheckResult};
use brix_semantic::{ContextId, Outcome};
use brix_syntax::parse;
use soc_regimes::type_realization::{
    g_arith, g_arith_input, g_arith_result, generator_is_tight, generator_name, typing_grounds,
    ClaimKind, TypingGroundsError,
};

fn check_one(src: &str) -> CheckResult {
    let module = parse(src).expect("parses");
    let mut results = check_module(&module);
    assert_eq!(results.len(), 1);
    results.remove(0).expect("checks")
}

fn budget() -> Budget {
    Budget::new(100_000, 4096)
}

fn count_prims(kind: &TermKind) -> usize {
    match kind {
        TermKind::PrimRealizes { .. } => 1,
        TermKind::Lam { body, .. } => count_prims(body),
        TermKind::RealizesComp { left, right } | TermKind::RealizesTensor { left, right } => {
            count_prims(left) + count_prims(right)
        }
        _ => 0,
    }
}

#[test]
fn let_one_plus_two_emits_and_retains_an_independently_accepted_primitive_proof() {
    let r = check_one("let x = 1 + 2");
    let proof = &r.typing_proof;

    // The actual emitted proof contains both arithmetic primitives (input bridge and result).
    assert_eq!(count_prims(&proof.term().kind), 2);
    let closed: Vec<_> = proof.closed_occurrences().collect();
    assert_eq!(closed.len(), 2);
    assert_eq!(*closed[0].generator(), g_arith_input());
    assert_eq!(
        closed[0].status(),
        LeafStatus::Closed(typing_arith_input_v1())
    );
    assert_eq!(*closed[1].generator(), g_arith());
    assert_eq!(closed[1].status(), LeafStatus::Closed(typing_arith_v3()));

    // Independent kernel acceptance of the retained (context, proposition, term).
    match acceptance(proof.context(), proof.proposition(), proof.term(), budget()) {
        Verdict::Accepted(cert) => assert_eq!(&cert, proof.certificate()),
        other => panic!("kernel must accept the retained proof: {other:?}"),
    }
    // …and against the public result's own identities.
    assert_eq!(r.proposition, proof.proposition_id());
    assert_eq!(r.context, *proof.context());
    assert_eq!(r.context, ContextId::root());

    // Independent re-verification from the retained derivation artifact.
    proof
        .recheck(&r.tree_derivation, &r.context, budget())
        .expect("recheck");
    assert!(proof.binds(&r.tree_derivation));
    assert_eq!(r.tree_derivation.tree(), &r.derivation);
}

#[test]
fn the_result_reaches_proven_and_both_bridges_are_resolved() {
    let r = check_one("let x = 1 + 2");
    assert_eq!(r.outcome, Outcome::Proven);

    let grounds = r.grounds().expect("proof binds its derivation");
    assert_eq!(grounds.outcome, Outcome::Proven);
    let closed: Vec<_> = grounds.closed.iter().map(|g| g.name.as_deref()).collect();
    assert_eq!(closed, vec![Some("g_arith_input"), Some("g_arith")]);
    assert!(grounds.obligations.is_empty());

    // Tightness is a capability hint, untouched: closure came from the proof.
    for g in [g_arith(), g_arith_input(), g_arith_result()] {
        assert!(!generator_is_tight(ClaimKind::Typing, &g));
        assert!(generator_name(&g).is_some());
    }
}

#[test]
fn nested_arithmetic_closes_every_occurrence_and_reaches_proven() {
    let r = check_one("let z = (1 + 2) * (3 + 4)");
    assert_eq!(r.outcome, Outcome::Proven);
    assert_eq!(count_prims(&r.typing_proof.term().kind), 6);
    assert_eq!(r.typing_proof.closed_occurrences().count(), 6);
    let g = r.grounds().unwrap();
    assert_eq!(g.closed.len(), 6);
    assert!(g.obligations.is_empty());
    r.typing_proof
        .recheck(&r.tree_derivation, &r.context, budget())
        .unwrap();
}

#[test]
fn division_and_mixed_float_arithmetic_close_with_the_same_relation() {
    for src in ["let q = 7 / 2", "let f = 1.5 + 2", "let m = 6 * 7 - 2"] {
        let r = check_one(src);
        assert_eq!(r.outcome, Outcome::Proven, "{src}");
        assert!(r.typing_proof.closed_occurrences().count() >= 2, "{src}");
        r.typing_proof
            .recheck(&r.tree_derivation, &r.context, budget())
            .unwrap_or_else(|e| panic!("{src}: {e:?}"));
    }
}

#[test]
fn non_arithmetic_bindings_close_nothing_and_keep_their_grade() {
    let lit = check_one("let x = 42");
    assert_eq!(lit.outcome, Outcome::Proven);
    assert_eq!(lit.typing_proof.closed_occurrences().count(), 0);
    assert_eq!(count_prims(&lit.typing_proof.term().kind), 0);
    assert!(lit.grounds().unwrap().closed.is_empty());
}

#[test]
fn a_proof_for_another_binding_or_context_is_refused() {
    let a = check_one("let x = 1 + 2");
    let b = check_one("let y = (1 + 2) * 3");

    // Transplanted onto another binding's derivation.
    assert!(!a.typing_proof.binds(&b.tree_derivation));
    assert_eq!(
        typing_grounds(Outcome::Proven, &b.tree_derivation, &a.typing_proof),
        Err(TypingGroundsError::ProofDoesNotBindDerivation)
    );
    assert!(a
        .typing_proof
        .recheck(&b.tree_derivation, &a.context, budget())
        .is_err());

    // Re-checked in a different context.
    let other = ContextId::from_canon(b"arith-leaf.lower.other-context");
    assert!(matches!(
        a.typing_proof.recheck(&a.tree_derivation, &other, budget()),
        Err(TreeProofError::NotAccepted(Verdict::ContextMismatch { .. }))
    ));
}

#[test]
fn a_certificate_digest_alone_does_not_authorize_closure() {
    // The artifact has no public constructor; the only way to get one is to
    // present a term that the kernel accepts. A term whose closure pattern is
    // relabelled (the closed primitive swapped for a hypothesis) is a
    // different canonical construction and cannot reuse the retained identity.
    let r = check_one("let x = 1 + 2");
    let mut forged = r.typing_proof.term().clone();
    fn swap(kind: &mut TermKind) {
        match kind {
            TermKind::PrimRealizes { .. } => {
                *kind = TermKind::Hyp(brix_kernel::Var::Index(0));
            }
            TermKind::Lam { body, .. } => swap(body),
            TermKind::RealizesComp { left, right } | TermKind::RealizesTensor { left, right } => {
                swap(left);
                swap(right);
            }
            _ => {}
        }
    }
    swap(&mut forged.kind);
    let forged = ExplicitTerm::new(forged.context, forged.kind);
    let verdict =
        brix_elaborate::verify_tree_proof(&r.tree_derivation, &r.context, &forged, budget());
    // Whatever it yields, it never reproduces the retained closure account.
    if let Ok(p) = verdict {
        assert_ne!(p.certificate(), r.typing_proof.certificate());
        assert_eq!(p.closed_occurrences().count(), 0);
    }
}

#[test]
fn typing_publishes_no_evaluation_or_policy_authority() {
    let r = check_one("let x = 1 + 2");
    // The relation that closed the arithmetic leaf is a typing relation.
    let relation = brix_kernel::resolve_primitive_relation(&typing_arith_v3()).unwrap();
    assert_eq!(relation.judgment_kind, JudgmentKind::Typing);
    // The proposition is the composition `leaves ⇒ x : Int` shape over
    // `Realizes` only; no value, equality or evaluation proposition is held.
    assert!(!format!("{:?}", r.typing_proof.proposition()).contains("Eq("));
    // The grade is Proven, not an evaluation or compliance claim.
    assert_eq!(r.outcome, Outcome::Proven);
}
