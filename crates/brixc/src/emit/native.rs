//! Checked Core IR projected into the runtime-owned semantic IR.

use brix_ir::core::{Expr as IrExpr, ExprKind, Head as IrHead, Severity as IrSeverity};
use brix_ir::pattern::{Arg, Clause as IrClause, Lit, RoleArg};
use brix_rt::engine::{self, Program, Relation, RelationKind};

use crate::lower::RuntimeRelationKind;
use crate::phase::Phased;

/// Project relation schemas first. Rule/body lowering is added alongside the
/// generated registration emitter; keeping schema projection here means the
/// runtime no longer needs to infer a relation's role/key layout from opaque
/// delta rows.
pub fn project_program(phased: &Phased) -> Program {
    let mut program = Program::default();
    for schema in phased.lowered.resolver.relations() {
        program.relations.insert(
            schema.name.to_string(),
            Relation {
                name: schema.name.to_string(),
                kind: if schema.derived {
                    RelationKind::Derived
                } else {
                    match phased.lowered.resolver.relation_kind(&schema.name) {
                        RuntimeRelationKind::Entity => RelationKind::Entity,
                        RuntimeRelationKind::Ground => RelationKind::Ground,
                        RuntimeRelationKind::State => RelationKind::State,
                        RuntimeRelationKind::Event => RelationKind::Event,
                    }
                },
                roles: schema
                    .roles
                    .iter()
                    .map(|(role, _)| role.to_string())
                    .collect(),
                key: schema.key.iter().map(ToString::to_string).collect(),
                open: !schema.model_closed,
            },
        );
    }
    for rule in &phased.lowered.source.rules {
        let Some(head) = convert_head(&rule.head, &rule.body) else {
            continue;
        };
        let phase = phased
            .phases
            .iter()
            .find(|phase| phase.rules.iter().any(|name| name == rule.name.as_str()))
            .expect("every checked rule has a phase")
            .id as u32;
        program.rules.insert(
            rule.name.to_string(),
            engine::Rule {
                id: rule.name.to_string(),
                phase,
                head,
                body: rule
                    .body
                    .clauses
                    .iter()
                    .filter_map(convert_clause)
                    .collect(),
            },
        );
    }
    // Functions compiled from source (issue #47): project each lowered FnDef
    // into a runtime fn-def the evaluator runs directly. A body form the
    // runtime can't yet represent (`convert_expr` returns `None`) is skipped —
    // the fn then falls back to its hand-registered native impl, if any.
    for def in &phased.lowered.source.functions {
        let Some(body) = convert_expr(&def.body) else {
            continue;
        };
        program.fn_defs.insert(
            def.name.to_string(),
            engine::FnDef {
                params: def.params.iter().map(|(p, _)| p.to_string()).collect(),
                body,
            },
        );
    }
    for constraint in &phased.lowered.source.constraints {
        program.constraints.insert(
            constraint.name.to_string(),
            engine::Constraint {
                id: constraint.name.to_string(),
                severity: match constraint.severity {
                    IrSeverity::Advisory => engine::Severity::Advisory,
                    IrSeverity::Strict => engine::Severity::Strict,
                    IrSeverity::Audit => engine::Severity::Audit,
                },
                body: constraint
                    .body
                    .clauses
                    .iter()
                    .filter_map(convert_clause)
                    .collect(),
            },
        );
    }
    program
}

fn convert_head(head: &IrHead, body: &brix_ir::pattern::Pattern) -> Option<engine::Head> {
    match head {
        IrHead::Tuple { relation, args } => Some(engine::Head::Tuple {
            relation: relation.to_string(),
            args: convert_args(args),
        }),
        IrHead::Mask { target, reason } => body.clauses.iter().find_map(|clause| match clause {
            IrClause::Edge {
                bind: Some(bound),
                relation,
                ..
            } if bound == target => Some(engine::Head::Mask {
                relation: relation.to_string(),
                target: target.to_string(),
                reason: reason.to_string(),
            }),
            _ => None,
        }),
        IrHead::Node { .. } => None,
    }
}

fn convert_clause(clause: &IrClause) -> Option<engine::Clause> {
    match clause {
        IrClause::Edge {
            bind,
            relation,
            args,
        } => Some(engine::Clause::Edge {
            relation: relation.to_string(),
            bind_id: bind.as_ref().map(ToString::to_string),
            args: convert_args(args),
        }),
        IrClause::Entity {
            var,
            entity,
            fields,
        } => Some(engine::Clause::Edge {
            relation: entity.to_string(),
            bind_id: Some(var.to_string()),
            args: convert_args(fields),
        }),
        IrClause::Without(inner) => Some(engine::Clause::Without(
            inner.clauses.iter().filter_map(convert_clause).collect(),
        )),
        IrClause::History { relation, args, .. } => Some(engine::Clause::History {
            relation: relation.to_string(),
            args: convert_args(args),
        }),
        IrClause::When(expr) => convert_expr(expr).map(engine::Clause::When),
        IrClause::Let { binds, expr } => {
            convert_expr(expr).map(|expr| engine::Clause::Let(binds.to_string(), expr))
        }
        _ => None,
    }
}

