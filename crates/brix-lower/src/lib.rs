//! `brix-lower` — Brix lowering (ADR-0010, L2): the bridge from the surface
//! AST ([`brix_syntax::ast`]) onto SOC realizations.
//!
//! **L2-first slice (this crate, initial):** lower the `{fn, let, Call, Num,
//! Var}` fragment of Brix onto [`soc_regimes::type_realization::Expr`]
//! (`Lam`/`App`/`Lit`/`Var`) and run it through the tree-elaboration path. The
//! kernel proves the *composition* theorem — given the primitive typing-rule
//! leaves, the derivation establishes `e : T` — so the honest status of the
//! typing result is **`Audited`** (see
//! [`soc_regimes::type_realization::honest_result_outcome`]); it upgrades to
//! `Proven` per-result once the leaf generators are discharged to tight (the
//! SOC tight-generator obligation). `fn` definitions are inlined into
//! `App(Lam, arg)`.
//!
//! Deliberately deferred (later L2 slices): `config`/record/`match`/`regime`/
//! `rule`; and the reconciliation of the two internal type reps
//! (`type_realization` for the Proven/positive path vs `soc_regimes::native`
//! for conflict detection/negative path) into one canonical checker.

use std::collections::{BTreeMap, BTreeSet};

use brix_elaborate::{elaborate_tree, ElaborationResult};
use brix_kernel::Budget;
use brix_semantic::{ContextId, Outcome};
use brix_syntax::ast::{self, Item};
use soc_regimes::type_realization::{
    audited_type_check_tree, honest_result_outcome, infer_tree, zonk, ArithOp, Expr as TrExpr,
    Infer, Pattern as TrPattern, Ty as TrTy, TyCtx, TypeError,
};

/// Lowering context holding top-level functions, config declarations, and constructors.
#[derive(Clone, Copy, Debug)]
pub struct LowerCtx<'a> {
    pub fns: &'a BTreeMap<String, &'a ast::Callable>,
    pub configs: &'a BTreeMap<String, &'a ast::ConfigBody>,
    pub ctors: &'a BTreeMap<String, (TrTy, String, Vec<TrTy>)>,
}

/// Errors surfaced while lowering a surface construct not yet supported by the
/// current L2 fragment (or an ill-formed reference or type/elaboration failure).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LowerError {
    /// A surface construct outside the current L2 fragment.
    Unsupported(String),
    /// A reference (variable / function name) that could not be resolved.
    Unresolved(String),
    /// Type checking error from `soc_regimes`.
    TypeError(TypeError),
    /// Elaboration to `Proven` failed.
    ElaborationFailed(String),
    /// A declared record field is missing from a record literal.
    MissingField { config: String, field: String },
    /// A record literal field is not present in the declared record config.
    UnknownField { config: String, field: String },
}

impl From<TypeError> for LowerError {
    fn from(err: TypeError) -> Self {
        LowerError::TypeError(err)
    }
}

fn lower_prim_ty(t: &ast::Ty) -> Result<TrTy, LowerError> {
    match t {
        ast::Ty::Graded(inner, _) => lower_prim_ty(inner),
        ast::Ty::Named(n) => match n.as_str() {
            "Int" => Ok(TrTy::Con("Int")),
            "Str" => Ok(TrTy::Con("Str")),
            "Float" => Ok(TrTy::Con("Float")),
            other => Err(LowerError::Unsupported(format!(
                "sum field type '{other}' not supported yet (only Int/Str/Float; recursive/custom sum fields are a follow-up)"
            ))),
        },
    }
}

fn lower_pattern(p: &ast::Pattern) -> Result<TrPattern, LowerError> {
    match p {
        ast::Pattern::Wildcard => Ok(TrPattern::Wildcard),
        ast::Pattern::Var(x) => Ok(TrPattern::Var(x.clone())),
        ast::Pattern::Ctor { name, args } => {
            let sub = args
                .iter()
                .map(lower_pattern)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(TrPattern::Ctor(name.clone(), sub))
        }
    }
}

