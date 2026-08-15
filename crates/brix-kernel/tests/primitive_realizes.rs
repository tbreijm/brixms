//! `PrimRealizes` — the kernel-side behaviour of ADR-0015 ⟨D-PRIM⟩'s
//! zero-premise primitive-realization rule.
//!
//! These tests drive the real `acceptance` entry point. They are the kernel's
//! half of Stage B; the half that runs the real `soc-regimes` generator and
//! submits its real leaf lives in that crate's tests, because `brix-kernel` may
//! not depend on a regime (TCB boundary, `scripts/check_tcb_dependencies.py`).
//!
//! What is being pinned here, in ADR order:
//!
//! - the rule synthesizes `Realizes(g, src, dst)` for an exact row and nothing
//!   else (§2 ⟨D-PRIM⟩);
//! - the **relation identity fixes the generator; the caller does not supply
//!   it** — so a caller cannot name a different generator and have it honoured;
//! - the rule is **closed**: it consults no hypothesis context, so nothing in Γ
//!   can influence the result;
//! - non-membership and unknown relation ids are **absence, never refutation**
//!   (§8.8) — `Rejected`, never `Refuted`, and never an `Accepted` fallback;
//! - the new `TermKind` ordinal is appended at 17 and no existing ordinal moved
//!   (§7), so existing certificate bytes are unchanged.

use brix_canon::{CanonWriter, Canonical, Digest, Domain};
use brix_kernel::{
    acceptance, resolve_primitive_relation, typing_arith_v2, ArithOperatorV1,
    ArithTypingInputV1, Budget, CoercionEdgeV1, CoercionKind, ExplicitTerm, NumericResultTypeV1,
    NumericTypeNameV1, ObjectTerm, PrimitiveRelationId, Prop, RejectionReason, TermKind, Var,
    Verdict,
};
use brix_semantic::{ContextId, GeneratorId, PropositionId};

fn ctx() -> ContextId {
    ContextId::from_canon(b"prim_realizes_tests")
}

fn budget() -> Budget {
    Budget::new(10_000, 256)
}

fn atom_term(id: PropositionId) -> ObjectTerm {
    ObjectTerm::Const(id)
}

/// `1 + 2`'s source object: `Add`, both operands `Int`, no promotion.
fn add_int_int() -> ArithTypingInputV1 {
    ArithTypingInputV1 {
        operator: ArithOperatorV1::Add,
        lhs_type: NumericTypeNameV1::Int,
        rhs_type: NumericTypeNameV1::Int,
        lhs_promotion_path: Vec::new(),
        rhs_promotion_path: Vec::new(),
    }
}

fn result(name: NumericTypeNameV1) -> NumericResultTypeV1 {
    NumericResultTypeV1 { name }
}

fn src_of(input: &ArithTypingInputV1) -> ObjectTerm {
    atom_term(PropositionId(input.config_id().digest()))
}

fn dst_of(r: &NumericResultTypeV1) -> ObjectTerm {
    atom_term(PropositionId(r.config_id().digest()))
}

/// The `Realizes` proposition the rule should synthesize for `(src, dst)`.
fn expected_realizes(src: ObjectTerm, dst: ObjectTerm) -> Prop {
    Prop::Realizes(
        atom_term(PropositionId(
            GeneratorId::named("type.rule.arith@1").digest(),
        )),
        src,
        dst,
    )
}

fn prim(src: ObjectTerm, dst: ObjectTerm) -> TermKind {
    // The *current* relation (ADR-0015 Stage E), and the only one compiled in:
    // `TypingArithV1` was retired (ADR-0024 §3), which
    // `the_retired_relation_does_not_resolve` covers.
    TermKind::PrimRealizes {
        relation: typing_arith_v2(),
        src,
        dst,
    }
}

