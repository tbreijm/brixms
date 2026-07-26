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
