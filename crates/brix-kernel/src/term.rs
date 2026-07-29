//! Proof term representations (`ExplicitTerm`, `TermKind`, `Var`), object terms (`ObjectTerm`), and propositions (`Prop`).

use brix_canon::{CanonWriter, Canonical};
use brix_semantic::{compose, ContextId, PropositionId, WitnessId};

/// Object terms (ADR-0003 §5).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ObjectTerm {
    /// Content-addressed object constant.
    Const(PropositionId),
    /// de Bruijn index over object binders.
    BoundVar(usize),
    /// Generator composition compose(g2, g1) meaning outer g2, inner g1 (Profile 1.1).
    Compose(Box<ObjectTerm>, Box<ObjectTerm>),
}

impl ObjectTerm {
    /// Compute the [`WitnessId`] identity of this object term.
    ///
    /// - `Const(id)`: returns `WitnessId(id.digest())`, matching `brix-semantic`'s primitive
    ///   generator witness identity derivation (`WitnessId(generator_id.digest())`).
    /// - `Compose(outer, inner)`: recursively computes and returns
    ///   `compose(outer.witness_digest(), inner.witness_digest())`.
    /// - `BoundVar(_)`: a bound variable in witness position is malformed; returns a deterministic
    ///   sentinel `WitnessId::from_canon(b"brix.kernel.witness_digest.bound_var")`.
    pub fn witness_digest(&self) -> WitnessId {
        match self {
            ObjectTerm::Const(id) => WitnessId(id.digest()),
            ObjectTerm::Compose(outer, inner) => {
                compose(outer.witness_digest(), inner.witness_digest())
            }
            ObjectTerm::BoundVar(_) => {
                WitnessId::from_canon(b"brix.kernel.witness_digest.bound_var")
            }
        }
    }
}

/// A proposition in intuitionistic propositional logic (Profile 1, extended in Slice 2b).
///
/// Supports atomic propositions, implications (\(P \to Q\)), finite products
/// (\(P \times Q\)), finite sums (\(P + Q\)), equality (\(t_1 = t_2\)),
/// existentials (\(\exists x. P(x)\)), predicate applications, realization, and
/// transformation preservation.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Prop {
    /// Atomic proposition identified by a content-addressed [`PropositionId`].
    Atom(PropositionId),
    /// Implication \(P \to Q\).
    Impl(Box<Prop>, Box<Prop>),
    /// Finite product / conjunction \(P \times Q\).
    Prod(Box<Prop>, Box<Prop>),
    /// Finite sum / disjunction \(P + Q\).
    Sum(Box<Prop>, Box<Prop>),

    // Slice 2b additions (append-only ordinals AFTER Sum=3)
    /// Equality construct \(t_1 = t_2\).
    Eq(ObjectTerm, ObjectTerm),
    /// Existential construct \(\exists x. P(x)\) (body under one fresh object binder).
    Exists(Box<Prop>),
    /// Predicate application symbol with arguments.
    Applied(PropositionId, Vec<ObjectTerm>),
    /// Realization statement: \(w\) realizes transition from \(x\) to \(y\).
    Realizes(ObjectTerm, ObjectTerm, ObjectTerm),
    /// Transformation preservation: witness \(w\) preserves motive \(P\).
    Preserves(ObjectTerm, Box<Prop>),
}

/// Variable reference into context \(\Gamma\).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Var {
    /// de Bruijn index (0-based from top of context stack).
    Index(usize),
    /// Named variable reference.
    Named(String),
}

