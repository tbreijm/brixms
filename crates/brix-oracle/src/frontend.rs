//! Adapter: `brix-ir`'s checked [`FrontendSource`] (real, parsed + lowered +
//! type-checked BrixMS source) → [`crate::program::Program`] (this crate's
//! own reduced settlement IR). Issue #24 (Ring 0 G1): replaces hand-built
//! `dsl.rs` construction as the *only* path a real program takes to reach
//! the oracle.
//!
//! Scope, matching exactly what the flagship program (spec Part I) uses —
//! fails closed (a clear [`AdapterError`], never a silent wrong answer) on
//! anything beyond it:
//! - Clauses: `Edge`, `Entity`, `Without`, `When`, `Let`, `History` (`Any`/
//!   `Exists`/`Optional`/`Cross` are real Core IR forms the flagship's own
//!   rules never use, so they're `Unsupported` rather than guessed at).
//! - Exprs: `Var`, `Lit`, `Call`, `Try` (`Field`/`Record`/`If`/
//!   `Comprehension` are unused by the flagship).
//! - Heads: `Tuple`, `Mask` (`Node`/Skolem heads are unused by the
//!   flagship).
//! - Queries are **not** converted — [`crate::program::Program`] has no
//!   query concept at all; see the crate-level scope note.
//!
//! Value erasure (the oracle has no float variant, by design — see
//! [`crate::value`]): the one place a float genuinely appears in Core IR,
//! a literal `Lit::F64Bits`, is decoded once at adapt time and rescaled to
//! `Value::Int` basis points (`0.8` -> `8000`) — the same convention every
//! `Probability`-typed value in a hand-authored transaction must follow
//! (see `crates/brix-oracle/tests/flagship.rs`).
//!
//! [`SchemaResolver`] only supports point lookup by name (no enumeration)
//! and carries no entity-ness or Ground/State/Event distinction (that's
//! discarded during lowering today — a known, narrow gap, not fixed here).
//! Callers supply a [`KindTable`] naming every relation's real
//! [`RelKind`]; only `Derived` is inferred automatically, from
//! `RelationSchema::derived`, which lowering does preserve correctly. Every
//! relation named in the table is also included in the adapted `Program`
//! even if no rule/constraint body ever reads or writes it — this is what
//! makes a relation reachable only as another entity's field type (`Order
//! .client: Client`, never independently pattern-matched) still available
//! to `ensure`/`assert` real transaction data into.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use brix_ir::core::{
    Constraint as IrConstraint, Expr as IrExpr, ExprKind, Head as IrHead, Rule as IrRule,
    Severity as IrSeverity,
};
use brix_ir::frontend::{FrontendSource, RelationSchema, SchemaResolver};
use brix_ir::ident::QualIdent;
use brix_ir::pattern::{Arg, Clause as IrClause, Lit, Pattern, RoleArg};

use crate::program::{
    BinOp, Clause, Constraint, Expr, Head, PartialFn, Program, RelKind, RelationDef, Rule,
    Severity, Term, TotalFn,
};
use crate::value::Value;

/// A relation's real oracle [`RelKind`] — see the module doc for why this
/// can't be recovered mechanically from [`RelationSchema`] alone. An entry
/// for a relation whose kind *is* inferable (`Derived`) is harmless
/// (checked, not required); every `Entity`/`Ground`/`State`/`Event`
/// relation needs one.
pub type KindTable = BTreeMap<String, RelKind>;

/// Hand-registered Rust implementations for every function a real
/// program's rule/constraint bodies call. `Program.fns`/`partial_fns` are
/// plain Rust closures the caller supplies — Core IR carries no executable
/// function bodies to derive them from (`program.rs`'s own doc:
/// "determinism is the caller's obligation").
#[derive(Default)]
pub struct FnLibrary {
    pub fns: BTreeMap<String, TotalFn>,
    pub partial_fns: BTreeMap<String, PartialFn>,
}

impl FnLibrary {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_fn(mut self, name: impl Into<String>, f: TotalFn) -> Self {
        self.fns.insert(name.into(), f);
        self
    }

