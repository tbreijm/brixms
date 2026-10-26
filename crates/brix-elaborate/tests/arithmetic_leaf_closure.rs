//! ADR-0015 §5 Stage D at the elaborator boundary: tree elaboration closes
//! `g_arith` leaf occurrences with kernel primitive instances, keeps every
//! other leaf a hypothesis, and retains the accepted proof.
//!
//! `brix-elaborate` cannot reach `soc-regimes`, so the arithmetic leaves here
//! are built from the **kernel's own schemas** (`ArithTypingInputV1`,
//! `NumericResultTypeV1`) and the generator the kernel relation names. Expected
//! propositions and terms are written out by hand in each test — never read back
//! from the code under test — and every acceptance claim is re-run against the
//! real kernel independently.

use brix_elaborate::{
    elaborate_tree, elaborate_tree_certified, tree_obj_to_object_term, verify_tree_proof,
    witness_object, AcceptedTreeProof, ElaborationResult, LeafStatus, RealizesTree,
    TreeElaboration, TreeObj, TreeProofError,
};
use brix_kernel::{
    acceptance, resolve_primitive_relation, typing_arith_v2, ArithOperatorV1, ArithTypingInputV1,
    Budget, ExplicitTerm, NumericResultTypeV1, NumericTypeNameV1, ObjectTerm, Prop,
    ResourceBudgetReason, TermKind, Var, Verdict,
};
use brix_semantic::{
    Authority, ConfigId, ContextId, GeneratorId, GeneratorRegistry, Judgement, Outcome,
    PropositionId, Support, TreeDerivation,
};

fn budget() -> Budget {
    Budget::new(10_000, 256)
}

fn g_arith() -> GeneratorId {
    resolve_primitive_relation(&typing_arith_v2())
        .expect("TypingArithV2")
        .generator
}

fn cfg(tag: &str) -> ConfigId {
    ConfigId::from_canon(format!("arith-leaf.{tag}").as_bytes())
}

fn atom(c: ConfigId) -> TreeObj {
    TreeObj::Atom(c)
}

fn input(l: NumericTypeNameV1, r: NumericTypeNameV1) -> ConfigId {
    ArithTypingInputV1 {
        operator: ArithOperatorV1::Add,
        lhs_type: l,
        rhs_type: r,
        lhs_promotion_path: Vec::new(),
        rhs_promotion_path: Vec::new(),
    }
    .config_id()
}

fn result(n: NumericTypeNameV1) -> ConfigId {
    NumericResultTypeV1 { name: n }.config_id()
}

/// `Int + Int : Int` — an exact row of the kernel relation.
fn int_in() -> ConfigId {
    input(NumericTypeNameV1::Int, NumericTypeNameV1::Int)
}

fn int_out() -> ConfigId {
    result(NumericTypeNameV1::Int)
}

fn leaf(generator: GeneratorId, src: ConfigId, dst: ConfigId) -> RealizesTree {
    RealizesTree::Leaf {
        generator,
        src: atom(src),
        dst: atom(dst),
    }
}

/// The real arithmetic leaf `Int + Int : Int`.
fn arith_leaf() -> RealizesTree {
    leaf(g_arith(), int_in(), int_out())
}

fn bridge(name: &str, src: ConfigId, dst: ConfigId) -> RealizesTree {
    leaf(
        GeneratorId::named(&format!("arith-leaf.bridge.{name}@1")),
        src,
        dst,
    )
}

fn seq(l: RealizesTree, r: RealizesTree) -> RealizesTree {
    RealizesTree::Seq {
        left: Box::new(l),
        right: Box::new(r),
    }
}

fn tensor(l: RealizesTree, r: RealizesTree) -> RealizesTree {
    RealizesTree::Tensor {
        left: Box::new(l),
        right: Box::new(r),
    }
}

/// The shape of `1 + 2`: `Seq(g_arith_input, Seq(g_arith, g_arith_result))`.
fn one_plus_two() -> RealizesTree {
    seq(
        bridge("input", cfg("judged"), int_in()),
        seq(arith_leaf(), bridge("result", int_out(), cfg("ty"))),
    )
}