/// The core explicit proof-term constructs admitted in Profile 1 (ADR-0003 §5).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum TermKind {
    /// Hypothesis lookup in context (\(\text{Hyp}\)).
    Hyp(Var),
    /// Implication introduction (\(\rightarrow I\)): \(\lambda x. t\).
    Lam {
        var_name: Option<String>,
        body: Box<TermKind>,
    },
    /// Implication elimination (\(\rightarrow E\)): \(f \, a\).
    App {
        function: Box<TermKind>,
        argument: Box<TermKind>,
    },
    /// Finite product introduction (\(\times I\)): \(\langle a, b \rangle\).
    Pair {
        fst: Box<TermKind>,
        snd: Box<TermKind>,
    },
    /// Finite product elimination 1 (\(\times E_1\)): \(\pi_1(p)\).
    Proj1(Box<TermKind>),
    /// Finite product elimination 2 (\(\times E_2\)): \(\pi_2(p)\).
    Proj2(Box<TermKind>),
    /// Finite sum introduction 1 (\(+ I_1\)): \(\mathsf{inl}(a)\).
    Inl(Box<TermKind>),
    /// Finite sum introduction 2 (\(+ I_2\)): \(\mathsf{inr}(b)\).
    Inr(Box<TermKind>),
    /// Finite sum elimination (\(+ E\)): \(\mathsf{case}(s, x. u, y. v)\).
    Case {
        discriminant: Box<TermKind>,
        left_var: Option<String>,
        left_body: Box<TermKind>,
        right_var: Option<String>,
        right_body: Box<TermKind>,
    },
    /// Placeholder for constructs explicitly unsupported in Slice 1.
    Unsupported(String),

    // Slice 2b additions (append-only ordinals AFTER Unsupported=9)
    /// Equality introduction (\(= I\)): \(\mathsf{refl}(t)\).
    Refl(ObjectTerm),
    /// Equality elimination / substitution (\(= E\)): \(\mathsf{subst}(e, P, p)\).
    Subst {
        eq: Box<TermKind>,
        motive: Box<Prop>,
        sub: Box<TermKind>,
    },
    /// Existential introduction (\(\exists I\)): \(\mathsf{pack}(w, p)\).
    Pack {
        witness: ObjectTerm,
        body_proof: Box<TermKind>,
    },
    /// Existential elimination (\(\exists E\)): \(\mathsf{unpack}(e, x.y. t)\).
    Unpack {
        scrutinee: Box<TermKind>,
        obj_var: Option<String>,
        proof_var: Option<String>,
        body: Box<TermKind>,
    },
    /// Transformation preservation (\(\text{Trans-Pres}\)): \(\mathsf{pres}(w, \pi, p)\).
    Pres {
        realizes: Box<TermKind>,
        preserves: Box<TermKind>,
        motive: Box<Prop>,
        sub: Box<TermKind>,
    },
    /// Realization composition (\(\text{RealizesComp}\)): \(\mathsf{realizes\_comp}(p, q)\) (Profile 1.1).
    RealizesComp {
        left: Box<TermKind>,
        right: Box<TermKind>,
    },
}

/// Fully explicit, canonical proof-term artifact carrying its embedded context digest.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ExplicitTerm {
    /// Embedded assumption context digest.
    pub context: ContextId,
    /// Root term construct.
    pub kind: TermKind,
}

impl ExplicitTerm {
    /// Create a new explicit term.
    pub fn new(context: ContextId, kind: TermKind) -> Self {
        Self { context, kind }
    }
}

/// Instantiate predicate `pred` (a [`Prop`] containing object `BoundVar(0)`)
/// with `replacement` term.
///
/// This is a total, capture-avoiding substitution.
pub fn instantiate(pred: &Prop, replacement: &ObjectTerm) -> Prop {
    subst_prop(pred, replacement, 0)
}

fn shift_object_term(term: &ObjectTerm, amount: usize) -> ObjectTerm {
    if amount == 0 {
        return term.clone();
    }
    match term {
        ObjectTerm::Const(c) => ObjectTerm::Const(*c),
        ObjectTerm::BoundVar(idx) => ObjectTerm::BoundVar(idx + amount),
        ObjectTerm::Compose(g2, g1) => ObjectTerm::Compose(
            Box::new(shift_object_term(g2, amount)),
            Box::new(shift_object_term(g1, amount)),
        ),
    }
}

fn subst_obj(term: &ObjectTerm, replacement: &ObjectTerm, depth: usize) -> ObjectTerm {
    match term {
        ObjectTerm::Const(c) => ObjectTerm::Const(*c),
        ObjectTerm::BoundVar(idx) => {
            if *idx == depth {
                shift_object_term(replacement, depth)
            } else if *idx > depth {
                ObjectTerm::BoundVar(idx - 1)
            } else {
                ObjectTerm::BoundVar(*idx)
            }
        }
        ObjectTerm::Compose(g2, g1) => ObjectTerm::Compose(
            Box::new(subst_obj(g2, replacement, depth)),
            Box::new(subst_obj(g1, replacement, depth)),
        ),
    }
}