    pub fn with_partial_fn(mut self, name: impl Into<String>, f: PartialFn) -> Self {
        self.partial_fns.insert(name.into(), f);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterError {
    /// A relation referenced by a rule/constraint has no schema entry.
    UnknownRelation(String),
    /// A `mask(target) by reason` head whose `target` binding could not be
    /// resolved to a relation via the rule's own body (Part III §6).
    MaskTargetUnresolved(String),
    /// A real Core IR construct this adapter deliberately does not convert
    /// (see the module doc's scope list).
    Unsupported(&'static str),
}

impl fmt::Display for AdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AdapterError::UnknownRelation(name) => {
                write!(f, "no schema entry for relation `{name}`")
            }
            AdapterError::MaskTargetUnresolved(target) => write!(
                f,
                "mask target `{target}` is not bound by an edge clause in its own rule body"
            ),
            AdapterError::Unsupported(what) => {
                write!(f, "unsupported by the oracle adapter: {what}")
            }
        }
    }
}

impl std::error::Error for AdapterError {}

/// Convert a checked [`FrontendSource`] into a [`Program`] the oracle can
/// settle. `kinds`/`fns` supply what Core IR cannot (decision 4/6 of the
/// issue #24 plan): relation kinds Core IR doesn't preserve, and function
/// implementations Core IR never carries.
pub fn program_from_source(
    source: &FrontendSource,
    resolver: &dyn SchemaResolver,
    kinds: &KindTable,
    fns: FnLibrary,
) -> Result<Program, AdapterError> {
    let mut relations_seen: BTreeSet<QualIdent> = BTreeSet::new();

    let mut rules = Vec::with_capacity(source.rules.len());
    for r in &source.rules {
        rules.push(convert_rule(r, &mut relations_seen)?);
    }
    let mut constraints = Vec::with_capacity(source.constraints.len());
    for c in &source.constraints {
        constraints.push(convert_constraint(c, &mut relations_seen)?);
    }

    // `SchemaResolver` has no enumeration method, so a relation that no
    // rule/constraint body ever reads or writes — one only referenced as
    // another entity's field type (`Order.client: Client`), never
    // pattern-matched directly — would otherwise never be discovered, even
    // though a transaction may still need to `ensure`/`assert` ground data
    // into it. `kinds` already has to name every such relation (to supply
    // the `RelKind` this adapter can't otherwise recover), so it doubles
    // as the second discovery source.
    for name in kinds.keys() {
        relations_seen.insert(QualIdent::from(name.as_str()));
    }

    let mut program = Program::new();
    program.fns = fns.fns;
    program.partial_fns = fns.partial_fns;

    // Functions compiled from source (issue #47): the oracle evaluates the
    // lowered body directly, so a total fn need not be hand-registered in the
    // `FnLibrary`. A body form the oracle expr language can't represent is a
    // hard error here (it should have been left unlowered by brixc).
    for def in &source.functions {
        let body = convert_expr(&def.body)?;
        program.fn_defs.insert(
            def.name.to_string(),
            crate::program::FnDef {
                params: def.params.iter().map(|(p, _)| p.to_string()).collect(),
                body,
            },
        );
    }

    for name in &relations_seen {
        let schema = resolver
            .relation(name)
            .ok_or_else(|| AdapterError::UnknownRelation(name.to_string()))?;
        let def = relation_def(name, schema, kinds);
        program.relations.insert(def.name.clone(), def);
    }
    for r in rules {
        program.rules.insert(r.id.clone(), r);
    }
    for c in constraints {
        program.constraints.insert(c.id.clone(), c);
    }

    Ok(program)
}

fn relation_def(name: &QualIdent, schema: &RelationSchema, kinds: &KindTable) -> RelationDef {
    let kind = kinds.get(name.to_string().as_str()).copied().unwrap_or({
        if schema.derived {
            RelKind::Derived
        } else {
            RelKind::Ground
        }
    });
    RelationDef {
        name: name.to_string(),
        kind,
        roles: schema
            .roles
            .iter()
            .map(|(ident, _ty)| ident.to_string())
            .collect(),
        key: schema.key.iter().map(|ident| ident.to_string()).collect(),
        open: !schema.model_closed,
    }
}

fn convert_rule(rule: &IrRule, relations: &mut BTreeSet<QualIdent>) -> Result<Rule, AdapterError> {
    let head = match &rule.head {
        IrHead::Tuple { relation, args } => {
            relations.insert(relation.clone());
            Head::Tuple {
                rel: relation.to_string(),
                args: convert_args(args)?,
            }
        }
        IrHead::Mask { target, reason } => {
            let relation = resolve_target_relation(&rule.body, target.as_str())
                .ok_or_else(|| AdapterError::MaskTargetUnresolved(target.to_string()))?;
            relations.insert(relation.clone());
            Head::Mask {
                relation: relation.to_string(),
                target: target.to_string(),
                reason: reason.to_string(),
            }
        }
        IrHead::Node { .. } => return Err(AdapterError::Unsupported("Head::Node")),
    };
    let body = convert_pattern(&rule.body, relations)?;
    Ok(Rule {
        id: rule.name.to_string(),
        head,
        body,
    })
}

