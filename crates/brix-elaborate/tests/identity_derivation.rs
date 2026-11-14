//! Structural identity derivation elaboration and verification (ADR-0025 §10.1).
//!
//! Verifies [P2]: Identity-only derivations correctly elaborate and verify.
//! They have zero generator leaves, but represent a valid closed structural derivation.

use brix_elaborate::{
    elaborate_tree_certified, tree_obj_to_object_term, verify_tree_proof, AcceptedTreeProof,
    RealizesTree, TreeElaboration, TreeObj,
};
use brix_kernel::{Budget, ObjectTerm, Prop, TermKind};
use brix_semantic::{
    Authority, ConfigId, ContextId, GeneratorRegistry, Judgement, Outcome, PropositionId, Support,
    TreeDerivation,
};

fn budget() -> Budget {
    Budget::new(10_000, 256)
}

fn cfg(tag: &str) -> ConfigId {
    ConfigId::from_canon(format!("id-test.{tag}").as_bytes())
}

fn atom(tag: &str) -> TreeObj {
    TreeObj::Atom(cfg(tag))
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
        PropositionId::from_canon(b"id-test.source"),
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

#[test]
fn identity_only_atomic_derivation_elaborates_and_verifies() {
    let tree = RealizesTree::Id(atom("T"));
    let (derivation, _, proof) = elaborate(tree.clone());

    let expected_t = tree_obj_to_object_term(&atom("T"));
    let expected_prop = Prop::Realizes(
        ObjectTerm::Id(Box::new(expected_t.clone())),
        expected_t.clone(),
        expected_t.clone(),
    );
    let expected_term = TermKind::RealizesId(expected_t);

    assert_eq!(proof.proposition(), &expected_prop);
    assert_eq!(proof.term().kind, expected_term);
    assert_eq!(proof.occurrences().len(), 0);

    // Untrusted verify_tree_proof must also pass
    let verified = verify_tree_proof(&derivation, &ContextId::root(), proof.term(), budget());
    assert!(
        verified.is_ok(),
        "untrusted verify_tree_proof must pass for identity-only derivation, got {verified:?}"
    );
}

#[test]
fn identity_only_tensor_derivation_elaborates_and_verifies() {
    let left = RealizesTree::Id(atom("Left"));
    let right = RealizesTree::Id(atom("Right"));
    let tree = RealizesTree::Tensor {
        left: Box::new(left),
        right: Box::new(right),
    };
    let (derivation, _, proof) = elaborate(tree.clone());

    assert_eq!(proof.occurrences().len(), 0);

    let verified = verify_tree_proof(&derivation, &ContextId::root(), proof.term(), budget());
    assert!(
        verified.is_ok(),
        "untrusted verify_tree_proof must pass for tensor-identity derivation, got {verified:?}"
    );
}
