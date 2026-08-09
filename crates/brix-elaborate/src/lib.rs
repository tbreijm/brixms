//! Elaboration bridge for BrixMS (Stage A & Stage B2).
//!
//! Bridges candidate proof terms produced by realization regimes into published
//! [`brix_semantic::Outcome::Proven`] judgements across an elaboration boundary.
//!
//! Soundness invariant: ONLY kernel acceptance (`brix_kernel::acceptance` returning
//! `Verdict::Accepted`) mints a `Proven` judgement. No other code path exists.
//!
//! **The boundary validates its source (ADR-0016 §6, audit finding A-2).**
//! ADR-0002 §5 ¶2: "only `Audited`-supported settlement evidence may enter an
//! `elaboration-boundary` edge — an unaudited commit is not even a certified
//! rule-match chain, let alone a theorem." [`elaborate_and_publish`] takes a
//! [`AuditedSource`], so that check cannot be skipped; the artifact-level
//! entry points ([`elaborate_decomposition`], [`elaborate_tree`]) perform it
//! and return [`ElaborationResult::Refused`] when it fails.

use brix_kernel::{ExplicitTerm, ObjectTerm, Prop, TermKind, Var};
use brix_semantic::{
    AuditedSource, Authority, Decomposition, Dependency, EdgeKind, Judgement, Outcome,
    PropositionId, PublicationError, Support, TreeDerivation,
};

/// Result of attempting to elaborate and publish a proof term.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ElaborationResult {
    /// acceptance returned Accepted: a Proven judgement + the elaboration-boundary edge linking it to the source.
    Proven {
        judgement: Judgement,
        edge: Dependency,
    },
    /// any non-Accepted verdict: NO Proven produced (carry the verdict for diagnosis).
    NotElaborated(brix_kernel::Verdict),
    /// The caller never had standing to ask: the source is not a valid
    /// [`AuditedSource`], or the kernel's own certificate did not open the
    /// `Proven` route (ADR-0016 §6). Kept distinct from `NotElaborated` on
    /// purpose — a kernel that rejects a term and a caller that may not cross
    /// the boundary are different facts, and collapsing them would lose
    /// exactly the signal this fence exists to produce.
    Refused(PublicationError),
}

/// Elaborate a candidate proof term against a proposition via `brix_kernel::acceptance`
/// and, upon acceptance, publish a [`Outcome::Proven`] judgement linked to `source` via an
/// [`EdgeKind::ElaborationBoundary`] dependency edge.
///
/// The `source` is an [`AuditedSource`] rather than a bare [`Judgement`]: the
/// audited-source boundary is in the signature, so there is no way to call
/// this with an unvalidated support chain (ADR-0016 §6).
///
/// Soundness-critical semantics:
/// 1. Calls `brix_kernel::acceptance(&source.judgement().context, proposition, term, budget)`.
/// 2. ONLY if it returns `Verdict::Accepted(certificate)`: publish a NEW Judgement with
///    the SAME context as `source`, the proved `proposition`, `Outcome::Proven`, and support =
///    `Support::KernelCertificate` wrapping that certificate — through
///    [`Judgement::publish`], so the `(ProofKernel, Proven, KernelCertificate)` route is
///    consulted rather than assumed. Also construct a `Dependency`
///    with `EdgeKind::ElaborationBoundary` FROM the new Proven judgement's id TO the source
///    judgement's id.
/// 3. For EVERY other verdict: return `ElaborationResult::NotElaborated(verdict)`.
pub fn elaborate_and_publish(
    source: &AuditedSource,
    proposition: &brix_kernel::Prop,
    term: &brix_kernel::ExplicitTerm,
    budget: brix_kernel::Budget,
) -> ElaborationResult {
    let source = source.judgement();
    match brix_kernel::acceptance(&source.context, proposition, term, budget) {
        brix_kernel::Verdict::Accepted(certificate) => {
            let support = Support::KernelCertificate {
                verifier: certificate.verifier,
                certificate: certificate.certificate_id,
            };
            match Judgement::publish(
                Authority::ProofKernel,
                source.context,
                proposition.proposition_id(),
                Outcome::Proven,
                support,
            ) {
                Ok(judgement) => {
                    let edge = Dependency::new(EdgeKind::ElaborationBoundary, source.id().digest());
                    ElaborationResult::Proven { judgement, edge }
                }
                Err(err) => ElaborationResult::Refused(err),
            }
        }
        verdict => ElaborationResult::NotElaborated(verdict),
    }
}