fn verified(tree: RealizesTree) -> TreeDerivation {
    let mut registry = GeneratorRegistry::new();
    for l in tree.leaves() {
        if let RealizesTree::Leaf { generator, .. } = l {
            registry.insert(*generator);
        }
    }
    TreeDerivation::recorded(tree.clone())
        .verify_structure(&tree.src(), &tree.dst(), &registry)
        .expect("well-formed fixture tree")
}

fn source(derivation: &TreeDerivation, ctx: ContextId) -> Judgement {
    Judgement::publish(
        Authority::AuditChecker,
        ctx,
        PropositionId::from_canon(b"arith-leaf.source"),
        Outcome::Audited,
        Support::Tree(derivation),
    )
    .expect("a StructureVerified tree legally publishes Audited")
}

fn elaborate(tree: RealizesTree) -> (TreeDerivation, Judgement, AcceptedTreeProof) {
    let d = verified(tree);
    let s = source(&d, ContextId::root());
    match elaborate_tree_certified(&s, &d, budget()) {
        TreeElaboration::Proven { proof, .. } => (d, s, *proof),
        other => panic!("expected Proven, got {other:?}"),
    }
}

fn c(id: &ConfigId) -> ObjectTerm {
    ObjectTerm::Const(PropositionId(id.digest()))
}

fn gen(g: &GeneratorId) -> ObjectTerm {
    ObjectTerm::Const(PropositionId(g.digest()))
}

fn realizes(g: &GeneratorId, src: &ConfigId, dst: &ConfigId) -> Prop {
    Prop::Realizes(gen(g), c(src), c(dst))
}

fn imp(a: Prop, b: Prop) -> Prop {
    Prop::Impl(Box::new(a), Box::new(b))
}

fn lam(name: &str, body: TermKind) -> TermKind {
    TermKind::Lam {
        var_name: Some(name.into()),
        body: Box::new(body),
    }
}

fn hyp(i: usize) -> TermKind {
    TermKind::Hyp(Var::Index(i))
}

fn comp(l: TermKind, r: TermKind) -> TermKind {
    TermKind::RealizesComp {
        left: Box::new(l),
        right: Box::new(r),
    }
}

fn tens(l: TermKind, r: TermKind) -> TermKind {
    TermKind::RealizesTensor {
        left: Box::new(l),
        right: Box::new(r),
    }
}

fn prim(src: &ConfigId, dst: &ConfigId) -> TermKind {
    TermKind::PrimRealizes {
        relation: typing_arith_v2(),
        src: c(src),
        dst: c(dst),
    }
}

/// Independently re-run the kernel against a hand-built proposition and term.
fn kernel_accepts(prop: &Prop, kind: TermKind) -> Option<brix_kernel::Certificate> {
    let term = ExplicitTerm::new(ContextId::root(), kind);
    match acceptance(&ContextId::root(), prop, &term, budget()) {
        Verdict::Accepted(cert) => Some(cert),
        _ => None,
    }
}

fn goal(tree: &RealizesTree) -> Prop {
    Prop::Realizes(
        witness_object(tree),
        tree_obj_to_object_term(&tree.src()),
        tree_obj_to_object_term(&tree.dst()),
    )
}

