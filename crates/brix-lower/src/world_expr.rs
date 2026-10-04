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
        let lower = |expr, names: &BTreeSet<String>| {
            l3_v2::lower_expr_v2(
                expr, names, names, &empty_set, &empty_map, &empty_map, &arities, false, true,
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
