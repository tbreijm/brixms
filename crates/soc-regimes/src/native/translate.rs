use std::collections::BTreeMap;

use brix_ir::core::{Expr, ExprKind};
use brix_ir::frontend::{FrontendSource, SchemaResolver, TableResolver};
use brix_ir::ident::QualIdent;
use brix_ir::pattern::{Arg, Clause, Lit, RoleArg};
use brix_ir::types::{IntWidth, Row, RowTail, Ty, TyVar};

use super::syntax::{
    NArg, NEdge, NEffect, NEffectRow, NExpr, NLit, NRelSchema, NRow, NRule, NSig, NTy, NativeQuery,
    NativeSource, Origin, SigTable, Sym,
};

/// Translates a `brix_ir::effects::Effect` to a native `NEffect`.
pub fn translate_effect_atom(effect: &brix_ir::effects::Effect) -> NEffect {
    match effect {
        brix_ir::effects::Effect::Net(_) => NEffect::Net,
        brix_ir::effects::Effect::Fs(_) => NEffect::Fs,
        brix_ir::effects::Effect::Clock => NEffect::Clock,
        brix_ir::effects::Effect::Random => NEffect::Random,
        brix_ir::effects::Effect::Console => NEffect::Console,
        brix_ir::effects::Effect::GraphRead(_) => NEffect::GraphRead,
        brix_ir::effects::Effect::GraphWrite(_) => NEffect::GraphWrite,
        brix_ir::effects::Effect::Panic => NEffect::Panic,
        brix_ir::effects::Effect::Diverge => NEffect::Diverge,
        brix_ir::effects::Effect::Solver(_) => NEffect::Solver,
    }
}

/// Translates a `brix_ir::effects::EffectRow` to a native `NEffectRow`.
pub fn translate_effect_row(row: &brix_ir::effects::EffectRow) -> NEffectRow {
    let atoms = row.atoms().iter().map(translate_effect_atom).collect();
    let open_tail = row.tail().is_some();
    NEffectRow { atoms, open_tail }
}

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

/// Scans `expr` for function calls and populates `called` names and `sigs`.
pub fn scan_expr_calls(
    expr: &Expr,
    resolver: &impl SchemaResolver,
    called: &mut Vec<Sym>,
    sigs: &mut SigTable,
) {
    match &*expr.kind {
        ExprKind::Call { func, args } => {
            let fname = func.to_string();
            called.push(fname.clone());
            for sig in resolver.functions(func) {
                if let (Some(params), Some(ret)) = (
                    sig.params.iter().map(translate_ty).collect(),
                    translate_ty(&sig.ret),
                ) {
                    sigs.insert(
                        fname.clone(),
                        NSig {
                            params,
                            ret,
                            may_diverge: sig.may_diverge,
                        },
                    );
                }
            }
            for arg in args {
                scan_expr_calls(arg, resolver, called, sigs);
            }
        }
        ExprKind::Field { base, .. } => scan_expr_calls(base, resolver, called, sigs),
        ExprKind::Record { fields } => {
            for (_, v) in fields {
                scan_expr_calls(v, resolver, called, sigs);
            }
        }
        ExprKind::If { cond, then, els } => {
            scan_expr_calls(cond, resolver, called, sigs);
            scan_expr_calls(then, resolver, called, sigs);
            scan_expr_calls(els, resolver, called, sigs);
        }
        ExprKind::Try { inner, .. } => scan_expr_calls(inner, resolver, called, sigs),
        ExprKind::Comprehension { pattern, yields } => {
            for e in pattern.body_exprs() {
                scan_expr_calls(e, resolver, called, sigs);
            }
            if let Some(y) = yields {
                scan_expr_calls(y, resolver, called, sigs);
            }
        }
        ExprKind::Let { value, body, .. } => {
            scan_expr_calls(value, resolver, called, sigs);
            scan_expr_calls(body, resolver, called, sigs);
        }
        ExprKind::Var(_) | ExprKind::Lit(_) => {}
    }
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
                    sigs.insert(
                        func.to_string(),
                        NSig {
                            params,
                            ret,
                            may_diverge: sig.may_diverge,
                        },
                    );
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

fn translate_edge(
    relation: &QualIdent,
    args: &[RoleArg],
    resolver: &impl SchemaResolver,
    relations: &mut BTreeMap<Sym, NRelSchema>,
) -> Option<NEdge> {
    let rel_name = relation.to_string();
    if let Some(schema) = resolver.relation(relation) {
        if !relations.contains_key(&rel_name) {
            let mut roles = Vec::with_capacity(schema.roles.len());
            for (rname, rty) in &schema.roles {
                let nty = translate_ty(rty)?;
                roles.push((rname.to_string(), nty));
            }
            relations.insert(rel_name.clone(), NRelSchema { roles });
        }
    }

    let mut nargs = Vec::with_capacity(args.len());
    for role_arg in args {
        let narg = match &role_arg.arg {
            Arg::Var(v) => NArg::Var(v.to_string()),
            Arg::Lit(l) => NArg::Lit(translate_lit(l)?),
        };
        nargs.push((role_arg.role.to_string(), narg));
    }

    Some(NEdge {
        relation: rel_name,
        args: nargs,
    })
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
    let mut relations = BTreeMap::new();
    let mut edges = Vec::new();
    let mut native_rules = Vec::new();

    for rule in &source.rules {
        let effects = translate_effect_row(&rule.effects);
        let mut called_fns = Vec::new();
        for expr in rule.body.body_exprs() {
            scan_expr_calls(expr, resolver, &mut called_fns, &mut sigs);
        }
        called_fns.sort();
        called_fns.dedup();

        for clause in &rule.body.clauses {
            match clause {
                Clause::When(expr) => {
                    let nexpr = translate_expr(expr, resolver, &mut sigs)?;
                    guards.push(nexpr);
                }
                Clause::Edge { relation, args, .. } => {
                    let edge = translate_edge(relation, args, resolver, &mut relations)?;
                    edges.push(edge);
                }
                Clause::Let { expr, .. } => {
                    let _nexpr = translate_expr(expr, resolver, &mut sigs)?;
                }
                _ => return None,
            }
        }

        native_rules.push(NRule {
            name: rule.name.to_string(),
            effects,
            called_fns,
        });
    }

    for constraint in &source.constraints {
        for clause in &constraint.body.clauses {
            match clause {
                Clause::When(expr) => {
                    let nexpr = translate_expr(expr, resolver, &mut sigs)?;
                    guards.push(nexpr);
                }
                Clause::Edge { relation, args, .. } => {
                    let edge = translate_edge(relation, args, resolver, &mut relations)?;
                    edges.push(edge);
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
        relations,
        edges,
        rules: native_rules,
    })
}

/// Convenience wrapper for translating a `FrontendSource` using a default `TableResolver`.
pub fn translate_source(source: &FrontendSource) -> Option<NativeSource> {
    let resolver = TableResolver::new();
    translate(source, &resolver)
}
