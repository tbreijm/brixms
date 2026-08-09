//! [`RealizesTree`] — a branching realization derivation — and
//! [`TreeDerivation`], its content-addressed evidence artifact (ADR-0007,
//! ADR-0017).
//!
//! A [`Decomposition`](crate::Decomposition) records a *linear* factorization
//! `k = g_n ∘ ⋯ ∘ g_1`. Real typing derivations branch: `App`'s function and
//! argument sub-derivations are independent, and forcing them into a chain
//! meant faking intermediate configurations (ADR-0007 §1). [`RealizesTree`] is
//! the tree generalization — `Seq` for `∘` (ADR-0004 Profile 1.1), `Tensor`
//! for `⊗` (ADR-0006 Profile 1.2).
//!
//! **The load-bearing distinction is the same one `Decomposition` draws
//! (ADR-0002 §5.1/§5.2), and for the same reason: it lives in the type and in
//! the canonical encoding, not in a comment.** A
//! [`TreeVerification::Recorded`] derivation and a
//! [`TreeVerification::StructureVerified`] one built from *identical* trees
//! have **different** [`TreeDerivationId`]s — a claim to have built a
//! derivation is not a claim to have checked it.
//!
//! **`StructureVerified` is deliberately not called `ReplayVerified`**
//! (ADR-0017 §5 D2). It names exactly what was checked — structural
//! well-formedness, endpoints against the real inference configs, and leaf
//! generators drawn from the regime's minted set — and *not* what was left
//! open: no leaf's realization relation `ρ_g` is verified. That is ADR-0007
//! §7's deferred tight direction and ADR-0015 ⟨D-PRIM⟩'s mechanism. A tag
//! reading `ReplayVerified` here would be the same class of error as an
//! `Evidence::SettlementReplay` that replays nothing — which is precisely the
//! defect ADR-0017 rules on.
//!
//! The checker that earns the tag lives with the regime that mints the
//! generators (`soc-regimes`), not here: this crate owns artifacts and their
//! canonical identity, never a verification procedure.

use brix_canon::{CanonWriter, Canonical};

use crate::id::digest_id;
use crate::witness::{compose, tensor};
use crate::{ConfigId, GeneratorId, WitnessId};

/// The object structure a realization tree acts on: a configuration, or a
/// tensor product of two. Canonical ordinals are **ABI** — append-only, never
/// reordered.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TreeObj {
    Atom(ConfigId),
    Prod(Box<TreeObj>, Box<TreeObj>),
}

impl TreeObj {
    /// Canonical ABI ordinal. Append-only; never reorder.
    const fn ordinal(&self) -> u64 {
        match self {
            TreeObj::Atom(_) => 0,
            TreeObj::Prod(_, _) => 1,
        }
    }
}

impl Canonical for TreeObj {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_enum(self.ordinal(), |w| match self {
            TreeObj::Atom(config) => config.canon_write(w),
            TreeObj::Prod(left, right) => {
                left.canon_write(w);
                right.canon_write(w);
            }
        });
    }
}

/// A tree-structured realization derivation (ADR-0007 §3). Leaves are single
/// generators of `𝒢`; `Seq` composes, `Tensor` runs in parallel. Canonical
/// ordinals are **ABI** — append-only, never reordered.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RealizesTree {
    Leaf {
        generator: GeneratorId,
        src: TreeObj,
        dst: TreeObj,
    },
    Seq {
        left: Box<RealizesTree>,
        right: Box<RealizesTree>,
    },
    Tensor {
        left: Box<RealizesTree>,
        right: Box<RealizesTree>,
    },
}

impl RealizesTree {
    /// The derivation's source object.
    pub fn src(&self) -> TreeObj {
        match self {
            RealizesTree::Leaf { src, .. } => src.clone(),
            RealizesTree::Seq { left, .. } => left.src(),
            RealizesTree::Tensor { left, right } => {
                TreeObj::Prod(Box::new(left.src()), Box::new(right.src()))
            }
        }
    }

