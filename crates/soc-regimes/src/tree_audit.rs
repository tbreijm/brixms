//! The tree-audit checker — what earns
//! [`TreeVerification::StructureVerified`] (ADR-0017 §5 D3).
//!
//! This is the typing lane's counterpart to `soc_core::audit::audit_step`, and
//! the honest way to describe it is by that comparison (ADR-0017 §4):
//!
//! | | `audit_step` (settlement) | [`audit_tree`] |
//! |---|---|---|
//! | a | log-integrity cross-check of the antecedent `Derived` judgement | — no journal on this lane |
//! | b | endpoint match | ✅ [`TreeAuditError::EndpointMismatch`] |
//! | c | `registry.contains(g)` per step | ✅ [`TreeAuditError::UnmintedGenerator`] |
//! | d | `semantics.realizes(g, x_i, x_{i+1})` per step | ❌ **not performed** |
//! | e | chain-shape invariant | ✅ [`TreeAuditError::MalformedTree`] |
//!
//! Row (d) is the one that matters and the one that is open: **no leaf's
//! realization relation `ρ_g` is checked here.** `elaborate_tree` admits every
//! leaf to the kernel as a *hypothesis*, and the kernel proves the composition
//! without ever inspecting one. That is ADR-0007 §7's deferred tight direction
//! and ADR-0015 ⟨D-PRIM⟩'s mechanism, and it is why the tag this checker
//! issues is called `StructureVerified` rather than `ReplayVerified`.
//!
//! Fails closed: every rejection is a typed [`TreeAuditError`] and no artifact
//! is produced. There is no path here that returns a weaker artifact instead
//! of an error.

use brix_semantic::{ConfigId, GeneratorId, RealizesTree, TreeDerivation, TreeObj};

use crate::type_realization::generator_name;

/// Why a tree derivation failed audit. Never a downgraded artifact — a
/// rejected derivation yields no [`TreeDerivation`] at all.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TreeAuditError {
    /// A `Seq` node's middle does not match: `left.dst() != right.src()`
    /// (ADR-0007 §6).
    MalformedTree,
    /// The derivation's endpoints are not the configurations the claim is
    /// about — the tree proves something, but not this.
    EndpointMismatch,
    /// A leaf cites a generator this regime does not mint. The settlement
    /// analogue is `registry.contains(g)` failing in `audit_step`; without it
    /// a derivation could name an arbitrary digest as a typing rule.
    UnmintedGenerator(GeneratorId),
}

/// Whether `g` is a generator this regime mints.
///
/// Reuses [`generator_name`]'s reverse lookup rather than duplicating the
/// list: that function is already the closed enumeration of everything
/// `type_realization` can produce — the named typing rules plus the
/// open-ended `NUMERIC`/`GRADE` promotion edges — and it returns `None`
/// precisely for a generator this module did not mint. One list, one place to
/// keep current.
pub fn is_minted_generator(g: &GeneratorId) -> bool {
    generator_name(g).is_some()
}

