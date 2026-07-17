//! Tiny hand-build DSL for programs — terse constructors so tests and
//! conformance fixtures read close to BrixMS surface syntax without a
//! parser (that is brix-ast's job; this crate proves the *evaluator*, not
//! the front end). Everything here is a thin wrapper over [`crate::program`]
//! types; it introduces no semantics of its own.

use crate::program::{BinOp, Clause, Constraint, Expr, Head, Rule, Severity, Term};
use crate::row::{RoleName, Row};
use crate::value::Value;

/// A bound rule variable term.
pub fn var(name: &str) -> Term {
    Term::Var(name.to_string())
}
/// A literal constant term.
pub fn lit(v: Value) -> Term {
    Term::Const(v)
}

/// An edge/entity read clause `R(role: term, ...)`.
pub fn edge(rel: &str, args: &[(&str, Term)]) -> Clause {
    Clause::Edge {
        rel: rel.to_string(),
        bind_id: None,
        args: to_args(args),
    }
}
/// `e @ R(role: term, ...)` — bind the edge/node reference to `bind`.
pub fn edge_bind(bind: &str, rel: &str, args: &[(&str, Term)]) -> Clause {
    Clause::Edge {
        rel: rel.to_string(),
        bind_id: Some(bind.to_string()),
        args: to_args(args),
    }
}
/// `without { R(...) ... }`.
pub fn without(inner: Vec<Clause>) -> Clause {
    Clause::Without(inner)
}
/// `history R(role: term, ...)`.
pub fn history(rel: &str, args: &[(&str, Term)]) -> Clause {
    Clause::History {
        rel: rel.to_string(),
        args: to_args(args),
    }
}
/// `when expr`.
pub fn when(e: Expr) -> Clause {
    Clause::When(e)
}
/// `let v = expr`.
pub fn let_(v: &str, e: Expr) -> Clause {
    Clause::Let(v.to_string(), e)
}

/// `x < y` etc.
pub fn binop(op: BinOp, a: Expr, b: Expr) -> Expr {
    Expr::BinOp(op, Box::new(a), Box::new(b))
}
/// Read a bound variable in an expression.
pub fn evar(name: &str) -> Expr {
    Expr::Var(name.to_string())
}
/// A partial-function call `f(args)?`.
pub fn try_call(f: &str, args: Vec<Expr>) -> Expr {
    Expr::Try(f.to_string(), args)
}

/// A tuple-head rule `derive Id: Rel(role: term, ...) from { body }`.
pub fn rule(id: &str, rel: &str, head_args: &[(&str, Term)], body: Vec<Clause>) -> Rule {
    Rule {
        id: id.to_string(),
        head: Head::Tuple {
            rel: rel.to_string(),
            args: to_args(head_args),
        },
        body,
    }
}

/// A mask rule `derive Id: mask(target) by reason from { body }`.
pub fn mask_rule(id: &str, relation: &str, target: &str, reason: &str, body: Vec<Clause>) -> Rule {
    Rule {
        id: id.to_string(),
        head: Head::Mask {
            relation: relation.to_string(),
            target: target.to_string(),
            reason: reason.to_string(),
        },
        body,
    }
}

/// A constraint `constraint Id severity { body }`.
pub fn constraint(id: &str, severity: Severity, body: Vec<Clause>) -> Constraint {
    Constraint {
        id: id.to_string(),
        severity,
        body,
    }
}

/// Build a `Row` from `(role, value)` pairs.
pub fn row(fields: &[(&str, Value)]) -> Row {
    Row::of(
        fields
            .iter()
            .map(|(r, v)| (r.to_string() as RoleName, v.clone())),
    )
}

fn to_args(args: &[(&str, Term)]) -> Vec<(RoleName, Term)> {
    args.iter()
        .map(|(r, t)| (r.to_string(), t.clone()))
        .collect()
}