#[test]
fn one_plus_two_shape_closes_the_arith_leaf_and_keeps_both_bridges_open() {
    let tree = one_plus_two();
    let (d, s, proof) = elaborate(tree.clone());

    // Hand-written expectation: two remaining hypotheses (the two bridges),
    // the arithmetic occurrence is a primitive instance between them.
    let h_in = realizes(
        &GeneratorId::named("arith-leaf.bridge.input@1"),
        &cfg("judged"),
        &int_in(),
    );
    let h_out = realizes(
        &GeneratorId::named("arith-leaf.bridge.result@1"),
        &int_out(),
        &cfg("ty"),
    );
    let expected_prop = imp(h_in, imp(h_out, goal(&tree)));
    let expected_term = lam(
        "h1",
        lam(
            "h2",
            comp(hyp(1), comp(prim(&int_in(), &int_out()), hyp(0))),
        ),
    );

    assert_eq!(proof.proposition(), &expected_prop);
    assert_eq!(proof.term().kind, expected_term);
    assert_eq!(proof.term().context, ContextId::root());
    assert_eq!(proof.context(), &ContextId::root());

    // The kernel, run independently on the hand-built pair, accepts and issues
    // the very certificate the artifact retains.
    let cert = kernel_accepts(&expected_prop, expected_term).expect("independent acceptance");
    assert_eq!(&cert, proof.certificate());

    // Per-occurrence account.
    let status: Vec<LeafStatus> = proof.occurrences().iter().map(|o| o.status()).collect();
    assert_eq!(
        status,
        vec![
            LeafStatus::Open,
            LeafStatus::Closed(typing_arith_v2()),
            LeafStatus::Open
        ]
    );
    let closed: Vec<usize> = proof.closed_occurrences().map(|o| o.index()).collect();
    let remaining: Vec<usize> = proof.remaining_occurrences().map(|o| o.index()).collect();
    assert_eq!((closed, remaining), (vec![1], vec![0, 2]));
    assert_eq!(*proof.occurrences()[1].generator(), g_arith());

    // Bound to the derivation it came from, and independently re-verifiable.
    assert!(proof.binds(&d));
    assert_eq!(proof.derivation_id(), d.id());
    proof
        .recheck(&d, &ContextId::root(), budget())
        .expect("independent re-verification");

    // The published judgement names exactly this certificate.
    let published = elaborate_tree(&s, &d, budget());
    let ElaborationResult::Proven { judgement, .. } = published else {
        panic!("elaborate_tree must still publish");
    };
    assert_eq!(judgement.proposition, proof.proposition_id());
    assert_eq!(
        judgement.evidence,
        Support::KernelCertificate {
            verifier: proof.certificate().verifier,
            certificate: proof.certificate().certificate_id,
        }
        .evidence_id()
    );
}

#[test]
fn primitive_only_derivation_has_zero_binders_and_no_implication() {
    let tree = arith_leaf();
    let (d, _, proof) = elaborate(tree);

    assert_eq!(
        proof.proposition(),
        &realizes(&g_arith(), &int_in(), &int_out()),
        "no remaining hypothesis: the proposition is the bare realization"
    );
    assert_eq!(proof.term().kind, prim(&int_in(), &int_out()));
    assert!(
        !matches!(proof.term().kind, TermKind::Lam { .. }),
        "no binder for a closed leaf"
    );
    assert_eq!(proof.remaining_occurrences().count(), 0);
    assert_eq!(proof.closed_occurrences().count(), 1);
    proof.recheck(&d, &ContextId::root(), budget()).unwrap();
}

#[test]
fn repeated_identical_leaves_are_closed_at_each_occurrence() {
    let tree = tensor(arith_leaf(), arith_leaf());
    let (d, _, proof) = elaborate(tree.clone());

    assert_eq!(proof.proposition(), &goal(&tree));
    assert_eq!(
        proof.term().kind,
        tens(prim(&int_in(), &int_out()), prim(&int_in(), &int_out()))
    );
    let closed: Vec<usize> = proof.closed_occurrences().map(|o| o.index()).collect();
    assert_eq!(closed, vec![0, 1], "occurrences, not generator membership");
    proof.recheck(&d, &ContextId::root(), budget()).unwrap();
}

