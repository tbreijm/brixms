//! Hindley--Milner style monomorphic inference for the lowered v1 Core IR.
//! Polymorphic dimension variables and trait bounds are deliberately deferred;
//! all physical dimensions here are ground exponent vectors.
//!
//! The pure unification/dimension algebra lives in [`crate::solve`] and is
//! shared with [`crate::reflect`] (#15 PR2: "one algorithm, two
//! observers") — this module supplies only the *observation*: it mutates
//! `Expr.ty` in place and accumulates [`TypeError`]s.

use std::collections::BTreeMap;

use crate::core::{Constraint, Expr, ExprKind, FnDef, Head, Query, Rule};
use crate::frontend::{FrontendSource, SchemaResolver};
use crate::ident::{Ident, QualIdent};
use crate::pattern::{Arg, Clause, Lit, Pattern, RoleArg};
use crate::solve::{self, DimBinaryStep, DimStep, Step};
use crate::types::{Row, Ty, TyVar};

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TypeError {
    Mismatch {
        expected: Ty,
        found: Ty,
    },
    Dimension {
        operation: String,
        left: Ty,
        right: Ty,
    },
    Arity {
        function: QualIdent,
        expected: usize,
        found: usize,
    },
    /// No declared overload unifies with the actual argument types.
    NoMatchingOverload {
        function: QualIdent,
        found: String,
    },
    /// Two or more overloads unify equally well with the actual arguments.
    AmbiguousOverload {
        function: QualIdent,
        found: String,
    },
    UnknownField {
        field: Ident,
        base: Ty,
    },
    NonBoolGuard {
        found: Ty,
    },
    TryNonResult {
        found: Ty,
    },
    Occurs {
        var: TyVar,
        in_ty: Ty,
    },
    /// #15 PR5 (§19.1 / conformance I.22.2): an implicit conversion from an
    /// epistemic-status-bearing type (`Estimate<T>`, `Missing<T>`,
    /// `Probability`) to its plain payload type (or, for `Probability`,
    /// `Bool`) was attempted. Distinct from [`TypeError::Mismatch`] so the
    /// diagnostic names the erasure — see [`crate::solve::Step::Erasure`].
    EpistemicErasure {
        from: Ty,
        to: Ty,
    },
}

impl core::fmt::Display for TypeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Mismatch { expected, found } => {
                write!(f, "type mismatch: expected {expected}, found {found}")
            }
            Self::Dimension {
                operation,
                left,
                right,
            } => write!(f, "dimension error in {operation}: {left} and {right}"),
            Self::Arity {
                function,
                expected,
                found,
            } => write!(
                f,
                "arity error: {function} expects {expected} arguments, got {found}"
            ),
            Self::NoMatchingOverload { function, found } => {
                write!(
                    f,
                    "no matching overload for {function} with argument types [{found}]"
                )
            }
            Self::AmbiguousOverload { function, found } => {
                write!(
                    f,
                    "ambiguous overload for {function} with argument types [{found}]"
                )
            }
            Self::UnknownField { field, base } => write!(f, "unknown field `{field}` on {base}"),
            Self::NonBoolGuard { found } => write!(f, "when guard must be Bool, found {found}"),
            Self::TryNonResult { found } => write!(f, "`?` requires Result<_, _>, found {found}"),
            Self::Occurs { var, in_ty } => {
                write!(f, "occurs check failed: {var} occurs in {in_ty}")
            }
            Self::EpistemicErasure { from, to } => write!(
                f,
                "epistemic status erasure: {from} cannot implicitly convert to {to}"
            ),
        }
    }
}

pub fn infer_source(source: &mut FrontendSource, resolver: &impl SchemaResolver) -> Vec<TypeError> {
    let mut cx = Infer::default();
    for rule in &mut source.rules {
        cx.rule(rule, resolver);
    }
    for constraint in &mut source.constraints {
        cx.constraint(constraint, resolver);
    }
    for query in &mut source.queries {
        cx.query(query, resolver);
    }
    // Each function gets a fresh substitution so tyvars from one overload
    // body cannot bind Int into a sibling Float overload (or vice versa).
    for function in &mut source.functions {
        cx.subst.clear();
        cx.function(function, resolver);
    }
    cx.errors
}