fn convert_constraint(
    c: &IrConstraint,
    relations: &mut BTreeSet<QualIdent>,
) -> Result<Constraint, AdapterError> {
    let body = convert_pattern(&c.body, relations)?;
    Ok(Constraint {
        id: c.name.to_string(),
        severity: convert_severity(c.severity),
        body,
    })
}

fn convert_severity(s: IrSeverity) -> Severity {
    match s {
        IrSeverity::Advisory => Severity::Advisory,
        IrSeverity::Strict => Severity::Strict,
        IrSeverity::Audit => Severity::Audit,
    }
}

/// A mask's `target` is an edge-ref bound somewhere in its own body (Part
/// III §6); resolve it to the relation it reads. Same pattern as
/// `crates/brixc/src/phase.rs::resolve_target_relation`, kept as a small
/// local copy since `brix-oracle` must not depend on `brixc`.
fn resolve_target_relation(body: &Pattern, target: &str) -> Option<QualIdent> {
    body.clauses.iter().find_map(|c| match c {
        IrClause::Edge {
            bind: Some(b),
            relation,
            ..
        } if b.as_str() == target => Some(relation.clone()),
        _ => None,
    })
}

fn convert_pattern(
    pattern: &Pattern,
    relations: &mut BTreeSet<QualIdent>,
) -> Result<Vec<Clause>, AdapterError> {
    pattern
        .clauses
        .iter()
        .map(|c| convert_clause(c, relations))
        .collect()
}

fn convert_clause(
    clause: &IrClause,
    relations: &mut BTreeSet<QualIdent>,
) -> Result<Clause, AdapterError> {
    match clause {
        IrClause::Edge {
            bind,
            relation,
            args,
        } => {
            relations.insert(relation.clone());
            Ok(Clause::Edge {
                rel: relation.to_string(),
                bind_id: bind.as_ref().map(|b| b.to_string()),
                args: convert_args(args)?,
            })
        }
        // The oracle unifies edge and entity-attribute clauses (program.rs's
        // own documented Entity/Edge unification): both bind an identity
        // reference to a variable, plus ordinary field constraints.
        IrClause::Entity {
            var,
            entity,
            fields,
        } => {
            let relation = QualIdent::from(entity.as_str());
            relations.insert(relation.clone());
            Ok(Clause::Edge {
                rel: relation.to_string(),
                bind_id: Some(var.to_string()),
                args: convert_args(fields)?,
            })
        }
        IrClause::Let { binds, expr } => Ok(Clause::Let(binds.to_string(), convert_expr(expr)?)),
        IrClause::When(e) => Ok(Clause::When(convert_expr(e)?)),
        IrClause::Without(inner) => {
            let inner_clauses = convert_pattern(inner, relations)?;
            Ok(Clause::Without(inner_clauses))
        }
        IrClause::History { relation, args, .. } => {
            relations.insert(relation.clone());
            Ok(Clause::History {
                rel: relation.to_string(),
                args: convert_args(args)?,
            })
        }
        IrClause::Any(_) => Err(AdapterError::Unsupported("Clause::Any")),
        IrClause::Exists(_) => Err(AdapterError::Unsupported("Clause::Exists")),
        IrClause::Optional(_) => Err(AdapterError::Unsupported("Clause::Optional")),
        IrClause::Cross(_) => Err(AdapterError::Unsupported("Clause::Cross")),
    }
}

fn convert_args(args: &[RoleArg]) -> Result<Vec<(String, Term)>, AdapterError> {
    args.iter()
        .map(|ra| Ok((ra.role.to_string(), convert_arg(&ra.arg)?)))
        .collect()
}

fn convert_arg(arg: &Arg) -> Result<Term, AdapterError> {
    Ok(match arg {
        Arg::Var(ident) => Term::Var(ident.to_string()),
        Arg::Lit(lit) => Term::Const(convert_lit(lit)?),
    })
}