fn convert_args(args: &[RoleArg]) -> Vec<(String, engine::Term)> {
    args.iter()
        .filter_map(|arg| match &arg.arg {
            Arg::Var(value) => Some((arg.role.to_string(), engine::Term::Var(value.to_string()))),
            Arg::Lit(value) => {
                convert_lit(value).map(|value| (arg.role.to_string(), engine::Term::Const(value)))
            }
        })
        .collect()
}

fn convert_expr(expr: &IrExpr) -> Option<engine::Expr> {
    match &*expr.kind {
        ExprKind::Var(value) => Some(engine::Expr::Var(value.to_string())),
        ExprKind::Lit(value) => convert_lit(value).map(engine::Expr::Const),
        ExprKind::Call { func, args } => {
            let name = func.to_string();
            // A unit constructor (`brix.units.EUR`) is a typing-only wrapper:
            // its arg was already scaled to the canonical minor unit at
            // lowering (issue #47 Slice 1.5), so unwrap to the scaled value.
            if name.starts_with("brix.units.") {
                return convert_expr(args.first()?);
            }
            let args = args.iter().map(convert_expr).collect::<Option<Vec<_>>>()?;
            if let (Some(operator), [left, right]) = (binop_for(&name), args.as_slice()) {
                return Some(engine::Expr::BinOp(
                    operator,
                    Box::new(left.clone()),
                    Box::new(right.clone()),
                ));
            }
            Some(engine::Expr::Call(name, args))
        }
        ExprKind::Try { inner, .. } => match &*inner.kind {
            ExprKind::Call { func, args } => Some(engine::Expr::Try(
                func.to_string(),
                args.iter().map(convert_expr).collect::<Option<Vec<_>>>()?,
            )),
            _ => None,
        },
        ExprKind::If { cond, then, els } => Some(engine::Expr::If {
            cond: Box::new(convert_expr(cond)?),
            then: Box::new(convert_expr(then)?),
            els: Box::new(convert_expr(els)?),
        }),
        ExprKind::Let { name, value, body } => Some(engine::Expr::Let {
            name: name.to_string(),
            value: Box::new(convert_expr(value)?),
            body: Box::new(convert_expr(body)?),
        }),
        _ => None,
    }
}

fn binop_for(name: &str) -> Option<engine::BinOp> {
    Some(match name {
        "brix.ops.add" => engine::BinOp::Add,
        "brix.ops.sub" => engine::BinOp::Sub,
        "brix.ops.mul" => engine::BinOp::Mul,
        "brix.ops.div" => engine::BinOp::Div,
        "brix.ops.eq" => engine::BinOp::Eq,
        "brix.ops.ne" => engine::BinOp::Ne,
        "brix.ops.lt" => engine::BinOp::Lt,
        "brix.ops.le" => engine::BinOp::Le,
        "brix.ops.gt" => engine::BinOp::Gt,
        "brix.ops.ge" => engine::BinOp::Ge,
        "brix.ops.and" => engine::BinOp::And,
        "brix.ops.or" => engine::BinOp::Or,
        _ => return None,
    })
}

fn convert_lit(literal: &Lit) -> Option<engine::Value> {
    Some(match literal {
        Lit::Unit => engine::Value::Unit,
        Lit::Bool(value) => engine::Value::Bool(*value),
        Lit::Int(value) => engine::Value::Int(*value),
        Lit::Str(value) => engine::Value::Str(value.clone()),
        Lit::F64Bits(bits) => engine::Value::Int((f64::from_bits(*bits) * 10_000.0).round() as i64),
        Lit::Enum { ty, ordinal } => engine::Value::Enum {
            ty: ty.to_string(),
            ordinal: *ordinal,
            name: ordinal.to_string(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::PhaseAssign;
    use crate::{lower_file, AstPhase};
    use brix_ast::parse_file;

    #[test]
    fn projected_schema_preserves_roles_keys_and_derived_relations() {
        let (file, diags) = parse_file(
            "package t @ 1.0.0\nrel Input { value: Int } key(value)\nrel Output { value: Int } key(value)\nderive Copy: Output(value: value) from { Input(value) }\n",
        );
        let phased = AstPhase.assign_phases(lower_file(&file, &diags)).unwrap();
        let program = project_program(&phased);
        assert_eq!(program.relations["Input"].key, ["value"]);
        assert_eq!(program.relations["Output"].kind, RelationKind::Derived);
    }
}