    /// The derivation's target object.
    pub fn dst(&self) -> TreeObj {
        match self {
            RealizesTree::Leaf { dst, .. } => dst.clone(),
            RealizesTree::Seq { right, .. } => right.dst(),
            RealizesTree::Tensor { left, right } => {
                TreeObj::Prod(Box::new(left.dst()), Box::new(right.dst()))
            }
        }
    }

    fn collect_leaves<'a>(&'a self, out: &mut Vec<&'a RealizesTree>) {
        match self {
            RealizesTree::Leaf { .. } => out.push(self),
            RealizesTree::Seq { left, right } | RealizesTree::Tensor { left, right } => {
                left.collect_leaves(out);
                right.collect_leaves(out);
            }
        }
    }

    /// The leaves in left-to-right depth-first order. This order is the
    /// linchpin of tree elaboration (ADR-0007 §5): it fixes the hypothesis /
    /// de Bruijn correspondence in the proof term `brix-elaborate` builds.
    pub fn leaves(&self) -> Vec<&RealizesTree> {
        let mut out = Vec::new();
        self.collect_leaves(&mut out);
        out
    }

    /// Structural well-formedness: every `Seq` middle matches
    /// (`left.dst() == right.src()`). `Tensor` branches are independent and
    /// carry no such constraint.
    ///
    /// This is exactly the check ADR-0007 §6 designates as the tree analogue
    /// of a replay, and exactly as far as it goes: it says nothing about
    /// whether any leaf's `ρ_g` holds.
    pub fn well_formed(&self) -> bool {
        match self {
            RealizesTree::Leaf { .. } => true,
            RealizesTree::Seq { left, right } => {
                left.well_formed() && right.well_formed() && left.dst() == right.src()
            }
            RealizesTree::Tensor { left, right } => left.well_formed() && right.well_formed(),
        }
    }

    /// The composite witness identity this derivation realizes.
    ///
    /// **This must stay byte-identical to `brix-elaborate`'s
    /// `tree.witness_object().witness_digest()`**, which is how every typing
    /// `PropositionId` is currently built — a divergence would silently move
    /// every typing judgement's identity. It holds because
    /// `ObjectTerm::witness_digest` bottoms out in these same
    /// [`compose`]/[`tensor`] functions and maps `Const(id)` to
    /// `WitnessId(id.digest())`; the three arms below mirror it exactly,
    /// including `Seq`'s argument swap (the *right* sub-derivation is the
    /// outer composition). Pinned by
    /// `witness_id_matches_the_object_term_path` in
    /// `brix-elaborate/tests/tree_witness_identity.rs`.
    pub fn witness_id(&self) -> WitnessId {
        match self {
            RealizesTree::Leaf { generator, .. } => WitnessId(generator.digest()),
            // Note the swap: `right` is the outer witness, matching
            // `ObjectTerm::Compose(right.witness_object(), left.witness_object())`.
            RealizesTree::Seq { left, right } => compose(right.witness_id(), left.witness_id()),
            RealizesTree::Tensor { left, right } => tensor(left.witness_id(), right.witness_id()),
        }
    }

    /// Canonical ABI ordinal. Append-only; never reorder.
    const fn ordinal(&self) -> u64 {
        match self {
            RealizesTree::Leaf { .. } => 0,
            RealizesTree::Seq { .. } => 1,
            RealizesTree::Tensor { .. } => 2,
        }
    }
}

impl Canonical for RealizesTree {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_enum(self.ordinal(), |w| match self {
            RealizesTree::Leaf {
                generator,
                src,
                dst,
            } => {
                // Field order is ABI: generator, src, dst.
                generator.canon_write(w);
                src.canon_write(w);
                dst.canon_write(w);
            }
            RealizesTree::Seq { left, right } | RealizesTree::Tensor { left, right } => {
                left.canon_write(w);
                right.canon_write(w);
            }
        });
    }
}