/// Functions the flagship's rule bodies call that map directly onto the
/// oracle's native `BinOp` (evaluated natively by `eval_binop`, needing no
/// hand-registered `FnLibrary` entry). `div`/`neg`/`not`/`in` have no
/// oracle `BinOp` counterpart and fall through to an ordinary `Expr::Call`
/// — unused by the flagship, but graceful (register a `FnLibrary` entry)
/// rather than a hard adapter error if some other program needs them.
fn binop_for(func: &str) -> Option<BinOp> {
    Some(match func {
        "brix.ops.add" => BinOp::Add,
        "brix.ops.sub" => BinOp::Sub,
        "brix.ops.mul" => BinOp::Mul,
        "brix.ops.eq" => BinOp::Eq,
        "brix.ops.ne" => BinOp::Ne,
        "brix.ops.lt" => BinOp::Lt,
        "brix.ops.le" => BinOp::Le,
        "brix.ops.gt" => BinOp::Gt,
        "brix.ops.ge" => BinOp::Ge,
        "brix.ops.and" => BinOp::And,
        "brix.ops.or" => BinOp::Or,
        _ => return None,
    })
}

fn convert_expr(e: &IrExpr) -> Result<Expr, AdapterError> {
    match &*e.kind {
        ExprKind::Var(ident) => Ok(Expr::Var(ident.to_string())),
        ExprKind::Lit(lit) => Ok(Expr::Const(convert_lit(lit)?)),
        ExprKind::Call { func, args } => {
            let name = func.to_string();
            // A unit constructor is a typing-only wrapper whose arg was scaled
            // to minor units at lowering (issue #47 Slice 1.5); unwrap to it.
            if name.starts_with("brix.units.") {
                let inner = args
                    .first()
                    .ok_or(AdapterError::Unsupported("empty unit constructor"))?;
                return convert_expr(inner);
            }
            let oargs = args
                .iter()
                .map(convert_expr)
                .collect::<Result<Vec<_>, _>>()?;
            if let (Some(op), 2) = (binop_for(&name), oargs.len()) {
                let mut it = oargs.into_iter();
                let a = it.next().expect("len checked above");
                let b = it.next().expect("len checked above");
                return Ok(Expr::BinOp(op, Box::new(a), Box::new(b)));
            }
            Ok(Expr::Call(name, oargs))
        }
        ExprKind::Try { inner, .. } => match &*inner.kind {
            ExprKind::Call { func, args } => {
                let oargs = args
                    .iter()
                    .map(convert_expr)
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Expr::Try(func.to_string(), oargs))
            }
            _ => Err(AdapterError::Unsupported("Try over a non-Call expression")),
        },
        ExprKind::Field { .. } => Err(AdapterError::Unsupported("ExprKind::Field")),
        ExprKind::Record { .. } => Err(AdapterError::Unsupported("ExprKind::Record")),
        ExprKind::If { cond, then, els } => Ok(Expr::If {
            cond: Box::new(convert_expr(cond)?),
            then: Box::new(convert_expr(then)?),
            els: Box::new(convert_expr(els)?),
        }),
        ExprKind::Let { name, value, body } => Ok(Expr::Let {
            name: name.to_string(),
            value: Box::new(convert_expr(value)?),
            body: Box::new(convert_expr(body)?),
        }),
        ExprKind::Comprehension { .. } => Err(AdapterError::Unsupported("ExprKind::Comprehension")),
    }
}

fn convert_lit(lit: &Lit) -> Result<Value, AdapterError> {
    Ok(match lit {
        Lit::Unit => Value::Unit,
        Lit::Bool(b) => Value::Bool(*b),
        Lit::Int(i) => Value::Int(*i),
        Lit::Str(s) => Value::Str(s.clone()),
        // The one place a float genuinely appears in Core IR (e.g.
        // `Escalate`'s `when risk > 0.8`, a `Probability`-typed literal) —
        // decoded once, here, and rescaled to the basis-points convention
        // every `Probability` value uses (see the module doc).
        Lit::F64Bits(bits) => {
            let f = f64::from_bits(*bits);
            Value::Int((f * 10_000.0).round() as i64)
        }
        // `Lit::Enum` carries only the declaration-order ordinal, not the
        // variant's declared name (Appendix G's canonical encoding — see
        // `pattern::Lit`'s own doc) — and `SchemaResolver` exposes no
        // variant-name lookup to recover it. This is provably harmless:
        // `Value::Enum`'s `name` field is purely a display aid and never
        // participates in equality/ordering/hashing/canonical bytes (see
        // `value.rs`'s own doc), so a placeholder here can never cause a
        // spurious match or mismatch against a "real"-named `Value::Enum`
        // constructed elsewhere (e.g. hand-authored transaction data).
        Lit::Enum { ty, ordinal } => Value::Enum {
            ty: ty.to_string().into(),
            ordinal: *ordinal,
            name: ordinal.to_string().into(),
        },
    })
}
