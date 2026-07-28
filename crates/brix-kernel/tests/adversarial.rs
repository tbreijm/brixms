//! Adversarial test suite for brix-kernel Slice 1 (ADR-0003 §8.1).

use brix_kernel::{
    acceptance, Budget, ExplicitTerm, Prop, RejectionReason, ResourceBudgetReason, TermKind,
    UnsupportedConstruct, Var, Verdict,
};
use brix_semantic::{ContextId, Outcome, PropositionId, VerifierId};

fn sample_context_a() -> ContextId {
    ContextId::from_canon(b"context_a")
}

fn sample_context_b() -> ContextId {
    ContextId::from_canon(b"context_b")
}

fn sample_prop_p() -> Prop {
    Prop::Atom(PropositionId::from_canon(b"P"))
}

fn sample_prop_q() -> Prop {
    Prop::Atom(PropositionId::from_canon(b"Q"))
}

#[test]
fn test_valid_implication_proof_returns_accepted_with_verifier_id() {
    let ctx = sample_context_a();
    let p = sample_prop_p();
    // Proposition: P -> P
    let goal = Prop::Impl(Box::new(p.clone()), Box::new(p));

    // Term: \x. x (using de Bruijn index 0)
    let term = ExplicitTerm::new(
        ctx,
        TermKind::Lam {
            var_name: Some("x".into()),
            body: Box::new(TermKind::Hyp(Var::Index(0))),
        },
    );

    let budget = Budget::new(100, 100);
    let verdict = acceptance(&ctx, &goal, &term, budget);

    match &verdict {
        Verdict::Accepted(cert) => {
            assert_eq!(cert.verifier, VerifierId::named("brix.kernel@0.1"));
            assert_eq!(verdict.outcome(), Some(Outcome::Proven));
        }
        other => panic!("Expected Accepted, got {other:?}"),
    }
}

#[test]
fn test_valid_product_proof_returns_accepted() {
    let ctx = sample_context_a();
    let p = sample_prop_p();
    let q = sample_prop_q();
    // Goal: P x Q -> P
    let goal = Prop::Impl(
        Box::new(Prop::Prod(Box::new(p.clone()), Box::new(q))),
        Box::new(p),
    );

    // Term: \pair. pi1(pair)
    let term = ExplicitTerm::new(
        ctx,
        TermKind::Lam {
            var_name: Some("p".into()),
            body: Box::new(TermKind::Proj1(Box::new(TermKind::Hyp(Var::Named(
                "p".into(),
            ))))),
        },
    );

    let budget = Budget::new(100, 100);
    let verdict = acceptance(&ctx, &goal, &term, budget);

    assert!(
        matches!(verdict, Verdict::Accepted(_)),
        "Expected Accepted for product proj1 proof"
    );
    assert_eq!(verdict.outcome(), Some(Outcome::Proven));
}

#[test]
fn test_valid_sum_proof_returns_accepted() {
    let ctx = sample_context_a();
    let p = sample_prop_p();
    let q = sample_prop_q();
    // Goal: P + Q -> Q + P
    let goal = Prop::Impl(
        Box::new(Prop::Sum(Box::new(p.clone()), Box::new(q.clone()))),
        Box::new(Prop::Sum(Box::new(q), Box::new(p))),
    );

    // Term: \s. case s of inl(x) => inr(x) | inr(y) => inl(y)
    let term = ExplicitTerm::new(
        ctx,
        TermKind::Lam {
            var_name: Some("s".into()),
            body: Box::new(TermKind::Case {
                discriminant: Box::new(TermKind::Hyp(Var::Named("s".into()))),
                left_var: Some("x".into()),
                left_body: Box::new(TermKind::Inr(Box::new(TermKind::Hyp(Var::Named(
                    "x".into(),
                ))))),
                right_var: Some("y".into()),
                right_body: Box::new(TermKind::Inl(Box::new(TermKind::Hyp(Var::Named(
                    "y".into(),
                ))))),
            }),
        },
    );

    let budget = Budget::new(100, 100);
    let verdict = acceptance(&ctx, &goal, &term, budget);

    assert!(
        matches!(verdict, Verdict::Accepted(_)),
        "Expected Accepted for sum swap proof"
    );
    assert_eq!(verdict.outcome(), Some(Outcome::Proven));
}

