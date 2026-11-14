//! Structural identity realization rule (ADR-0025 §10.1).
//!
//! Checks the kernel's enforcement of `TermKind::RealizesId(t)`:
//! - ⊢ Realizes(id(t), t, t)
//! - Source and target endpoints must match `t`
//! - Witness must match `id(t)` (inductively over products)
//! - Unsupported shapes (e.g. `Compose`, `BoundVar`, error sentinels) are strictly rejected
//! - Traversal operations and product branches are strictly resource-bounded
//! - Budget steps and depth are enforced before and during traversal

use brix_kernel::{
    acceptance, Budget, ExplicitTerm, ObjectTerm, Prop, RejectionReason, ResourceBudgetReason,
    TermKind, Verdict,
};
use brix_semantic::{ContextId, PropositionId};

fn ctx() -> ContextId {
    ContextId::from_canon(b"realizes_id_tests")
}

fn budget() -> Budget {
    Budget::new(10_000, 256)
}

fn obj(name: &[u8]) -> ObjectTerm {
    ObjectTerm::Const(PropositionId::from_canon(name))
}

#[test]
fn structural_identity_atom_is_accepted() {
    let t = obj(b"AtomType");
    let witness = ObjectTerm::Id(Box::new(t.clone()));
    let goal = Prop::Realizes(witness, t.clone(), t.clone());
    let proof = ExplicitTerm::new(ctx(), TermKind::RealizesId(t));

    let verdict = acceptance(&ctx(), &goal, &proof, budget());
    assert!(
        matches!(verdict, Verdict::Accepted(_)),
        "atomic RealizesId must be Accepted, got {verdict:?}"
    );
}

#[test]
fn structural_identity_tensor_product_is_accepted() {
    let t = ObjectTerm::Tensor(Box::new(obj(b"Left")), Box::new(obj(b"Right")));
    let witness = ObjectTerm::Id(Box::new(t.clone()));
    let goal = Prop::Realizes(witness, t.clone(), t.clone());
    let proof = ExplicitTerm::new(ctx(), TermKind::RealizesId(t));

    let verdict = acceptance(&ctx(), &goal, &proof, budget());
    assert!(
        matches!(verdict, Verdict::Accepted(_)),
        "product RealizesId must be Accepted, got {verdict:?}"
    );
}

#[test]
fn structural_identity_tensor_of_id_witness_is_accepted() {
    let t_l = obj(b"Left");
    let t_r = obj(b"Right");
    let t = ObjectTerm::Tensor(Box::new(t_l.clone()), Box::new(t_r.clone()));
    let witness = ObjectTerm::Tensor(
        Box::new(ObjectTerm::Id(Box::new(t_l))),
        Box::new(ObjectTerm::Id(Box::new(t_r))),
    );
    let goal = Prop::Realizes(witness, t.clone(), t.clone());
    let proof = ExplicitTerm::new(ctx(), TermKind::RealizesId(t));

    let verdict = acceptance(&ctx(), &goal, &proof, budget());
    assert!(
        matches!(verdict, Verdict::Accepted(_)),
        "product RealizesId with Tensor(Id, Id) witness must be Accepted, got {verdict:?}"
    );
}

#[test]
fn rejects_source_endpoint_mismatch() {
    let t = obj(b"ExpectedType");
    let wrong = obj(b"WrongType");
    let witness = ObjectTerm::Id(Box::new(t.clone()));
    let goal = Prop::Realizes(witness, wrong, t.clone());
    let proof = ExplicitTerm::new(ctx(), TermKind::RealizesId(t));

    let verdict = acceptance(&ctx(), &goal, &proof, budget());
    assert!(
        matches!(
            verdict,
            Verdict::Rejected(RejectionReason::TypeMismatch { .. })
        ),
        "mismatched source must be Rejected with TypeMismatch, got {verdict:?}"
    );
}

#[test]
fn rejects_target_endpoint_mismatch() {
    let t = obj(b"ExpectedType");
    let wrong = obj(b"WrongType");
    let witness = ObjectTerm::Id(Box::new(t.clone()));
    let goal = Prop::Realizes(witness, t.clone(), wrong);
    let proof = ExplicitTerm::new(ctx(), TermKind::RealizesId(t));

    let verdict = acceptance(&ctx(), &goal, &proof, budget());
    assert!(
        matches!(
            verdict,
            Verdict::Rejected(RejectionReason::TypeMismatch { .. })
        ),
        "mismatched target must be Rejected with TypeMismatch, got {verdict:?}"
    );
}

#[test]
fn rejects_witness_digest_mismatch() {
    let t = obj(b"ExpectedType");
    let other = obj(b"OtherType");
    let forged_witness = ObjectTerm::Id(Box::new(other));
    let goal = Prop::Realizes(forged_witness, t.clone(), t.clone());
    let proof = ExplicitTerm::new(ctx(), TermKind::RealizesId(t));

    let verdict = acceptance(&ctx(), &goal, &proof, budget());
    assert!(
        matches!(
            verdict,
            Verdict::Rejected(RejectionReason::TypeMismatch { .. })
        ),
        "forged witness must be Rejected with TypeMismatch, got {verdict:?}"
    );
}

