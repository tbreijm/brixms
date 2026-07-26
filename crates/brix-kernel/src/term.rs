//! Proof term representations (`ExplicitTerm`, `TermKind`, `Var`) and propositions (`Prop`).

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
