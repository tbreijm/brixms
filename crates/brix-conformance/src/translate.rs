//! `brix_oracle::program`/`txn` → `brix_rt::engine` translation.
//!
//! The two representations were designed to mirror each other closely
//! (`brix_rt::engine`'s own module doc: "a separate runtime representation",
//! not a redesign) — this module is a field-by-field adapter, not a second
//! semantics. Two structural gaps exist and are handled explicitly rather
//! than silently:
//!
//! - The oracle's `Program` carries no per-rule phase; phase is computed
//!   on demand via [`brix_oracle::phase::infer_phases`]. `brix_rt::engine::
//!   Rule` requires one, so this module runs phase inference once per
//!   translated program and assigns each rule its phase id.
//! - `brix_oracle::program::Expr::{Count, Sum}` (aggregate sub-patterns)
//!   have no `brix_rt::engine::Expr` equivalent — the native evaluator's
//!   expression language is a strict subset. Translating one panics with a
//!   clear message rather than silently dropping the aggregate; no current
//!   fixture uses either.
//! - Custom registered functions (`Program::fns`/`partial_fns`) are **not**
//!   translated: both are plain `fn(&[Value]) -> ...` pointers over
//!   engine-specific `Value` types, so a capturing/converting wrapper isn't
//!   possible. A fixture that needs a custom function must register the
//!   native-side equivalent directly under the same name in `brix-rt`'s own
//!   `builtin_total`/`builtin_partial` (mirroring how the flagship's
//!   `surcharge`/`riskModel` already work) — none of the current fixtures
//!   call a registered function at all, so this is a documented limit, not
//!   a gap exercised today.

use std::collections::BTreeMap;

use brix_oracle::program as oracle;
use brix_oracle::row::Row as OracleRow;
use brix_oracle::txn::{Op as OracleOp, Transaction as OracleTransaction};
use brix_oracle::value::Value as OracleValue;
use brix_rt::engine as rt;

/// Translate a full oracle `Program` into the equivalent native `rt::engine::
/// Program`, running phase inference once to assign each rule its phase.
///
/// Panics (fixture bug, not a fact worth swallowing — matching
/// `OracleEngine`'s own fail-closed convention) if the program does not
/// phase-assign, or if a rule/constraint body uses an expression form the
/// native evaluator has no equivalent for.
pub fn program(program: &oracle::Program) -> rt::Program {
    let phases = brix_oracle::phase::infer_phases(program)
        .unwrap_or_else(|e| panic!("program must phase-assign cleanly: {e:?}"));
    let mut phase_of: BTreeMap<&str, u32> = BTreeMap::new();
    for phase in &phases {
        for rule_id in &phase.rules {
            phase_of.insert(rule_id.as_str(), phase.id as u32);
        }
    }

    let relations = program
        .relations
        .iter()
        .map(|(name, def)| (name.clone(), relation(def)))
        .collect();
    let rules = program
        .rules
        .iter()
        .map(|(id, r)| {
            let phase = *phase_of
                .get(id.as_str())
                .unwrap_or_else(|| panic!("rule `{id}` has no inferred phase"));
            (id.clone(), rule(r, phase))
        })
        .collect();
    let constraints = program
        .constraints
        .iter()
        .map(|(id, c)| (id.clone(), constraint(c)))
        .collect();
    // Functions compiled from source (issue #47) carry a translatable body, so
    // unlike hand-registered `fns`/`partial_fns` (opaque native pointers) they
    // DO cross the oracle->rt boundary — the differential harness runs the same
    // compiled body on both engines.
    let fn_defs = program
        .fn_defs
        .iter()
        .map(|(name, def)| {
            (
                name.clone(),
                rt::FnDef {
                    params: def.params.clone(),
                    body: expr(&def.body),
                },
            )
        })
        .collect();

    rt::Program {
        relations,
        rules,
        constraints,
        fns: BTreeMap::new(),
        partial_fns: BTreeMap::new(),
        fn_defs,
    }
}

fn relation(def: &oracle::RelationDef) -> rt::Relation {
    rt::Relation {
        name: def.name.clone(),
        kind: rel_kind(def.kind),
        roles: def.roles.clone(),
        key: def.key.clone(),
        open: def.open,
    }
}

fn rel_kind(kind: oracle::RelKind) -> rt::RelationKind {
    match kind {
        oracle::RelKind::Entity => rt::RelationKind::Entity,
        oracle::RelKind::Ground => rt::RelationKind::Ground,
        oracle::RelKind::State => rt::RelationKind::State,
        oracle::RelKind::Event => rt::RelationKind::Event,
        oracle::RelKind::Derived => rt::RelationKind::Derived,
    }
}

fn rule(r: &oracle::Rule, phase: u32) -> rt::Rule {
    rt::Rule {
        id: r.id.clone(),
        phase,
        head: head(&r.head),
        body: r.body.iter().map(clause).collect(),
    }
}

fn head(h: &oracle::Head) -> rt::Head {
    match h {
        oracle::Head::Tuple { rel, args } => rt::Head::Tuple {
            relation: rel.clone(),
            args: args
                .iter()
                .map(|(role, term)| (role.clone(), t(term)))
                .collect(),
        },
        oracle::Head::Mask {
            relation,
            target,
            reason,
        } => rt::Head::Mask {
            relation: relation.clone(),
            target: target.clone(),
            reason: reason.clone(),
        },
    }
}

fn constraint(c: &oracle::Constraint) -> rt::Constraint {
    rt::Constraint {
        id: c.id.clone(),
        severity: severity(c.severity),
        body: c.body.iter().map(clause).collect(),
    }
}