#[test]
fn mixed_open_and_closed_leaves_renumber_hypotheses_from_the_remaining_list() {
    // Seq(Tensor(arith, openB), openC): leaves [arith, openB, openC].
    let g_b = GeneratorId::named("arith-leaf.openB@1");
    let g_c = GeneratorId::named("arith-leaf.openC@1");
    let b_dst = cfg("b.dst");
    let tree = seq(
        tensor(arith_leaf(), leaf(g_b, cfg("b.src"), b_dst)),
        RealizesTree::Leaf {
            generator: g_c,
            src: TreeObj::Prod(Box::new(atom(int_out())), Box::new(atom(b_dst))),
            dst: atom(cfg("c.dst")),
        },
    );
    let (d, _, proof) = elaborate(tree.clone());

    let h_b = realizes(&g_b, &cfg("b.src"), &b_dst);
    let h_c = Prop::Realizes(
        gen(&g_c),
        ObjectTerm::Tensor(Box::new(c(&int_out())), Box::new(c(&b_dst))),
        c(&cfg("c.dst")),
    );
    assert_eq!(proof.proposition(), &imp(h_b, imp(h_c, goal(&tree))));
    // openB is the first *remaining* binder (index 1 of 2), openC the second (0).
    assert_eq!(
        proof.term().kind,
        lam(
            "h1",
            lam(
                "h2",
                comp(tens(prim(&int_in(), &int_out()), hyp(1)), hyp(0))
            )
        )
    );
    proof.recheck(&d, &ContextId::root(), budget()).unwrap();
}

#[test]
fn nested_arithmetic_keeps_traversal_order_and_structure() {
    // Tensor(Seq(open1, arith), Seq(arith, open2)): leaves [open1, arith, arith, open2].
    let g1 = GeneratorId::named("arith-leaf.open1@1");
    let g2 = GeneratorId::named("arith-leaf.open2@1");
    let tree = tensor(
        seq(leaf(g1, cfg("o1.src"), int_in()), arith_leaf()),
        seq(arith_leaf(), leaf(g2, int_out(), cfg("o2.dst"))),
    );
    let (d, _, proof) = elaborate(tree.clone());

    let h1 = realizes(&g1, &cfg("o1.src"), &int_in());
    let h2 = realizes(&g2, &int_out(), &cfg("o2.dst"));
    assert_eq!(proof.proposition(), &imp(h1, imp(h2, goal(&tree))));
    assert_eq!(
        proof.term().kind,
        lam(
            "h1",
            lam(
                "h2",
                tens(
                    comp(hyp(1), prim(&int_in(), &int_out())),
                    comp(prim(&int_in(), &int_out()), hyp(0))
                )
            )
        )
    );
    let closed: Vec<usize> = proof.closed_occurrences().map(|o| o.index()).collect();
    assert_eq!(closed, vec![1, 2]);
    proof.recheck(&d, &ContextId::root(), budget()).unwrap();
}

#[test]
fn derivation_without_closable_leaves_is_byte_identical_to_the_all_hypothesis_proof() {
    // Existing non-arithmetic behaviour: no leaf is closable, so the proof is
    // exactly the construction `elaborate_tree` always produced.
    let g1 = GeneratorId::named("arith-leaf.plain1@1");
    let g2 = GeneratorId::named("arith-leaf.plain2@1");
    let tree = seq(
        leaf(g1, cfg("p0"), cfg("p1")),
        leaf(g2, cfg("p1"), cfg("p2")),
    );
    let (_, _, proof) = elaborate(tree.clone());
    let old_prop = imp(
        realizes(&g1, &cfg("p0"), &cfg("p1")),
        imp(realizes(&g2, &cfg("p1"), &cfg("p2")), goal(&tree)),
    );
    assert_eq!(proof.proposition(), &old_prop);
    assert_eq!(
        proof.term().kind,
        lam("h1", lam("h2", comp(hyp(1), hyp(0))))
    );
    assert_eq!(proof.closed_occurrences().count(), 0);
}

