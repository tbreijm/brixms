//! SLICE 2 of the native type-realization regime (ADR-0005 Stage 2).
//!
//! Native type inference as SOC realization: produces real `HasType` `Derived`
//! judgements through the SOC proof substrate with App/Lam typing, declarative
//! unification, and multi-step composed derivations.

use std::collections::BTreeMap;

use brix_canon::{CanonWriter, Canonical};
use brix_semantic::{
    compose_chain, ConfigId, ContextId, Decomposition, Evidence, GeneratorId, Judgement, Outcome,
    Realizes, WitnessId,
};

/// Native representation of types in the type-realization regime (ADR-0005).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Ty {
    Con(&'static str),
    Fn(Box<Ty>, Box<Ty>),
    Var(u32),
}

impl Canonical for Ty {
    fn canon_write(&self, w: &mut CanonWriter) {
        match self {
            Ty::Con(name) => {
                w.write_enum(0, |w| w.write_str(name));
            }
            Ty::Fn(param, ret) => {
                w.write_enum(1, |w| {
                    param.canon_write(w);
                    ret.canon_write(w);
                });
            }
            Ty::Var(v) => {
                w.write_enum(2, |w| w.write_uint(*v as u64));
            }
        }
    }
}

impl Ty {
    /// Content-addressed type identity (`ConfigId`).
    pub fn config_id(&self) -> ConfigId {
        ConfigId::of(self)
    }
}

/// Expression AST for the native type-realization regime (ADR-0005).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Expr {
    Lit(i64),
    Var(&'static str),
    App(Box<Expr>, Box<Expr>),
    Lam(&'static str, Box<Expr>),
}

impl Canonical for Expr {
    fn canon_write(&self, w: &mut CanonWriter) {
        match self {
            Expr::Lit(val) => {
                w.write_enum(0, |w| w.write_int(*val));
            }
            Expr::Var(name) => {
                w.write_enum(1, |w| w.write_str(name));
            }
            Expr::App(f, x) => {
                w.write_enum(2, |w| {
                    f.canon_write(w);
                    x.canon_write(w);
                });
            }
            Expr::Lam(param, body) => {
                w.write_enum(3, |w| {
                    w.write_str(param);
                    body.canon_write(w);
                });
            }
        }
    }
}

impl Expr {
    /// Content-addressed expression identity (`ConfigId`).
    pub fn config_id(&self) -> ConfigId {
        ConfigId::of(self)
    }
}

/// Typing-rule generator for literal constants (`"type.rule.lit@1"`).
pub fn g_lit() -> GeneratorId {
    GeneratorId::named("type.rule.lit@1")
}

/// Typing-rule generator for variable lookups (`"type.rule.var@1"`).
pub fn g_var() -> GeneratorId {
    GeneratorId::named("type.rule.var@1")
}

/// Typing-rule generator for function application (`"type.rule.app@1"`).
pub fn g_app() -> GeneratorId {
    GeneratorId::named("type.rule.app@1")
}

/// Typing-rule generator for lambda abstraction (`"type.rule.lam@1"`).
pub fn g_lam() -> GeneratorId {
    GeneratorId::named("type.rule.lam@1")
}

/// Typing-rule generator for unification steps (`"type.rule.unify@1"`).
pub fn g_unify() -> GeneratorId {
    GeneratorId::named("type.rule.unify@1")
}

/// Immutable typing context mapping variable names to types for variable lookup.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct TyCtx(pub BTreeMap<&'static str, Ty>);

impl TyCtx {
    pub fn new() -> Self {
        Self(BTreeMap::new())
    }

    pub fn extend(&self, var: &'static str, ty: Ty) -> Self {
        let mut map = self.0.clone();
        map.insert(var, ty);
        Self(map)
    }

    pub fn get(&self, var: &str) -> Option<&Ty> {
        self.0.get(var)
    }
}

/// Errors during type checking in slice 2.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeError {
    Unbound(String),
    Mismatch,
    InfiniteType,
}

/// Immutable inference state containing substitution map and fresh variable counter.
///
/// NOTE (ADR-0005): Unification threads plain `BTreeMap<u32, Ty>` without hashing or
/// `ConfigId` materialization inside the inference loop.
/// A persistent HAMT (e.g. `im::HashMap`) is the future performance path for cheap structural
/// sharing; `BTreeMap` clone-extend is used here for slice-2 correctness without external dependencies.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Infer {
    pub subst: BTreeMap<u32, Ty>,
    pub next_var: u32,
}