#[test]
fn test_well_formed_false_proof_returns_rejected() {
    let ctx = sample_context_a();
    let p = sample_prop_p();
    let q = sample_prop_q();
    // Goal: P -> Q (where P != Q)
    let goal = Prop::Impl(Box::new(p), Box::new(q));

    // Term: \x. x (identity term cannot prove P -> Q when P != Q)
    let term = ExplicitTerm::new(
        ctx,
        TermKind::Lam {
            var_name: Some("x".into()),
            body: Box::new(TermKind::Hyp(Var::Index(0))),
        },
    );

    let budget = Budget::new(100, 100);
    let verdict = acceptance(&ctx, &goal, &term, budget);

    assert!(
        matches!(
            verdict,
            Verdict::Rejected(RejectionReason::TypeMismatch { .. })
        ),
        "Expected Rejected for false proof term, got {verdict:?}"
    );
    // Rejected publishes nothing (Outcome is None)
    assert_eq!(verdict.outcome(), None);
}

#[test]
fn test_eigenvariable_freshness_violation_returns_malformed() {
    let ctx = sample_context_a();
    let p = sample_prop_p();
    let q = sample_prop_q();
    // Goal: P -> (P + Q) -> P
    let goal = Prop::Impl(
        Box::new(p.clone()),
        Box::new(Prop::Impl(
            Box::new(Prop::Sum(Box::new(p.clone()), Box::new(q))),
            Box::new(p.clone()),
        )),
    );

    // Outer term binds "x" : P, inner term binds "s" : P + Q.
    // Case tries to bind left_var as "x", which collides with "x" already in context!
    let term = ExplicitTerm::new(
        ctx,
        TermKind::Lam {
            var_name: Some("x".into()),
            body: Box::new(TermKind::Lam {
                var_name: Some("s".into()),
                body: Box::new(TermKind::Case {
                    discriminant: Box::new(TermKind::Hyp(Var::Named("s".into()))),
                    left_var: Some("x".into()), // Collision with "x" in context!
                    left_body: Box::new(TermKind::Hyp(Var::Named("x".into()))),
                    right_var: Some("y".into()),
                    right_body: Box::new(TermKind::Hyp(Var::Named("x".into()))),
                }),
            }),
        },
    );

    let budget = Budget::new(100, 100);
    let verdict = acceptance(&ctx, &goal, &term, budget);

    assert!(
        matches!(verdict, Verdict::Malformed(_)),
        "Expected Malformed due to eigenvariable collision, got {verdict:?}"
    );
    assert_eq!(verdict.outcome(), None);
}

#[test]
fn test_unsupported_construct_returns_unsupported() {
    let ctx = sample_context_a();
    let p = sample_prop_p();
    let goal = Prop::Impl(Box::new(p.clone()), Box::new(p));

    // Term using an out-of-slice construct
    let term = ExplicitTerm::new(ctx, TermKind::Unsupported("Existential witness".into()));

    let budget = Budget::new(100, 100);
    let verdict = acceptance(&ctx, &goal, &term, budget);

    assert!(
        matches!(
            verdict,
            Verdict::Unsupported(UnsupportedConstruct::Construct(_))
        ),
        "Expected Unsupported, got {verdict:?}"
    );
    assert_eq!(verdict.outcome(), None);
}

#[test]
fn test_context_mismatch_returns_context_mismatch() {
    let ctx_claimed = sample_context_a();
    let ctx_term = sample_context_b();
    let p = sample_prop_p();
    let goal = Prop::Impl(Box::new(p.clone()), Box::new(p));

    let term = ExplicitTerm::new(
        ctx_term,
        TermKind::Lam {
            var_name: Some("x".into()),
            body: Box::new(TermKind::Hyp(Var::Index(0))),
        },
    );

    let budget = Budget::new(100, 100);
    let verdict = acceptance(&ctx_claimed, &goal, &term, budget);

    match verdict {
        Verdict::ContextMismatch {
            claimed,
            term_context,
        } => {
            assert_eq!(claimed, ctx_claimed);
            assert_eq!(term_context, ctx_term);
        }
        other => panic!("Expected ContextMismatch, got {other:?}"),
    }
    assert_eq!(verdict.outcome(), None);
}