#[test]
fn charges_budget_step() {
    let t = obj(b"AtomType");
    let witness = ObjectTerm::Id(Box::new(t.clone()));
    let goal = Prop::Realizes(witness, t.clone(), t.clone());
    let proof = ExplicitTerm::new(ctx(), TermKind::RealizesId(t));

    // Budget of 0 steps must fail with StepLimitExceeded
    let tight_budget = Budget::new(0, 256);
    let verdict = acceptance(&ctx(), &goal, &proof, tight_budget);
    assert_eq!(
        verdict,
        Verdict::ResourceExhausted(ResourceBudgetReason::StepLimitExceeded),
        "zero budget must be ResourceExhausted(StepLimitExceeded), got {verdict:?}"
    );
}

/// [P1 Probe] Unsupported identity target shapes (e.g. `Compose`) must be rejected
/// before digest comparison; never use an error sentinel as an admissible witness identity.
#[test]
fn rejects_unsupported_identity_target_compose() {
    let a = obj(b"A");
    let b = obj(b"B");
    let c = obj(b"C");
    let d = obj(b"D");

    let comp_ab = ObjectTerm::Compose(Box::new(a), Box::new(b));
    let comp_cd = ObjectTerm::Compose(Box::new(c), Box::new(d));

    let goal = Prop::Realizes(
        ObjectTerm::Id(Box::new(comp_cd)),
        comp_ab.clone(),
        comp_ab.clone(),
    );
    let proof = ExplicitTerm::new(ctx(), TermKind::RealizesId(comp_ab));

    let verdict = acceptance(&ctx(), &goal, &proof, budget());
    assert!(
        matches!(
            verdict,
            Verdict::Rejected(RejectionReason::TypeMismatch { .. })
        ),
        "Id(Compose(c, d)) for Compose(a, b) must be Rejected with TypeMismatch, got {verdict:?}"
    );
}

/// [P1 Probe] Bound variables anywhere inside identity targets must be rejected.
#[test]
fn rejects_bound_var_in_identity_target() {
    let bv = ObjectTerm::BoundVar(0);
    let goal = Prop::Realizes(ObjectTerm::Id(Box::new(bv.clone())), bv.clone(), bv.clone());
    let proof = ExplicitTerm::new(ctx(), TermKind::RealizesId(bv));

    let verdict = acceptance(&ctx(), &goal, &proof, budget());
    assert!(
        matches!(
            verdict,
            Verdict::Rejected(RejectionReason::TypeMismatch { .. })
        ),
        "BoundVar in identity target must be Rejected with TypeMismatch, got {verdict:?}"
    );
}

/// [P1 Probe] Identity checking respects depth bounds.
/// A 256-level product identity must fail Budget::new(3, 1) with DepthLimitExceeded.
#[test]
fn identity_checking_fails_shallow_depth_budget() {
    let mut deep_term = obj(b"Leaf");
    for i in 0..256 {
        deep_term = ObjectTerm::Tensor(
            Box::new(deep_term),
            Box::new(ObjectTerm::Const(PropositionId::from_canon(
                format!("r_{i}").as_bytes(),
            ))),
        );
    }

    let witness = ObjectTerm::Id(Box::new(deep_term.clone()));
    let goal = Prop::Realizes(witness, deep_term.clone(), deep_term.clone());
    let proof = ExplicitTerm::new(ctx(), TermKind::RealizesId(deep_term));

    let shallow_budget = Budget::new(3, 1);
    let verdict = acceptance(&ctx(), &goal, &proof, shallow_budget);
    assert_eq!(
        verdict,
        Verdict::ResourceExhausted(ResourceBudgetReason::DepthLimitExceeded),
        "256-level product with Budget::new(3, 1) must fail with DepthLimitExceeded, got {verdict:?}"
    );
}

/// [P1 Probe] Identity checking respects step bounds.
/// A 256-level product identity must fail Budget::new(3, 300) with StepLimitExceeded.
#[test]
fn identity_checking_fails_shallow_step_budget() {
    let mut deep_term = obj(b"Leaf");
    for i in 0..256 {
        deep_term = ObjectTerm::Tensor(
            Box::new(deep_term),
            Box::new(ObjectTerm::Const(PropositionId::from_canon(
                format!("r_{i}").as_bytes(),
            ))),
        );
    }

    let witness = ObjectTerm::Id(Box::new(deep_term.clone()));
    let goal = Prop::Realizes(witness, deep_term.clone(), deep_term.clone());
    let proof = ExplicitTerm::new(ctx(), TermKind::RealizesId(deep_term));

    let small_step_budget = Budget::new(3, 300);
    let verdict = acceptance(&ctx(), &goal, &proof, small_step_budget);
    assert_eq!(
        verdict,
        Verdict::ResourceExhausted(ResourceBudgetReason::StepLimitExceeded),
        "256-level product with Budget::new(3, 300) must fail with StepLimitExceeded, got {verdict:?}"
    );
}