/// The rule accepts an exact row, and the proposition it establishes names the
/// relation's generator and the two endpoints — nothing else.
#[test]
fn an_exact_row_is_accepted_and_names_the_relations_generator() {
    let src = src_of(&add_int_int());
    let dst = dst_of(&result(NumericTypeNameV1::Int));

    let verdict = acceptance(
        &ctx(),
        &expected_realizes(src.clone(), dst.clone()),
        &ExplicitTerm::new(ctx(), prim(src, dst)),
        budget(),
    );
    assert!(
        matches!(verdict, Verdict::Accepted(_)),
        "an exact row must be Accepted, got {verdict:?}"
    );
}

/// **The caller does not supply the generator** (⟨D-PRIM⟩). Naming a different
/// one in the goal must not be honoured: the synthesized proposition comes from
/// the resolved relation, and the comparison then fails.
#[test]
fn the_caller_cannot_supply_the_generator() {
    let src = src_of(&add_int_int());
    let dst = dst_of(&result(NumericTypeNameV1::Int));

    let forged = Prop::Realizes(
        atom_term(PropositionId(
            // The *input bridge*'s generator, not the arithmetic rule's.
            GeneratorId::named("type.rule.arith.input@1").digest(),
        )),
        src.clone(),
        dst.clone(),
    );

    let verdict = acceptance(
        &ctx(),
        &forged,
        &ExplicitTerm::new(ctx(), prim(src, dst)),
        budget(),
    );
    assert!(
        !matches!(verdict, Verdict::Accepted(_)),
        "a goal naming another generator must not be Accepted, got {verdict:?}"
    );
}

/// The rule is closed: it reads no hypothesis context. Proved by putting an
/// unrelated assumption in Γ (via a `Lam`) and confirming the inner
/// `PrimRealizes` still decides the same way.
#[test]
fn the_rule_consults_no_hypothesis_context() {
    let src = src_of(&add_int_int());
    let dst = dst_of(&result(NumericTypeNameV1::Int));
    let junk = Prop::Atom(PropositionId::from_canon(b"an unrelated assumption"));

    // Γ non-empty: `junk -> Realizes(...)`, proved by ignoring the hypothesis.
    let under_assumption = acceptance(
        &ctx(),
        &Prop::Impl(
            Box::new(junk.clone()),
            Box::new(expected_realizes(src.clone(), dst.clone())),
        ),
        &ExplicitTerm::new(
            ctx(),
            TermKind::Lam {
                var_name: Some("h".into()),
                body: Box::new(prim(src.clone(), dst.clone())),
            },
        ),
        budget(),
    );
    assert!(matches!(under_assumption, Verdict::Accepted(_)));

    // And a non-row is still refused with the assumption in scope — Γ cannot
    // be used to smuggle a row in.
    let non_row = acceptance(
        &ctx(),
        &Prop::Impl(
            Box::new(junk),
            Box::new(expected_realizes(
                src.clone(),
                dst_of(&result(NumericTypeNameV1::Float)),
            )),
        ),
        &ExplicitTerm::new(
            ctx(),
            TermKind::Lam {
                var_name: Some("h".into()),
                body: Box::new(prim(src, dst_of(&result(NumericTypeNameV1::Float)))),
            },
        ),
        budget(),
    );
    assert!(!matches!(non_row, Verdict::Accepted(_)));
}

/// `Add` on two `Int`s does not conclude `Float`. Absence of the row is
/// `Rejected` — never `Refuted`, and never a silent acceptance (§8.8).
#[test]
fn a_pair_that_is_not_a_row_is_rejected_not_refuted() {
    let src = src_of(&add_int_int());
    let dst = dst_of(&result(NumericTypeNameV1::Float));

    let verdict = acceptance(
        &ctx(),
        &expected_realizes(src.clone(), dst.clone()),
        &ExplicitTerm::new(ctx(), prim(src, dst)),
        budget(),
    );
    assert!(
        matches!(
            verdict,
            Verdict::Rejected(RejectionReason::PrimitiveRowNotFound { .. })
        ),
        "expected a row-not-found rejection, got {verdict:?}"
    );
    // The epistemic mapping: a rejection publishes no judgement at all.
    assert_eq!(verdict.outcome(), None);
}

