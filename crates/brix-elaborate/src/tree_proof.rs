//! The accepted proof behind a tree elaboration, and the per-occurrence account
//! of which leaves it closed (ADR-0015 §5 Stage D; Type Realization Contract
//! §9.5).
//!
//! **What this module is for.** `elaborate_tree` used to emit every derivation
//! leaf as a hypothesis, so the kernel proved `leaves ⇒ conclusion` and never
//! checked any leaf's realization. It now classifies each leaf *occurrence*
//! deterministically: a leaf the kernel's primitive-relation registry can decide
//! is emitted as a closed [`TermKind::PrimRealizes`] instance, every other leaf
//! stays a hypothesis, and the implication proposition is built from the
//! remaining hypotheses only.
//!
//! **What counts as closure evidence.** Only an [`AcceptedTreeProof`], and that
//! type has no public constructor and no public field. It is minted in exactly
//! two places, and both of them run [`brix_kernel::acceptance`] and require
//! `Verdict::Accepted`:
//!
//! - `elaborate_tree_certified`, which builds the term itself; and
//! - [`verify_tree_proof`], which takes an *untrusted* term and re-derives, from
//!   the derivation and the term's own shape, the proposition that term must
//!   prove — it never believes a caller-supplied proposition, closure list, or
//!   certificate digest.
//!
//! The closure account is read **off the accepted term**, occurrence by
//! occurrence, not off a set of generator ids: two `g_arith` leaves in one tree
//! are two occurrences, one of which can be closed while the other is not, and
//! evidence for one is not evidence for the other.
//!
//! **What it does not say.** Closing a leaf establishes a typing relation for
//! that occurrence. It proves no evaluation, no computed value and no policy
//! compliance, and it does not discharge the regime↔kernel vocabulary bridges
//! on either side of the `g_arith` leaf (ADR-0023 §4).

use std::sync::OnceLock;

use brix_kernel::{
    acceptance, resolve_primitive_relation, typing_arith_input_v1, typing_arith_v2,
    typing_arith_v3, Budget, Certificate, ExplicitTerm, JudgmentKind, ObjectTerm,
    PrimitiveRelationId, Prop, TermKind, Var, Verdict,
};
use brix_semantic::{
    ContextId, GeneratorId, PropositionId, RealizesTree, TreeDerivation, TreeDerivationId, TreeObj,
};

use crate::{tree_obj_to_object_term, witness_object};

/// Whether one leaf occurrence was closed by a primitive instance or left as a
/// hypothesis of the proof.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LeafStatus {
    /// The accepted term contains a `PrimRealizes` instance of this relation
    /// at this occurrence.
    Closed(PrimitiveRelationId),
    /// The occurrence is still a hypothesis: the proposition is conditional on
    /// it.
    Open,
}

/// One leaf occurrence of the derivation, in traversal order.
///
/// Identity is the **position** `index` plus the leaf's own generator and
/// endpoints, never the generator alone.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LeafOccurrence {
    index: usize,
    generator: GeneratorId,
    src: TreeObj,
    dst: TreeObj,
    status: LeafStatus,
}

impl LeafOccurrence {
    /// Position in the left-to-right depth-first leaf order.
    pub fn index(&self) -> usize {
        self.index
    }

    /// The leaf's generator.
    pub fn generator(&self) -> &GeneratorId {
        &self.generator
    }

    /// The leaf's source endpoint.
    pub fn src(&self) -> &TreeObj {
        &self.src
    }

    /// The leaf's destination endpoint.
    pub fn dst(&self) -> &TreeObj {
        &self.dst
    }

    /// Closed by a primitive instance, or still a hypothesis.
    pub fn status(&self) -> LeafStatus {
        self.status
    }

    /// Whether this occurrence is closed by an accepted primitive instance.
    pub fn is_closed(&self) -> bool {
        matches!(self.status, LeafStatus::Closed(_))
    }
}