#[test]
fn an_all_hypothesis_arithmetic_proof_stays_conditional_and_gains_no_closure() {
    let tree = one_plus_two();
    let d = verified(tree.clone());

    // The pre-Stage-D proof, built by hand: every leaf a hypothesis.
    let g_in = GeneratorId::named("arith-leaf.bridge.input@1");
    let g_res = GeneratorId::named("arith-leaf.bridge.result@1");
    let old_prop = imp(
        realizes(&g_in, &cfg("judged"), &int_in()),
        imp(
            realizes(&g_arith(), &int_in(), &int_out()),
            imp(realizes(&g_res, &int_out(), &cfg("ty")), goal(&tree)),
        ),
    );
    let old_kind = lam(
        "h1",
        lam("h2", lam("h3", comp(hyp(2), comp(hyp(1), hyp(0))))),
    );
    assert!(
        kernel_accepts(&old_prop, old_kind.clone()).is_some(),
        "the old proof remains valid as a conditional proof"
    );

    // The verification boundary accepts it and reports no closure.
    let old_term = ExplicitTerm::new(ContextId::root(), old_kind);
    let verified_old = verify_tree_proof(&d, &ContextId::root(), &old_term, budget())
        .expect("conditional proof verifies");
    assert_eq!(verified_old.closed_occurrences().count(), 0);
    assert_eq!(verified_old.remaining_occurrences().count(), 3);
    assert_eq!(verified_old.proposition(), &old_prop);

    // It does not prove the closed proposition the new elaboration proves.
    let (_, _, closed_proof) = elaborate(tree);
    let against_new = acceptance(
        &ContextId::root(),
        closed_proof.proposition(),
        &old_term,
        budget(),
    );
    assert!(
        !matches!(against_new, Verdict::Accepted(_)),
        "an all-hypothesis term must not stand in for the closed proof: {against_new:?}"
    );
    assert_ne!(
        verified_old.certificate().certificate_id,
        closed_proof.certificate().certificate_id
    );
}

#[test]
fn wrong_endpoint_relation_or_generator_cannot_close_the_expected_occurrence() {
    let tree = one_plus_two();
    let d = verified(tree.clone());
    let ctx = ContextId::root();
    let good = |kind: TermKind| ExplicitTerm::new(ctx, kind);
    let body = |mid: TermKind| lam("h1", lam("h2", comp(hyp(1), comp(mid, hyp(0)))));

    // Sanity: the honest term verifies.
    verify_tree_proof(&d, &ctx, &good(body(prim(&int_in(), &int_out()))), budget())
        .expect("honest term");

    // Wrong endpoint: a different (still admitted) row than the leaf's own.
    let other_in = input(NumericTypeNameV1::Nat, NumericTypeNameV1::Nat);
    let other_out = result(NumericTypeNameV1::Nat);
    assert_eq!(
        verify_tree_proof(&d, &ctx, &good(body(prim(&other_in, &other_out))), budget()),
        Err(TreeProofError::NonCanonicalTerm),
        "a primitive for another row is not a closure of this occurrence"
    );

    // Wrong relation: unknown id.
    let unknown = TermKind::PrimRealizes {
        relation: brix_kernel::PrimitiveRelationId(brix_canon::Digest::of(
            brix_canon::Domain::Value,
            b"TypingArithV99",
        )),
        src: c(&int_in()),
        dst: c(&int_out()),
    };
    assert_eq!(
        verify_tree_proof(&d, &ctx, &good(body(unknown)), budget()),
        Err(TreeProofError::PrimitiveNotAdmitted { index: 1 })
    );

    // Wrong generator: a bridge leaf cannot be closed by the arithmetic
    // relation even when its endpoints are exactly arithmetic ones.
    let spoof_tree = seq(
        bridge("spoof", int_in(), int_out()),
        bridge("tail", int_out(), cfg("ty")),
    );
    let spoof_d = verified(spoof_tree);
    let spoof_term = good(lam("h1", comp(prim(&int_in(), &int_out()), hyp(0))));
    assert_eq!(
        verify_tree_proof(&spoof_d, &ctx, &spoof_term, budget()),
        Err(TreeProofError::PrimitiveNotAdmitted { index: 0 })
    );
    let (_, _, spoof_proof) = elaborate(seq(
        bridge("spoof", int_in(), int_out()),
        bridge("tail", int_out(), cfg("ty")),
    ));
    assert_eq!(
        spoof_proof.closed_occurrences().count(),
        0,
        "elaboration never closes a non-arithmetic generator"
    );

    // Wrong endpoint at the leaf: the arithmetic generator with a non-member
    // pair stays a hypothesis.
    let (_, _, bad_pair) = elaborate(leaf(g_arith(), int_in(), result(NumericTypeNameV1::Float)));
    assert_eq!(bad_pair.closed_occurrences().count(), 0);
    assert_eq!(bad_pair.remaining_occurrences().count(), 1);
}