/// An unknown relation id fails closed (§7). This is also the forward/backward
/// case: an old kernel meeting a relation minted by a newer release lands here
/// rather than reinterpreting it.
#[test]
fn an_unknown_relation_id_fails_closed() {
    let src = src_of(&add_int_int());
    let dst = dst_of(&result(NumericTypeNameV1::Int));

    let verdict = acceptance(
        &ctx(),
        &expected_realizes(src.clone(), dst.clone()),
        &ExplicitTerm::new(
            ctx(),
            TermKind::PrimRealizes {
                relation: PrimitiveRelationId(Digest::of(Domain::Value, b"TypingArithV99")),
                src,
                dst,
            },
        ),
        budget(),
    );
    assert!(
        matches!(
            verdict,
            Verdict::Rejected(RejectionReason::UnknownPrimitiveRelation(_))
        ),
        "expected an unknown-relation rejection, got {verdict:?}"
    );
    assert_eq!(verdict.outcome(), None);
}

/// An endpoint that is not an object constant can never be a row member, and is
/// refused as such rather than reported as a missing row.
#[test]
fn endpoints_must_be_object_constants() {
    let src = src_of(&add_int_int());
    let dst = dst_of(&result(NumericTypeNameV1::Int));
    let composed = ObjectTerm::Compose(Box::new(src.clone()), Box::new(dst.clone()));

    let verdict = acceptance(
        &ctx(),
        &expected_realizes(composed.clone(), dst.clone()),
        &ExplicitTerm::new(ctx(), prim(composed, dst)),
        budget(),
    );
    assert!(!matches!(verdict, Verdict::Accepted(_)));

    let bound = ObjectTerm::BoundVar(0);
    let verdict = acceptance(
        &ctx(),
        &expected_realizes(src.clone(), bound.clone()),
        &ExplicitTerm::new(ctx(), prim(src, bound)),
        budget(),
    );
    assert!(!matches!(verdict, Verdict::Accepted(_)));
}

/// Every material field of the source object is bound into the decision: a row
/// authorizing `Int + Int : Int` must not also authorize a mutation of it.
///
/// This is ADR-0015 §5 Stage B gate 2 at the kernel level — one field at a time,
/// asserting `Accepted` versus **not**-`Accepted`, never manufacturing
/// `Refuted`. The corresponding gate over the *real generator's* leaf lives in
/// `soc-regimes`.
#[test]
fn every_material_field_is_bound_into_the_decision() {
    let base = add_int_int();
    let base_dst = result(NumericTypeNameV1::Int);

    // The baseline is Accepted, so a mutation failing means the field mattered
    // and not that the whole setup was broken.
    assert!(matches!(
        acceptance(
            &ctx(),
            &expected_realizes(src_of(&base), dst_of(&base_dst)),
            &ExplicitTerm::new(ctx(), prim(src_of(&base), dst_of(&base_dst))),
            budget(),
        ),
        Verdict::Accepted(_)
    ));

    let lossy_int_float = CoercionEdgeV1 {
        generator: GeneratorId::named("type.rule.num.promote.Int_Float@1"),
        kind: CoercionKind::Lossy,
    };

    let mutations: Vec<(&str, ArithTypingInputV1, NumericResultTypeV1)> = vec![
        (
            "operator + -> /",
            ArithTypingInputV1 {
                operator: ArithOperatorV1::Div,
                ..base.clone()
            },
            base_dst,
        ),
        (
            "an operand type",
            ArithTypingInputV1 {
                lhs_type: NumericTypeNameV1::Rat,
                ..base.clone()
            },
            base_dst,
        ),
        (
            "the result type",
            base.clone(),
            result(NumericTypeNameV1::Rat),
        ),
        (
            "a promotion edge appearing where there was none",
            ArithTypingInputV1 {
                lhs_promotion_path: vec![lossy_int_float.clone()],
                ..base.clone()
            },
            base_dst,
        ),
        (
            "an exact edge substituted for a lossy one",
            ArithTypingInputV1 {
                lhs_type: NumericTypeNameV1::Int,
                rhs_type: NumericTypeNameV1::Int,
                operator: ArithOperatorV1::Div,
                lhs_promotion_path: vec![CoercionEdgeV1 {
                    kind: CoercionKind::Exact,
                    ..lossy_int_float.clone()
                }],
                rhs_promotion_path: vec![lossy_int_float.clone()],
            },
            result(NumericTypeNameV1::Float),
        ),
        (
            "operand order",
            ArithTypingInputV1 {
                operator: ArithOperatorV1::Div,
                lhs_type: NumericTypeNameV1::Nat,
                rhs_type: NumericTypeNameV1::Int,
                // The paths belong to the *other* operand order.
                lhs_promotion_path: vec![lossy_int_float.clone()],
                rhs_promotion_path: vec![
                    CoercionEdgeV1 {
                        generator: GeneratorId::named("type.rule.num.promote.Nat_Int@1"),
                        kind: CoercionKind::Exact,
                    },
                    lossy_int_float,
                ],
            },
            result(NumericTypeNameV1::Float),
        ),
    ];

    for (label, mutated_src, mutated_dst) in mutations {
        let verdict = acceptance(
            &ctx(),
            &expected_realizes(src_of(&mutated_src), dst_of(&mutated_dst)),
            &ExplicitTerm::new(ctx(), prim(src_of(&mutated_src), dst_of(&mutated_dst))),
            budget(),
        );
        assert!(
            !matches!(verdict, Verdict::Accepted(_)),
            "mutating {label} must not be Accepted, got {verdict:?}"
        );
    }
}

