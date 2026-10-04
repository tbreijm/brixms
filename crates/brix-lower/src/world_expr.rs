//! World expressions use the established scalar lowerer and evaluator.
//! Compilation is separate from evaluation so helper bodies are never rebuilt per row.
use crate::l3_v2::{self, EvalEnv, L3ExprV2, L3FunctionDef, L3Schema, L3SchemaType, L3ValueV2};
use brix_syntax::ast;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct CompiledWorldExpr {
    expr: L3ExprV2,
    functions: Arc<BTreeMap<String, L3FunctionDef>>,
    schemas: Arc<BTreeMap<String, L3Schema>>,
}

/// Rewrite `and` (`ast::BinOp::And`) to logical `&&` (`ast::BinOp::AndAnd`)
/// throughout an expression tree.
///
/// The shared scalar lowerer (`l3_v2::lower_expr_v2`) refuses `BinOp::And`
/// because in the legacy finite-decision profile `and`/`then` are the
/// witness-tensor/sequential composition operators (ADR-0002), not logical
/// conjunction, and that refusal must stay intact for legacy programs.
///
/// The world/relational profile (ADR-0046 §3.5) has no witness-composition
/// surface at all and uses the keyword `and` purely as logical conjunction in
/// `where`/`when` predicates (mirroring `relation_dag::split_and_predicates`,
/// which already folds `and` and `&&` together for join/filter predicates).
/// Guard and value expressions inside `decide ... propose ... when` blocks —
/// and helper function bodies reachable from the world profile — never go
/// through `relation_dag`'s predicate splitting, so they must be normalized
/// here, at the one place all world-profile scalar expressions are compiled.
fn normalize_world_and(expr: &ast::Expr) -> ast::Expr {
    use ast::Expr;
    match expr {
        Expr::Num(_) | Expr::Str(_) | Expr::Bool(_) | Expr::Var(_) => expr.clone(),
        Expr::Record { config, fields } => Expr::Record {
            config: config.clone(),
            fields: fields
                .iter()
                .map(|(n, e)| (n.clone(), normalize_world_and(e)))
                .collect(),
        },
        Expr::Field(base, field) => Expr::Field(Box::new(normalize_world_and(base)), field.clone()),
        Expr::Call { func, args } => Expr::Call {
            func: func.clone(),
            args: args.iter().map(normalize_world_and).collect(),
        },
        Expr::Bin { op, lhs, rhs } => {
            let op = if matches!(op, ast::BinOp::And) {
                ast::BinOp::AndAnd
            } else {
                *op
            };
            Expr::Bin {
                op,
                lhs: Box::new(normalize_world_and(lhs)),
                rhs: Box::new(normalize_world_and(rhs)),
            }
        }
        Expr::Match {
            scrutinee,
            arms,
            proving_exhaustive,
        } => Expr::Match {
            scrutinee: Box::new(normalize_world_and(scrutinee)),
            arms: arms
                .iter()
                .map(|a| ast::MatchArm {
                    pattern: a.pattern.clone(),
                    body: normalize_world_and(&a.body),
                })
                .collect(),
            proving_exhaustive: *proving_exhaustive,
        },
        Expr::Prove(e) => Expr::Prove(Box::new(normalize_world_and(e))),
        Expr::Why(e) => Expr::Why(Box::new(normalize_world_and(e))),
        Expr::Audit(e) => Expr::Audit(Box::new(normalize_world_and(e))),
        Expr::Not(e) => Expr::Not(Box::new(normalize_world_and(e))),
        Expr::Lambda { param, body } => Expr::Lambda {
            param: param.clone(),
            body: Box::new(normalize_world_and(body)),
        },
        Expr::ListLit(items) => Expr::ListLit(items.iter().map(normalize_world_and).collect()),
        Expr::Comprehension {
            generators,
            where_clause,
            yield_expr,
        } => Expr::Comprehension {
            generators: generators
                .iter()
                .map(|(n, e)| (n.clone(), normalize_world_and(e)))
                .collect(),
            where_clause: where_clause
                .as_ref()
                .map(|e| Box::new(normalize_world_and(e))),
            yield_expr: Box::new(normalize_world_and(yield_expr)),
        },
        Expr::AnonRecord(fields) => Expr::AnonRecord(
            fields
                .iter()
                .map(|(n, e)| (n.clone(), normalize_world_and(e)))
                .collect(),
        ),
    }
}

fn contract(ty: &ast::Ty) -> Result<L3SchemaType, String> {
    match ty {
        ast::Ty::Named(name) => Ok(match name.as_str() {
            "Int" => L3SchemaType::Int,
            "Bool" => L3SchemaType::Bool,
            "Str" => L3SchemaType::Str,
            "F64" => L3SchemaType::F64,
            "Decimal" => L3SchemaType::Decimal,
            other => L3SchemaType::Named(other.to_owned()),
        }),
        ast::Ty::Graded(_, _) => Err("graded helper contracts require grade evidence".into()),
        other => Err(format!("unsupported world helper contract: {other:?}")),
    }
}

impl CompiledWorldExpr {
    pub fn new(
        expr: &ast::Expr,
        helpers: &BTreeMap<String, ast::Callable>,
        bindings: &BTreeSet<String>,
    ) -> Result<Self, String> {
        Self::with_schemas(expr, helpers, bindings, Arc::new(BTreeMap::new()))
    }

    /// Supply linked nominal schemas when helpers accept or return records.
    pub fn with_schemas(
        expr: &ast::Expr,
        helpers: &BTreeMap<String, ast::Callable>,
        bindings: &BTreeSet<String>,
        schemas: Arc<BTreeMap<String, L3Schema>>,
    ) -> Result<Self, String> {
        let arities = helpers
            .iter()
            .map(|(name, f)| (name.clone(), f.params.len()))
            .collect();
        let empty_set = BTreeSet::new();
        let empty_map = BTreeMap::new();
        let lower = |expr: &ast::Expr, names: &BTreeSet<String>| {
            let normalized = normalize_world_and(expr);
            l3_v2::lower_expr_v2(
                &normalized,
                names,
                names,
                &empty_set,
                &empty_map,
                &empty_map,
                &arities,
                false,
                true,
            )
            .map_err(|e| format!("world expression lowering: {e:?}"))
        };
        let mut functions = BTreeMap::new();
        for (name, f) in helpers {
            let names = f.params.iter().map(|p| p.name.clone()).collect();
            let params = f
                .params
                .iter()
                .map(|p| Ok((p.name.clone(), p.ty.as_ref().map(contract).transpose()?)))
                .collect::<Result<Vec<_>, String>>()?;
            functions.insert(
                name.clone(),
                L3FunctionDef {
                    name: name.clone(),
                    params,
                    ret_contract: f.ret.as_ref().map(contract).transpose()?,
                    schemas: schemas.clone(),
                    body: lower(&f.body, &names)?,
                },
            );
        }
        Ok(Self {
            expr: lower(expr, bindings)?,
            functions: Arc::new(functions),
            schemas,
        })
    }

    pub fn eval(&self, bindings: &BTreeMap<String, L3ValueV2>) -> Result<L3ValueV2, String> {
        let mut env = EvalEnv::new()
            .with_functions(self.functions.clone())
            .with_schemas(self.schemas.clone());
        for (name, value) in bindings {
            env = env.with_let(name, value.clone());
        }
        l3_v2::eval(&self.expr, &env).map_err(|fault| format!("Unknown(EvaluationFault): {fault}"))
    }
}