#[test]
fn unsupported_operand_combination_yields_no_arithmetic_primitive_proof() {
    // Float + Rat has no join: no row exists, so the leaf stays a hypothesis.
    let unjoinable = input(NumericTypeNameV1::Float, NumericTypeNameV1::Rat);
    let (d, _, proof) = elaborate(leaf(
        g_arith(),
        unjoinable,
        result(NumericTypeNameV1::Float),
    ));
    assert_eq!(proof.closed_occurrences().count(), 0);
    let mut stack = vec![&proof.term().kind];
    while let Some(k) = stack.pop() {
        match k {
            TermKind::PrimRealizes { .. } => panic!("no primitive instance may be emitted"),
            TermKind::Lam { body, .. } => stack.push(body),
            _ => {}
        }
    }

    // Presenting a primitive for it at the verification boundary is refused.
    let forced = ExplicitTerm::new(
        ContextId::root(),
        prim(&unjoinable, &result(NumericTypeNameV1::Float)),
    );
    assert_eq!(
        verify_tree_proof(&d, &ContextId::root(), &forced, budget()),
        Err(TreeProofError::PrimitiveNotAdmitted { index: 0 })
    );
}

#[test]
fn proof_cannot_be_transplanted_to_another_tree_or_context() {
    let (d, _, proof) = elaborate(one_plus_two());

    // Another tree with the same shape and generators but different endpoints.
    let other = verified(seq(
        bridge("input", cfg("judged-2"), int_in()),
        seq(arith_leaf(), bridge("result", int_out(), cfg("ty"))),
    ));
    assert!(!proof.binds(&other), "artifact identity differs");
    assert!(proof.recheck(&other, &ContextId::root(), budget()).is_err());
    // The bare term re-verifies against the other derivation only by becoming
    // a *different* artifact: its own proposition, its own certificate, bound
    // to that derivation — nothing about the original carries over.
    let rebuilt = verify_tree_proof(&other, &ContextId::root(), proof.term(), budget())
        .expect("shape-compatible term verifies as a new proof");
    assert!(rebuilt.binds(&other) && !rebuilt.binds(&d));
    assert_ne!(rebuilt.proposition(), proof.proposition());
    assert_ne!(
        rebuilt.certificate().certificate_id,
        proof.certificate().certificate_id
    );

    // Another tree that differs in *which* occurrence is arithmetic.
    let swapped = verified(seq(
        seq(bridge("input", cfg("judged"), int_in()), arith_leaf()),
        bridge("result", int_out(), cfg("ty")),
    ));
    assert!(!proof.binds(&swapped));

    // Another context: re-verification against an independently expected
    // context fails, even though the term and proposition are untouched.
    let other_ctx = ContextId::from_canon(b"arith-leaf.other-context");
    assert_ne!(other_ctx, ContextId::root());
    assert!(matches!(
        proof.recheck(&d, &other_ctx, budget()),
        Err(TreeProofError::NotAccepted(Verdict::ContextMismatch { .. }))
    ));
}