/// Gate 3: `Float` mixed with `Rat` has no join, so no arithmetic typing proof
/// exists for it. There is no row, and none is invented from a host-side
/// closure (§8.5).
#[test]
fn arithmetic_rule_has_no_unchecked_join() {
    let relation = resolve_primitive_relation(&typing_arith_v2()).expect("resolves");

    for (lhs, rhs) in [
        (NumericTypeNameV1::Float, NumericTypeNameV1::Rat),
        (NumericTypeNameV1::Rat, NumericTypeNameV1::Float),
        (NumericTypeNameV1::Float, NumericTypeNameV1::Real),
        (NumericTypeNameV1::Complex, NumericTypeNameV1::Float),
    ] {
        for operator in [
            ArithOperatorV1::Add,
            ArithOperatorV1::Sub,
            ArithOperatorV1::Mul,
            ArithOperatorV1::Div,
        ] {
            let input = ArithTypingInputV1 {
                operator,
                lhs_type: lhs,
                rhs_type: rhs,
                lhs_promotion_path: Vec::new(),
                rhs_promotion_path: Vec::new(),
            };
            let src = PropositionId(input.config_id().digest());
            // No result type whatsoever is admitted for this operand mixture.
            for name in [
                NumericTypeNameV1::Nat,
                NumericTypeNameV1::Int,
                NumericTypeNameV1::Rat,
                NumericTypeNameV1::Real,
                NumericTypeNameV1::Complex,
                NumericTypeNameV1::Float,
            ] {
                let dst = PropositionId(result(name).config_id().digest());
                assert!(
                    !relation.admits(&src, &dst),
                    "{lhs:?} {operator:?} {rhs:?} must admit no result type, but admitted {name:?}"
                );
            }
        }
    }
}

