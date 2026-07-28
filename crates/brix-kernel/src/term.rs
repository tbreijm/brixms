//! Proof term representations (`ExplicitTerm`, `TermKind`, `Var`) and propositions (`Prop`).

use brix_canon::{CanonWriter, Canonical};
use brix_semantic::{ContextId, PropositionId};

/// A proposition in intuitionistic propositional logic (Profile 1).
///
/// Supports atomic propositions, implications (\(P \to Q\)), finite products
/// (\(P \times Q\)), and finite sums (\(P + Q\)).
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
}

/// Canonical ABI ordinals for [`Prop`]. Append-only, never reorder.
impl Prop {
    const fn ordinal(&self) -> u64 {
        match self {
            Prop::Atom(_) => 0,
            Prop::Impl(_, _) => 1,
            Prop::Prod(_, _) => 2,
            Prop::Sum(_, _) => 3,
        }
    }
}

impl Canonical for Prop {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_enum(self.ordinal(), |w| match self {
            Prop::Atom(id) => id.canon_write(w),
            Prop::Impl(p1, p2) | Prop::Prod(p1, p2) | Prop::Sum(p1, p2) => {
                p1.canon_write(w);
                p2.canon_write(w);
            }
        });
    }
}

/// Variable reference into context \(\Gamma\).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Var {
    /// de Bruijn index (0-based from top of context stack).
    Index(usize),
    /// Named variable reference.
    Named(String),
}

/// Canonical ABI ordinals for [`Var`]. Append-only, never reorder.
impl Var {
    const fn ordinal(&self) -> u64 {
        match self {
            Var::Index(_) => 0,
            Var::Named(_) => 1,
        }
    }
}

impl Canonical for Var {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_enum(self.ordinal(), |w| match self {
            Var::Index(idx) => w.write_uint(*idx as u64),
            Var::Named(name) => w.write_str(name),
        });
    }
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
}

/// Canonical ABI ordinals for [`TermKind`]. Append-only, never reorder.
impl TermKind {
    const fn ordinal(&self) -> u64 {
        match self {
            TermKind::Hyp(_) => 0,
            TermKind::Lam { .. } => 1,
            TermKind::App { .. } => 2,
            TermKind::Pair { .. } => 3,
            TermKind::Proj1(_) => 4,
            TermKind::Proj2(_) => 5,
            TermKind::Inl(_) => 6,
            TermKind::Inr(_) => 7,
            TermKind::Case { .. } => 8,
            TermKind::Unsupported(_) => 9,
        }
    }
}

impl Canonical for TermKind {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_enum(self.ordinal(), |w| match self {
            TermKind::Hyp(var) => var.canon_write(w),
            TermKind::Lam { var_name, body } => {
                var_name.canon_write(w);
                body.canon_write(w);
            }
            TermKind::App { function, argument } => {
                function.canon_write(w);
                argument.canon_write(w);
            }
            TermKind::Pair { fst, snd } => {
                fst.canon_write(w);
                snd.canon_write(w);
            }
            TermKind::Proj1(inner)
            | TermKind::Proj2(inner)
            | TermKind::Inl(inner)
            | TermKind::Inr(inner) => {
                inner.canon_write(w);
            }
            TermKind::Case {
                discriminant,
                left_var,
                left_body,
                right_var,
                right_body,
            } => {
                discriminant.canon_write(w);
                left_var.canon_write(w);
                left_body.canon_write(w);
                right_var.canon_write(w);
                right_body.canon_write(w);
            }
            TermKind::Unsupported(msg) => w.write_str(msg),
        });
    }
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

impl Canonical for ExplicitTerm {
    fn canon_write(&self, w: &mut CanonWriter) {
        self.context.canon_write(w);
        self.kind.canon_write(w);
    }
}
