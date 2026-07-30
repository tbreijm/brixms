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
}

impl NExpr {
    pub fn origin(&self) -> Origin {
        match self {
            NExpr::Lit { origin, .. } => *origin,
            NExpr::Var { origin, .. } => *origin,
            NExpr::Call { origin, .. } => *origin,
            NExpr::Field { origin, .. } => *origin,
            NExpr::Record { origin, .. } => *origin,
        }
    }
}

/// Native function signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NSig {
    pub params: Vec<NTy>,
    pub ret: NTy,
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

/// Native query representation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeQuery {
    pub name: Sym,
    pub params: Vec<(Sym, NTy)>,
    pub yields: NExpr,
    pub result: NTy,
}

/// Native source container holding queries, function signatures, and guard expressions.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct NativeSource {
    pub queries: Vec<NativeQuery>,
    pub sigs: SigTable,
    pub guards: Vec<NExpr>,
}