/// The new constructor is appended at ordinal **17** and no existing ordinal
/// moved. `RealizesTensor` at 16 and `Unsupported` at 9 are spot-checked with
/// literals, because §7's compatibility guarantee is exactly that existing
/// certificate bytes are unchanged.
#[test]
fn the_new_term_ordinal_is_appended_at_seventeen() {
    let src = src_of(&add_int_int());
    let dst = dst_of(&result(NumericTypeNameV1::Int));
    let relation = typing_arith_v2();

    let mut expected = CanonWriter::new();
    expected.write_enum(17, |w| {
        w.write_bytes(relation.digest().as_bytes());
        src.canon_write(w);
        dst.canon_write(w);
    });
    assert_eq!(prim(src, dst).canon_bytes(), expected.finish());

    // Untouched neighbours.
    let mut sixteen = CanonWriter::new();
    sixteen.write_enum(16, |w| {
        TermKind::Hyp(Var::Index(0)).canon_write(w);
        TermKind::Hyp(Var::Index(1)).canon_write(w);
    });
    assert_eq!(
        TermKind::RealizesTensor {
            left: Box::new(TermKind::Hyp(Var::Index(0))),
            right: Box::new(TermKind::Hyp(Var::Index(1))),
        }
        .canon_bytes(),
        sixteen.finish()
    );

    let mut nine = CanonWriter::new();
    nine.write_enum(9, |w| "x".canon_write(w));
    assert_eq!(
        TermKind::Unsupported("x".into()).canon_bytes(),
        nine.finish()
    );
}

/// The relation id is bound into the proof term, so two certificates over the
/// same endpoints under different relations are different artifacts. Without
/// this, a certificate would not record *which* trusted table authorized it —
/// the gap ADR-0019 §6 names on the settlement side.
#[test]
fn the_relation_id_is_bound_into_the_term() {
    let src = src_of(&add_int_int());
    let dst = dst_of(&result(NumericTypeNameV1::Int));

    let real = prim(src.clone(), dst.clone());
    let other = TermKind::PrimRealizes {
        relation: PrimitiveRelationId(Digest::of(Domain::Value, b"some other relation")),
        src,
        dst,
    };
    assert_ne!(real.canon_bytes(), other.canon_bytes());
}

/// The retired `TypingArithV1` is gone from the registry, and its absence fails
/// closed (ADR-0024 §3, per the maintainer ruling on #53).
///
/// The id is spelled as a literal rather than computed, because that is the
/// whole property under test: this exact digest was a real relation identity in
/// #282, and after retirement the kernel must resolve it to `None`. Per
/// ADR-0015 §7 that means the kernel has not introduced the fact — never that
/// its negation holds, and never that the id has been reinterpreted as some
/// other row set.
///
/// Retiring rather than retaining follows ⟨D-EXACTCOVERED⟩'s own reasoning: no
/// certificate naming V1 exists or can exist yet, since `elaborate_tree` still
/// emits every leaf as a `Hyp`, so retaining it would be trusted TCB data with
/// nothing consulting it — and its rows spelled a generator family the lattice
/// no longer declares.
const RETIRED_TYPING_ARITH_V1: &str =
    "f285a12c39abc6a493938646ac06d063bbc8ed88df60c015805d5be2516db338";

#[test]
fn the_retired_relation_does_not_resolve() {
    let mut bytes = [0u8; 32];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&RETIRED_TYPING_ARITH_V1[i * 2..i * 2 + 2], 16).expect("hex");
    }
    let retired = PrimitiveRelationId(Digest::from_bytes(bytes));

    assert_ne!(retired, typing_arith_v2(), "V2 did not inherit V1's identity");
    assert!(
        resolve_primitive_relation(&retired).is_none(),
        "the retired relation must not resolve"
    );

    // And a term naming it is rejected rather than silently checked against the
    // surviving relation. `1 + 2` is a row both versions shared, so if the
    // kernel were falling back to V2 this would be Accepted.
    let src = src_of(&add_int_int());
    let dst = dst_of(&result(NumericTypeNameV1::Int));
    let verdict = acceptance(
        &ctx(),
        &expected_realizes(src.clone(), dst.clone()),
        &ExplicitTerm::new(
            ctx(),
            TermKind::PrimRealizes {
                relation: retired,
                src,
                dst,
            },
        ),
        budget(),
    );
    assert!(
        matches!(
            verdict,
            Verdict::Rejected(RejectionReason::UnknownPrimitiveRelation(_))
        ),
        "an unmoved row must not be laundered through the retired id, got {verdict:?}"
    );
}