/// Elaborate an Audited settlement [`Decomposition`] into a kernel-proved implication
/// proposition and publish a [`Outcome::Proven`] judgement (Stage B2).
///
/// Steps:
/// 1. Extract ordered generators g_1..g_n and intermediate configs x_0..x_n from `decomposition`.
///    Build per-step antecedent propositions H_i = Prop::Realizes(Const(g_i), Const(x_{i-1}), Const(x_i)).
/// 2. Build the left-nested composite witness object term:
///    k_term = compose(g_n, compose(g_{n-1}, ... compose(g_2, g_1)...)) using ObjectTerm::Compose (outer, inner).
///    For n=1, k_term = Const(g_1).
/// 3. Build the closed implication proposition: H_1 -> H_2 -> ... -> H_n -> Realizes(k_term, x_0, x_n)
///    (right-associated Prop::Impl nesting).
/// 4. Build the proof term: n nested Lams binding h_1..h_n (Hyp de Bruijn index for h_i is n - i),
///    whose body is the left-nested RealizesComp fold over hypotheses h_1..h_n.
/// 5. Delegate to [`elaborate_and_publish`].
///
/// Step 0, before any of that: verify that `source` is genuinely `Audited` on
/// *this* decomposition — replay-verified, and named by the source's own
/// evidence id (ADR-0016 §6). A source that fails the check yields
/// [`ElaborationResult::Refused`]; the kernel is never invoked.
pub fn elaborate_decomposition(
    source: &Judgement,
    decomposition: &Decomposition,
    budget: brix_kernel::Budget,
) -> ElaborationResult {
    let source = match AuditedSource::verify(source, Support::Settlement(decomposition)) {
        Ok(verified) => verified,
        Err(err) => return ElaborationResult::Refused(err),
    };

    let n = decomposition.generators().len();
    if n == 0 {
        return ElaborationResult::NotElaborated(brix_kernel::Verdict::Rejected(
            brix_kernel::RejectionReason::Custom("Empty decomposition".into()),
        ));
    }

    let generators = decomposition.generators();
    let configs = decomposition.configs();

    // 1. Build per-step antecedent propositions H_i = Realizes(Const(g_i), Const(x_{i-1}), Const(x_i))
    let mut h_props = Vec::with_capacity(n);
    for i in 0..n {
        let g_term = ObjectTerm::Const(PropositionId(generators[i].digest()));
        let x_prev = ObjectTerm::Const(PropositionId(configs[i].digest()));
        let x_curr = ObjectTerm::Const(PropositionId(configs[i + 1].digest()));
        h_props.push(Prop::Realizes(g_term, x_prev, x_curr));
    }

    // 2. Build composite witness k_term = compose(g_n, compose(g_{n-1}, ... compose(g_2, g_1)...))
    let mut k_term = ObjectTerm::Const(PropositionId(generators[0].digest()));
    for g in &generators[1..] {
        let g_i_term = ObjectTerm::Const(PropositionId(g.digest()));
        k_term = ObjectTerm::Compose(Box::new(g_i_term), Box::new(k_term));
    }

    let x_0 = ObjectTerm::Const(PropositionId(configs[0].digest()));
    let x_n = ObjectTerm::Const(PropositionId(configs[n].digest()));
    let goal_prop = Prop::Realizes(k_term, x_0, x_n);

    // 3. Build closed implication proposition: H_1 -> H_2 -> ... -> H_n -> Realizes(k_term, x_0, x_n)
    let mut implication_prop = goal_prop;
    for h_i in h_props.into_iter().rev() {
        implication_prop = Prop::Impl(Box::new(h_i), Box::new(implication_prop));
    }

    // 4. Build proof term: n nested Lams binding h_1..h_n.
    // De Bruijn index mapping for h_i (1-indexed, i=1..n): index = n - i.
    let mut body = TermKind::Hyp(Var::Index(n - 1)); // h_1 has index n - 1
    for i in 1..n {
        let right_hyp = TermKind::Hyp(Var::Index(n - 1 - i)); // h_{i+1} has index n - 1 - i
        body = TermKind::RealizesComp {
            left: Box::new(body),
            right: Box::new(right_hyp),
        };
    }

    let mut kind = body;
    for i in (0..n).rev() {
        kind = TermKind::Lam {
            var_name: Some(format!("h{}", i + 1)),
            body: Box::new(kind),
        };
    }

    let term = ExplicitTerm::new(source.judgement().context, kind);

    // 5. Call existing elaborate_and_publish
    elaborate_and_publish(&source, &implication_prop, &term, budget)
}