/// Why a tree proof could not be accepted. Every variant means **no**
/// [`AcceptedTreeProof`] exists; none is a logical refutation of anything.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TreeProofError {
    /// The derivation does not carry `StructureVerified`, or is malformed or
    /// empty.
    DerivationNotVerified,
    /// The term's shape does not follow the derivation's `Seq`/`Tensor`
    /// structure.
    ShapeMismatch,
    /// This occurrence holds a primitive instance the occurrence cannot be
    /// closed by: the relation is unknown, is not a typing relation, names a
    /// different generator, or does not admit the leaf's own endpoints.
    PrimitiveNotAdmitted { index: usize },
    /// The term is not the canonical construction for the closure pattern it
    /// exhibits (binder count, names, or indices differ).
    NonCanonicalTerm,
    /// The kernel did not accept the proof. Carries the verdict verbatim:
    /// `Rejected`, `Malformed`, `Unsupported` and `ContextMismatch` are
    /// absence of acceptance, and `ResourceExhausted` is *Unknown*, never
    /// falsity.
    NotAccepted(Verdict),
}

/// A kernel-accepted tree proof together with the account of what it closed.
///
/// **Private fields, no constructor**: this is the only thing that may
/// authorize a closure claim, so it cannot be assembled from parts.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct AcceptedTreeProof {
    context: ContextId,
    proposition: Prop,
    term: ExplicitTerm,
    certificate: Certificate,
    derivation: TreeDerivationId,
    occurrences: Vec<LeafOccurrence>,
}

impl AcceptedTreeProof {
    /// The context the proof was accepted in.
    pub fn context(&self) -> &ContextId {
        &self.context
    }

    /// The exact proposition the kernel accepted.
    pub fn proposition(&self) -> &Prop {
        &self.proposition
    }

    /// The identity of [`Self::proposition`].
    pub fn proposition_id(&self) -> PropositionId {
        self.proposition.proposition_id()
    }

    /// The exact proof term the kernel accepted.
    pub fn term(&self) -> &ExplicitTerm {
        &self.term
    }

    /// The kernel certificate for `(context, proposition, term)`.
    pub fn certificate(&self) -> &Certificate {
        &self.certificate
    }

    /// The derivation artifact this proof was constructed from.
    pub fn derivation_id(&self) -> TreeDerivationId {
        self.derivation
    }

    /// Every leaf occurrence, in traversal order.
    pub fn occurrences(&self) -> &[LeafOccurrence] {
        &self.occurrences
    }

    /// The occurrences closed by an accepted primitive instance.
    pub fn closed_occurrences(&self) -> impl Iterator<Item = &LeafOccurrence> {
        self.occurrences.iter().filter(|o| o.is_closed())
    }

    /// The occurrences that remain hypotheses — the proposition's antecedents.
    pub fn remaining_occurrences(&self) -> impl Iterator<Item = &LeafOccurrence> {
        self.occurrences.iter().filter(|o| !o.is_closed())
    }

    /// Whether this proof was constructed from exactly `derivation`: same
    /// artifact identity, and the same leaf occurrences (generator and
    /// endpoints) at the same positions.
    pub fn binds(&self, derivation: &TreeDerivation) -> bool {
        if self.derivation != derivation.id() {
            return false;
        }
        let leaves = derivation.tree().leaves();
        leaves.len() == self.occurrences.len()
            && leaves.iter().zip(&self.occurrences).all(|(leaf, occ)| {
                matches!(leaf, RealizesTree::Leaf { generator, src, dst }
                    if *generator == occ.generator && *src == occ.src && *dst == occ.dst)
            })
    }

    /// Independently re-verify this proof: reconstruct, from `derivation` and
    /// the **independently expected** `context`, the proposition this term must
    /// prove, and run the kernel again.
    ///
    /// A proof that was accepted in another context, built from another
    /// derivation, or whose stored fields were disturbed fails here. Encoding
    /// or digest validation alone is not acceptance, and this does not stop at
    /// either.
    pub fn recheck(
        &self,
        derivation: &TreeDerivation,
        context: &ContextId,
        budget: Budget,
    ) -> Result<(), TreeProofError> {
        let again = verify_tree_proof(derivation, context, &self.term, budget)?;
        if again == *self {
            Ok(())
        } else {
            Err(TreeProofError::NonCanonicalTerm)
        }
    }
}