/// Lower a surface AST expression into a native [`soc_regimes::type_realization::Expr`].
pub fn lower_expr(e: &ast::Expr, ctx: LowerCtx) -> Result<TrExpr, LowerError> {
    match e {
        ast::Expr::Num(s) => {
            if let Ok(n) = s.parse::<i64>() {
                Ok(TrExpr::Lit(n))
            } else if s.parse::<f64>().is_ok() {
                // A well-formed decimal literal that is not an integer → Float.
                Ok(TrExpr::FloatLit(s.clone()))
            } else {
                Err(LowerError::Unsupported(format!(
                    "unrecognized numeric literal '{s}'"
                )))
            }
        }
        ast::Expr::Var(name) => {
            if let Some((sum_ty, variant, field_tys)) = ctx.ctors.get(name) {
                if field_tys.is_empty() {
                    return Ok(TrExpr::Ctor(sum_ty.clone(), variant.clone(), vec![]));
                }
            }
            Ok(TrExpr::Var(name.clone()))
        }
        ast::Expr::Call { func, args } => {
            if let Some(c) = ctx.fns.get(func) {
                if c.params.len() != args.len() {
                    return Err(LowerError::Unsupported(format!(
                        "arity mismatch for function '{func}': expected {}, got {}",
                        c.params.len(),
                        args.len()
                    )));
                }
                let mut acc = lower_fn(c, ctx)?;
                for arg in args {
                    let lowered_arg = lower_expr(arg, ctx)?;
                    acc = TrExpr::App(Box::new(acc), Box::new(lowered_arg));
                }
                Ok(acc)
            } else if let Some((sum_ty, variant, field_tys)) = ctx.ctors.get(func) {
                if args.len() != field_tys.len() {
                    return Err(LowerError::Unsupported(format!(
                        "constructor '{func}' expects {} args, got {}",
                        field_tys.len(),
                        args.len()
                    )));
                }
                let lowered_args = args
                    .iter()
                    .map(|arg| lower_expr(arg, ctx))
                    .collect::<Result<Vec<_>, LowerError>>()?;
                Ok(TrExpr::Ctor(sum_ty.clone(), variant.clone(), lowered_args))
            } else {
                Err(LowerError::Unresolved(func.clone()))
            }
        }
        ast::Expr::Str(s) => Ok(TrExpr::StrLit(s.clone())),
        ast::Expr::Record { config, fields } => {
            if let Some(body) = ctx.configs.get(config) {
                match body {
                    ast::ConfigBody::Sum(_) => {
                        return Err(LowerError::Unsupported(format!(
                            "'{config}' is a sum config, not a record"
                        )));
                    }
                    ast::ConfigBody::Record(decls) => {
                        for decl in decls {
                            if !fields.iter().any(|(name, _)| name == &decl.name) {
                                return Err(LowerError::MissingField {
                                    config: config.clone(),
                                    field: decl.name.clone(),
                                });
                            }
                        }
                        for (name, _) in fields {
                            if !decls.iter().any(|decl| &decl.name == name) {
                                return Err(LowerError::UnknownField {
                                    config: config.clone(),
                                    field: name.clone(),
                                });
                            }
                        }
                    }
                }
            }
            let lowered_fields: Result<Vec<(String, TrExpr)>, LowerError> = fields
                .iter()
                .map(|(name, e)| Ok((name.clone(), lower_expr(e, ctx)?)))
                .collect();
            Ok(TrExpr::Record(lowered_fields?))
        }
        ast::Expr::Field(base, name) => Ok(TrExpr::Field(
            Box::new(lower_expr(base, ctx)?),
            name.clone(),
        )),
        ast::Expr::Bin { op, lhs, rhs } => {
            let arith_op = match op {
                ast::BinOp::Add => ArithOp::Add,
                ast::BinOp::Sub => ArithOp::Sub,
                ast::BinOp::Mul => ArithOp::Mul,
                ast::BinOp::Div => ArithOp::Div,
                // `then`/`and` are witness composition, not numeric arithmetic —
                // deferred to the L4 witness/proof surface.
                ast::BinOp::Then | ast::BinOp::And => {
                    return Err(LowerError::Unsupported(
                        "witness composition ('then'/'and') not in L2-first fragment".to_string(),
                    ))
                }
            };
            Ok(TrExpr::Arith(
                arith_op,
                Box::new(lower_expr(lhs, ctx)?),
                Box::new(lower_expr(rhs, ctx)?),
            ))
        }
        ast::Expr::Match { scrutinee, arms } => {
            let scrutinee_tr = lower_expr(scrutinee, ctx)?;
            let arms_tr = arms
                .iter()
                .map(|arm| {
                    let pat_tr = lower_pattern(&arm.pattern)?;
                    let body_tr = lower_expr(&arm.body, ctx)?;
                    Ok((pat_tr, body_tr))
                })
                .collect::<Result<Vec<_>, LowerError>>()?;
            Ok(TrExpr::Match(Box::new(scrutinee_tr), arms_tr))
        }
        ast::Expr::Prove(..) => Err(LowerError::Unsupported(
            "Prove not in L2-first fragment".to_string(),
        )),
        ast::Expr::Why(..) => Err(LowerError::Unsupported(
            "Why not in L2-first fragment".to_string(),
        )),
        ast::Expr::Audit(..) => Err(LowerError::Unsupported(
            "Audit not in L2-first fragment".to_string(),
        )),
    }
}

