//! Regression coverage for the pinned arithmetic-input bridge boundary.

use brix_kernel::{
    acceptance, typing_arith_input_v1, ArithOperatorV1, ArithTypingInputV1, Budget, ExplicitTerm,
    NumericTypeNameV1, ObjectTerm, PinnedArithOp, PinnedNumericTy, Prop, RejectionReason,
    ResourceBudgetReason, TermKind, Verdict,
};
use brix_semantic::{ContextId, GeneratorId, PropositionId};

fn ctx() -> ContextId {
    ContextId::from_canon(b"arith_bridge_boundary")
}
fn atom(id: PropositionId) -> ObjectTerm {
    ObjectTerm::Const(id)
}
fn prim(
    relation: brix_kernel::PrimitiveRelationId,
    src: ObjectTerm,
    dst: ObjectTerm,
) -> ExplicitTerm {
    ExplicitTerm::new(ctx(), TermKind::PrimRealizes { relation, src, dst })
}
fn input() -> ArithTypingInputV1 {
    ArithTypingInputV1 {
        operator: ArithOperatorV1::Add,
        lhs_type: NumericTypeNameV1::Int,
        rhs_type: NumericTypeNameV1::Int,
        lhs_promotion_path: vec![],
        rhs_promotion_path: vec![],
    }
}
fn bridge_src(op: PinnedArithOp, lhs: PinnedNumericTy, rhs: PinnedNumericTy) -> ObjectTerm {
    ObjectTerm::Tensor(
        Box::new(atom(op.proposition_id())),
        Box::new(ObjectTerm::Tensor(
            Box::new(atom(lhs.proposition_id())),
            Box::new(atom(rhs.proposition_id())),
        )),
    )
}
fn goal_for(generator: &str, src: ObjectTerm, dst: ObjectTerm) -> Prop {
    Prop::Realizes(
        atom(PropositionId(GeneratorId::named(generator).digest())),
        src,
        dst,
    )
}

#[test]
fn input_relation_never_accepts_a_wrapped_digest_as_its_structured_source() {
    let relation = brix_kernel::resolve_primitive_relation(&typing_arith_input_v1()).unwrap();
    assert!(!relation.rows.is_empty());
    for (src, dst) in &relation.rows {
        let wrapped = atom(*src);
        let destination = atom(*dst);
        assert!(!relation.accepts_endpoint_shapes(&wrapped, &destination));
        let verdict = acceptance(
            &ctx(),
            &goal_for(
                "type.rule.arith.input@1",
                wrapped.clone(),
                destination.clone(),
            ),
            &prim(typing_arith_input_v1(), wrapped, destination),
            Budget::new(100, 32),
        );
        assert!(
            matches!(
                verdict,
                Verdict::Rejected(RejectionReason::TypeMismatch { .. })
            ),
            "wrapped source digest must fail the endpoint shape check: {verdict:?}"
        );
    }
}

#[test]
fn bridge_accepts_only_the_ordered_pinned_add_int_int_shape() {
    let relation = brix_kernel::resolve_primitive_relation(&typing_arith_input_v1()).unwrap();
    let src = bridge_src(
        PinnedArithOp::Add,
        PinnedNumericTy::Int,
        PinnedNumericTy::Int,
    );
    let dst = atom(PropositionId(input().config_id().digest()));
    assert!(relation.accepts_endpoint_shapes(&src, &dst));
    assert!(relation.admits(
        &PropositionId::of(&src),
        match &dst {
            ObjectTerm::Const(id) => id,
            _ => unreachable!(),
        }
    ));
    let verdict = acceptance(
        &ctx(),
        &goal_for("type.rule.arith.input@1", src.clone(), dst.clone()),
        &prim(typing_arith_input_v1(), src, dst.clone()),
        Budget::new(200, 32),
    );
    assert!(
        matches!(verdict, Verdict::Accepted(_)),
        "valid bridge row: {verdict:?}"
    );

    let variants = [
        (
            "wrong operator",
            bridge_src(
                PinnedArithOp::Sub,
                PinnedNumericTy::Int,
                PinnedNumericTy::Int,
            ),
        ),
        (
            "swapped distinct operands",
            bridge_src(
                PinnedArithOp::Add,
                PinnedNumericTy::Nat,
                PinnedNumericTy::Int,
            ),
        ),
        (
            "reassociated product",
            ObjectTerm::Tensor(
                Box::new(ObjectTerm::Tensor(
                    Box::new(atom(PinnedArithOp::Add.proposition_id())),
                    Box::new(atom(PinnedNumericTy::Int.proposition_id())),
                )),
                Box::new(atom(PinnedNumericTy::Int.proposition_id())),
            ),
        ),
        (
            "nested composition",
            ObjectTerm::Compose(
                Box::new(bridge_src(
                    PinnedArithOp::Add,
                    PinnedNumericTy::Int,
                    PinnedNumericTy::Int,
                )),
                Box::new(atom(PropositionId::from_canon(b"x"))),
            ),
        ),
        ("bound variable", ObjectTerm::BoundVar(0)),
        ("identity wrapper", ObjectTerm::Id(Box::new(src_shape()))),
    ];
    for (label, bad_src) in variants {
        let v = acceptance(
            &ctx(),
            &goal_for("type.rule.arith.input@1", bad_src.clone(), dst.clone()),
            &prim(typing_arith_input_v1(), bad_src, dst.clone()),
            Budget::new(200, 32),
        );
        assert!(
            !matches!(v, Verdict::Accepted(_)),
            "{label} unexpectedly accepted: {v:?}"
        );
    }
    let other_dst = atom(PropositionId::from_canon(b"wrong-dst"));
    let v = acceptance(
        &ctx(),
        &goal_for("type.rule.arith.input@1", src_shape(), other_dst.clone()),
        &prim(typing_arith_input_v1(), src_shape(), other_dst),
        Budget::new(200, 32),
    );
    assert!(
        !matches!(v, Verdict::Accepted(_)),
        "wrong destination accepted: {v:?}"
    );
    let unknown = brix_kernel::PrimitiveRelationId(brix_canon::Digest::of(
        brix_canon::Domain::Value,
        b"unknown",
    ));
    let v = acceptance(
        &ctx(),
        &goal_for("type.rule.arith.input@1", src_shape(), dst.clone()),
        &prim(unknown, src_shape(), dst),
        Budget::new(200, 32),
    );
    assert!(matches!(
        v,
        Verdict::Rejected(RejectionReason::UnknownPrimitiveRelation(_))
    ));
}