/// The primitive relations a leaf may be closed by, in the order they are
/// tried. Resolved once: a relation id is content-derived, and
/// deriving it walks the whole row set.
fn closing_candidates() -> &'static [PrimitiveRelationId] {
    static CANDIDATES: OnceLock<[PrimitiveRelationId; 3]> = OnceLock::new();
    CANDIDATES.get_or_init(|| {
        [
            typing_arith_v3(),
            typing_arith_input_v1(),
            typing_arith_v2(),
        ]
    })
}

/// Whether `relation` may close the leaf `(generator, src, dst)`.
///
/// The kernel still decides acceptance. This is the *selection* rule: the
/// relation must be a **typing** relation, its own generator must be the
/// leaf's (the generator comes from the relation identity, never from the
/// caller), and the leaf's own endpoints must be exact members. Membership is
/// asked of the kernel's relation, not of a table kept here.
fn relation_closes(
    relation: &PrimitiveRelationId,
    generator: &GeneratorId,
    src: &TreeObj,
    dst: &TreeObj,
) -> bool {
    let Some(resolved) = resolve_primitive_relation(relation) else {
        return false;
    };
    if resolved.judgment_kind != JudgmentKind::Typing || resolved.generator != *generator {
        return false;
    }
    let src_term = tree_obj_to_object_term(src);
    let dst_term = tree_obj_to_object_term(dst);

    if !resolved.accepts_endpoint_shapes(&src_term, &dst_term) {
        return false;
    }

    let src_id = match &src_term {
        ObjectTerm::Const(id) => *id,
        ObjectTerm::Tensor(..) => PropositionId::of(&src_term),
        _ => return false,
    };
    let dst_id = match &dst_term {
        ObjectTerm::Const(id) => *id,
        ObjectTerm::Tensor(..) => PropositionId::of(&dst_term),
        _ => return false,
    };

    resolved.admits(&src_id, &dst_id)
}

/// The deterministic closure choice for one leaf occurrence.
fn classify_leaf(
    generator: &GeneratorId,
    src: &TreeObj,
    dst: &TreeObj,
) -> Option<PrimitiveRelationId> {
    closing_candidates()
        .iter()
        .find(|relation| relation_closes(relation, generator, src, dst))
        .copied()
}

type Closures = Vec<Option<PrimitiveRelationId>>;

/// The leaves of `tree` as `(generator, src, dst)`, or `None` if the tree is
/// malformed, empty, or `leaves()` yielded a non-leaf.
fn leaf_triples(tree: &RealizesTree) -> Option<Vec<(GeneratorId, &TreeObj, &TreeObj)>> {
    if !tree.well_formed() {
        return None;
    }
    let leaves = tree.leaves();
    if leaves.is_empty() && !tree.contains_id() {
        return None;
    }
    leaves
        .into_iter()
        .map(|leaf| match leaf {
            RealizesTree::Leaf {
                generator,
                src,
                dst,
            } => Some((*generator, src, dst)),
            _ => None,
        })
        .collect()
}

/// The deterministic closure choice for every leaf of `tree`, in traversal
/// order.
fn classify(tree: &RealizesTree) -> Option<Closures> {
    leaf_triples(tree).map(|leaves| {
        leaves
            .into_iter()
            .map(|(generator, src, dst)| classify_leaf(&generator, src, dst))
            .collect()
    })
}

struct Emit<'a> {
    closures: &'a [Option<PrimitiveRelationId>],
    leaves: &'a [(GeneratorId, &'a TreeObj, &'a TreeObj)],
    /// Hypotheses in the remaining list.
    open: usize,
    /// Leaf occurrence cursor.
    next: usize,
    /// Number of remaining hypotheses, fixed before the walk.
    binders: usize,
}

