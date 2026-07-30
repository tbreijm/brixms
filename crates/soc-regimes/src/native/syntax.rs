//! Native syntax definitions for the native type checker (ADR-0009 §5).

use std::collections::BTreeMap;

/// Stable identity per expression node for the type-mirror.
pub type Origin = u64;

/// Symbol identifier for names.
pub type Sym = String;

/// Native row representation for record and rel types.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NRow {
    pub fields: Vec<(Sym, NTy)>,
    pub open: bool,
}

/// Native type representation in the native checker (ADR-0009 §5).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NTy {
    Unit,
    Bool,
    Str,
    Int,
    F64,
    Fn {
        params: Vec<NTy>,
        ret: Box<NTy>,
    },
    Record(NRow),
    Rel(NRow),
    Option(Box<NTy>),
    Result(Box<NTy>, Box<NTy>),
    Estimate(Box<NTy>),
    Missing(Box<NTy>),
    Probability,
    Quantity(Sym),
    Money(Sym),
    Dimensioned(Vec<(Sym, i64)>),
    Var(u32),
    /// Error unifies ONLY with itself; never bindable — mirrors brix_ir Ty::Error isolation.
    Error,
}

/// Native literal values.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NLit {
    Int(i64),
    Bool(bool),
    Str(String),
    Unit,
}

/// Native expression AST.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NExpr {
    Lit {
        origin: Origin,
        lit: NLit,
    },
    Var {
        origin: Origin,
        name: Sym,
        /// The node's own type annotation (`brix_ir::Expr::ty`). Used as the
        /// fallback when `name` is absent from the environment, mirroring
        /// `reflect`'s `env.get(name).unwrap_or(expr.ty)` (reflect.rs:1554).
        ty: Option<NTy>,
    },
    Call {
        origin: Origin,
        func: Sym,
        args: Vec<NExpr>,
    },
    Field {
        origin: Origin,
        base: Box<NExpr>,
        field: Sym,
    },
    Record {
        origin: Origin,
        fields: Vec<(Sym, NExpr)>,
    },
    Try {
        origin: Origin,
        inner: Box<NExpr>,
    },
}

impl NExpr {
    pub fn origin(&self) -> Origin {
        match self {
            NExpr::Lit { origin, .. } => *origin,
            NExpr::Var { origin, .. } => *origin,
            NExpr::Call { origin, .. } => *origin,
            NExpr::Field { origin, .. } => *origin,
            NExpr::Record { origin, .. } => *origin,
            NExpr::Try { origin, .. } => *origin,
        }
    }
}

/// Native effect representation (ADR-0009 §5).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NEffect {
    Net,
    Fs,
    Clock,
    Random,
    Console,
    GraphRead,
    GraphWrite,
    Panic,
    Diverge,
    Solver,
}

/// Native effect row representation.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct NEffectRow {
    pub atoms: Vec<NEffect>,
    pub open_tail: bool,
}

impl NEffectRow {
    pub fn is_pure(&self) -> bool {
        !self.open_tail
            && self
                .atoms
                .iter()
                .all(|a| matches!(a, NEffect::Panic | NEffect::Diverge))
    }

    pub fn nondet(&self) -> bool {
        self.open_tail
            || self.atoms.iter().any(|a| {
                matches!(
                    a,
                    NEffect::Random | NEffect::Clock | NEffect::Net | NEffect::Fs | NEffect::Solver
                )
            })
    }

    pub fn may_diverge(&self) -> bool {
        self.open_tail || self.atoms.contains(&NEffect::Diverge)
    }
}

/// Native function signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NSig {
    pub params: Vec<NTy>,
    pub ret: NTy,
    pub is_aggregate: bool,
    pub may_diverge: bool,
}

/// Signature table mapping function names to candidate signatures.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SigTable {
    pub sigs: BTreeMap<Sym, Vec<NSig>>,
}

impl SigTable {
    pub fn new() -> Self {
        Self {
            sigs: BTreeMap::new(),
        }
    }

    pub fn insert(&mut self, name: Sym, sig: NSig) {
        self.sigs.entry(name).or_default().push(sig);
    }

    pub fn get(&self, name: &str) -> Option<&[NSig]> {
        self.sigs.get(name).map(|v| v.as_slice())
    }
}

/// Relation schema specification mapping role names to native types.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct NRelSchema {
    pub roles: Vec<(Sym, NTy)>,
    pub derived: bool,
}

/// Argument inside an edge clause (literal value or variable name).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NArg {
    Lit(NLit),
    Var(Sym),
}

/// Relation Edge clause representation (ADR-0009 §5).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NEdge {
    pub relation: Sym,
    pub args: Vec<(Sym, NArg)>,
}

/// Native query representation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeQuery {
    pub name: Sym,
    pub params: Vec<(Sym, NTy)>,
    pub yields: NExpr,
    pub result: NTy,
}

/// Native rule head shape (ADR-0009 N8b-2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NHead {
    Tuple,
    Node { keyed_by: Vec<Sym> },
    Mask { target: Sym, reason: Sym },
}

/// Native rule representation (ADR-0009 N8b-1 / N8b-2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NRule {
    pub name: Sym,
    pub head: NHead,
    pub effects: NEffectRow,
    pub called_fns: Vec<Sym>,
    pub bound_vars: Vec<Sym>,
    pub edge_refs: Vec<Sym>,
    pub derived_rel_ordinary_consumption: Vec<Sym>,
}

/// Native source container holding queries, function signatures, guard expressions, relations, edges, and rules.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct NativeSource {
    pub queries: Vec<NativeQuery>,
    pub sigs: SigTable,
    pub guards: Vec<NExpr>,
    pub relations: BTreeMap<Sym, NRelSchema>,
    pub edges: Vec<NEdge>,
    pub rules: Vec<NRule>,
}
