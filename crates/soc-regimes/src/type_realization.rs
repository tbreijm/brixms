//! SLICE 1 of the native type-realization regime (ADR-0005 Stage 2).
//!
//! Native type inference as SOC realization: produces real `HasType` `Derived`
//! judgements through the SOC proof substrate (literal + variable rules, single generator steps).

use std::collections::BTreeMap;

use brix_canon::{CanonWriter, Canonical};
use brix_semantic::{
    ConfigId, ContextId, Decomposition, Evidence, GeneratorId, Judgement, Outcome, Realizes,
    WitnessId,
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

/// Minimal expression AST for slice 1 of the native type-realization regime.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Expr {
    Lit(i64),
    Var(&'static str),
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

/// Immutable typing context mapping variable names to types for variable lookup.
///
/// BTreeMap used for determinism in slice 1; persistent-HAMT optimization
/// lands in slice 2 when unification/search arrives.
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

/// Errors during type checking in slice 1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeError {
    Unbound(String),
}

/// Type-checks `expr` in environment `ctx` under context `context`.
///
/// Produces a native SOC `Judgement` with `Outcome::Derived` asserting
/// `HasType(expr, ty)` = `Realizes(derivation_witness, expr_config, ty_config)`.
pub fn type_check(expr: &Expr, ctx: &TyCtx, context: ContextId) -> Result<Judgement, TypeError> {
    let (ty, generator) = match expr {
        Expr::Lit(_) => (Ty::Con("Int"), g_lit()),
        Expr::Var(name) => {
            let ty = ctx
                .get(name)
                .cloned()
                .ok_or_else(|| TypeError::Unbound((*name).to_string()))?;
            (ty, g_var())
        }
    };

    let derivation_witness: WitnessId = generator.witness_id();
    let src = expr.config_id();
    let dst = ty.config_id();

    let prop = Realizes::new(derivation_witness, src, dst).proposition_id();

    let decomp = Decomposition::recorded(vec![generator], vec![src, dst])
        .expect("single generator step decomposition is valid");
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
    fn test_determinism() {
        let expr = Expr::Var("x");
        let ctx = TyCtx::new().extend("x", Ty::Con("Int"));
        let context = ContextId::root();

        let j1 = type_check(&expr, &ctx, context).unwrap();
        let j2 = type_check(&expr, &ctx, context).unwrap();

        assert_eq!(j1, j2);
        assert_eq!(j1.id(), j2.id());
    }
}