#[test]
fn closure_status_is_read_off_the_accepted_term_not_off_metadata() {
    // Two identical occurrences. A term closing only the first one is a
    // different proposition, a different certificate, and a different account.
    let tree = tensor(arith_leaf(), arith_leaf());
    let d = verified(tree.clone());
    let ctx = ContextId::root();
    let (_, _, both) = elaborate(tree.clone());

    let first_only = ExplicitTerm::new(ctx, lam("h1", tens(prim(&int_in(), &int_out()), hyp(0))));
    let one = verify_tree_proof(&d, &ctx, &first_only, budget()).expect("conditional on #1");
    let status: Vec<bool> = one.occurrences().iter().map(|o| o.is_closed()).collect();
    assert_eq!(status, vec![true, false]);
    assert_ne!(one.proposition(), both.proposition());
    assert_ne!(
        one.certificate().certificate_id,
        both.certificate().certificate_id
    );

    // The `both` artifact cannot be re-verified as if it were `one`: its term
    // is the wrong canonical construction for the other pattern.
    assert_eq!(*both.occurrences()[1].generator(), g_arith());
    assert_ne!(both, one);
}

#[test]
fn exhausted_budget_creates_no_closure_evidence_and_no_refutation() {
    let tree = one_plus_two();
    let d = verified(tree);
    let s = source(&d, ContextId::root());
    let starved = Budget::new(1, 1);

    match elaborate_tree_certified(&s, &d, starved) {
        TreeElaboration::NotElaborated(verdict) => {
            assert!(
                matches!(verdict, Verdict::ResourceExhausted(_)),
                "budget exhaustion must be reported as such, got {verdict:?}"
            );
            assert_eq!(
                verdict.outcome(),
                Some(Outcome::Unknown),
                "exhaustion is Unknown — never Refuted"
            );
            assert_ne!(verdict.outcome(), Some(Outcome::Refuted));
        }
        other => panic!("a starved kernel must not accept: {other:?}"),
    }
    assert!(matches!(
        elaborate_tree(&s, &d, starved),
        ElaborationResult::NotElaborated(Verdict::ResourceExhausted(
            ResourceBudgetReason::StepLimitExceeded | ResourceBudgetReason::DepthLimitExceeded
        ))
    ));

    // The verification boundary agrees.
    let honest = elaborate(one_plus_two()).2;
    assert!(matches!(
        verify_tree_proof(&d, &ContextId::root(), honest.term(), starved),
        Err(TreeProofError::NotAccepted(Verdict::ResourceExhausted(_)))
    ));
}

#[test]
fn recorded_or_unverified_derivation_is_never_verified_into_a_proof() {
    let recorded = TreeDerivation::recorded(one_plus_two());
    let honest = elaborate(one_plus_two()).2;
    assert_eq!(
        verify_tree_proof(&recorded, &ContextId::root(), honest.term(), budget()),
        Err(TreeProofError::DerivationNotVerified)
    );
}

#[test]
fn a_malformed_term_shape_is_refused_not_misread() {
    let d = verified(one_plus_two());
    // A tensor node where the derivation has a Seq.
    let bad = ExplicitTerm::new(
        ContextId::root(),
        lam(
            "h1",
            lam(
                "h2",
                tens(hyp(1), comp(prim(&int_in(), &int_out()), hyp(0))),
            ),
        ),
    );
    assert_eq!(
        verify_tree_proof(&d, &ContextId::root(), &bad, budget()),
        Err(TreeProofError::ShapeMismatch)
    );
}

#[test]
fn typing_proof_publishes_no_evaluation_authority() {
    // Typing versus evaluation: the artifact speaks of `Realizes` over typing
    // endpoints and carries no value, no evaluation proposition and no
    // evaluation-capable relation.
    let (_, _, proof) = elaborate(one_plus_two());
    fn only_realizes_and_impl(p: &Prop) -> bool {
        match p {
            Prop::Realizes(..) => true,
            Prop::Impl(a, b) => only_realizes_and_impl(a) && only_realizes_and_impl(b),
            _ => false,
        }
    }
    assert!(only_realizes_and_impl(proof.proposition()));
    let relation = resolve_primitive_relation(&typing_arith_v2()).unwrap();
    assert_eq!(relation.judgment_kind, brix_kernel::JudgmentKind::Typing);
    for o in proof.closed_occurrences() {
        let LeafStatus::Closed(r) = o.status() else {
            unreachable!()
        };
        assert_eq!(r, typing_arith_v2(), "only the typing relation closes");
    }
}