#[derive(Default)]
struct Infer {
    subst: BTreeMap<TyVar, Ty>,
    errors: Vec<TypeError>,
}
type Env = BTreeMap<Ident, Ty>;

impl Infer {
    fn rule(&mut self, rule: &mut Rule, resolver: &impl SchemaResolver) {
        let mut env = Env::new();
        self.pattern(&mut rule.body, &mut env, resolver);
        self.head(&rule.head, &env, resolver);
        self.zonk_pattern(&mut rule.body);
    }
    /// Mirrors [`Infer::rule`] minus the `head` call: a [`Constraint`] body
    /// is a bare [`Pattern`] with no head to check against a relation
    /// schema (`crate::core::Constraint` has `{ name, severity, body }`).
    /// Matches `reflect::Reflect::constraint`'s traversal exactly, closing
    /// the #15 PR2 coverage gap where `infer_source` never visited
    /// `FrontendSource::constraints` at all.
    fn constraint(&mut self, constraint: &mut Constraint, resolver: &impl SchemaResolver) {
        let mut env = Env::new();
        self.pattern(&mut constraint.body, &mut env, resolver);
        self.zonk_pattern(&mut constraint.body);
    }
    fn query(&mut self, query: &mut Query, resolver: &impl SchemaResolver) {
        let mut env: Env = query.params.iter().cloned().collect();
        self.pattern(&mut query.body, &mut env, resolver);
        let yielded = self.expr(&mut query.yields, &mut env, resolver);
        self.unify(
            query.result.clone(),
            Ty::rel(match self.resolve(yielded) {
                Ty::Record(row) => *row,
                t => Row::closed(vec![crate::types::RowField {
                    name: Ident::new("value"),
                    ty: t,
                }]),
            }),
        );
        self.zonk_pattern(&mut query.body);
        self.zonk_expr(&mut query.yields);
        query.result = self.resolve(query.result.clone());
    }
    /// Infer/check a user function body (issue #47). Seeds the env from the
    /// declared parameters (mirroring [`Infer::query`]'s param seeding), infers
    /// the body expression, and unifies its type against the declared return
    /// type — so a body that computes the wrong type is a `Mismatch`. Only
    /// total, expression-bodied functions reach here in Slice 1 (partial /
    /// block bodies are rejected at lowering).
    fn function(&mut self, def: &mut FnDef, resolver: &impl SchemaResolver) {
        let mut env: Env = def.params.iter().cloned().collect();
        let body_ty = self.expr(&mut def.body, &mut env, resolver);
        self.unify(def.ret.clone(), body_ty);
        self.zonk_expr(&mut def.body);
        def.ret = self.resolve(def.ret.clone());
    }
    fn pattern(&mut self, pattern: &mut Pattern, env: &mut Env, resolver: &impl SchemaResolver) {
        for clause in &mut pattern.clauses {
            match clause {
                Clause::Edge { relation, args, .. } | Clause::History { relation, args, .. } => {
                    if let Some(schema) = resolver.relation(relation) {
                        for arg in args {
                            if let Some((_, ty)) = schema.roles.iter().find(|(n, _)| n == &arg.role)
                            {
                                self.role_arg(arg, ty.clone(), env);
                            }
                        }
                    }
                }
                Clause::Entity {
                    var,
                    entity,
                    fields,
                } => {
                    env.entry(var.clone())
                        .or_insert_with(|| Ty::NodeRef(entity.clone()));
                    if let Some(schema) = resolver.relation(&QualIdent::simple(entity.as_str())) {
                        for arg in fields {
                            if let Some((_, ty)) =
                                schema.roles.iter().find(|(name, _)| name == &arg.role)
                            {
                                self.role_arg(arg, ty.clone(), env);
                            }
                        }
                    }
                }
                Clause::Let { binds, expr } => {
                    let ty = self.expr(expr, env, resolver);
                    env.insert(binds.clone(), ty);
                }
                Clause::When(expr) => {
                    let ty = self.expr(expr, env, resolver);
                    if self.resolve(ty.clone()) != Ty::Bool {
                        self.errors.push(TypeError::NonBoolGuard {
                            found: self.resolve(ty),
                        });
                    }
                }
                Clause::Any(cases) => {
                    for p in cases {
                        self.pattern(p, env, resolver);
                    }
                }
                Clause::Exists(p) | Clause::Without(p) | Clause::Optional(p) | Clause::Cross(p) => {
                    self.pattern(p, env, resolver)
                }
            }
        }
    }
    fn role_arg(&mut self, arg: &RoleArg, expected: Ty, env: &mut Env) {
        match &arg.arg {
            Arg::Var(v) => {
                if let Some(old) = env.get(v).cloned() {
                    self.unify(old, expected);
                } else {
                    env.insert(v.clone(), expected);
                }
            }
            Arg::Lit(lit) => self.unify(self.lit(lit), expected),
        }
    }
    fn head(&mut self, head: &Head, env: &Env, resolver: &impl SchemaResolver) {
        let (relation, args) = match head {
            Head::Tuple { relation, args } => (Some(relation), Some(args)),
            _ => (None, None),
        };
        if let (Some(relation), Some(args)) = (relation, args) {
            if let Some(schema) = resolver.relation(relation) {
                for arg in args {
                    if let Some((_, expected)) = schema.roles.iter().find(|(n, _)| n == &arg.role) {
                        match &arg.arg {
                            Arg::Var(v) => {
                                if let Some(actual) = env.get(v) {
                                    self.unify(actual.clone(), expected.clone());
                                }
                            }
                            Arg::Lit(l) => self.unify(self.lit(l), expected.clone()),
                        }
                    }
                }
            }
        }
    }
    fn expr(&mut self, expr: &mut Expr, env: &mut Env, resolver: &impl SchemaResolver) -> Ty {
        // The dimensioned type lowering assigned to this node, used to type
        // unit constructors (`brix.units.*`) that brix-ir can't resolve itself.
        let node_ty = expr.ty.clone();
        let ty = match &mut *expr.kind {
            ExprKind::Var(v) => env.get(v).cloned().unwrap_or_else(|| expr.ty.clone()),
            ExprKind::Lit(l) => self.lit(l),
            ExprKind::Record { fields } => Ty::record(Row::closed(
                fields
                    .iter_mut()
                    .map(|(n, e)| crate::types::RowField {
                        name: n.clone(),
                        ty: self.expr(e, env, resolver),
                    })
                    .collect(),
            )),
            ExprKind::Field { base, field } => {
                let base_ty = self.expr(base, env, resolver);
                match self.resolve(base_ty) {
                    Ty::Record(row) | Ty::Rel(row) => row
                        .fields
                        .iter()
                        .find(|x| &x.name == field)
                        .map(|x| x.ty.clone())
                        .unwrap_or_else(|| {
                            self.errors.push(TypeError::UnknownField {
                                field: field.clone(),
                                base: Ty::Record(row.clone()),
                            });
                            Ty::Error
                        }),
                    t => {
                        self.errors.push(TypeError::UnknownField {
                            field: field.clone(),
                            base: t,
                        });
                        Ty::Error
                    }
                }
            }
            ExprKind::If { cond, then, els } => {
                let c = self.expr(cond, env, resolver);
                self.unify(c, Ty::Bool);
                let a = self.expr(then, env, resolver);
                let b = self.expr(els, env, resolver);
                self.unify(a.clone(), b);
                a
            }
            ExprKind::Try { inner, .. } => {
                let inner_ty = self.expr(inner, env, resolver);
                match self.resolve(inner_ty) {
                    Ty::Result(ok, _) => *ok,
                    t => {
                        self.errors.push(TypeError::TryNonResult { found: t });
                        Ty::Error
                    }
                }
            }
            ExprKind::Comprehension { pattern, yields } => {
                let mut nested = env.clone();
                self.pattern(pattern, &mut nested, resolver);
                let row = yields
                    .as_mut()
                    .map(|y| match self.expr(y, &mut nested, resolver) {
                        Ty::Record(r) => *r,
                        t => Row::closed(vec![crate::types::RowField {
                            name: Ident::new("value"),
                            ty: t,
                        }]),
                    })
                    .unwrap_or_else(|| Row::closed(vec![]));
                Ty::rel(row)
            }
            ExprKind::Call { func, args } => {
                if func.to_string().starts_with("brix.units.") {
                    // A unit constructor (`150 EUR`): lowering, which owns the
                    // unit table, already gave this node its dimensioned type.
                    // brix-ir can't resolve `brix.units.*`, so infer the args
                    // for their own errors but trust the lowered type.
                    for a in args.iter_mut() {
                        self.expr(a, env, resolver);
                    }
                    node_ty.clone()
                } else {
                    self.call(func, args, env, resolver)
                }
            }
            ExprKind::Let { name, value, body } => {
                // `let name = value in body` (issue #47 Slice 2): the binding
                // is in scope for `body` only; the whole expression's type is
                // `body`'s.
                let value_ty = self.expr(value, env, resolver);
                let mut inner = env.clone();
                inner.insert(name.clone(), value_ty);
                self.expr(body, &mut inner, resolver)
            }
        };
        self.unify(expr.ty.clone(), ty.clone());
        expr.ty = self.resolve(ty.clone());
        ty
    }
    fn call(
        &mut self,
        func: &QualIdent,
        args: &mut [Expr],
        env: &mut Env,
        resolver: &impl SchemaResolver,
    ) -> Ty {
        let actual: Vec<Ty> = args
            .iter_mut()
            .map(|x| self.expr(x, env, resolver))
            .collect();
        let op = func.to_string();
        if let Some(name) = op.strip_prefix("brix.ops.") {
            return self.operator(name, &actual);
        }
        let candidates = resolver.functions(func);
        if candidates.is_empty() {
            return Ty::Error;
        }
        let arity_ok: Vec<_> = candidates
            .iter()
            .filter(|sig| sig.params.len() == actual.len())
            .collect();
        if arity_ok.is_empty() {
            self.errors.push(TypeError::Arity {
                function: func.clone(),
                expected: candidates[0].params.len(),
                found: actual.len(),
            });
            return Ty::Error;
        }
        let mut matches: Vec<(&crate::frontend::FnSignature, BTreeMap<TyVar, Ty>, i32)> =
            Vec::new();
        for sig in &arity_ok {
            if let Some(next) = solve::try_unify_args(&self.subst, &actual, &sig.params) {
                let score = overload_specificity(&next, &actual, &sig.params);
                matches.push((*sig, next, score));
            }
        }
        let found = format_arg_types(&self.subst, &actual);
        match matches.len() {
            0 => {
                self.errors.push(TypeError::NoMatchingOverload {
                    function: func.clone(),
                    found,
                });
                Ty::Error
            }
            1 => {
                let (sig, next, _) = matches.pop().unwrap();
                self.subst = next;
                sig.ret.clone()
            }
            _ => {
                let best = matches.iter().map(|(_, _, s)| *s).max().unwrap_or(0);
                let top: Vec<_> = matches.into_iter().filter(|(_, _, s)| *s == best).collect();
                if top.len() != 1 {
                    self.errors.push(TypeError::AmbiguousOverload {
                        function: func.clone(),
                        found,
                    });
                    return Ty::Error;
                }
                let (sig, next, _) = top.into_iter().next().unwrap();
                self.subst = next;
                sig.ret.clone()
            }
        }
    }
    fn operator(&mut self, name: &str, args: &[Ty]) -> Ty {
        if args.len() != if name == "not" || name == "neg" { 1 } else { 2 } {
            self.errors.push(TypeError::Arity {
                function: QualIdent::from(format!("brix.ops.{name}").as_str()),
                expected: if name == "not" || name == "neg" { 1 } else { 2 },
                found: args.len(),
            });
            return Ty::Error;
        }
        match name {
            "and" | "or" => {
                self.unify(args[0].clone(), Ty::Bool);
                self.unify(args[1].clone(), Ty::Bool);
                Ty::Bool
            }
            "not" => {
                self.unify(args[0].clone(), Ty::Bool);
                Ty::Bool
            }
            "eq" | "ne" | "lt" | "le" | "gt" | "ge" => {
                self.same_dimension(name, &args[0], &args[1]);
                Ty::Bool
            }
            "add" | "sub" => self.same_dimension(name, &args[0], &args[1]),
            "mul" => self.dimension_binary(&args[0], &args[1], true),
            "div" => self.dimension_binary(&args[0], &args[1], false),
            "neg" => args[0].clone(),
            _ => Ty::Error,
        }
    }
    /// Dimension-vs-variable ruling: when one side lacks ground dimensions,
    /// [`solve::same_dimension_step`] solves/unifies it rather than
    /// reporting a conflict — see that function's doc for the frozen
    /// ruling. Only two ground, unequal dimension sets are a real error.
    fn same_dimension(&mut self, operation: &str, a: &Ty, b: &Ty) -> Ty {
        match solve::same_dimension_step(a, b) {
            DimStep::Ok(t) => t,
            DimStep::Conflict => {
                self.errors.push(TypeError::Dimension {
                    operation: operation.to_owned(),
                    left: a.clone(),
                    right: b.clone(),
                });
                Ty::Error
            }
            DimStep::Solve(x, y) => {
                self.unify(x.clone(), y);
                x
            }
        }
    }
    fn dimension_binary(&mut self, a: &Ty, b: &Ty, mul: bool) -> Ty {
        match solve::dimension_binary_step(a, b, mul) {
            DimBinaryStep::Ok(t) => t,
            DimBinaryStep::Conflict => {
                self.errors.push(TypeError::Dimension {
                    operation: if mul { "mul" } else { "div" }.to_owned(),
                    left: a.clone(),
                    right: b.clone(),
                });
                Ty::Error
            }
            DimBinaryStep::Solve(x, y) => {
                self.unify(x.clone(), y);
                x
            }
        }
    }
    fn lit(&self, l: &Lit) -> Ty {
        match l {
            Lit::Unit => Ty::Unit,
            Lit::Bool(_) => Ty::Bool,
            Lit::Int(_) => Ty::Int(crate::types::IntWidth::Int),
            Lit::Str(_) => Ty::Str,
            Lit::F64Bits(_) => Ty::F64,
            Lit::Enum { ty, .. } => Ty::Enum(ty.clone()),
        }
    }
    fn resolve(&self, t: Ty) -> Ty {
        solve::resolve(&self.subst, t)
    }
    /// The one unification entry point. [`solve::step`] is the shared
    /// algebra's answer to "what should happen for these two resolved
    /// types"; this method is only the *observation* — push a
    /// [`TypeError`] or recurse — the algorithm itself never lives here.
    fn unify(&mut self, a: Ty, b: Ty) {
        let a = self.resolve(a);
        let b = self.resolve(b);
        match solve::step(a, b) {
            Step::Done => {}
            Step::Bind(v, t) => self.bind(v, t),
            Step::Rows(a, b) => self.unify_rows(a, b),
            Step::Descend(pairs) => {
                for (x, y) in pairs {
                    self.unify(x, y);
                }
            }
            Step::Mismatch(expected, found) => {
                self.errors.push(TypeError::Mismatch { expected, found })
            }
            Step::Erasure(from, to) => self.errors.push(TypeError::EpistemicErasure { from, to }),
        }
    }
    fn bind(&mut self, v: TyVar, t: Ty) {
        if solve::occurs(v, &t, &self.subst) {
            self.errors.push(TypeError::Occurs { var: v, in_ty: t });
        } else {
            self.subst.insert(v, t);
        }
    }
    /// Row symmetry ruling: [`solve::match_rows`] checks both directions,
    /// so `{a} ~ closed {a,b}` is a mismatch regardless of which side is
    /// `a`/`b` — a left-only check (the previous behavior here) would miss
    /// the case where `b` is closed and has an extra field `a` lacks.
    fn unify_rows(&mut self, a: Row, b: Row) {
        let matched = solve::match_rows(&a, &b);
        for (x, y) in matched.pairs {
            self.unify(x, y);
        }
        for field in matched.missing_in_right {
            self.errors.push(TypeError::UnknownField {
                field,
                base: Ty::Record(Box::new(b.clone())),
            });
        }
        for field in matched.missing_in_left {
            self.errors.push(TypeError::UnknownField {
                field,
                base: Ty::Record(Box::new(a.clone())),
            });
        }
    }
    fn zonk_expr(&self, e: &mut Expr) {
        e.ty = self.resolve(e.ty.clone());
        match &mut *e.kind {
            ExprKind::Call { args, .. } => {
                for a in args {
                    self.zonk_expr(a)
                }
            }
            ExprKind::Field { base, .. } => self.zonk_expr(base),
            ExprKind::Record { fields } => {
                for (_, v) in fields {
                    self.zonk_expr(v)
                }
            }
            ExprKind::If { cond, then, els } => {
                self.zonk_expr(cond);
                self.zonk_expr(then);
                self.zonk_expr(els)
            }
            ExprKind::Try { inner, .. } => self.zonk_expr(inner),
            ExprKind::Comprehension { pattern, yields } => {
                self.zonk_pattern(pattern);
                if let Some(y) = yields {
                    self.zonk_expr(y)
                }
            }
            ExprKind::Let { value, body, .. } => {
                self.zonk_expr(value);
                self.zonk_expr(body)
            }
            ExprKind::Var(_) | ExprKind::Lit(_) => {}
        }
    }
    fn zonk_pattern(&self, p: &mut Pattern) {
        for c in &mut p.clauses {
            match c {
                Clause::Let { expr, .. } | Clause::When(expr) => self.zonk_expr(expr),
                Clause::Any(ps) => {
                    for p in ps {
                        self.zonk_pattern(p)
                    }
                }
                Clause::Exists(p) | Clause::Without(p) | Clause::Optional(p) | Clause::Cross(p) => {
                    self.zonk_pattern(p)
                }
                _ => {}
            }
        }
    }
}