fn subst_prop(prop: &Prop, replacement: &ObjectTerm, depth: usize) -> Prop {
    match prop {
        Prop::Atom(id) => Prop::Atom(*id),
        Prop::Impl(p1, p2) => Prop::Impl(
            Box::new(subst_prop(p1, replacement, depth)),
            Box::new(subst_prop(p2, replacement, depth)),
        ),
        Prop::Prod(p1, p2) => Prop::Prod(
            Box::new(subst_prop(p1, replacement, depth)),
            Box::new(subst_prop(p2, replacement, depth)),
        ),
        Prop::Sum(p1, p2) => Prop::Sum(
            Box::new(subst_prop(p1, replacement, depth)),
            Box::new(subst_prop(p2, replacement, depth)),
        ),
        Prop::Eq(t1, t2) => Prop::Eq(
            subst_obj(t1, replacement, depth),
            subst_obj(t2, replacement, depth),
        ),
        Prop::Exists(body) => Prop::Exists(Box::new(subst_prop(body, replacement, depth + 1))),
        Prop::Applied(sym, args) => Prop::Applied(
            *sym,
            args.iter()
                .map(|arg| subst_obj(arg, replacement, depth))
                .collect(),
        ),
        Prop::Realizes(w, x, y) => Prop::Realizes(
            subst_obj(w, replacement, depth),
            subst_obj(x, replacement, depth),
            subst_obj(y, replacement, depth),
        ),
        Prop::Preserves(w, motive) => Prop::Preserves(
            subst_obj(w, replacement, depth),
            Box::new(subst_prop(motive, replacement, depth)),
        ),
    }
}

impl Canonical for ObjectTerm {
    fn canon_write(&self, w: &mut CanonWriter) {
        match self {
            ObjectTerm::Const(id) => w.write_enum(0, |w| id.canon_write(w)),
            ObjectTerm::BoundVar(idx) => w.write_enum(1, |w| (*idx as u64).canon_write(w)),
            ObjectTerm::Compose(g2, g1) => w.write_enum(2, |w| {
                g2.canon_write(w);
                g1.canon_write(w);
            }),
        }
    }
}

impl Canonical for Prop {
    fn canon_write(&self, w: &mut CanonWriter) {
        match self {
            Prop::Atom(id) => w.write_enum(0, |w| id.canon_write(w)),
            Prop::Impl(p1, p2) => w.write_enum(1, |w| {
                p1.canon_write(w);
                p2.canon_write(w);
            }),
            Prop::Prod(p1, p2) => w.write_enum(2, |w| {
                p1.canon_write(w);
                p2.canon_write(w);
            }),
            Prop::Sum(p1, p2) => w.write_enum(3, |w| {
                p1.canon_write(w);
                p2.canon_write(w);
            }),
            Prop::Eq(t1, t2) => w.write_enum(4, |w| {
                t1.canon_write(w);
                t2.canon_write(w);
            }),
            Prop::Exists(body) => w.write_enum(5, |w| body.canon_write(w)),
            Prop::Applied(sym, args) => w.write_enum(6, |w| {
                sym.canon_write(w);
                args.canon_write(w);
            }),
            Prop::Realizes(w_term, x_term, y_term) => w.write_enum(7, |w| {
                w_term.canon_write(w);
                x_term.canon_write(w);
                y_term.canon_write(w);
            }),
            Prop::Preserves(w_term, motive) => w.write_enum(8, |w| {
                w_term.canon_write(w);
                motive.canon_write(w);
            }),
        }
    }
}

impl Canonical for Var {
    fn canon_write(&self, w: &mut CanonWriter) {
        match self {
            Var::Index(idx) => w.write_enum(0, |w| (*idx as u64).canon_write(w)),
            Var::Named(name) => w.write_enum(1, |w| name.canon_write(w)),
        }
    }
}