fn severity(s: oracle::Severity) -> rt::Severity {
    match s {
        oracle::Severity::Advisory => rt::Severity::Advisory,
        oracle::Severity::Strict => rt::Severity::Strict,
        oracle::Severity::Audit => rt::Severity::Audit,
    }
}

fn clause(c: &oracle::Clause) -> rt::Clause {
    match c {
        oracle::Clause::Edge { rel, bind_id, args } => rt::Clause::Edge {
            relation: rel.clone(),
            bind_id: bind_id.clone(),
            args: args
                .iter()
                .map(|(role, term)| (role.clone(), t(term)))
                .collect(),
        },
        oracle::Clause::Without(inner) => rt::Clause::Without(inner.iter().map(clause).collect()),
        oracle::Clause::History { rel, args } => rt::Clause::History {
            relation: rel.clone(),
            args: args
                .iter()
                .map(|(role, term)| (role.clone(), t(term)))
                .collect(),
        },
        oracle::Clause::When(e) => rt::Clause::When(expr(e)),
        oracle::Clause::Let(var, e) => rt::Clause::Let(var.clone(), expr(e)),
    }
}

fn t(term: &oracle::Term) -> rt::Term {
    match term {
        oracle::Term::Var(v) => rt::Term::Var(v.clone()),
        oracle::Term::Const(v) => rt::Term::Const(value(v)),
    }
}

fn expr(e: &oracle::Expr) -> rt::Expr {
    match e {
        oracle::Expr::Var(v) => rt::Expr::Var(v.clone()),
        oracle::Expr::Const(v) => rt::Expr::Const(value(v)),
        oracle::Expr::BinOp(op, l, r) => {
            rt::Expr::BinOp(bin_op(*op), Box::new(expr(l)), Box::new(expr(r)))
        }
        oracle::Expr::Call(name, args) => {
            rt::Expr::Call(name.clone(), args.iter().map(expr).collect())
        }
        oracle::Expr::Try(name, args) => {
            rt::Expr::Try(name.clone(), args.iter().map(expr).collect())
        }
        oracle::Expr::If { cond, then, els } => rt::Expr::If {
            cond: Box::new(expr(cond)),
            then: Box::new(expr(then)),
            els: Box::new(expr(els)),
        },
        oracle::Expr::Count(_) | oracle::Expr::Sum(_, _) => panic!(
            "translate::expr: aggregate expressions (Count/Sum) have no brix_rt::engine::Expr \
             equivalent — no current fixture should reach this"
        ),
    }
}

fn bin_op(op: oracle::BinOp) -> rt::BinOp {
    match op {
        oracle::BinOp::Add => rt::BinOp::Add,
        oracle::BinOp::Sub => rt::BinOp::Sub,
        oracle::BinOp::Mul => rt::BinOp::Mul,
        oracle::BinOp::Eq => rt::BinOp::Eq,
        oracle::BinOp::Ne => rt::BinOp::Ne,
        oracle::BinOp::Lt => rt::BinOp::Lt,
        oracle::BinOp::Le => rt::BinOp::Le,
        oracle::BinOp::Gt => rt::BinOp::Gt,
        oracle::BinOp::Ge => rt::BinOp::Ge,
        oracle::BinOp::And => rt::BinOp::And,
        oracle::BinOp::Or => rt::BinOp::Or,
    }
}

fn value(v: &OracleValue) -> rt::Value {
    match v {
        OracleValue::Nat(n) => rt::Value::Nat(*n),
        OracleValue::Int(n) => rt::Value::Int(*n),
        OracleValue::Bool(b) => rt::Value::Bool(*b),
        OracleValue::Str(s) => rt::Value::Str(s.clone()),
        OracleValue::Node(id) => rt::Value::Node(*id),
        OracleValue::Edge(id) => rt::Value::Edge(*id),
        OracleValue::Claim(id) => rt::Value::Claim(*id),
        OracleValue::Enum { ty, ordinal, name } => rt::Value::Enum {
            ty: ty.to_string(),
            ordinal: *ordinal,
            name: name.to_string(),
        },
        OracleValue::Unit => rt::Value::Unit,
    }
}

fn row(r: &OracleRow) -> rt::Row {
    rt::Row(
        r.0.iter()
            .map(|(role, v)| (role.clone(), value(v)))
            .collect(),
    )
}

/// Translate one oracle `Transaction` into the equivalent native one. Op
/// ordering is preserved (claim ids are ordinal-derived on both sides from
/// the same transaction intent bytes, so this must match exactly for claim
/// identity to agree).
pub fn transaction(txn: &OracleTransaction) -> rt::Transaction {
    rt::Transaction {
        intent: txn.intent.clone(),
        ops: txn.ops.iter().map(op).collect(),
    }
}

fn op(o: &OracleOp) -> rt::TransactionOp {
    match o {
        OracleOp::Ensure { rel, row: r } => rt::TransactionOp::Ensure {
            relation: rel.clone(),
            row: row(r),
        },
        OracleOp::Assert { rel, row: r } => rt::TransactionOp::Assert {
            relation: rel.clone(),
            row: row(r),
        },
        OracleOp::Set { rel, row: r } => rt::TransactionOp::Set {
            relation: rel.clone(),
            row: row(r),
        },
        OracleOp::Event { rel, row: r } => rt::TransactionOp::Event {
            relation: rel.clone(),
            row: row(r),
        },
        OracleOp::Retract { rel, claim } => rt::TransactionOp::Retract {
            relation: rel.clone(),
            claim: *claim,
        },
    }
}