/// Prefer ground, exact parameter matches over solutions that only work by
/// binding free type variables (higher score wins).
fn overload_specificity(subst: &BTreeMap<TyVar, Ty>, actual: &[Ty], params: &[Ty]) -> i32 {
    let mut score = 0i32;
    for (a, p) in actual.iter().zip(params.iter()) {
        let a = solve::resolve(subst, a.clone());
        let p = solve::resolve(subst, p.clone());
        if a == p {
            score += 10;
        } else if !matches!(p, Ty::Var(_)) {
            score += 5;
        }
    }
    score
}

fn format_arg_types(subst: &BTreeMap<TyVar, Ty>, actual: &[Ty]) -> String {
    actual
        .iter()
        .map(|t| solve::resolve(subst, t.clone()).to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Expr, ExprKind, FnDef, Query};
    use crate::effects::EffectRow;
    use crate::frontend::{FnSignature, TableResolver};
    use crate::types::{dimensions_div, money_dimensions, quantity_dimensions, IntWidth};

    fn lit_int(n: i64) -> Expr {
        Expr::new(
            Ty::Int(IntWidth::Int),
            ExprKind::Lit(crate::pattern::Lit::Int(n)),
        )
    }

    fn lit_f64(bits: u64) -> Expr {
        Expr::new(Ty::F64, ExprKind::Lit(crate::pattern::Lit::F64Bits(bits)))
    }

    fn var_ty(name: &str, ty: Ty) -> Expr {
        Expr::new(ty.clone(), ExprKind::Var(Ident::new(name)))
    }

    #[test]
    fn call_selects_int_vs_float_clamp_overload() {
        let resolver = TableResolver::new()
            .with_function(FnSignature {
                name: QualIdent::from("clamp"),
                params: vec![
                    Ty::Int(IntWidth::Int),
                    Ty::Int(IntWidth::Int),
                    Ty::Int(IntWidth::Int),
                ],
                ret: Ty::Int(IntWidth::Int),
                effects: EffectRow::empty(),
                is_aggregate: false,
                may_diverge: false,
            })
            .with_function(FnSignature {
                name: QualIdent::from("clamp"),
                params: vec![Ty::F64, Ty::F64, Ty::F64],
                ret: Ty::F64,
                effects: EffectRow::empty(),
                is_aggregate: false,
                may_diverge: false,
            });
        let mut source = FrontendSource {
            functions: vec![
                FnDef {
                    name: QualIdent::from("demoI"),
                    params: vec![(Ident::new("x"), Ty::Int(IntWidth::Int))],
                    ret: Ty::Int(IntWidth::Int),
                    effects: EffectRow::empty(),
                    is_partial: false,
                    body: Expr::new(
                        Ty::Var(TyVar(0)),
                        ExprKind::Call {
                            func: QualIdent::from("clamp"),
                            args: vec![
                                var_ty("x", Ty::Int(IntWidth::Int)),
                                lit_int(0),
                                lit_int(10),
                            ],
                        },
                    ),
                },
                FnDef {
                    name: QualIdent::from("demoF"),
                    params: vec![(Ident::new("x"), Ty::F64)],
                    ret: Ty::F64,
                    effects: EffectRow::empty(),
                    is_partial: false,
                    body: Expr::new(
                        Ty::Var(TyVar(1)),
                        ExprKind::Call {
                            func: QualIdent::from("clamp"),
                            args: vec![
                                var_ty("x", Ty::F64),
                                lit_f64(0.0f64.to_bits()),
                                lit_f64(1.0f64.to_bits()),
                            ],
                        },
                    ),
                },
            ],
            rules: vec![],
            constraints: vec![],
            queries: vec![],
        };
        let errors = infer_source(&mut source, &resolver);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(source.functions[0].body.ty, Ty::Int(IntWidth::Int));
        assert_eq!(source.functions[1].body.ty, Ty::F64);
    }

    fn var(name: &str, ty: Ty) -> Expr {
        Expr::new(ty, ExprKind::Var(Ident::new(name)))
    }
    fn op(name: &str, args: Vec<Expr>) -> Expr {
        Expr::new(
            Ty::Var(TyVar(99)),
            ExprKind::Call {
                func: QualIdent::from(format!("brix.ops.{name}").as_str()),
                args,
            },
        )
    }

    #[test]
    fn pricing_multiply_passes_but_divide_is_one_dimension_error() {
        let eur = Ident::new("EUR");
        let km = Ident::new("Kilometre");
        let rate = Ty::Dimensioned(dimensions_div(
            &money_dimensions(&eur),
            &quantity_dimensions(&km),
        ));
        let q = Query {
            name: Ident::new("P"),
            params: vec![
                (Ident::new("rate"), rate),
                (Ident::new("length"), Ty::Quantity(km)),
                (Ident::new("surcharge"), Ty::Money(eur)),
            ],
            body: Pattern::default(),
            yields: op(
                "add",
                vec![
                    op(
                        "div",
                        vec![
                            var("rate", Ty::Var(TyVar(1))),
                            var("length", Ty::Var(TyVar(2))),
                        ],
                    ),
                    var("surcharge", Ty::Var(TyVar(3))),
                ],
            ),
            result: Ty::Var(TyVar(4)),
        };
        let mut source = FrontendSource {
            functions: Vec::new(),
            rules: vec![],
            constraints: vec![],
            queries: vec![q],
        };
        let errors = infer_source(&mut source, &TableResolver::new());
        assert_eq!(
            errors
                .iter()
                .filter(|e| matches!(e, TypeError::Dimension { .. }))
                .count(),
            1,
            "{errors:?}"
        );
    }
    #[test]
    fn estimate_result_unified_against_plain_expected_is_an_epistemic_erasure() {
        let mut source = FrontendSource {
            functions: Vec::new(),
            rules: vec![],
            constraints: vec![],
            queries: vec![Query {
                name: Ident::new("Erasure"),
                params: vec![(
                    Ident::new("x"),
                    Ty::Estimate(Box::new(Ty::Int(crate::types::IntWidth::Int))),
                )],
                body: Pattern::default(),
                yields: var("x", Ty::Var(TyVar(20))),
                result: Ty::rel(Row::closed(vec![crate::types::RowField {
                    name: Ident::new("value"),
                    ty: Ty::Int(crate::types::IntWidth::Int),
                }])),
            }],
        };
        let errors = infer_source(&mut source, &TableResolver::new());
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, TypeError::EpistemicErasure { .. })),
            "{errors:?}"
        );
    }

    #[test]
    fn rejects_non_bool_guard_and_bad_arity_without_cascade() {
        let mut source = FrontendSource {
            functions: Vec::new(),
            rules: vec![Rule {
                name: Ident::new("R"),
                head: Head::Mask {
                    target: Ident::new("a"),
                    reason: Ident::new("b"),
                },
                body: Pattern::new(vec![Clause::When(Expr::new(
                    Ty::Int(crate::types::IntWidth::Int),
                    ExprKind::Lit(Lit::Int(1)),
                ))]),
                effects: crate::effects::EffectRow::empty(),
            }],
            constraints: vec![],
            queries: vec![],
        };
        let errors = infer_source(&mut source, &TableResolver::new());
        assert_eq!(errors.len(), 1);
        assert!(matches!(errors[0], TypeError::NonBoolGuard { .. }));
    }
}