impl Infer {
    pub fn new() -> Self {
        Self {
            subst: BTreeMap::new(),
            next_var: 0,
        }
    }

    /// Returns a fresh type variable index and an updated `Infer` state with `next_var` incremented.
    pub fn fresh_var(&self) -> (u32, Self) {
        let v = self.next_var;
        let next_st = Self {
            subst: self.subst.clone(),
            next_var: v + 1,
        };
        (v, next_st)
    }
}

/// Resolves top-level type variable indirections in `ty` under `subst`.
pub fn resolve<'a>(ty: &'a Ty, subst: &'a BTreeMap<u32, Ty>) -> &'a Ty {
    let mut curr = ty;
    while let Ty::Var(v) = curr {
        if let Some(next) = subst.get(v) {
            curr = next;
        } else {
            break;
        }
    }
    curr
}

/// Fully resolves (zonks) a type, recursively replacing all bound variables.
pub fn zonk(ty: &Ty, subst: &BTreeMap<u32, Ty>) -> Ty {
    match resolve(ty, subst) {
        Ty::Con(name) => Ty::Con(name),
        Ty::Var(v) => Ty::Var(*v),
        Ty::Fn(a, b) => Ty::Fn(Box::new(zonk(a, subst)), Box::new(zonk(b, subst))),
    }
}

/// Occurs-check: checks if type variable `v` occurs free in `ty` under `subst`.
pub fn occurs(v: u32, ty: &Ty, subst: &BTreeMap<u32, Ty>) -> bool {
    match resolve(ty, subst) {
        Ty::Var(v2) => v == *v2,
        Ty::Con(_) => false,
        Ty::Fn(a, b) => occurs(v, a, subst) || occurs(v, b, subst),
    }
}

/// Declarative unification as SOC context narrowing over explicit immutable substitution `subst`.
///
/// Returns the updated substitution and recorded `g_unify()` generator steps.
/// Does NOT mutate state or perform hashing inside the unification loop.
pub fn unify(
    t1: &Ty,
    t2: &Ty,
    subst: &BTreeMap<u32, Ty>,
) -> Result<(BTreeMap<u32, Ty>, Vec<GeneratorId>), TypeError> {
    let r1 = resolve(t1, subst);
    let r2 = resolve(t2, subst);

    match (r1, r2) {
        (Ty::Var(v1), Ty::Var(v2)) if v1 == v2 => Ok((subst.clone(), vec![g_unify()])),
        (Ty::Var(v1), t) => {
            if occurs(*v1, t, subst) {
                Err(TypeError::InfiniteType)
            } else {
                let mut next_subst = subst.clone();
                next_subst.insert(*v1, t.clone());
                Ok((next_subst, vec![g_unify()]))
            }
        }
        (t, Ty::Var(v2)) => {
            if occurs(*v2, t, subst) {
                Err(TypeError::InfiniteType)
            } else {
                let mut next_subst = subst.clone();
                next_subst.insert(*v2, t.clone());
                Ok((next_subst, vec![g_unify()]))
            }
        }
        (Ty::Con(a), Ty::Con(b)) => {
            if a == b {
                Ok((subst.clone(), vec![g_unify()]))
            } else {
                Err(TypeError::Mismatch)
            }
        }
        (Ty::Fn(a1, b1), Ty::Fn(a2, b2)) => {
            let (s1, mut steps1) = unify(a1, a2, subst)?;
            let (s2, steps2) = unify(b1, b2, &s1)?;
            let mut steps = vec![g_unify()];
            steps.append(&mut steps1);
            steps.extend(steps2);
            Ok((s2, steps))
        }
        _ => Err(TypeError::Mismatch),
    }
}

