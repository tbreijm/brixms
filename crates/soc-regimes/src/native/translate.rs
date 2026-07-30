//! Translation bridge from `brix_ir::frontend::FrontendSource` to `NativeSource` (ADR-0009 §5).

use brix_ir::core::{Expr, ExprKind};
use brix_ir::frontend::{FrontendSource, SchemaResolver, TableResolver};
use brix_ir::pattern::{Clause, Lit};
use brix_ir::types::{IntWidth, Row, RowTail, Ty, TyVar};

use super::syntax::{NExpr, NLit, NRow, NSig, NTy, NativeQuery, NativeSource, Origin, SigTable};

/// Translates a `brix_ir::types::Row` to a native `NRow`.
pub fn translate_row(row: &Row) -> Option<NRow> {
    let mut fields = Vec::with_capacity(row.fields.len());
    for field in &row.fields {
        let fty = translate_ty(&field.ty)?;
        fields.push((field.name.to_string(), fty));
    }
    let open = matches!(row.tail, RowTail::Open(_));
    Some(NRow { fields, open })
}

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
        Ty::Option(inner) => Some(NTy::Option(Box::new(translate_ty(inner)?))),
        Ty::Result(ok, err) => Some(NTy::Result(
            Box::new(translate_ty(ok)?),
            Box::new(translate_ty(err)?),
        )),
        Ty::Estimate(inner) => Some(NTy::Estimate(Box::new(translate_ty(inner)?))),
        Ty::Missing(inner) => Some(NTy::Missing(Box::new(translate_ty(inner)?))),
        Ty::Probability => Some(NTy::Probability),
        Ty::Record(row) => Some(NTy::Record(translate_row(row)?)),
        Ty::Rel(row) => Some(NTy::Rel(translate_row(row)?)),
        Ty::Quantity(m) => Some(NTy::Quantity(m.to_string())),
        Ty::Money(c) => Some(NTy::Money(c.to_string())),
        Ty::Dimensioned(ds) => Some(NTy::Dimensioned(
            ds.iter()
                .map(|d| (d.name.to_string(), d.exponent as i64))
                .collect(),
        )),
        Ty::Var(TyVar(v)) => Some(NTy::Var(*v)),
        Ty::Error => Some(NTy::Error),
        // Return None for all unsupported type constructs
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
            // Carry the node's own type annotation, so an env-miss falls back to
            // it (mirrors reflect's `env.get(name).unwrap_or(expr.ty)`). `None`
            // when the annotation is an unsupported type.
            ty: translate_ty(&expr.ty),
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
        ExprKind::Field { base, field } => {
            let nbase = translate_expr(base, resolver, sigs)?;
            Some(NExpr::Field {
                origin,
                base: Box::new(nbase),
                field: field.to_string(),
            })
        }
        ExprKind::Record { fields } => {
            let mut nfields = Vec::with_capacity(fields.len());
            for (name, expr) in fields {
                let nexpr = translate_expr(expr, resolver, sigs)?;
                nfields.push((name.to_string(), nexpr));
            }
            Some(NExpr::Record {
                origin,
                fields: nfields,
            })
        }
        ExprKind::Try { inner, .. } => {
            let ninner = translate_expr(inner, resolver, sigs)?;
            Some(NExpr::Try {
                origin,
                inner: Box::new(ninner),
            })
        }
        // Return None for unsupported expression kinds (If, Comprehension, Let)
        _ => None,
    }
}

/// Translates a `FrontendSource` with a `SchemaResolver` to `NativeSource`.
///
/// Total and panic-safe: returns `None` for any unsupported construct.
pub fn translate(source: &FrontendSource, resolver: &impl SchemaResolver) -> Option<NativeSource> {
    if !source.functions.is_empty() {
        return None;
    }

    let mut sigs = SigTable::new();
    let mut native_queries = Vec::new();
    let mut guards = Vec::new();

    for rule in &source.rules {
        for clause in &rule.body.clauses {
            match clause {
                Clause::When(expr) => {
                    let nexpr = translate_expr(expr, resolver, &mut sigs)?;
                    guards.push(nexpr);
                }
                _ => return None,
            }
        }
    }

    for constraint in &source.constraints {
        for clause in &constraint.body.clauses {
            match clause {
                Clause::When(expr) => {
                    let nexpr = translate_expr(expr, resolver, &mut sigs)?;
                    guards.push(nexpr);
                }
                _ => return None,
            }
        }
    }

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
        let result = translate_ty(&query.result)?;

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
        guards,
    })
}

/// Convenience wrapper for translating a `FrontendSource` using a default `TableResolver`.
pub fn translate_source(source: &FrontendSource) -> Option<NativeSource> {
    let resolver = TableResolver::new();
    translate(source, &resolver)
}