fn emit_term(tree: &RealizesTree, e: &mut Emit<'_>) -> TermKind {
    match tree {
        RealizesTree::Leaf { .. } => {
            let i = e.next;
            e.next += 1;
            match e.closures[i] {
                Some(relation) => {
                    let (_, src, dst) = e.leaves[i];
                    TermKind::PrimRealizes {
                        relation,
                        src: tree_obj_to_object_term(src),
                        dst: tree_obj_to_object_term(dst),
                    }
                }
                None => {
                    // De Bruijn index from the position in the *remaining*
                    // hypothesis list: the first remaining binder is outermost.
                    let idx = e.binders - 1 - e.open;
                    e.open += 1;
                    TermKind::Hyp(Var::Index(idx))
                }
            }
        }
        RealizesTree::Seq { left, right } => {
            let left = emit_term(left, e);
            let right = emit_term(right, e);
            TermKind::RealizesComp {
                left: Box::new(left),
                right: Box::new(right),
            }
        }
        RealizesTree::Tensor { left, right } => {
            let left = emit_term(left, e);
            let right = emit_term(right, e);
            TermKind::RealizesTensor {
                left: Box::new(left),
                right: Box::new(right),
            }
        }
        RealizesTree::Id(obj) => TermKind::RealizesId(tree_obj_to_object_term(obj)),
    }
}

/// Build the proposition and proof term for `tree` under `closures`.
///
/// The proposition is `H_1 → … → H_k → conclusion` over the **remaining**
/// hypotheses only, and the term is `k` lambdas over the body in leaf order.
/// With no closure this is byte-identical to the construction `elaborate_tree`
/// always had, which is what keeps every pre-existing identity unchanged.
fn build_proof(
    tree: &RealizesTree,
    leaves: &[(GeneratorId, &TreeObj, &TreeObj)],
    closures: &[Option<PrimitiveRelationId>],
    context: ContextId,
) -> (Prop, ExplicitTerm) {
    let mut proposition = Prop::Realizes(
        witness_object(tree),
        tree_obj_to_object_term(&tree.src()),
        tree_obj_to_object_term(&tree.dst()),
    );
    let open_props: Vec<Prop> = leaves
        .iter()
        .zip(closures)
        .filter(|(_, closure)| closure.is_none())
        .map(|((generator, src, dst), _)| {
            Prop::Realizes(
                ObjectTerm::Const(PropositionId(generator.digest())),
                tree_obj_to_object_term(src),
                tree_obj_to_object_term(dst),
            )
        })
        .collect();
    let binders = open_props.len();
    for h in open_props.into_iter().rev() {
        proposition = Prop::Impl(Box::new(h), Box::new(proposition));
    }

    let mut emit = Emit {
        closures,
        leaves,
        open: 0,
        next: 0,
        binders,
    };
    let mut kind = emit_term(tree, &mut emit);
    for i in (0..binders).rev() {
        kind = TermKind::Lam {
            var_name: Some(format!("h{}", i + 1)),
            body: Box::new(kind),
        };
    }
    (proposition, ExplicitTerm::new(context, kind))
}

/// The two pieces `elaborate_tree_certified` needs before the kernel runs.
pub(crate) struct Candidate {
    pub(crate) proposition: Prop,
    pub(crate) term: ExplicitTerm,
    closures: Closures,
}

/// Classify `tree` and construct the candidate proof, or `None` if the tree is
/// malformed or empty.
pub(crate) fn candidate(tree: &RealizesTree, context: ContextId) -> Option<Candidate> {
    let leaves = leaf_triples(tree)?;
    let closures = classify(tree)?;
    let (proposition, term) = build_proof(tree, &leaves, &closures, context);
    Some(Candidate {
        proposition,
        term,
        closures,
    })
}

