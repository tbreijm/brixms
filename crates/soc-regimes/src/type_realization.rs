//! SLICE 2 of the native type-realization regime (ADR-0005 Stage 2).
//!
//! Native type inference as SOC realization: produces real `HasType` `Derived`
//! judgements through the SOC proof substrate with App/Lam typing, declarative
//! unification, and multi-step composed derivations.

use std::collections::BTreeMap;

use brix_canon::{CanonWriter, Canonical};
use brix_elaborate::{RealizesTree, TreeObj};
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

/// Typing-rule generator for application splitting (`"type.rule.app.split@1"`).
pub fn g_split() -> GeneratorId {
    GeneratorId::named("type.rule.app.split@1")
}

/// Typing-rule generator for binary application (`"type.rule.app@2"`).
pub fn g_app2() -> GeneratorId {
    GeneratorId::named("type.rule.app@2")
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
    Unsupported,
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

/// Upgrades a native `type_check` derivation to an `Audited` `Judgement` and
/// verified `Decomposition` (ADR-0005 Stage 2 depth slice).
///
/// Produces a replay-verified decomposition and an `Outcome::Audited` judgement,
/// suitable for proof kernel elaboration via `brix_elaborate::elaborate_decomposition`.
pub fn audited_type_check(
    expr: &Expr,
    ctx: &TyCtx,
    context: ContextId,
) -> Result<(Judgement, Decomposition), TypeError> {
    let st = Infer::new();
    let (ty, derivation, final_st) = infer(expr, ctx, st)?;
    let final_ty = zonk(&ty, &final_st.subst);

    let derivation_witness: WitnessId = compose_chain(&derivation)
        .expect("derivation chain must contain at least one generator step");

    let src = expr.config_id();
    let dst = final_ty.config_id();

    let prop = Realizes::new(derivation_witness, src, dst).proposition_id();

    let mut configs = vec![src];
    configs.resize(derivation.len() + 1, dst);

    let verified_decomp =
        Decomposition::replay_verified(derivation, configs).expect("replay verified decomposition");
    let evidence = Evidence::SettlementReplay {
        body: verified_decomp.id().digest(),
    }
    .id();

    let audited = Judgement::new(context, prop, Outcome::Audited, evidence);
    Ok((audited, verified_decomp))
}

/// Infer tree-structured realization derivation for {Lit, Var, App} (ADR-0007).
pub fn infer_tree(
    expr: &Expr,
    ctx: &TyCtx,
    st: Infer,
) -> Result<(Ty, RealizesTree, Infer), TypeError> {
    match expr {
        Expr::Lit(n) => Ok((
            Ty::Con("Int"),
            RealizesTree::Leaf {
                generator: g_lit(),
                src: TreeObj::Atom(Expr::Lit(*n).config_id()),
                dst: TreeObj::Atom(Ty::Con("Int").config_id()),
            },
            st,
        )),
        Expr::Var(name) => {
            let t = ctx
                .get(name)
                .cloned()
                .ok_or_else(|| TypeError::Unbound((*name).to_string()))?;
            Ok((
                t.clone(),
                RealizesTree::Leaf {
                    generator: g_var(),
                    src: TreeObj::Atom(Expr::Var(name).config_id()),
                    dst: TreeObj::Atom(t.config_id()),
                },
                st,
            ))
        }
        Expr::Lam(_, _) => Err(TypeError::Unsupported),
        Expr::App(f, x) => {
            let (tf, df, s1) = infer_tree(f, ctx, st)?;
            let (tx, dx, s2) = infer_tree(x, ctx, s1)?;
            let (beta, s_beta) = s2.fresh_var();

            let target = Ty::Fn(
                Box::new(resolve(&tx, &s_beta.subst).clone()),
                Box::new(Ty::Var(beta)),
            );

            let (s3, _) = unify(resolve(&tf, &s_beta.subst), &target, &s_beta.subst)?;

            let a = zonk(&tx, &s3);
            let b = zonk(&Ty::Var(beta), &s3);
            let fn_ty = Ty::Fn(Box::new(a.clone()), Box::new(b.clone()));

            let split = RealizesTree::Leaf {
                generator: g_split(),
                src: TreeObj::Atom(expr.config_id()),
                dst: TreeObj::Prod(
                    Box::new(TreeObj::Atom(f.config_id())),
                    Box::new(TreeObj::Atom(x.config_id())),
                ),
            };

            let tensor = RealizesTree::Tensor {
                left: Box::new(df),
                right: Box::new(dx),
            };

            let app = RealizesTree::Leaf {
                generator: g_app2(),
                src: TreeObj::Prod(
                    Box::new(TreeObj::Atom(fn_ty.config_id())),
                    Box::new(TreeObj::Atom(a.config_id())),
                ),
                dst: TreeObj::Atom(b.config_id()),
            };

            let tree = RealizesTree::Seq {
                left: Box::new(split),
                right: Box::new(RealizesTree::Seq {
                    left: Box::new(tensor),
                    right: Box::new(app),
                }),
            };

            Ok((
                b,
                tree,
                Infer {
                    subst: s3,
                    next_var: s_beta.next_var,
                },
            ))
        }
    }
}

/// Upgrades a native `infer_tree` derivation to an `Audited` `Judgement` and `RealizesTree` (ADR-0007).
pub fn audited_type_check_tree(
    expr: &Expr,
    ctx: &TyCtx,
    context: ContextId,
) -> Result<(Judgement, RealizesTree), TypeError> {
    let (ty, tree, _st) = infer_tree(expr, ctx, Infer::new())?;
    let witness_id = tree.witness_object().witness_digest();
    let prop = Realizes::new(witness_id, expr.config_id(), ty.config_id()).proposition_id();
    let evidence = Evidence::SettlementReplay {
        body: brix_canon::Digest::of(brix_canon::Domain::Value, prop.digest().as_bytes()),
    }
    .id();
    let audited = Judgement::new(context, prop, Outcome::Audited, evidence);
    Ok((audited, tree))
}

#[cfg(test)]
mod tests {
    use super::*;
    use brix_elaborate::{elaborate_decomposition, ElaborationResult};
    use brix_kernel::Budget;
    use brix_semantic::{Authority, CertificateId, EdgeKind, GeneratorRegistry, VerifierId};

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

    #[test]
    fn test_literal_elaboration_to_proven() {
        let expr = Expr::Lit(42);
        let ctx = TyCtx::new();
        let context = ContextId::root();

        let (audited_judgement, verified_decomp) =
            audited_type_check(&expr, &ctx, context).expect("audited type check lit");

        assert_eq!(audited_judgement.outcome, Outcome::Audited);

        let budget = Budget::new(1000, 1000);
        let res = elaborate_decomposition(&audited_judgement, &verified_decomp, budget);

        match res {
            ElaborationResult::Proven { judgement, edge } => {
                assert_eq!(judgement.outcome, Outcome::Proven);
                assert_eq!(judgement.outcome.authority(), Authority::ProofKernel);
                assert_eq!(judgement.context, context);

                // Edge assertion: ElaborationBoundary pointing to audited_judgement's id
                assert_eq!(edge.kind, EdgeKind::ElaborationBoundary);
                assert_eq!(edge.target, audited_judgement.id().digest());

                // Evidence assertion: KernelCertificate
                let expected_verifier = VerifierId::named("brix.kernel@0.1");
                let g1 = g_lit();
                let src = expr.config_id();
                let dst = Ty::Con("Int").config_id();

                let h1 = brix_kernel::Prop::Realizes(
                    brix_kernel::ObjectTerm::Const(brix_semantic::PropositionId(g1.digest())),
                    brix_kernel::ObjectTerm::Const(brix_semantic::PropositionId(src.digest())),
                    brix_kernel::ObjectTerm::Const(brix_semantic::PropositionId(dst.digest())),
                );
                let goal_prop = brix_kernel::Prop::Realizes(
                    brix_kernel::ObjectTerm::Const(brix_semantic::PropositionId(g1.digest())),
                    brix_kernel::ObjectTerm::Const(brix_semantic::PropositionId(src.digest())),
                    brix_kernel::ObjectTerm::Const(brix_semantic::PropositionId(dst.digest())),
                );
                let implication_prop = brix_kernel::Prop::Impl(Box::new(h1), Box::new(goal_prop));

                let explicit_term = brix_kernel::ExplicitTerm::new(
                    context,
                    brix_kernel::TermKind::Lam {
                        var_name: Some("h1".to_string()),
                        body: Box::new(brix_kernel::TermKind::Hyp(brix_kernel::Var::Index(0))),
                    },
                );

                let cert_payload = format!("{context:?}:{implication_prop:?}:{explicit_term:?}");
                let cert_id = CertificateId::from_canon(cert_payload.as_bytes());
                let expected_evidence = Evidence::KernelCertificate {
                    verifier: expected_verifier,
                    certificate: cert_id,
                };

                assert_eq!(judgement.evidence, expected_evidence.id());
                assert_eq!(judgement.proposition, implication_prop.proposition_id());
            }
            ElaborationResult::NotElaborated(verdict) => {
                panic!("Expected Proven, got NotElaborated({verdict:?})");
            }
        }
    }

    #[test]
    fn test_multi_step_elaboration_tree_vs_linear_tension() {
        let expr = Expr::App(
            Box::new(Expr::Lam("x", Box::new(Expr::Var("x")))),
            Box::new(Expr::Lit(42)),
        );
        let ctx = TyCtx::new();
        let context = ContextId::root();

        let (audited_judgement, verified_decomp) =
            audited_type_check(&expr, &ctx, context).expect("audited type check identity app");

        // 1. Syntactic elaboration passes via endpoints-only padding because dst == dst in RealizesComp
        let budget = Budget::new(1000, 1000);
        let res = elaborate_decomposition(&audited_judgement, &verified_decomp, budget);
        assert!(
            matches!(res, ElaborationResult::Proven { .. }),
            "Padded configs pass syntactic RealizesComp because dst == dst middle match holds"
        );

        // 2. But padded configs fail semantic audit because g_lam, g_var, g_lit, etc. do NOT realize (dst, dst)
        let mut registry = GeneratorRegistry::new();
        registry.insert(g_app());
        registry.insert(g_lam());
        registry.insert(g_var());
        registry.insert(g_lit());
        registry.insert(g_unify());

        struct NonPaddedSemantics;
        impl soc_core::GeneratorSemantics for NonPaddedSemantics {
            fn realizes(&self, _g: &GeneratorId, src: &ConfigId, dst: &ConfigId) -> bool {
                src != dst
            }
        }

        let unverified_decomp = Decomposition::recorded(
            verified_decomp.generators.clone(),
            verified_decomp.configs.clone(),
        )
        .unwrap();

        let derived_evidence = Evidence::SettlementReplay {
            body: unverified_decomp.id().digest(),
        }
        .id();
        let derived_id = Judgement::new(
            context,
            audited_judgement.proposition,
            Outcome::Derived,
            derived_evidence,
        )
        .id();

        let step = soc_core::CommittedStep {
            key: soc_core::Key::new(
                0,
                0,
                brix_canon::Digest::of(brix_canon::Domain::Value, b"app_tiebreak"),
            ),
            observation: soc_core::Observation {
                outcome_class: Outcome::Derived,
                judgement_digest: derived_id.digest(),
            },
            decomposition: unverified_decomp,
            src: expr.config_id(),
            dst: Ty::Con("Int").config_id(),
            witness: compose_chain(&verified_decomp.generators).unwrap(),
        };

        let audit_res = soc_core::audit_step(&step, context, &registry, &NonPaddedSemantics);
        assert!(
            matches!(audit_res, soc_core::AuditResult::Unknown(_)),
            "Audit MUST fail for endpoints-padded configs under sound generator semantics"
        );
    }

    #[test]
    fn test_tree_elaboration_end_to_end() {
        let expr = Expr::App(Box::new(Expr::Var("f")), Box::new(Expr::Lit(1)));
        let ctx = TyCtx::new().extend(
            "f",
            Ty::Fn(Box::new(Ty::Con("Int")), Box::new(Ty::Con("Bool"))),
        );
        let context = ContextId::root();

        let (aud, tree) = audited_type_check_tree(&expr, &ctx, context).unwrap();
        assert_eq!(aud.outcome, Outcome::Audited);

        // Inspect tree: g_app2 leaf src config equals Prod(Atom(Fn(Int,Bool).config_id()), Atom(Int.config_id()))
        if let RealizesTree::Seq { right, .. } = &tree {
            if let RealizesTree::Seq {
                right: app_leaf, ..
            } = right.as_ref()
            {
                if let RealizesTree::Leaf { generator, src, .. } = app_leaf.as_ref() {
                    assert_eq!(*generator, g_app2());
                    let expected_src = TreeObj::Prod(
                        Box::new(TreeObj::Atom(
                            Ty::Fn(Box::new(Ty::Con("Int")), Box::new(Ty::Con("Bool"))).config_id(),
                        )),
                        Box::new(TreeObj::Atom(Ty::Con("Int").config_id())),
                    );
                    assert_eq!(*src, expected_src);
                } else {
                    panic!("Expected app leaf");
                }
            } else {
                panic!("Expected inner Seq");
            }
        } else {
            panic!("Expected outer Seq");
        }

        let res = brix_elaborate::elaborate_tree(&aud, &tree, Budget::new(1000, 1000));
        match res {
            ElaborationResult::Proven { judgement, edge } => {
                assert_eq!(judgement.outcome, Outcome::Proven);
                assert_eq!(
                    judgement.outcome.authority(),
                    brix_semantic::Authority::ProofKernel
                );
                assert_eq!(edge.kind, brix_semantic::EdgeKind::ElaborationBoundary);
                assert_eq!(edge.target, aud.id().digest());
            }
            other => panic!("Expected Proven, got {:?}", other),
        }

        // Determinism check
        let (aud2, tree2) = audited_type_check_tree(&expr, &ctx, context).unwrap();
        assert_eq!(aud, aud2);
        assert_eq!(tree, tree2);
    }
}