// `RealizesTree`/`TreeObj` now live in `brix-semantic` (ADR-0017 §5 D1): the
// `TreeDerivation` artifact built over them is evidence, and evidence belongs
// inside the TCB closures where `Decomposition` already is. `brix-semantic`
// may depend on `brix-canon` only, so the *kernel projections* stay here —
// they are what needs `brix-kernel`. Re-exported so no caller's import path
// changes.
pub use brix_semantic::{RealizesTree, TreeObj};

/// The kernel object term a [`TreeObj`] denotes.
pub fn tree_obj_to_object_term(obj: &TreeObj) -> ObjectTerm {
    match obj {
        TreeObj::Atom(c) => ObjectTerm::Const(PropositionId(c.digest())),
        TreeObj::Prod(a, b) => ObjectTerm::Tensor(
            Box::new(tree_obj_to_object_term(a)),
            Box::new(tree_obj_to_object_term(b)),
        ),
    }
}

/// The composite witness [`ObjectTerm`] a derivation realizes.
///
/// [`RealizesTree::witness_id`] is the `brix-semantic`-native digest of this
/// same term and **must** agree with `witness_object(tree).witness_digest()` —
/// every typing `PropositionId` is built from one or the other. Pinned by
/// `witness_id_matches_the_object_term_path` in `tests/tree_witness_identity.rs`.
pub fn witness_object(tree: &RealizesTree) -> ObjectTerm {
    match tree {
        RealizesTree::Leaf { generator, .. } => {
            ObjectTerm::Const(PropositionId(generator.digest()))
        }
        RealizesTree::Seq { left, right } => ObjectTerm::Compose(
            Box::new(witness_object(right)),
            Box::new(witness_object(left)),
        ),
        RealizesTree::Tensor { left, right } => ObjectTerm::Tensor(
            Box::new(witness_object(left)),
            Box::new(witness_object(right)),
        ),
    }
}

/// Elaborate a tree-structured typing derivation into a kernel proof term (ADR-0007).
///
/// Takes the checked [`TreeDerivation`] artifact rather than a bare
/// [`RealizesTree`] (ADR-0017 §5): the source judgement's evidence id must be
/// the id of *this* artifact, and the artifact must carry
/// [`TreeVerification::StructureVerified`] — a tag only the tree-audit checker
/// can produce. A derivation the inference pass merely built is refused here,
/// before the kernel is reached.
///
/// This replaces the provisional binding ADR-0016 §7 reported, where the
/// support was a digest of the proposition being claimed and so bound to
/// nothing.
pub fn elaborate_tree(
    source: &Judgement,
    derivation: &TreeDerivation,
    budget: brix_kernel::Budget,
) -> ElaborationResult {
    let source = match AuditedSource::verify(source, Support::Tree(derivation)) {
        Ok(verified) => verified,
        Err(err) => return ElaborationResult::Refused(err),
    };
    let tree = derivation.tree();

    if !tree.well_formed() {
        return ElaborationResult::NotElaborated(brix_kernel::Verdict::Rejected(
            brix_kernel::RejectionReason::Custom("malformed Seq middle".into()),
        ));
    }

    let leaves = tree.leaves();
    let m = leaves.len();
    if m == 0 {
        return ElaborationResult::NotElaborated(brix_kernel::Verdict::Rejected(
            brix_kernel::RejectionReason::Custom("empty leaves".into()),
        ));
    }

    let mut h_props = Vec::with_capacity(m);
    for leaf_node in &leaves {
        match leaf_node {
            RealizesTree::Leaf {
                generator,
                src,
                dst,
            } => {
                let g_term = ObjectTerm::Const(PropositionId(generator.digest()));
                let src_term = tree_obj_to_object_term(src);
                let dst_term = tree_obj_to_object_term(dst);
                h_props.push(Prop::Realizes(g_term, src_term, dst_term));
            }
            _ => {
                return ElaborationResult::NotElaborated(brix_kernel::Verdict::Rejected(
                    brix_kernel::RejectionReason::Custom("non-leaf in leaves enumeration".into()),
                ));
            }
        }
    }

    let goal_prop = Prop::Realizes(
        witness_object(tree),
        tree_obj_to_object_term(&tree.src()),
        tree_obj_to_object_term(&tree.dst()),
    );

    let mut implication_prop = goal_prop;
    for h_i in h_props.into_iter().rev() {
        implication_prop = Prop::Impl(Box::new(h_i), Box::new(implication_prop));
    }

    fn to_term(tree: &RealizesTree, m: usize, k: &mut usize) -> TermKind {
        match tree {
            RealizesTree::Leaf { .. } => {
                let idx = m - 1 - *k;
                *k += 1;
                TermKind::Hyp(Var::Index(idx))
            }
            RealizesTree::Seq { left, right } => TermKind::RealizesComp {
                left: Box::new(to_term(left, m, k)),
                right: Box::new(to_term(right, m, k)),
            },
            RealizesTree::Tensor { left, right } => TermKind::RealizesTensor {
                left: Box::new(to_term(left, m, k)),
                right: Box::new(to_term(right, m, k)),
            },
        }
    }

    let mut k = 0;
    let body_kind = to_term(tree, m, &mut k);

    let mut kind = body_kind;
    for i in (0..m).rev() {
        kind = TermKind::Lam {
            var_name: Some(format!("h{}", i + 1)),
            body: Box::new(kind),
        };
    }

    let term = ExplicitTerm::new(source.judgement().context, kind);
    elaborate_and_publish(&source, &implication_prop, &term, budget)
}