/// Core inference procedure threading immutable `Infer` state.
pub fn infer(
    expr: &Expr,
    ctx: &TyCtx,
    st: Infer,
) -> Result<(Ty, Vec<GeneratorId>, Infer), TypeError> {
    match expr {
        Expr::Lit(_) => Ok((Ty::Con("Int"), vec![g_lit()], st)),
        Expr::Var(name) => {
            let ty = ctx
                .get(name)
                .cloned()
                .ok_or_else(|| TypeError::Unbound((*name).to_string()))?;
            Ok((ty, vec![g_var()], st))
        }
        Expr::Lam(p, body) => {
            let (alpha, st_alpha) = st.fresh_var();
            let ctx_ext = ctx.extend(p, Ty::Var(alpha));
            let (tb, dv, st_prime) = infer(body, &ctx_ext, st_alpha)?;
            let param_ty = resolve(&Ty::Var(alpha), &st_prime.subst).clone();
            let fn_ty = Ty::Fn(Box::new(param_ty), Box::new(tb));
            let mut deriv = vec![g_lam()];
            deriv.extend(dv);
            Ok((fn_ty, deriv, st_prime))
        }
        Expr::App(f, x) => {
            let (tf, df, s1) = infer(f, ctx, st)?;
            let (tx, dx, s2) = infer(x, ctx, s1)?;
            let (beta, s_beta) = s2.fresh_var();

            let target_fn = Ty::Fn(
                Box::new(resolve(&tx, &s_beta.subst).clone()),
                Box::new(Ty::Var(beta)),
            );

            let (s3_subst, unify_steps) =
                unify(resolve(&tf, &s_beta.subst), &target_fn, &s_beta.subst)?;

            let res_ty = resolve(&Ty::Var(beta), &s3_subst).clone();

            let mut deriv = vec![g_app()];
            deriv.extend(df);
            deriv.extend(dx);
            deriv.extend(unify_steps);

            let st_final = Infer {
                subst: s3_subst,
                next_var: s_beta.next_var,
            };

            Ok((res_ty, deriv, st_final))
        }
    }
}

