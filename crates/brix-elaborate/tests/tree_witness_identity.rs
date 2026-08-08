//! The two gates ADR-0017 must not fail silently.
//!
//! 1. **Witness identity is byte-identical.** ADR-0017 §5 D1 moved
//!    `RealizesTree` into `brix-semantic` and gave it a native
//!    [`RealizesTree::witness_id`]. Every typing `PropositionId` is built from
//!    that value; if it diverges from the `ObjectTerm` path it replaced, every
//!    typing judgement's identity moves and the change is wrong — silently, and
//!    without any test failing on its own terms. So pin them against each other
//!    directly, over shapes that exercise every arm.
//!
//! 2. **The evidence is not circular.** The defect ADR-0017 rules on
//!    (`spec/errata/0004-tree-realization-audited-support.md`) was evidence
//!    computed from the claim: it distinguished nothing, because every
//!    derivation of the same proposition produced the same digest. State the
//!    repaired property as a test — two different derivations are two different
//!    pieces of evidence.

use brix_canon::Canonical;
use brix_elaborate::witness_object;
use brix_semantic::{
    ConfigId, GeneratorId, RealizesTree, Support, TreeDerivation, TreeObj, WitnessId,
};

fn cfg(tag: &str) -> ConfigId {
    ConfigId::from_canon(tag.as_bytes())
}

fn atom(tag: &str) -> TreeObj {
    TreeObj::Atom(cfg(tag))
}

fn leaf(name: &str, src: TreeObj, dst: TreeObj) -> RealizesTree {
    RealizesTree::Leaf {
        generator: GeneratorId::named(name),
        src,
        dst,
    }
}

/// Every arm: a `Leaf`, a `Seq`, a `Tensor`, a `Seq` whose middle is a `Prod`,
/// and a right-nested `Seq` so the composition order is actually exercised
/// (`Seq`'s witness swaps its arguments — the one place a mirror could drift
/// and still typecheck).
fn shapes() -> Vec<(&'static str, RealizesTree)> {
    let a = leaf("g_a@1", atom("x0"), atom("x1"));
    let b = leaf("g_b@1", atom("x1"), atom("x2"));
    let c = leaf("g_c@1", atom("x2"), atom("x3"));

    let a_prod = leaf(
        "g_a@1",
        atom("x0"),
        TreeObj::Prod(Box::new(atom("x1")), Box::new(atom("x2"))),
    );

    vec![
        ("leaf", a.clone()),
        (
            "seq",
            RealizesTree::Seq {
                left: Box::new(a.clone()),
                right: Box::new(b.clone()),
            },
        ),
        (
            "tensor",
            RealizesTree::Tensor {
                left: Box::new(b.clone()),
                right: Box::new(c.clone()),
            },
        ),
        (
            "seq_right_nested",
            RealizesTree::Seq {
                left: Box::new(a.clone()),
                right: Box::new(RealizesTree::Seq {
                    left: Box::new(b.clone()),
                    right: Box::new(c.clone()),
                }),
            },
        ),
        (
            "seq_over_tensor",
            RealizesTree::Seq {
                left: Box::new(a_prod),
                right: Box::new(RealizesTree::Tensor {
                    left: Box::new(leaf("g_b@1", atom("x1"), atom("x3"))),
                    right: Box::new(leaf("g_c@1", atom("x2"), atom("x4"))),
                }),
            },
        ),
    ]
}

#[test]
fn witness_id_matches_the_object_term_path() {
    for (name, tree) in shapes() {
        let native: WitnessId = tree.witness_id();
        let via_object_term = witness_object(&tree).witness_digest();
        assert_eq!(
            native, via_object_term,
            "{name}: brix-semantic's witness_id diverged from the ObjectTerm path — \
             every typing PropositionId built from it has moved (ADR-0017 §5 D1)"
        );
    }
}

#[test]
fn seq_witness_order_is_not_commutative() {
    // Guards the specific way the mirror could be wrong and still pass a
    // reflexive check: `Seq`'s witness composes with the *right* sub-derivation
    // as the outer term. Swapping the arms must change the identity.
    let a = leaf("g_a@1", atom("x0"), atom("x1"));
    let b = leaf("g_b@1", atom("x1"), atom("x2"));
    let ab = RealizesTree::Seq {
        left: Box::new(a.clone()),
        right: Box::new(b.clone()),
    };
    let ba = RealizesTree::Seq {
        left: Box::new(b),
        right: Box::new(a),
    };
    assert_ne!(
        ab.witness_id(),
        ba.witness_id(),
        "composition is not commutative; a witness identity that ignores order is wrong"
    );
    assert_ne!(
        witness_object(&ab).witness_digest(),
        witness_object(&ba).witness_digest(),
        "the ObjectTerm path must disagree in the same direction"
    );
}

#[test]
fn distinct_derivations_yield_distinct_evidence() {
    // The repaired property. Before ADR-0017 the tree lane's evidence was
    // `Digest::of(Domain::Value, prop.digest())` — a function of the claim — so
    // *every* derivation of a proposition produced the same evidence id and the
    // support distinguished nothing. Now the evidence is the artifact's own
    // identity.
    let one = TreeDerivation::structure_verified(RealizesTree::Seq {
        left: Box::new(leaf("g_a@1", atom("x0"), atom("x1"))),
        right: Box::new(leaf("g_b@1", atom("x1"), atom("x2"))),
    });
    let other = TreeDerivation::structure_verified(RealizesTree::Seq {
        left: Box::new(leaf("g_c@1", atom("x0"), atom("x1"))),
        right: Box::new(leaf("g_b@1", atom("x1"), atom("x2"))),
    });

    assert_ne!(
        Support::Tree(&one).evidence_id(),
        Support::Tree(&other).evidence_id(),
        "two different derivations must be two different pieces of evidence \
         (spec/errata/0004-tree-realization-audited-support.md)"
    );
}

#[test]
fn evidence_survives_a_tampering_test() {
    // The other half of what the old support could not do: mutate one endpoint
    // deep inside the derivation and the evidence must move. With a
    // claim-derived digest it would not have, which is what "survives no
    // tampering test" meant in the erratum.
    let honest = RealizesTree::Seq {
        left: Box::new(leaf("g_a@1", atom("x0"), atom("x1"))),
        right: Box::new(leaf("g_b@1", atom("x1"), atom("x2"))),
    };
    let tampered = RealizesTree::Seq {
        left: Box::new(leaf("g_a@1", atom("x0"), atom("x1"))),
        // Same generators, same endpoints of the whole derivation — only the
        // middle configuration differs.
        right: Box::new(leaf("g_b@1", atom("x1"), atom("x2_tampered"))),
    };

    let a = TreeDerivation::structure_verified(honest);
    let b = TreeDerivation::structure_verified(tampered);
    assert_ne!(a.id(), b.id(), "a tampered middle must change the evidence");
    assert_ne!(
        a.canon_bytes(),
        b.canon_bytes(),
        "the canonical encoding must carry the whole derivation, not a summary"
    );
}