/// Whether a [`TreeDerivation`] has merely been *built* by the inference pass,
/// or *checked* by the tree-audit checker. Canonical ABI ordinals —
/// append-only, never reordered — because, exactly as with
/// [`crate::DecompVerification`], this distinction is part of the artifact's
/// identity: the ordinal here *is* the built-vs-checked boundary.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum TreeVerification {
    /// The inference pass built this derivation; nothing has checked it.
    /// Supports no published outcome on its own.
    Recorded,
    /// The tree-audit checker verified structural well-formedness, endpoints
    /// against the real inference configurations, and that every leaf cites a
    /// generator in the regime's minted set (ADR-0017 §4 rows b, c, e).
    ///
    /// **Not a replay.** No leaf's realization relation `ρ_g` was checked
    /// (row d) — see the module doc.
    StructureVerified,
}

impl TreeVerification {
    /// Canonical ABI ordinal. Append-only; never reorder.
    const fn ordinal(self) -> u64 {
        match self {
            TreeVerification::Recorded => 0,
            TreeVerification::StructureVerified => 1,
        }
    }
}

impl Canonical for TreeVerification {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_enum(self.ordinal(), |_| {});
    }
}

/// A realization derivation together with its verification status — the
/// evidence artifact behind an `Audited` typing judgement (ADR-0017 §5 D1).
///
/// Why [`TreeDerivation::verify_structure`] refused to issue the
/// `StructureVerified` tag (ADR-0019 D5).
///
/// Rust-side validation only: never canonically encoded or hashed, because a
/// derivation that fails the check never becomes a verified artifact. Every
/// variant means *no artifact was produced* — there is deliberately no
/// downgraded-artifact variant (ADR-0002 §4's fail-closed discipline).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TreeVerificationError {
    /// The receiver was not in [`TreeVerification::Recorded`] form. This
    /// transition upgrades a record; it never re-tags a verified one.
    NotRecorded { found: TreeVerification },
    /// A `Seq` node's middle does not match (`left.dst() != right.src()`), so
    /// the tree does not compose (ADR-0007 §6).
    MalformedTree,
    /// The derivation's endpoints are not the ones the claim is about — the
    /// tree proves something, but not this.
    EndpointMismatch,
    /// A leaf cites a generator outside the supplied registry. Without this
    /// check an arbitrary digest could pose as a typing rule.
    GeneratorNotInRegistry(GeneratorId),
}

/// Fields are private: the verification tag is the whole point, and a caller
/// able to set it would be able to mint the claim this artifact exists to
/// support. Construct through [`TreeDerivation::recorded`] or, for the
/// verified form, through [`TreeDerivation::verify_structure`], which earns
/// it (ADR-0019 D5).
///
/// The two doors that used to be open, as executable gates. **The removed
/// stamp constructor:**
///
/// ```compile_fail
/// use brix_semantic::{ConfigId, GeneratorId, RealizesTree, TreeDerivation, TreeObj};
/// let tree = RealizesTree::Leaf {
///     generator: GeneratorId::named("g@1"),
///     src: TreeObj::Atom(ConfigId::from_canon(b"x0")),
///     dst: TreeObj::Atom(ConfigId::from_canon(b"x1")),
/// };
/// let d = TreeDerivation::structure_verified(tree);
/// ```
///
/// **Struct-literal construction**, which would bypass it anyway:
///
/// ```compile_fail
/// use brix_semantic::{ConfigId, GeneratorId, RealizesTree, TreeDerivation, TreeObj, TreeVerification};
/// let d = TreeDerivation {
///     tree: RealizesTree::Leaf {
///         generator: GeneratorId::named("g@1"),
///         src: TreeObj::Atom(ConfigId::from_canon(b"x0")),
///         dst: TreeObj::Atom(ConfigId::from_canon(b"x1")),
///     },
///     verification: TreeVerification::StructureVerified,
/// };
/// ```
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TreeDerivation {
    tree: RealizesTree,
    verification: TreeVerification,
}