/// Type-checks `expr` in environment `ctx` under context `context`.
///
/// Produces a native SOC `Judgement` with `Outcome::Derived` asserting
/// `HasType(expr, ty)` = `Realizes(derivation_witness, expr_config, ty_config)`.
pub fn type_check(expr: &Expr, ctx: &TyCtx, context: ContextId) -> Result<Judgement, TypeError> {
    let st = Infer::new();
    let (ty, derivation, final_st) = infer(expr, ctx, st)?;

    // Fully resolve (zonk) all bound type variables in the resulting type.
    let final_ty = zonk(&ty, &final_st.subst);

    let derivation_witness: WitnessId = compose_chain(&derivation)
        .expect("derivation chain must contain at least one generator step");

    // CRITICAL DISCIPLINE (ADR-0005): Materialize canonical digests / ConfigIds ONLY here
    // at the commit boundary — NOT per unify/infer step.
    let src = expr.config_id();
    let dst = final_ty.config_id();

    let prop = Realizes::new(derivation_witness, src, dst).proposition_id();

    // INTERMEDIATE-CONFIG CHAIN DISCIPLINE:
    // For multi-step derivations in slice 2, we use endpoint padding (`[src, dst, ..., dst]`)
    // of length `derivation.len() + 1` to fulfill Decomposition's structural invariant
    // (`configs.len() == generators.len() + 1`) without creating intermediate ConfigIds during inference.
    let mut configs = vec![src];
    configs.resize(derivation.len() + 1, dst);

    let decomp = Decomposition::recorded(derivation, configs)
        .expect("multi-step derivation decomposition is valid");
    let evidence = Evidence::SettlementReplay {
        body: decomp.id().digest(),
    }
    .id();

    Ok(Judgement::new(context, prop, Outcome::Derived, evidence))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lit_type_check() {
        let expr = Expr::Lit(42);
        let ctx = TyCtx::new();
        let context = ContextId::root();

        let judgement = type_check(&expr, &ctx, context).expect("type check lit");
        assert_eq!(judgement.outcome, Outcome::Derived);
        assert_eq!(judgement.context, context);

        let expected_prop = Realizes::new(
            g_lit().witness_id(),
            expr.config_id(),
            Ty::Con("Int").config_id(),
        )
        .proposition_id();

        assert_eq!(judgement.proposition, expected_prop);
    }

    #[test]
    fn test_var_type_check() {
        let expr = Expr::Var("x");
        let ctx = TyCtx::new().extend("x", Ty::Con("Bool"));
        let context = ContextId::root();

        let judgement = type_check(&expr, &ctx, context).expect("type check var");
        assert_eq!(judgement.outcome, Outcome::Derived);

        let expected_prop = Realizes::new(
            g_var().witness_id(),
            expr.config_id(),
            Ty::Con("Bool").config_id(),
        )
        .proposition_id();

        assert_eq!(judgement.proposition, expected_prop);
    }

    #[test]
    fn test_unbound_var() {
        let expr = Expr::Var("y");
        let ctx = TyCtx::new();
        let context = ContextId::root();

        let res = type_check(&expr, &ctx, context);
        assert_eq!(res, Err(TypeError::Unbound("y".to_string())));
    }

    #[test]
    fn test_identity_applied() {
        // App(Lam("x", Var("x")), Lit(42))
        let expr = Expr::App(
            Box::new(Expr::Lam("x", Box::new(Expr::Var("x")))),
            Box::new(Expr::Lit(42)),
        );
        let ctx = TyCtx::new();
        let context = ContextId::root();

        let (ty, derivation, st) = infer(&expr, &ctx, Infer::new()).expect("infer identity app");
        let final_ty = zonk(&ty, &st.subst);
        assert_eq!(final_ty, Ty::Con("Int"));

        // Multi-generator derivation
        assert!(derivation.contains(&g_app()));
        assert!(derivation.contains(&g_lam()));
        assert!(derivation.contains(&g_var()));
        assert!(derivation.contains(&g_lit()));
        assert!(derivation.contains(&g_unify()));

        let witness = compose_chain(&derivation).expect("witness composition");

        let judgement = type_check(&expr, &ctx, context).expect("type check identity app");
        assert_eq!(judgement.outcome, Outcome::Derived);

        let expected_prop =
            Realizes::new(witness, expr.config_id(), Ty::Con("Int").config_id()).proposition_id();
        assert_eq!(judgement.proposition, expected_prop);
    }

    #[test]
    fn test_const_function() {
        // Lam("x", Lit(7))
        let expr = Expr::Lam("x", Box::new(Expr::Lit(7)));
        let ctx = TyCtx::new();
        let context = ContextId::root();

        let judgement = type_check(&expr, &ctx, context).expect("type check const fn");
        assert_eq!(judgement.outcome, Outcome::Derived);

        let (ty, _, st) = infer(&expr, &ctx, Infer::new()).expect("infer const fn");
        let final_ty = zonk(&ty, &st.subst);
        assert_eq!(
            final_ty,
            Ty::Fn(Box::new(Ty::Var(0)), Box::new(Ty::Con("Int")))
        );
    }

    #[test]
    fn test_mismatch_non_function_application() {
        // App(Lit(1), Lit(2)) -> applying non-function Int
        let expr = Expr::App(Box::new(Expr::Lit(1)), Box::new(Expr::Lit(2)));
        let ctx = TyCtx::new();
        let context = ContextId::root();

        let res = type_check(&expr, &ctx, context);
        assert_eq!(res, Err(TypeError::Mismatch));
    }

    #[test]
    fn test_occurs_check_via_app() {
        // ctx: f : Var(0)
        // expr: App(Var("f"), Var("f")) -> unifying Var(0) with Fn(Var(0), Var(beta)) triggers InfiniteType
        let expr = Expr::App(Box::new(Expr::Var("f")), Box::new(Expr::Var("f")));
        let ctx = TyCtx::new().extend("f", Ty::Var(0));
        let context = ContextId::root();

        let res = type_check(&expr, &ctx, context);
        assert_eq!(res, Err(TypeError::InfiniteType));
    }

    #[test]
    fn test_determinism() {
        let expr = Expr::App(
            Box::new(Expr::Lam("x", Box::new(Expr::Var("x")))),
            Box::new(Expr::Lit(42)),
        );
        let ctx = TyCtx::new();
        let context = ContextId::root();

        let j1 = type_check(&expr, &ctx, context).unwrap();
        let j2 = type_check(&expr, &ctx, context).unwrap();

        assert_eq!(j1, j2);
        assert_eq!(j1.id(), j2.id());
    }

    #[test]
    fn test_decomposition_round_trip() {
        let expr = Expr::App(
            Box::new(Expr::Lam("x", Box::new(Expr::Var("x")))),
            Box::new(Expr::Lit(42)),
        );
        let ctx = TyCtx::new();

        let (ty, derivation, st) = infer(&expr, &ctx, Infer::new()).unwrap();
        let final_ty = zonk(&ty, &st.subst);

        let src = expr.config_id();
        let dst = final_ty.config_id();
        let mut configs = vec![src];
        configs.resize(derivation.len() + 1, dst);

        let decomp = Decomposition::recorded(derivation.clone(), configs);
        assert!(decomp.is_ok());

        let witness = compose_chain(&derivation).unwrap();
        let j = type_check(&expr, &ctx, ContextId::root()).unwrap();

        let expected_prop = Realizes::new(witness, src, dst).proposition_id();
        assert_eq!(j.proposition, expected_prop);
    }
}