#[cfg(test)]
mod tree_tests {
    use super::*;
    // `Verdict`/`RejectionReason` are no longer needed here: the two tests
    // that drove `elaborate_tree`'s internal `well_formed` rejection built a
    // malformed tree carrying `StructureVerified`, a state ADR-0019 D5 made
    // unconstructible. Those assertions moved to `verify_structure`, which
    // now refuses the tree outright. `elaborate_tree` keeps the check as
    // defence in depth; it is simply no longer reachable by any constructible
    // input, which is the stronger position.
    use brix_kernel::Budget;
    use brix_semantic::{ConfigId, ContextId, GeneratorId, PropositionId, TreeVerificationError};

    #[test]
    fn test_malformed_seq_rejected() {
        let context = ContextId::root();
        let proposition = PropositionId::from_canon(b"src");

        let g1 = GeneratorId::named("g1");
        let g2 = GeneratorId::named("g2");
        let c1 = ConfigId::from_canon(b"c1");
        let c2 = ConfigId::from_canon(b"c2");
        let c3 = ConfigId::from_canon(b"c3");
        let c4 = ConfigId::from_canon(b"c4");

        // Leaf1: c1 -> c2
        let leaf1 = RealizesTree::Leaf {
            generator: g1,
            src: TreeObj::Atom(c1),
            dst: TreeObj::Atom(c2),
        };

        // Leaf2: c3 -> c4  (middle mismatch: c2 != c3)
        let leaf2 = RealizesTree::Leaf {
            generator: g2,
            src: TreeObj::Atom(c3),
            dst: TreeObj::Atom(c4),
        };

        let tree = RealizesTree::Seq {
            left: Box::new(leaf1),
            right: Box::new(leaf2),
        };

        assert!(!tree.well_formed());

        // This test used to build a malformed tree carrying the
        // `StructureVerified` tag and assert that `elaborate_tree` caught it
        // anyway. ADR-0019 D5 made that state **unconstructible**: the tag is
        // now issued only by `verify_structure`, which rejects a malformed
        // tree. So the assertion moves to the boundary where the state is
        // refused — per ADR-0019 D7, a test that required an impossible state
        // becomes a test that the state cannot be produced.
        let mut registry = brix_semantic::GeneratorRegistry::new();
        registry.insert(g1);
        registry.insert(g2);
        let refused = TreeDerivation::recorded(tree.clone()).verify_structure(
            &tree.src(),
            &tree.dst(),
            &registry,
        );
        assert!(
            matches!(refused, Err(TreeVerificationError::MalformedTree)),
            "a mismatched Seq middle must never earn the tag, got {refused:?}"
        );

        // `elaborate_tree`'s own `well_formed` check is defensive depth and
        // still runs — drive it with the honest `Recorded` artifact, which is
        // the strongest form this tree can legitimately reach.
        let derivation = TreeDerivation::recorded(tree);
        let source = Judgement::publish(
            Authority::SettlementKernel,
            context,
            proposition,
            Outcome::Derived,
            Support::Tree(&derivation),
        );
        // A `Recorded` tree cannot support `Audited`, which is the fence
        // doing its job; the elaboration path is therefore unreachable for a
        // malformed tree from either direction.
        assert!(
            source.is_err(),
            "a Recorded tree must not support an elaboration-boundary source"
        );
    }