/// Audit a realization derivation and, if it passes, return the verified
/// artifact (ADR-0017 §5 D3).
///
/// `expr_config` and `ty_config` are the configurations the *claim* is about —
/// the subject expression and its inferred type. They are supplied by the
/// caller rather than read off the tree on purpose: a checker that took the
/// derivation's word for its own endpoints would be checking nothing, the same
/// discipline `brix-lower`'s `L3GeneratorSemantics` states as
/// "re-derivation, not trust".
pub fn audit_tree(
    tree: &RealizesTree,
    expr_config: ConfigId,
    ty_config: ConfigId,
) -> Result<TreeDerivation, TreeAuditError> {
    // (e) + (b): structural well-formedness, then the endpoints of the whole
    // derivation against the claim's own configurations.
    if !tree.well_formed() {
        return Err(TreeAuditError::MalformedTree);
    }
    if tree.src() != TreeObj::Atom(expr_config) || tree.dst() != TreeObj::Atom(ty_config) {
        return Err(TreeAuditError::EndpointMismatch);
    }

    // (c): every leaf must cite a generator this regime mints.
    for leaf in tree.leaves() {
        match leaf {
            RealizesTree::Leaf { generator, .. } => {
                if !is_minted_generator(generator) {
                    return Err(TreeAuditError::UnmintedGenerator(*generator));
                }
            }
            // `leaves()` yields only `Leaf` nodes; treat anything else as a
            // failed audit rather than trusting the invariant silently.
            _ => return Err(TreeAuditError::MalformedTree),
        }
    }

    Ok(TreeDerivation::structure_verified(tree.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::type_realization::{g_app2, g_lit, g_split};

    fn cfg(tag: &str) -> ConfigId {
        ConfigId::from_canon(tag.as_bytes())
    }

    fn leaf(g: GeneratorId, src: &str, dst: &str) -> RealizesTree {
        RealizesTree::Leaf {
            generator: g,
            src: TreeObj::Atom(cfg(src)),
            dst: TreeObj::Atom(cfg(dst)),
        }
    }

    /// `Seq(lit: e → t0, app: t0 → t1)` over minted generators.
    fn good_tree() -> RealizesTree {
        RealizesTree::Seq {
            left: Box::new(leaf(g_lit(), "expr", "t0")),
            right: Box::new(leaf(g_app2(), "t0", "ty")),
        }
    }

    #[test]
    fn a_well_formed_tree_over_minted_generators_is_verified() {
        let d = audit_tree(&good_tree(), cfg("expr"), cfg("ty")).expect("honest tree audits");
        assert!(d.is_structure_verified());
        assert_eq!(d.tree(), &good_tree());
    }

    #[test]
    fn a_mismatched_seq_middle_never_audits() {
        let broken = RealizesTree::Seq {
            left: Box::new(leaf(g_lit(), "expr", "t0")),
            right: Box::new(leaf(g_app2(), "t9", "ty")),
        };
        match audit_tree(&broken, cfg("expr"), cfg("ty")) {
            Err(TreeAuditError::MalformedTree) => {}
            other => panic!("a mismatched Seq middle must never audit clean, got {other:?}"),
        }
    }

    #[test]
    fn endpoints_that_disagree_with_the_claim_never_audit() {
        // The tree is internally fine; it just does not derive *this* claim.
        match audit_tree(&good_tree(), cfg("some other expr"), cfg("ty")) {
            Err(TreeAuditError::EndpointMismatch) => {}
            other => {
                panic!("a derivation of a different claim must never audit clean, got {other:?}")
            }
        }
        match audit_tree(&good_tree(), cfg("expr"), cfg("some other type")) {
            Err(TreeAuditError::EndpointMismatch) => {}
            other => {
                panic!("a derivation to a different type must never audit clean, got {other:?}")
            }
        }
    }

    #[test]
    fn a_leaf_citing_an_unminted_generator_never_audits() {
        // The check `audit_step` has and the tree lane did not (ADR-0017 §4
        // row c): without it, any digest could pose as a typing rule.
        let forged = GeneratorId::named("g_not_a_real_typing_rule@1");
        let tree = RealizesTree::Seq {
            left: Box::new(leaf(g_lit(), "expr", "t0")),
            right: Box::new(leaf(forged, "t0", "ty")),
        };
        match audit_tree(&tree, cfg("expr"), cfg("ty")) {
            Err(TreeAuditError::UnmintedGenerator(g)) => assert_eq!(g, forged),
            other => panic!("an unminted generator must never audit clean, got {other:?}"),
        }
    }

    #[test]
    fn every_generator_this_regime_mints_is_recognised() {
        for g in [g_lit(), g_split(), g_app2()] {
            assert!(
                is_minted_generator(&g),
                "{:?} is minted by this regime and must be recognised",
                generator_name(&g)
            );
        }
        assert!(!is_minted_generator(&GeneratorId::named("nonsense@1")));
    }
}