#[test]
fn test_tiny_budget_returns_resource_exhausted_mapping_to_unknown_never_refuted() {
    let ctx = sample_context_a();
    let p = sample_prop_p();
    let goal = Prop::Impl(Box::new(p.clone()), Box::new(p));

    let term = ExplicitTerm::new(
        ctx,
        TermKind::Lam {
            var_name: Some("x".into()),
            body: Box::new(TermKind::Hyp(Var::Index(0))),
        },
    );

    // Budget of 0 steps
    let budget = Budget::new(0, 100);
    let verdict = acceptance(&ctx, &goal, &term, budget);

    assert!(
        matches!(
            verdict,
            Verdict::ResourceExhausted(ResourceBudgetReason::StepLimitExceeded)
        ),
        "Expected ResourceExhausted, got {verdict:?}"
    );

    // STRICT CHECK: ResourceExhausted MUST map to Outcome::Unknown, NEVER Refuted or Rejected
    let outcome = verdict.outcome();
    assert_eq!(
        outcome,
        Some(Outcome::Unknown),
        "ResourceExhausted MUST map to Outcome::Unknown"
    );
    assert_ne!(
        outcome,
        Some(Outcome::Refuted),
        "ResourceExhausted must NEVER map to Refuted"
    );
}

// =========================================================================
// Slice 2b Tests (ADR-0003 §5 — Existentials, Equality, Trans-Pres)
// =========================================================================

use brix_kernel::{instantiate, ObjectTerm};

fn sample_obj_const_a() -> ObjectTerm {
    ObjectTerm::Const(PropositionId::from_canon(b"obj_a"))
}

fn sample_obj_const_b() -> ObjectTerm {
    ObjectTerm::Const(PropositionId::from_canon(b"obj_b"))
}

fn sample_obj_const_c() -> ObjectTerm {
    ObjectTerm::Const(PropositionId::from_canon(b"obj_c"))
}

#[test]
fn test_refl_equal_terms_returns_accepted() {
    let ctx = sample_context_a();
    let a = sample_obj_const_a();
    let goal = Prop::Eq(a.clone(), a.clone());

    let term = ExplicitTerm::new(ctx, TermKind::Refl(a));
    let budget = Budget::new(100, 100);
    let verdict = acceptance(&ctx, &goal, &term, budget);

    assert!(
        matches!(verdict, Verdict::Accepted(_)),
        "Refl on equal terms should be Accepted"
    );
}

#[test]
fn test_refl_non_equal_terms_returns_rejected() {
    let ctx = sample_context_a();
    let a = sample_obj_const_a();
    let b = sample_obj_const_b();
    let goal = Prop::Eq(a.clone(), b);

    let term = ExplicitTerm::new(ctx, TermKind::Refl(a));
    let budget = Budget::new(100, 100);
    let verdict = acceptance(&ctx, &goal, &term, budget);

    assert!(
        matches!(verdict, Verdict::Rejected(_)),
        "Refl on non-equal terms should be Rejected"
    );
}

#[test]
fn test_subst_valid_returns_accepted() {
    let ctx = sample_context_a();
    let a = sample_obj_const_a();
    let b = sample_obj_const_b();
    let c = sample_obj_const_c();

    let motive = Prop::Eq(ObjectTerm::BoundVar(0), c.clone());
    let goal = instantiate(&motive, &b);

    let full_goal = Prop::Impl(
        Box::new(Prop::Eq(a.clone(), b.clone())),
        Box::new(Prop::Impl(
            Box::new(instantiate(&motive, &a)),
            Box::new(goal),
        )),
    );

    let term = ExplicitTerm::new(
        ctx,
        TermKind::Lam {
            var_name: Some("h_eq".into()),
            body: Box::new(TermKind::Lam {
                var_name: Some("h_sub".into()),
                body: Box::new(TermKind::Subst {
                    eq: Box::new(TermKind::Hyp(Var::Named("h_eq".into()))),
                    motive: Box::new(motive),
                    sub: Box::new(TermKind::Hyp(Var::Named("h_sub".into()))),
                }),
            }),
        },
    );

    let budget = Budget::new(100, 100);
    let verdict = acceptance(&ctx, &full_goal, &term, budget);

    assert!(
        matches!(verdict, Verdict::Accepted(_)),
        "Valid Subst proof should be Accepted, got {verdict:?}"
    );
}

#[test]
fn test_subst_producing_wrong_instantiation_returns_rejected() {
    let ctx = sample_context_a();
    let a = sample_obj_const_a();
    let b = sample_obj_const_b();
    let c = sample_obj_const_c();

    let motive = Prop::Eq(ObjectTerm::BoundVar(0), c.clone());
    let wrong_goal = instantiate(&motive, &a);

    let full_goal = Prop::Impl(
        Box::new(Prop::Eq(a.clone(), b.clone())),
        Box::new(Prop::Impl(
            Box::new(instantiate(&motive, &a)),
            Box::new(wrong_goal),
        )),
    );

    let term = ExplicitTerm::new(
        ctx,
        TermKind::Lam {
            var_name: Some("h_eq".into()),
            body: Box::new(TermKind::Lam {
                var_name: Some("h_sub".into()),
                body: Box::new(TermKind::Subst {
                    eq: Box::new(TermKind::Hyp(Var::Named("h_eq".into()))),
                    motive: Box::new(motive),
                    sub: Box::new(TermKind::Hyp(Var::Named("h_sub".into()))),
                }),
            }),
        },
    );

    let budget = Budget::new(100, 100);
    let verdict = acceptance(&ctx, &full_goal, &term, budget);

    assert!(
        matches!(verdict, Verdict::Rejected(_)),
        "Subst producing wrong instantiation should be Rejected"
    );
}