/// A row whose path crosses the relocated edge is accepted **only** when it
/// names the edge the way the surviving relation does. The same `7 / 2` source
/// object under the two namings is two different configurations, and retiring
/// V1 removed the one that admitted the legacy spelling — so the legacy row is
/// now simply not a row anywhere.
#[test]
fn a_legacy_named_row_is_not_admitted_by_the_current_relation() {
    let lossy_edge = |prefix: &str| CoercionEdgeV1 {
        generator: GeneratorId::named(&format!("{prefix}.Int_Float@1")),
        kind: CoercionKind::Lossy,
    };
    // `7 / 2` — Div on two Ints, both operands crossing Int -> Float.
    let div = |prefix: &str| ArithTypingInputV1 {
        operator: ArithOperatorV1::Div,
        lhs_type: NumericTypeNameV1::Int,
        rhs_type: NumericTypeNameV1::Int,
        lhs_promotion_path: vec![lossy_edge(prefix)],
        rhs_promotion_path: vec![lossy_edge(prefix)],
    };

    let legacy = div("type.rule.num.promote");
    let current = div("type.rule.num.convert.lossy");
    assert_ne!(legacy.config_id(), current.config_id());

    let dst = dst_of(&result(NumericTypeNameV1::Float));
    for (label, input, want_accepted) in [
        ("legacy naming", &legacy, false),
        ("current naming", &current, true),
    ] {
        let src = src_of(input);
        let verdict = acceptance(
            &ctx(),
            &expected_realizes(src.clone(), dst.clone()),
            &ExplicitTerm::new(
                ctx(),
                TermKind::PrimRealizes {
                    relation: typing_arith_v2(),
                    src,
                    dst: dst.clone(),
                },
            ),
            budget(),
        );
        assert_eq!(
            matches!(verdict, Verdict::Accepted(_)),
            want_accepted,
            "{label}: got {verdict:?}"
        );
    }
}

/// ⟨D-PROMOTE⟩ read literally against the current relation: no accepted
/// arithmetic typing proof names `Int -> Float` under the promotion family.
///
/// Asserted on the generator *id*, not on the `CoercionKind` tag. The tag was
/// already correct before Stage E; the id was the part that claimed an
/// embedding for a map that is not injective.
#[test]
fn the_current_relation_never_names_the_lossy_edge_as_a_promotion() {
    let relation = resolve_primitive_relation(&typing_arith_v2()).expect("V2 resolves");

    for operator in [
        ArithOperatorV1::Add,
        ArithOperatorV1::Sub,
        ArithOperatorV1::Mul,
        ArithOperatorV1::Div,
    ] {
        for (lhs, rhs) in [
            (NumericTypeNameV1::Int, NumericTypeNameV1::Float),
            (NumericTypeNameV1::Float, NumericTypeNameV1::Int),
            (NumericTypeNameV1::Nat, NumericTypeNameV1::Float),
            (NumericTypeNameV1::Int, NumericTypeNameV1::Int),
        ] {
            // Spell the path with the *promotion* family, as V1 would have.
            let promoted = ArithTypingInputV1 {
                operator,
                lhs_type: lhs,
                rhs_type: rhs,
                lhs_promotion_path: vec![CoercionEdgeV1 {
                    generator: GeneratorId::named("type.rule.num.promote.Int_Float@1"),
                    kind: CoercionKind::Lossy,
                }],
                rhs_promotion_path: Vec::new(),
            };
            let src = PropositionId(promoted.config_id().digest());
            for name in [NumericTypeNameV1::Float, NumericTypeNameV1::Int] {
                let dst = PropositionId(result(name).config_id().digest());
                assert!(
                    !relation.admits(&src, &dst),
                    "{operator:?} {lhs:?} {rhs:?}: V2 must admit no row naming the \
                     lossy edge as a promotion"
                );
            }
        }
    }
}