impl TreeDerivation {
    /// A derivation the inference pass built and nothing has checked
    /// (ADR-0007 §6's pre-audit state).
    pub fn recorded(tree: RealizesTree) -> Self {
        TreeDerivation {
            tree,
            verification: TreeVerification::Recorded,
        }
    }

    /// **Earn** the `StructureVerified` tag by checking this derivation
    /// (ADR-0019 D5).
    ///
    /// This replaces the old `structure_verified` stamp, which set the tag on
    /// whatever tree it was handed and delegated honesty to its caller — the
    /// tree-lane instance of the defect ADR-0016 §7.1 recorded for
    /// `Decomposition`. It performs ADR-0017 §4's rows itself:
    ///
    /// - **(e)** structural well-formedness — every `Seq` middle matches, so
    ///   the tree composes;
    /// - **(b)** the derivation's endpoints equal the **independently
    ///   supplied** `expected_src`/`expected_dst`;
    /// - **(c)** every leaf cites a generator in `registry`.
    ///
    /// The endpoints and registry are parameters rather than values read off
    /// the tree on purpose: a checker that took the derivation's word for its
    /// own endpoints, or built its registry from the leaves it is checking,
    /// would be checking nothing — "every cited generator is among the cited
    /// generators" is not a membership test.
    ///
    /// **Scope of the resulting tag** (ADR-0017 §4 row d). `StructureVerified`
    /// still does **not** attest that any leaf relation `ρ_g` holds — that is
    /// ADR-0007 §7's deferred tight direction and ADR-0015 ⟨D-PRIM⟩'s
    /// mechanism, and it is why this tag is not called `ReplayVerified`.
    ///
    /// Fails closed: every rejection is a typed [`TreeVerificationError`] and
    /// **no artifact is produced**.
    pub fn verify_structure(
        self,
        expected_src: &TreeObj,
        expected_dst: &TreeObj,
        registry: &crate::GeneratorRegistry,
    ) -> Result<Self, TreeVerificationError> {
        if self.verification != TreeVerification::Recorded {
            return Err(TreeVerificationError::NotRecorded {
                found: self.verification,
            });
        }

        // (e) structural well-formedness.
        if !self.tree.well_formed() {
            return Err(TreeVerificationError::MalformedTree);
        }

        // (b) endpoints against the independently supplied claim.
        if self.tree.src() != *expected_src || self.tree.dst() != *expected_dst {
            return Err(TreeVerificationError::EndpointMismatch);
        }

        // (c) every leaf cites a generator the registry mints.
        for leaf in self.tree.leaves() {
            match leaf {
                RealizesTree::Leaf { generator, .. } => {
                    if !registry.contains(generator) {
                        return Err(TreeVerificationError::GeneratorNotInRegistry(*generator));
                    }
                }
                // `leaves()` yields only `Leaf` nodes; treat anything else as
                // a failed check rather than trusting the invariant silently.
                _ => return Err(TreeVerificationError::MalformedTree),
            }
        }

        Ok(TreeDerivation {
            tree: self.tree,
            verification: TreeVerification::StructureVerified,
        })
    }

    /// The derivation.
    pub fn tree(&self) -> &RealizesTree {
        &self.tree
    }

    /// This derivation's verification status.
    pub const fn verification(&self) -> TreeVerification {
        self.verification
    }

    /// Whether the tree-audit checker verified this derivation — the only
    /// form that supports an `Audited` typing judgement.
    pub const fn is_structure_verified(&self) -> bool {
        matches!(self.verification, TreeVerification::StructureVerified)
    }

    /// The content-addressed id of this artifact.
    pub fn id(&self) -> TreeDerivationId {
        TreeDerivationId::of(self)
    }
}

impl Canonical for TreeDerivation {
    fn canon_write(&self, w: &mut CanonWriter) {
        // Field order is ABI: tree, then verification. The verification tag is
        // part of the encoding by design (module doc): recorded vs
        // structure-verified over an identical tree MUST NOT share an id.
        self.tree.canon_write(w);
        self.verification.canon_write(w);
    }
}