#[test]
fn test_pack_valid_witness_returns_accepted() {
    let ctx = sample_context_a();
    let a = sample_obj_const_a();

    let pred = Prop::Eq(ObjectTerm::BoundVar(0), a.clone());
    let goal = Prop::Exists(Box::new(pred));

    let term = ExplicitTerm::new(
        ctx,
        TermKind::Pack {
            witness: a.clone(),
            body_proof: Box::new(TermKind::Refl(a)),
        },
    );

    let budget = Budget::new(100, 100);
    let verdict = acceptance(&ctx, &goal, &term, budget);

    assert!(
        matches!(verdict, Verdict::Accepted(_)),
        "Valid Pack proof should be Accepted"
    );
}

#[test]
fn test_unpack_valid_returns_accepted() {
    let ctx = sample_context_a();
    let a = sample_obj_const_a();
    let r = sample_prop_p();

    let pred = Prop::Eq(ObjectTerm::BoundVar(0), a.clone());
    let ex_prop = Prop::Exists(Box::new(pred));

    let goal = Prop::Impl(
        Box::new(ex_prop),
        Box::new(Prop::Impl(Box::new(r.clone()), Box::new(r))),
    );

    let term = ExplicitTerm::new(
        ctx,
        TermKind::Lam {
            var_name: Some("h_ex".into()),
            body: Box::new(TermKind::Lam {
                var_name: Some("h_r".into()),
                body: Box::new(TermKind::Unpack {
                    scrutinee: Box::new(TermKind::Hyp(Var::Named("h_ex".into()))),
                    obj_var: Some("x".into()),
                    proof_var: Some("h_proof".into()),
                    body: Box::new(TermKind::Hyp(Var::Named("h_r".into()))),
                }),
            }),
        },
    );

    let budget = Budget::new(100, 100);
    let verdict = acceptance(&ctx, &goal, &term, budget);

    assert!(
        matches!(verdict, Verdict::Accepted(_)),
        "Valid Unpack proof without witness escape should be Accepted, got {verdict:?}"
    );
}

#[test]
fn test_unpack_body_leaking_eigenvariable_returns_malformed() {
    let ctx = sample_context_a();
    let a = sample_obj_const_a();

    let pred = Prop::Eq(ObjectTerm::BoundVar(0), a.clone());
    let ex_prop = Prop::Exists(Box::new(pred));

    // Unpack body returns `h_proof` (which has type `x_fresh = a`).
    let unpack_term = TermKind::Unpack {
        scrutinee: Box::new(TermKind::Hyp(Var::Named("h_ex".into()))),
        obj_var: Some("x".into()),
        proof_var: Some("h_proof".into()),
        body: Box::new(TermKind::Hyp(Var::Named("h_proof".into()))),
    };

    // Use `unpack_term` as discriminant of a Case, forcing infer_type on Unpack.
    let term = ExplicitTerm::new(
        ctx,
        TermKind::Lam {
            var_name: Some("h_ex".into()),
            body: Box::new(TermKind::Case {
                discriminant: Box::new(unpack_term),
                left_var: Some("l".into()),
                left_body: Box::new(TermKind::Hyp(Var::Named("h_ex".into()))),
                right_var: Some("r".into()),
                right_body: Box::new(TermKind::Hyp(Var::Named("h_ex".into()))),
            }),
        },
    );

    let budget = Budget::new(100, 100);
    let goal = Prop::Impl(Box::new(ex_prop.clone()), Box::new(ex_prop));
    let verdict = acceptance(&ctx, &goal, &term, budget);

    assert!(
        matches!(verdict, Verdict::Malformed(_)),
        "Unpack leaking eigenvariable should return Malformed (witness escape), got {verdict:?}"
    );
}