/// Lower a function definition (`ast::Callable`) to a curried [`soc_regimes::type_realization::Expr::Lam`].
pub fn lower_fn(c: &ast::Callable, ctx: LowerCtx) -> Result<TrExpr, LowerError> {
    let body_tr = lower_expr(&c.body, ctx)?;
    Ok(c.params.iter().rfold(body_tr, |acc, param| {
        TrExpr::Lam(param.name.clone(), Box::new(acc))
    }))
}

/// The outcome of lowering and checking a `let` binding in a Brix module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckResult {
    /// The name of the `let` binding.
    pub name: String,
    /// The final elaboration outcome (e.g. `Proven`).
    pub outcome: Outcome,
    /// The inferred type of the binding value.
    pub ty: Option<TrTy>,
}

/// Lower each `let` binding in a parsed surface AST module to native SOC expressions,
/// type-check them, and elaborate them to `Proven HasType`.
pub fn check_module(m: &ast::Module) -> Vec<Result<CheckResult, (String, LowerError)>> {
    let mut fns = BTreeMap::new();
    let mut configs = BTreeMap::new();
    for item in &m.items {
        match item {
            Item::Fn(c) => {
                fns.insert(c.name.clone(), c);
            }
            Item::Config(c) => {
                configs.insert(c.name.clone(), &c.body);
            }
            _ => {}
        }
    }

    let mut sums = BTreeMap::new();
    let mut ctors = BTreeMap::new();
    let mut ambiguous_ctors = BTreeSet::new();

    for item in &m.items {
        if let Item::Config(c) = item {
            if let ast::ConfigBody::Sum(variants) = &c.body {
                let mut variant_tys = Vec::new();
                let mut valid = true;
                for v in variants {
                    let mut field_tys = Vec::new();
                    for param in &v.params {
                        match lower_prim_ty(param) {
                            Ok(ty) => field_tys.push(ty),
                            Err(_) => {
                                valid = false;
                                break;
                            }
                        }
                    }
                    if !valid {
                        break;
                    }
                    variant_tys.push((v.name.clone(), field_tys));
                }
                if valid {
                    let sum_ty = TrTy::Sum(c.name.clone(), variant_tys);
                    sums.insert(c.name.clone(), sum_ty);
                }
            }
        }
    }

    for sum_ty in sums.values() {
        if let TrTy::Sum(_, variants) = sum_ty {
            for (vname, field_tys) in variants {
                if ambiguous_ctors.contains(vname) {
                    continue;
                }
                if ctors.contains_key(vname) {
                    ctors.remove(vname);
                    ambiguous_ctors.insert(vname.clone());
                } else {
                    ctors.insert(
                        vname.clone(),
                        (sum_ty.clone(), vname.clone(), field_tys.clone()),
                    );
                }
            }
        }
    }

    let ctx = LowerCtx {
        fns: &fns,
        configs: &configs,
        ctors: &ctors,
    };

    let mut ty_ctx = TyCtx::new();
    let mut results = Vec::new();

    for item in &m.items {
        if let Item::Let(let_decl) = item {
            let res = (|| {
                let tr_expr = lower_expr(&let_decl.value, ctx)?;
                let (ty, _ty_tree, st) = infer_tree(&tr_expr, &ty_ctx, Infer::new())?;
                let inferred_ty = zonk(&ty, &st.subst);
                let (audited_judgement, tree) =
                    audited_type_check_tree(&tr_expr, &ty_ctx, ContextId::root())?;
                match elaborate_tree(&audited_judgement, &tree, Budget::new(2000, 2000)) {
                    ElaborationResult::Proven { judgement, .. } => Ok((
                        CheckResult {
                            // The kernel proves the *composition* (judgement.outcome,
                            // e.g. Proven) conditional on the primitive typing-rule
                            // leaves. The honest status of the typing result is that
                            // capped by leaf discharge — Audited until the leaves are
                            // proven tight (the SOC tight-generator obligation).
                            name: let_decl.name.clone(),
                            outcome: honest_result_outcome(judgement.outcome, &tree),
                            ty: Some(inferred_ty.clone()),
                        },
                        inferred_ty,
                    )),
                    ElaborationResult::NotElaborated(verdict) => {
                        Err(LowerError::ElaborationFailed(format!("{verdict:?}")))
                    }
                }
            })();

            match res {
                Ok((check_res, inferred_ty)) => {
                    ty_ctx = ty_ctx.extend(let_decl.name.clone(), inferred_ty);
                    results.push(Ok(check_res));
                }
                Err(err) => {
                    results.push(Err((let_decl.name.clone(), err)));
                }
            }
        }
    }

    results
}
