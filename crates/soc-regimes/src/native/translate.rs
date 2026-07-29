//! Translation bridge from `brix_ir::frontend::FrontendSource` to `NativeSource` (ADR-0009 §5).

use brix_ir::core::{Expr, ExprKind};
use brix_ir::frontend::{FrontendSource, SchemaResolver, TableResolver};
use brix_ir::pattern::Lit;
use brix_ir::types::{IntWidth, Ty, TyVar};

use super::syntax::{NExpr, NLit, NSig, NTy, NativeQuery, NativeSource, Origin, SigTable};

/// Translates a `brix_ir::types::Ty` to a native `NTy`.
///
/// Returns `None` for any unsupported type construct.
pub fn translate_ty(ty: &Ty) -> Option<NTy> {
    match ty {
        Ty::Unit => Some(NTy::Unit),
        Ty::Bool => Some(NTy::Bool),
        Ty::Str => Some(NTy::Str),
        Ty::Int(IntWidth::Int) | Ty::Int(IntWidth::I64) | Ty::Int(IntWidth::I32) => Some(NTy::Int),
        Ty::Int(_) => Some(NTy::Int),
        Ty::F64 => Some(NTy::F64),
        Ty::Fn { params, ret, .. } => {
            let nparams = params
                .iter()
                .map(translate_ty)
                .collect::<Option<Vec<_>>>()?;
            let nret = Box::new(translate_ty(ret)?);
            Some(NTy::Fn {
                params: nparams,
                ret: nret,
            })
        }
        Ty::Var(TyVar(v)) => Some(NTy::Var(*v)),
        Ty::Error => Some(NTy::Error),
        // Return None for all unsupported type constructs (Option, Result, Rel, Record, Dimensions, etc.)
        _ => None,
    }
}

/// Translates a `brix_ir::pattern::Lit` to a native `NLit`.
pub fn translate_lit(lit: &Lit) -> Option<NLit> {
    match lit {
        Lit::Int(n) => Some(NLit::Int(*n)),
        Lit::Bool(b) => Some(NLit::Bool(*b)),
        Lit::Str(s) => Some(NLit::Str(s.clone())),
        Lit::Unit => Some(NLit::Unit),
        _ => None,
    }
}

/// Helper to derive a u64 `Origin` from an `Expr`.
pub fn expr_origin(expr: &Expr) -> Origin {
    let digest = expr.origin.id.digest();
    let bytes = digest.as_bytes();
    u64::from_le_bytes(bytes[..8].try_into().unwrap_or([0; 8]))
}

/// Translates a `brix_ir::core::Expr` to `NExpr`, collecting function signatures into `sigs`.
pub fn translate_expr(
    expr: &Expr,
    resolver: &impl SchemaResolver,
    sigs: &mut SigTable,
) -> Option<NExpr> {
    let origin = expr_origin(expr);
    match &*expr.kind {
        ExprKind::Var(ident) => Some(NExpr::Var {
            origin,
            name: ident.to_string(),
        }),
        ExprKind::Lit(lit) => {
            let nlit = translate_lit(lit)?;
            Some(NExpr::Lit { origin, lit: nlit })
        }
        ExprKind::Call { func, args } => {
            let nargs = args
                .iter()
                .map(|a| translate_expr(a, resolver, sigs))
                .collect::<Option<Vec<_>>>()?;

            for sig in resolver.functions(func) {
                if let (Some(params), Some(ret)) = (
                    sig.params
                        .iter()
                        .map(translate_ty)
                        .collect::<Option<Vec<_>>>(),
                    translate_ty(&sig.ret),
                ) {
                    sigs.insert(func.to_string(), NSig { params, ret });
                }
            }

            Some(NExpr::Call {
                origin,
                func: func.to_string(),
                args: nargs,
            })
        }
        // Return None for unsupported expression kinds (Field, Record, If, Try, Comprehension, Let)
        _ => None,
    }
}

/// Translates a `FrontendSource` with a `SchemaResolver` to `NativeSource`.
///
/// Total and panic-safe: returns `None` for any unsupported construct.
pub fn translate(source: &FrontendSource, resolver: &impl SchemaResolver) -> Option<NativeSource> {
    if !source.rules.is_empty() || !source.constraints.is_empty() || !source.functions.is_empty() {
        return None;
    }

    let mut sigs = SigTable::new();
    let mut native_queries = Vec::new();

    for query in &source.queries {
        if !query.body.clauses.is_empty() {
            return None;
        }

        let mut params = Vec::new();
        for (name, ty) in &query.params {
            let nty = translate_ty(ty)?;
            params.push((name.to_string(), nty));
        }

        let yields = translate_expr(&query.yields, resolver, &mut sigs)?;
        let result = match &query.result {
            Ty::Rel(row) => {
                if row.fields.len() == 1 && row.fields[0].name.as_str() == "value" {
                    translate_ty(&row.fields[0].ty)?
                } else {
                    translate_ty(&query.result)?
                }
            }
            other => translate_ty(other)?,
        };

        native_queries.push(NativeQuery {
            name: query.name.to_string(),
            params,
            yields,
            result,
        });
    }

    Some(NativeSource {
        queries: native_queries,
        sigs,
    })
}

/// Convenience wrapper for translating a `FrontendSource` using a default `TableResolver`.
pub fn translate_source(source: &FrontendSource) -> Option<NativeSource> {
    let resolver = TableResolver::new();
    translate(source, &resolver)
}