impl Canonical for TermKind {
    fn canon_write(&self, w: &mut CanonWriter) {
        match self {
            TermKind::Hyp(var) => w.write_enum(0, |w| var.canon_write(w)),
            TermKind::Lam { var_name, body } => w.write_enum(1, |w| {
                var_name.canon_write(w);
                body.canon_write(w);
            }),
            TermKind::App { function, argument } => w.write_enum(2, |w| {
                function.canon_write(w);
                argument.canon_write(w);
            }),
            TermKind::Pair { fst, snd } => w.write_enum(3, |w| {
                fst.canon_write(w);
                snd.canon_write(w);
            }),
            TermKind::Proj1(inner) => w.write_enum(4, |w| inner.canon_write(w)),
            TermKind::Proj2(inner) => w.write_enum(5, |w| inner.canon_write(w)),
            TermKind::Inl(inner) => w.write_enum(6, |w| inner.canon_write(w)),
            TermKind::Inr(inner) => w.write_enum(7, |w| inner.canon_write(w)),
            TermKind::Case {
                discriminant,
                left_var,
                left_body,
                right_var,
                right_body,
            } => w.write_enum(8, |w| {
                discriminant.canon_write(w);
                left_var.canon_write(w);
                left_body.canon_write(w);
                right_var.canon_write(w);
                right_body.canon_write(w);
            }),
            TermKind::Unsupported(msg) => w.write_enum(9, |w| msg.canon_write(w)),
            TermKind::Refl(t) => w.write_enum(10, |w| t.canon_write(w)),
            TermKind::Subst { eq, motive, sub } => w.write_enum(11, |w| {
                eq.canon_write(w);
                motive.canon_write(w);
                sub.canon_write(w);
            }),
            TermKind::Pack {
                witness,
                body_proof,
            } => w.write_enum(12, |w| {
                witness.canon_write(w);
                body_proof.canon_write(w);
            }),
            TermKind::Unpack {
                scrutinee,
                obj_var,
                proof_var,
                body,
            } => w.write_enum(13, |w| {
                scrutinee.canon_write(w);
                obj_var.canon_write(w);
                proof_var.canon_write(w);
                body.canon_write(w);
            }),
            TermKind::Pres {
                realizes,
                preserves,
                motive,
                sub,
            } => w.write_enum(14, |w| {
                realizes.canon_write(w);
                preserves.canon_write(w);
                motive.canon_write(w);
                sub.canon_write(w);
            }),
            TermKind::RealizesComp { left, right } => w.write_enum(15, |w| {
                left.canon_write(w);
                right.canon_write(w);
            }),
        }
    }
}

impl Canonical for ExplicitTerm {
    fn canon_write(&self, w: &mut CanonWriter) {
        self.context.canon_write(w);
        self.kind.canon_write(w);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brix_semantic::{compose_chain, GeneratorId};

    #[test]
    fn test_witness_digest_alignment_with_brix_semantic_compose() {
        let g1_gen = GeneratorId::named("gen-step-1");
        let g2_gen = GeneratorId::named("gen-step-2");

        let g1_const = ObjectTerm::Const(PropositionId(g1_gen.digest()));
        let g2_const = ObjectTerm::Const(PropositionId(g2_gen.digest()));

        let term_comp = ObjectTerm::Compose(Box::new(g2_const), Box::new(g1_const));

        let expected = compose(WitnessId::from(g2_gen), WitnessId::from(g1_gen));
        assert_eq!(term_comp.witness_digest(), expected);
    }

    #[test]
    fn test_witness_digest_alignment_with_brix_semantic_compose_chain() {
        let g1 = GeneratorId::named("gen-1");
        let g2 = GeneratorId::named("gen-2");
        let g3 = GeneratorId::named("gen-3");

        let c1 = ObjectTerm::Const(PropositionId(g1.digest()));
        let c2 = ObjectTerm::Const(PropositionId(g2.digest()));
        let c3 = ObjectTerm::Const(PropositionId(g3.digest()));

        let c12 = ObjectTerm::Compose(Box::new(c2), Box::new(c1));
        let c123 = ObjectTerm::Compose(Box::new(c3), Box::new(c12));

        let expected_chain = compose_chain(&[g1, g2, g3]).unwrap();
        assert_eq!(c123.witness_digest(), expected_chain);
    }

    #[test]
    fn test_bound_var_witness_digest_returns_sentinel() {
        let bv = ObjectTerm::BoundVar(0);
        let sentinel = WitnessId::from_canon(b"brix.kernel.witness_digest.bound_var");
        assert_eq!(bv.witness_digest(), sentinel);
    }
}
