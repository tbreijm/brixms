//! `brix-lower` — Brix lowering (ADR-0010, L2): the bridge from the surface
//! AST ([`brix_syntax::ast`]) onto SOC realizations.
//!
//! **L2-first slice (this crate, initial):** lower the `{fn, let, Call, Num,
//! Var}` fragment of Brix onto [`soc_regimes::type_realization::Expr`]
//! (`Lam`/`App`/`Lit`/`Var`) and run it through the existing tree-elaboration
//! path to **`Proven HasType`** — a real `.brix` program whose type is a
//! machine-checked theorem. `fn` definitions are inlined into `App(Lam, arg)`.
//!
//! Deliberately deferred (later L2 slices): `config`/record/`match`/`regime`/
//! `rule`; and the reconciliation of the two internal type reps
//! (`type_realization` for the Proven/positive path vs `soc_regimes::native`
//! for conflict detection/negative path) into one canonical checker.

use std::collections::BTreeMap;

use brix_elaborate::{elaborate_tree, ElaborationResult};
use brix_kernel::Budget;
use brix_semantic::{ContextId, Outcome};
use brix_syntax::ast::{self, Item};
use soc_regimes::type_realization::{
    audited_type_check_tree, infer_tree, zonk, ArithOp, Expr as TrExpr, Infer, Ty as TrTy, TyCtx,
    TypeError,
};

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
}

impl From<TypeError> for LowerError {
    fn from(err: TypeError) -> Self {
        LowerError::TypeError(err)
    }
}

/// Lower a surface AST expression into a native [`soc_regimes::type_realization::Expr`].
pub fn lower_expr(
    e: &ast::Expr,
    fns: &BTreeMap<String, &ast::Callable>,
) -> Result<TrExpr, LowerError> {
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
        ast::Expr::Var(name) => Ok(TrExpr::Var(name.clone())),
        ast::Expr::Call { func, args } => {
            if let Some(c) = fns.get(func) {
                if c.params.len() != args.len() {
                    return Err(LowerError::Unsupported(format!(
                        "arity mismatch for function '{func}': expected {}, got {}",
                        c.params.len(),
                        args.len()
                    )));
                }
                let mut acc = lower_fn(c, fns)?;
                for arg in args {
                    let lowered_arg = lower_expr(arg, fns)?;
                    acc = TrExpr::App(Box::new(acc), Box::new(lowered_arg));
                }
                Ok(acc)
            } else {
                Err(LowerError::Unresolved(func.clone()))
            }
        }
        ast::Expr::Str(s) => Ok(TrExpr::StrLit(s.clone())),
        ast::Expr::Record { config: _, fields } => {
            let lowered_fields: Result<Vec<(String, TrExpr)>, LowerError> = fields
                .iter()
                .map(|(name, e)| Ok((name.clone(), lower_expr(e, fns)?)))
                .collect();
            Ok(TrExpr::Record(lowered_fields?))
        }
        ast::Expr::Field(base, name) => Ok(TrExpr::Field(
            Box::new(lower_expr(base, fns)?),
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
                Box::new(lower_expr(lhs, fns)?),
                Box::new(lower_expr(rhs, fns)?),
            ))
        }
        ast::Expr::Match { .. } => Err(LowerError::Unsupported(
            "Match not in L2-first fragment".to_string(),
        )),
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
pub fn lower_fn(
    c: &ast::Callable,
    fns: &BTreeMap<String, &ast::Callable>,
) -> Result<TrExpr, LowerError> {
    let body_tr = lower_expr(&c.body, fns)?;
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
    for item in &m.items {
        if let Item::Fn(c) = item {
            fns.insert(c.name.clone(), c);
        }
    }

    let mut ty_ctx = TyCtx::new();
    let mut results = Vec::new();

    for item in &m.items {
        if let Item::Let(let_decl) = item {
            let res = (|| {
                let tr_expr = lower_expr(&let_decl.value, &fns)?;
                let (ty, _ty_tree, st) = infer_tree(&tr_expr, &ty_ctx, Infer::new())?;
                let inferred_ty = zonk(&ty, &st.subst);
                let (audited_judgement, tree) =
                    audited_type_check_tree(&tr_expr, &ty_ctx, ContextId::root())?;
                match elaborate_tree(&audited_judgement, &tree, Budget::new(2000, 2000)) {
                    ElaborationResult::Proven { judgement, .. } => Ok((
                        CheckResult {
                            name: let_decl.name.clone(),
                            outcome: judgement.outcome,
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