#[test]
fn test_trans_pres_valid_returns_accepted() {
    let ctx = sample_context_a();
    let w = ObjectTerm::Const(PropositionId::from_canon(b"w"));
    let x = sample_obj_const_a();
    let y = sample_obj_const_b();
    let c = sample_obj_const_c();

    let motive = Prop::Eq(ObjectTerm::BoundVar(0), c.clone());

    let realizes_prop = Prop::Realizes(w.clone(), x.clone(), y.clone());
    let preserves_prop = Prop::Preserves(w.clone(), Box::new(motive.clone()));
    let sub_prop = instantiate(&motive, &x);
    let goal_prop = instantiate(&motive, &y);

    let full_goal = Prop::Impl(
        Box::new(realizes_prop),
        Box::new(Prop::Impl(
            Box::new(preserves_prop),
            Box::new(Prop::Impl(Box::new(sub_prop), Box::new(goal_prop))),
        )),
    );

    let term = ExplicitTerm::new(
        ctx,
        TermKind::Lam {
            var_name: Some("h_realizes".into()),
            body: Box::new(TermKind::Lam {
                var_name: Some("h_preserves".into()),
                body: Box::new(TermKind::Lam {
                    var_name: Some("h_sub".into()),
                    body: Box::new(TermKind::Pres {
                        realizes: Box::new(TermKind::Hyp(Var::Named("h_realizes".into()))),
                        preserves: Box::new(TermKind::Hyp(Var::Named("h_preserves".into()))),
                        motive: Box::new(motive),
                        sub: Box::new(TermKind::Hyp(Var::Named("h_sub".into()))),
                    }),
                }),
            }),
        },
    );

    let budget = Budget::new(100, 100);
    let verdict = acceptance(&ctx, &full_goal, &term, budget);

    assert!(
        matches!(verdict, Verdict::Accepted(_)),
        "Valid Trans-Pres proof should be Accepted, got {verdict:?}"
    );
}

#[test]
fn test_trans_pres_without_matching_preserves_premise_returns_rejected() {
    let ctx = sample_context_a();
    let w = ObjectTerm::Const(PropositionId::from_canon(b"w"));
    let w_other = ObjectTerm::Const(PropositionId::from_canon(b"w_other"));
    let x = sample_obj_const_a();
    let y = sample_obj_const_b();
    let c = sample_obj_const_c();

    let motive = Prop::Eq(ObjectTerm::BoundVar(0), c.clone());

    let realizes_prop = Prop::Realizes(w.clone(), x.clone(), y.clone());
    let wrong_preserves_prop = Prop::Preserves(w_other, Box::new(motive.clone()));
    let sub_prop = instantiate(&motive, &x);
    let goal_prop = instantiate(&motive, &y);

    let full_goal = Prop::Impl(
        Box::new(realizes_prop),
        Box::new(Prop::Impl(
            Box::new(wrong_preserves_prop),
            Box::new(Prop::Impl(Box::new(sub_prop), Box::new(goal_prop))),
        )),
    );

    let term = ExplicitTerm::new(
        ctx,
        TermKind::Lam {
            var_name: Some("h_realizes".into()),
            body: Box::new(TermKind::Lam {
                var_name: Some("h_preserves".into()),
                body: Box::new(TermKind::Lam {
                    var_name: Some("h_sub".into()),
                    body: Box::new(TermKind::Pres {
                        realizes: Box::new(TermKind::Hyp(Var::Named("h_realizes".into()))),
                        preserves: Box::new(TermKind::Hyp(Var::Named("h_preserves".into()))),
                        motive: Box::new(motive),
                        sub: Box::new(TermKind::Hyp(Var::Named("h_sub".into()))),
                    }),
                }),
            }),
        },
    );

    let budget = Budget::new(100, 100);
    let verdict = acceptance(&ctx, &full_goal, &term, budget);

    assert!(
        matches!(verdict, Verdict::Rejected(_)),
        "Trans-Pres without matching Preserves premise should be Rejected"
    );
}

#[test]
fn test_instantiate_capture_avoidance_nested_exists() {
    let target_var_at_depth_2 = ObjectTerm::BoundVar(2);
    let inner_bound_var = ObjectTerm::BoundVar(0);
    let inner_eq = Prop::Eq(target_var_at_depth_2, inner_bound_var);
    let inner_exists = Prop::Exists(Box::new(inner_eq));
    let outer_exists = Prop::Exists(Box::new(inner_exists));

    let replacement = ObjectTerm::BoundVar(0);
    let instantiated = instantiate(&outer_exists, &replacement);

    let expected = Prop::Exists(Box::new(Prop::Exists(Box::new(Prop::Eq(
        ObjectTerm::BoundVar(2),
        ObjectTerm::BoundVar(0),
    )))));

    assert_eq!(instantiated, expected);
}