digest_id!(
    /// Content-addressed identity of a [`TreeDerivation`]. Depends on the
    /// whole tree **and** its verification status — a recorded and a
    /// structure-verified derivation over identical data are different ids
    /// (module doc).
    TreeDerivationId
);

#[cfg(test)]
mod tests {

    /// Earn the verified form the honest way (ADR-0019 D5): registry from the
    /// tree's own generators, real transition, tree's own endpoints. A
    /// fixture, not a membership test — see `verify_structure`'s doc.
    fn verified(tree: RealizesTree) -> TreeDerivation {
        let mut registry = crate::GeneratorRegistry::new();
        for leaf in tree.leaves() {
            if let RealizesTree::Leaf { generator, .. } = leaf {
                registry.insert(*generator);
            }
        }
        TreeDerivation::recorded(tree.clone())
            .verify_structure(&tree.src(), &tree.dst(), &registry)
            .expect("a well-formed fixture tree earns the tag")
    }
    use super::*;

    fn cfg(tag: &str) -> ConfigId {
        ConfigId::from_canon(tag.as_bytes())
    }

    fn leaf(name: &str, src: &str, dst: &str) -> RealizesTree {
        RealizesTree::Leaf {
            generator: GeneratorId::named(name),
            src: TreeObj::Atom(cfg(src)),
            dst: TreeObj::Atom(cfg(dst)),
        }
    }

    /// `Seq(a: x0 → x1⊗x2, Tensor(b: x1→x3, c: x2→x4))` — exercises every arm.
    /// The `Seq` middle matches because `a`'s target is exactly the `Prod` the
    /// `Tensor` branch consumes; that is the whole content of `well_formed`.
    fn mixed_tree() -> RealizesTree {
        RealizesTree::Seq {
            left: Box::new(RealizesTree::Leaf {
                generator: GeneratorId::named("g_a@1"),
                src: TreeObj::Atom(cfg("x0")),
                dst: TreeObj::Prod(
                    Box::new(TreeObj::Atom(cfg("x1"))),
                    Box::new(TreeObj::Atom(cfg("x2"))),
                ),
            }),
            right: Box::new(RealizesTree::Tensor {
                left: Box::new(leaf("g_b@1", "x1", "x3")),
                right: Box::new(leaf("g_c@1", "x2", "x4")),
            }),
        }
    }

    #[test]
    fn recorded_and_structure_verified_over_identical_data_have_distinct_ids() {
        let recorded = TreeDerivation::recorded(mixed_tree());
        let verified = verified(mixed_tree());
        assert_ne!(
            recorded.id(),
            verified.id(),
            "recorded vs structure-verified MUST NOT collide (ADR-0017 §5 D1)"
        );
        assert!(!recorded.is_structure_verified());
        assert!(verified.is_structure_verified());
    }

    #[test]
    fn distinct_trees_have_distinct_ids() {
        // The property the old proposition-derived evidence could not have:
        // two different derivations are two different artifacts.
        let a = verified(leaf("g_a@1", "x0", "x1"));
        let b = verified(leaf("g_b@1", "x0", "x1"));
        assert_ne!(
            a.id(),
            b.id(),
            "a different derivation is different evidence"
        );
    }

    #[test]
    fn canon_ordinals_are_stable() {
        // Freeze the wire ordinals. A reorder would silently merge recorded
        // and structure-verified derivations, or move every TreeDerivationId.
        for (v, ord) in [
            (TreeVerification::Recorded, 0u64),
            (TreeVerification::StructureVerified, 1u64),
        ] {
            let mut w = CanonWriter::new();
            v.canon_write(&mut w);
            let mut expected = CanonWriter::new();
            expected.write_enum(ord, |_| {});
            assert_eq!(w.finish(), expected.finish(), "{v:?} ordinal drifted");
        }

        let atom = TreeObj::Atom(cfg("x0"));
        assert_eq!(atom.ordinal(), 0);
        assert_eq!(
            TreeObj::Prod(Box::new(atom.clone()), Box::new(atom)).ordinal(),
            1
        );

        let l = leaf("g_a@1", "x0", "x1");
        assert_eq!(l.ordinal(), 0);
        assert_eq!(
            RealizesTree::Seq {
                left: Box::new(l.clone()),
                right: Box::new(l.clone())
            }
            .ordinal(),
            1
        );
        assert_eq!(
            RealizesTree::Tensor {
                left: Box::new(l.clone()),
                right: Box::new(l)
            }
            .ordinal(),
            2
        );
    }