    #[test]
    fn test_3_leaf_tree_well_formed_and_misbuilt() {
        let context = ContextId::root();
        let proposition = PropositionId::from_canon(b"src");

        let ga = GeneratorId::named("ga");
        let gb = GeneratorId::named("gb");
        let gc = GeneratorId::named("gc");
        let gd = GeneratorId::named("gd");

        let c1 = ConfigId::from_canon(b"c1");
        let c2 = ConfigId::from_canon(b"c2");
        let c3 = ConfigId::from_canon(b"c3");
        let c4 = ConfigId::from_canon(b"c4");
        let c5 = ConfigId::from_canon(b"c5");
        let c6 = ConfigId::from_canon(b"c6");

        // Seq(Leaf a, Seq(Tensor(Leaf b, Leaf c), Leaf d))
        let leaf_a = RealizesTree::Leaf {
            generator: ga,
            src: TreeObj::Atom(c1),
            dst: TreeObj::Prod(Box::new(TreeObj::Atom(c2)), Box::new(TreeObj::Atom(c3))),
        };
        let leaf_b = RealizesTree::Leaf {
            generator: gb,
            src: TreeObj::Atom(c2),
            dst: TreeObj::Atom(c4),
        };
        let leaf_c = RealizesTree::Leaf {
            generator: gc,
            src: TreeObj::Atom(c3),
            dst: TreeObj::Atom(c5),
        };
        let leaf_d = RealizesTree::Leaf {
            generator: gd,
            src: TreeObj::Prod(Box::new(TreeObj::Atom(c4)), Box::new(TreeObj::Atom(c5))),
            dst: TreeObj::Atom(c6),
        };

        let tensor_bc = RealizesTree::Tensor {
            left: Box::new(leaf_b.clone()),
            right: Box::new(leaf_c.clone()),
        };

        let seq_bc_d = RealizesTree::Seq {
            left: Box::new(tensor_bc),
            right: Box::new(leaf_d.clone()),
        };

        let well_formed_tree = RealizesTree::Seq {
            left: Box::new(leaf_a.clone()),
            right: Box::new(seq_bc_d),
        };

        assert!(well_formed_tree.well_formed());

        let mut registry = brix_semantic::GeneratorRegistry::new();
        for leaf in well_formed_tree.leaves() {
            if let RealizesTree::Leaf { generator, .. } = leaf {
                registry.insert(*generator);
            }
        }
        let derivation = TreeDerivation::recorded(well_formed_tree.clone())
            .verify_structure(&well_formed_tree.src(), &well_formed_tree.dst(), &registry)
            .expect("a well-formed tree over its own generators earns the tag");
        let source = Judgement::publish(
            Authority::AuditChecker,
            context,
            proposition,
            Outcome::Audited,
            Support::Tree(&derivation),
        )
        .expect("AuditChecker/Audited/Tree(StructureVerified) is a legal route");

        let res = elaborate_tree(&source, &derivation, Budget::new(1000, 1000));
        assert!(matches!(res, ElaborationResult::Proven { .. }));

        // Mis-built tree (mismatched middle)
        let misbuilt_leaf_b = RealizesTree::Leaf {
            generator: gb,
            src: TreeObj::Atom(c3), // Wrong src (c3 instead of c2)
            dst: TreeObj::Atom(c4),
        };
        let misbuilt_tensor = RealizesTree::Tensor {
            left: Box::new(misbuilt_leaf_b),
            right: Box::new(leaf_c),
        };
        let misbuilt_tree = RealizesTree::Seq {
            left: Box::new(leaf_a),
            right: Box::new(RealizesTree::Seq {
                left: Box::new(misbuilt_tensor),
                right: Box::new(leaf_d),
            }),
        };

        assert!(!misbuilt_tree.well_formed());

        // ADR-0019 D5/D7: a misbuilt tree can no longer carry the verified
        // tag at all, so the assertion moves to the boundary that refuses it
        // rather than to a downstream check on an unconstructible artifact.
        let mut registry = brix_semantic::GeneratorRegistry::new();
        for leaf in misbuilt_tree.leaves() {
            if let RealizesTree::Leaf { generator, .. } = leaf {
                registry.insert(*generator);
            }
        }
        let refused = TreeDerivation::recorded(misbuilt_tree.clone()).verify_structure(
            &misbuilt_tree.src(),
            &misbuilt_tree.dst(),
            &registry,
        );
        assert!(
            matches!(refused, Err(TreeVerificationError::MalformedTree)),
            "a misbuilt tensor must never earn the tag, got {refused:?}"
        );
    }
}
