//! Checked Core IR projected into the runtime-owned semantic IR.

use brix_ir::core::{Expr as IrExpr, ExprKind, Head as IrHead};
use brix_ir::pattern::{Arg, Clause as IrClause, Lit, RoleArg};
use brix_rt::engine::{self, Program, Relation, RelationKind};

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
                    RelationKind::Ground
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
        _ => None,
    }
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