fn src_shape() -> ObjectTerm {
    bridge_src(
        PinnedArithOp::Add,
        PinnedNumericTy::Int,
        PinnedNumericTy::Int,
    )
}

fn huge_tensor(depth: usize) -> ObjectTerm {
    let mut term = atom(PropositionId::from_canon(b"leaf"));
    for _ in 0..depth {
        term = ObjectTerm::Tensor(
            Box::new(term),
            Box::new(atom(PropositionId::from_canon(b"leaf"))),
        );
    }
    term
}

#[test]
fn hostile_compose_and_identity_inputs_stay_within_small_budgets() {
    let huge = huge_tensor(256);
    let composed = ObjectTerm::Compose(Box::new(huge), Box::new(src_shape()));
    let dst = atom(PropositionId::from_canon(b"dst"));
    let verdict = acceptance(
        &ctx(),
        &goal_for("type.rule.arith.input@1", composed.clone(), dst.clone()),
        &prim(typing_arith_input_v1(), composed, dst),
        Budget::new(3, 2),
    );
    assert!(
        matches!(
            verdict,
            Verdict::ResourceExhausted(_) | Verdict::Rejected(_) | Verdict::Malformed(_)
        ),
        "unexpected: {verdict:?}"
    );

    if let Verdict::Rejected(RejectionReason::TypeMismatch { found, .. }) = &verdict {
        assert!(
            found.len() < 100,
            "unchecked children leaked into diagnostics"
        );
    }
    let atom_target = atom(PropositionId::from_canon(b"target"));
    let bad_endpoint = huge_tensor(256);
    let mismatched = Prop::Realizes(
        ObjectTerm::Id(Box::new(atom_target.clone())),
        bad_endpoint,
        atom_target.clone(),
    );
    let term = ExplicitTerm::new(ctx(), TermKind::RealizesId(atom_target));
    let Verdict::Rejected(RejectionReason::TypeMismatch { found, .. }) =
        acceptance(&ctx(), &mismatched, &term, Budget::new(3, 2))
    else {
        panic!("mismatched endpoint must reject without rendering children")
    };
    assert!(found.len() < 100);

    for (target, expected, witness) in [
        (
            ObjectTerm::BoundVar(0),
            ObjectTerm::BoundVar(0),
            ObjectTerm::Id(Box::new(ObjectTerm::BoundVar(1))),
        ),
        (
            atom(PropositionId::from_canon(b"target")),
            atom(PropositionId::from_canon(b"other")),
            ObjectTerm::Id(Box::new(atom(PropositionId::from_canon(b"target")))),
        ),
        (
            huge_tensor(256),
            atom(PropositionId::from_canon(b"wrong")),
            ObjectTerm::Tensor(Box::new(huge_tensor(256)), Box::new(huge_tensor(256))),
        ),
    ] {
        let proposition = Prop::Realizes(witness, target.clone(), expected);
        let proof = ExplicitTerm::new(ctx(), TermKind::RealizesId(target));
        let v = acceptance(&ctx(), &proposition, &proof, Budget::new(2, 3));
        assert!(
            matches!(
                v,
                Verdict::ResourceExhausted(_) | Verdict::Rejected(_) | Verdict::Malformed(_)
            ),
            "unexpected: {v:?}"
        );
        std::mem::forget(proposition);
        std::mem::forget(proof);
    }

    let deep = huge_tensor(256);
    let witness = ObjectTerm::Id(Box::new(deep.clone()));
    let proposition = Prop::Realizes(witness, deep.clone(), deep.clone());
    let proof = ExplicitTerm::new(ctx(), TermKind::RealizesId(deep));
    let v = acceptance(
        &ctx(),
        &proposition,
        &proof,
        Budget::new(usize::MAX, usize::MAX),
    );
    assert!(
        matches!(
            v,
            Verdict::ResourceExhausted(ResourceBudgetReason::DepthLimitExceeded)
        ),
        "hard depth guard: {v:?}"
    );
    std::mem::forget(proposition);
    std::mem::forget(proof);
}