/// Mint the artifact from an **accepted** candidate. The only caller passes a
/// certificate it got from `Verdict::Accepted`.
pub(crate) fn accepted(
    derivation: &TreeDerivation,
    context: ContextId,
    candidate: Candidate,
    certificate: Certificate,
) -> AcceptedTreeProof {
    let leaves = derivation.tree().leaves();
    let occurrences = leaves
        .into_iter()
        .zip(&candidate.closures)
        .enumerate()
        .filter_map(|(index, (leaf, closure))| match leaf {
            RealizesTree::Leaf {
                generator,
                src,
                dst,
            } => Some(LeafOccurrence {
                index,
                generator: *generator,
                src: src.clone(),
                dst: dst.clone(),
                status: match closure {
                    Some(relation) => LeafStatus::Closed(*relation),
                    None => LeafStatus::Open,
                },
            }),
            _ => None,
        })
        .collect();
    AcceptedTreeProof {
        context,
        proposition: candidate.proposition,
        term: candidate.term,
        certificate,
        derivation: derivation.id(),
        occurrences,
    }
}

fn read_closures(
    tree: &RealizesTree,
    kind: &TermKind,
    out: &mut Closures,
) -> Result<(), TreeProofError> {
    match (tree, kind) {
        (RealizesTree::Leaf { .. }, TermKind::Hyp(_)) => {
            out.push(None);
            Ok(())
        }
        (RealizesTree::Leaf { .. }, TermKind::PrimRealizes { relation, .. }) => {
            out.push(Some(*relation));
            Ok(())
        }
        (
            RealizesTree::Seq { left, right },
            TermKind::RealizesComp {
                left: tl,
                right: tr,
            },
        )
        | (
            RealizesTree::Tensor { left, right },
            TermKind::RealizesTensor {
                left: tl,
                right: tr,
            },
        ) => {
            read_closures(left, tl, out)?;
            read_closures(right, tr, out)
        }
        (RealizesTree::Id(obj), TermKind::RealizesId(t)) => {
            if tree_obj_to_object_term(obj) == *t {
                Ok(())
            } else {
                Err(TreeProofError::ShapeMismatch)
            }
        }
        _ => Err(TreeProofError::ShapeMismatch),
    }
}

/// Verify an **untrusted** proof term against a derivation and an
/// independently expected context, and mint an [`AcceptedTreeProof`] only if
/// the kernel accepts it.
///
/// The proposition is never an input. It is reconstructed from `derivation`
/// and from which occurrences the term closes, and each claimed closure must be
/// one the leaf could have been closed by ([`TreeProofError::PrimitiveNotAdmitted`]).
/// The term must then equal the canonical construction for that pattern, and
/// the kernel runs against the reconstructed proposition in `context`.
///
/// An all-hypothesis term is a valid *conditional* proof: it verifies, and the
/// result closes nothing.
pub fn verify_tree_proof(
    derivation: &TreeDerivation,
    context: &ContextId,
    term: &ExplicitTerm,
    budget: Budget,
) -> Result<AcceptedTreeProof, TreeProofError> {
    if !derivation.is_structure_verified() {
        return Err(TreeProofError::DerivationNotVerified);
    }
    let tree = derivation.tree();
    let leaves = leaf_triples(tree).ok_or(TreeProofError::DerivationNotVerified)?;

    let mut body = &term.kind;
    while let TermKind::Lam { body: inner, .. } = body {
        body = inner;
    }
    let mut closures = Closures::with_capacity(leaves.len());
    read_closures(tree, body, &mut closures)?;
    if closures.len() != leaves.len() {
        return Err(TreeProofError::ShapeMismatch);
    }

    for (index, (closure, (generator, src, dst))) in closures.iter().zip(&leaves).enumerate() {
        if let Some(relation) = closure {
            if !relation_closes(relation, generator, src, dst) {
                return Err(TreeProofError::PrimitiveNotAdmitted { index });
            }
        }
    }

    let (proposition, expected) = build_proof(tree, &leaves, &closures, *context);
    // Kinds only: a term embedding a different context is the kernel's
    // `ContextMismatch` to report, not a shape complaint.
    if expected.kind != term.kind {
        return Err(TreeProofError::NonCanonicalTerm);
    }

    match acceptance(context, &proposition, term, budget) {
        Verdict::Accepted(certificate) => Ok(accepted(
            derivation,
            *context,
            Candidate {
                proposition,
                term: expected,
                closures,
            },
            certificate,
        )),
        verdict => Err(TreeProofError::NotAccepted(verdict)),
    }
}