    /// Golden vector, reproduced independently with a fresh `CanonWriter` (not
    /// via `TreeDerivation::canon_write`), so it cannot be vacuously satisfied
    /// by the code it guards.
    #[test]
    fn golden_vector_structure_verified_leaf() {
        let g = GeneratorId::named("g_a@1");
        let d = verified(RealizesTree::Leaf {
            generator: g,
            src: TreeObj::Atom(cfg("x0")),
            dst: TreeObj::Atom(cfg("x1")),
        });

        let mut got = CanonWriter::new();
        d.canon_write(&mut got);

        let mut expected = CanonWriter::new();
        expected.write_enum(0, |w| {
            // Leaf
            g.canon_write(w);
            w.write_enum(0, |w| cfg("x0").canon_write(w)); // TreeObj::Atom
            w.write_enum(0, |w| cfg("x1").canon_write(w));
        });
        expected.write_enum(1, |_| {}); // StructureVerified

        assert_eq!(got.finish(), expected.finish());
    }

    /// Same tree, `Recorded` form: only the trailing ordinal differs, and that
    /// is exactly what must change the id.
    #[test]
    fn golden_vector_recorded_leaf_differs_only_in_the_tag() {
        let g = GeneratorId::named("g_a@1");
        let tree = RealizesTree::Leaf {
            generator: g,
            src: TreeObj::Atom(cfg("x0")),
            dst: TreeObj::Atom(cfg("x1")),
        };

        let mut got = CanonWriter::new();
        TreeDerivation::recorded(tree.clone()).canon_write(&mut got);

        let mut expected = CanonWriter::new();
        tree.canon_write(&mut expected);
        expected.write_enum(0, |_| {}); // Recorded

        assert_eq!(got.finish(), expected.finish());
    }

    #[test]
    fn well_formed_requires_matching_seq_middles() {
        assert!(mixed_tree().well_formed());
        let broken = RealizesTree::Seq {
            left: Box::new(leaf("g_a@1", "x0", "x1")),
            right: Box::new(leaf("g_b@1", "x9", "x2")),
        };
        assert!(
            !broken.well_formed(),
            "a Seq whose middle does not match must never be well-formed"
        );
    }

    #[test]
    fn witness_id_mirrors_compose_and_tensor_with_the_seq_swap() {
        // The independent reconstruction of `witness_id`, spelled out: if this
        // drifts, every typing PropositionId moves.
        let wa = WitnessId(GeneratorId::named("g_a@1").digest());
        let wb = WitnessId(GeneratorId::named("g_b@1").digest());
        let wc = WitnessId(GeneratorId::named("g_c@1").digest());
        let expected = compose(tensor(wb, wc), wa);
        assert_eq!(mixed_tree().witness_id(), expected);
    }

    #[test]
    fn leaves_are_left_to_right_depth_first() {
        let tree = mixed_tree();
        let leaves = tree.leaves();
        let names: Vec<GeneratorId> = leaves
            .iter()
            .map(|l| match l {
                RealizesTree::Leaf { generator, .. } => *generator,
                _ => panic!("leaves() must yield only Leaf nodes"),
            })
            .collect();
        assert_eq!(
            names,
            vec![
                GeneratorId::named("g_a@1"),
                GeneratorId::named("g_b@1"),
                GeneratorId::named("g_c@1"),
            ],
            "leaf order is the hypothesis/de Bruijn correspondence (ADR-0007 §5)"
        );
    }
}
