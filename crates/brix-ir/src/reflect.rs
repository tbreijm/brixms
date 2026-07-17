//! A bootstrap-safe, fact-oriented prototype of the future BrixMS-native
//! type-analysis package.
//!
//! The compiler continues to use [`crate::infer`]. This module deliberately
//! has a narrower job: mirror a useful Core-IR subset as deterministic facts,
//! saturate local typing constraints, and retain each conflict's derivation.
//! It is the executable reference shape for a later `brix.type` rule package;
//! it does not pretend that the package can self-host before `brix build`
//! executes function/rule bodies.
//!
//! The pure unification/dimension algebra lives in [`crate::solve`] and is
//! shared with [`crate::infer`] (#15 PR2: "one algorithm, two observers") —
//! this module supplies only the *observation*: it records [`Fact`]s with
//! [`Derivation`] provenance and [`TypeConflict`]s instead of mutating
//! `Expr.ty` and accumulating a flat error list.

use std::collections::BTreeMap;

use crate::core::{Constraint, Expr, ExprKind, ExprOrigin, Head, Query, Rule};
use crate::frontend::{FrontendSource, SchemaResolver};
use crate::ident::{Ident, QualIdent};
use crate::pattern::{Arg, Clause, Lit, Pattern, RoleArg};
use crate::solve::{self, DimBinaryStep, DimStep, Step};
use crate::types::{IntWidth, Row, Ty, TyVar};

/// A stable, declaration-local subject in the reflective fact graph.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Subject {
    Binding { declaration: Ident, name: Ident },
    Expr { origin: ExprOrigin },
    Head { declaration: Ident, role: Ident },
}

/// One derivable relation in the future `brix.type` package.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Fact {
    HasType { subject: Subject, ty: Ty },
    RequiresBool { subject: Subject },
    Applies { subject: Subject, operator: String },
}

/// A fact plus the earlier facts from which it follows. IDs are append-only,
/// deterministic within a single `analyze` run, and therefore work as
/// compact provenance handles. (Content-addressed fact IDs are PR 3's job —
/// this positional scheme is an explicitly accepted stopgap for this PR.)
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Derivation {
    pub id: usize,
    pub fact: Fact,
    pub because: Vec<usize>,
}

/// A derived incompatibility. It is intentionally distinct from a kernel key
/// conflict: competing provisional facts can be legitimate while solving.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TypeConflict {
    pub operation: String,
    pub left: Ty,
    pub right: Ty,
    pub because: Vec<usize>,
}

/// Saturated facts and explainable conflicts for a Core source.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ReflectiveReport {
    pub facts: Vec<Derivation>,
    pub conflicts: Vec<TypeConflict>,
}

impl ReflectiveReport {
    pub fn is_consistent(&self) -> bool {
        self.conflicts.is_empty()
    }
}

type Env = BTreeMap<Ident, (Ty, usize)>;
const NO_FACT: usize = usize::MAX;

/// Run the fact-oriented checker. It covers the v1 expression subset that is
/// currently lowered: bindings from relation schemas, lets, guards, heads,
/// literals, records, fields, calls, and the ground-dimensional operators.
pub fn analyze(source: &FrontendSource, resolver: &impl SchemaResolver) -> ReflectiveReport {
    let mut cx = Reflect::default();
    for rule in &source.rules {
        cx.rule(rule, resolver);
    }
    for constraint in &source.constraints {
        cx.constraint(constraint, resolver);
    }
    for query in &source.queries {
        cx.query(query, resolver);
    }
    cx.normalize_facts();
    cx.report
}

#[derive(Default)]
struct Reflect {
    report: ReflectiveReport,
    subst: BTreeMap<TyVar, Ty>,
}

impl Reflect {
    fn normalize_facts(&mut self) {
        let subst = self.subst.clone();
        for derivation in &mut self.report.facts {
            if let Fact::HasType { ty, .. } = &mut derivation.fact {
                *ty = solve::resolve(&subst, ty.clone());
            }
        }
    }

    fn fact(&mut self, fact: Fact, because: Vec<usize>) -> usize {
        let id = self.report.facts.len();
        let because: Vec<usize> = because
            .into_iter()
            .filter(|dependency| *dependency != NO_FACT)
            .collect();
        self.report.facts.push(Derivation { id, fact, because });
        id
    }

    fn conflict(&mut self, operation: &str, left: Ty, right: Ty, because: Vec<usize>) {
        self.report.conflicts.push(TypeConflict {
            operation: operation.to_owned(),
            left,
            right,
            because: because
                .into_iter()
                .filter(|dependency| *dependency != NO_FACT)
                .collect(),
        });
    }

    fn resolve(&self, ty: Ty) -> Ty {
        solve::resolve(&self.subst, ty)
    }

    /// The one unification entry point. [`solve::step`] is the shared
    /// algebra's answer to "what should happen for these two resolved
    /// types"; this method is only the *observation* — record a `Fact`
    /// binding or a [`TypeConflict`] — the algorithm itself never lives
    /// here (see [`crate::infer::Infer::unify`] for the other observer).
    fn unify(&mut self, expected: Ty, found: Ty, operation: &str, because: Vec<usize>) {
        let expected = self.resolve(expected);
        let found = self.resolve(found);
        match solve::step(expected, found) {
            Step::Done => {}
            Step::Bind(variable, ty) => self.bind_ty(variable, ty, because),
            Step::Rows(left, right) => self.unify_rows(left, right, operation, because),
            Step::Descend(pairs) => {
                for (left, right) in pairs {
                    self.unify(left, right, operation, because.clone());
                }
            }
            Step::Mismatch(expected, found) => self.conflict(operation, expected, found, because),
        }
    }

    fn bind_ty(&mut self, variable: TyVar, ty: Ty, because: Vec<usize>) {
        let ty = self.resolve(ty);
        if ty == Ty::Var(variable) {
            return;
        }
        if solve::occurs(variable, &ty, &self.subst) {
            self.conflict("occurs", Ty::Var(variable), ty, because);
        } else {
            self.subst.insert(variable, ty);
        }
    }

    /// Row symmetry ruling: [`solve::match_rows`] checks both directions,
    /// so `{a} ~ closed {a,b}` is a mismatch regardless of which side is
    /// `left`/`right`.
    fn unify_rows(&mut self, left: Row, right: Row, operation: &str, because: Vec<usize>) {
        let matched = solve::match_rows(&left, &right);
        for (a, b) in matched.pairs {
            self.unify(a, b, operation, because.clone());
        }
        if !matched.missing_in_right.is_empty() {
            self.conflict(
                "row",
                Ty::record(left.clone()),
                Ty::record(right.clone()),
                because.clone(),
            );
        }
        if !matched.missing_in_left.is_empty() {
            self.conflict("row", Ty::record(left), Ty::record(right), because);
        }
    }

    fn rule(&mut self, rule: &Rule, resolver: &impl SchemaResolver) {
        let mut env = Env::new();
        self.pattern(&rule.name, &rule.body, &mut env, resolver, &mut vec![]);
        self.head(&rule.name, &rule.head, &env, resolver);
    }

    fn query(&mut self, query: &Query, resolver: &impl SchemaResolver) {
        let mut env = Env::new();
        for (name, ty) in &query.params {
            let subject = Subject::Binding {
                declaration: query.name.clone(),
                name: name.clone(),
            };
            let id = self.fact(
                Fact::HasType {
                    subject,
                    ty: ty.clone(),
                },
                vec![],
            );
            env.insert(name.clone(), (ty.clone(), id));
        }
        self.pattern(&query.name, &query.body, &mut env, resolver, &mut vec![]);
        let (yielded, evidence) =
            self.expr(&query.name, &query.yields, &env, resolver, &mut vec![]);
        let expected = Ty::rel(match yielded {
            Ty::Record(row) => *row,
            ty => Row::closed(vec![crate::types::RowField {
                name: Ident::new("value"),
                ty,
            }]),
        });
        self.unify(
            query.result.clone(),
            expected,
            "query-result",
            vec![evidence],
        );
    }

    fn constraint(&mut self, constraint: &Constraint, resolver: &impl SchemaResolver) {
        let mut env = Env::new();
        self.pattern(
            &constraint.name,
            &constraint.body,
            &mut env,
            resolver,
            &mut vec![],
        );
    }

    fn pattern(
        &mut self,
        declaration: &Ident,
        pattern: &Pattern,
        env: &mut Env,
        resolver: &impl SchemaResolver,
        path: &mut Vec<u32>,
    ) {
        for (ordinal, clause) in pattern.clauses.iter().enumerate() {
            path.push(ordinal as u32);
            match clause {
                Clause::Edge { relation, args, .. } | Clause::History { relation, args, .. } => {
                    if let Some(schema) = resolver.relation(relation) {
                        for arg in args {
                            if let Some((_, ty)) =
                                schema.roles.iter().find(|(name, _)| name == &arg.role)
                            {
                                self.role_arg(declaration, arg, ty.clone(), env);
                            }
                        }
                    }
                }
                Clause::Entity {
                    var,
                    entity,
                    fields,
                } => {
                    self.bind(declaration, var, Ty::NodeRef(entity.clone()), env, vec![]);
                    if let Some(schema) = resolver.relation(&QualIdent::simple(entity.as_str())) {
                        for arg in fields {
                            if let Some((_, ty)) =
                                schema.roles.iter().find(|(name, _)| name == &arg.role)
                            {
                                self.role_arg(declaration, arg, ty.clone(), env);
                            }
                        }
                    }
                }
                Clause::Let { binds, expr } => {
                    let (ty, evidence) = self.expr(declaration, expr, env, resolver, path);
                    self.bind(declaration, binds, ty, env, vec![evidence]);
                }
                Clause::When(expr) => {
                    let (ty, evidence) = self.expr(declaration, expr, env, resolver, path);
                    let subject = Subject::Expr {
                        origin: expr.origin,
                    };
                    self.fact(Fact::RequiresBool { subject }, vec![evidence]);
                    if ty != Ty::Bool && !matches!(ty, Ty::Var(_)) {
                        self.conflict("when", Ty::Bool, ty, vec![evidence]);
                    }
                }
                Clause::Any(cases) => {
                    for case in cases {
                        self.pattern(declaration, case, env, resolver, path);
                    }
                }
                Clause::Exists(p) | Clause::Without(p) | Clause::Optional(p) | Clause::Cross(p) => {
                    self.pattern(declaration, p, env, resolver, path)
                }
            }
            path.pop();
        }
    }

    fn role_arg(&mut self, declaration: &Ident, arg: &RoleArg, expected: Ty, env: &mut Env) {
        match &arg.arg {
            Arg::Var(name) => self.bind(declaration, name, expected, env, vec![]),
            Arg::Lit(lit) => {
                let found = lit_ty(lit);
                if found != expected {
                    self.conflict("role", expected, found, vec![]);
                }
            }
        }
    }

    fn bind(
        &mut self,
        declaration: &Ident,
        name: &Ident,
        ty: Ty,
        env: &mut Env,
        because: Vec<usize>,
    ) {
        if let Some((old, old_fact)) = env.get(name).cloned() {
            self.unify(old, ty, "unify", vec![old_fact]);
            return;
        }
        let subject = Subject::Binding {
            declaration: declaration.clone(),
            name: name.clone(),
        };
        let id = self.fact(
            Fact::HasType {
                subject,
                ty: ty.clone(),
            },
            because,
        );
        env.insert(name.clone(), (ty, id));
    }

    fn head(
        &mut self,
        declaration: &Ident,
        head: &Head,
        env: &Env,
        resolver: &impl SchemaResolver,
    ) {
        let Head::Tuple { relation, args } = head else {
            return;
        };
        let Some(schema) = resolver.relation(relation) else {
            return;
        };
        for arg in args {
            let Some((_, expected)) = schema.roles.iter().find(|(name, _)| name == &arg.role)
            else {
                continue;
            };
            let (found, because) = match &arg.arg {
                Arg::Var(name) => env.get(name).cloned().unwrap_or((Ty::Error, NO_FACT)),
                Arg::Lit(lit) => (lit_ty(lit), NO_FACT),
            };
            let head_fact = self.fact(
                Fact::HasType {
                    subject: Subject::Head {
                        declaration: declaration.clone(),
                        role: arg.role.clone(),
                    },
                    ty: expected.clone(),
                },
                vec![because],
            );
            self.unify(expected.clone(), found, "head", vec![head_fact, because]);
        }
    }

    fn expr(
        &mut self,
        declaration: &Ident,
        expr: &Expr,
        env: &Env,
        resolver: &impl SchemaResolver,
        path: &mut Vec<u32>,
    ) -> (Ty, usize) {
        let subject = Subject::Expr {
            origin: expr.origin,
        };
        let (ty, because) = match &*expr.kind {
            ExprKind::Var(name) => env.get(name).cloned().unwrap_or((expr.ty.clone(), NO_FACT)),
            ExprKind::Lit(lit) => (lit_ty(lit), NO_FACT),
            ExprKind::Record { fields } => {
                let mut row = Vec::new();
                let mut deps = Vec::new();
                for (ordinal, (name, value)) in fields.iter().enumerate() {
                    path.push(ordinal as u32);
                    let (ty, id) = self.expr(declaration, value, env, resolver, path);
                    path.pop();
                    row.push(crate::types::RowField {
                        name: name.clone(),
                        ty,
                    });
                    deps.push(id);
                }
                (
                    Ty::record(Row::closed(row)),
                    deps.first().copied().unwrap_or(NO_FACT),
                )
            }
            ExprKind::Field { base, field } => {
                path.push(0);
                let (base_ty, id) = self.expr(declaration, base, env, resolver, path);
                path.pop();
                match self.resolve(base_ty) {
                    Ty::Record(row) | Ty::Rel(row) => {
                        if let Some(found) = row.fields.iter().find(|x| &x.name == field) {
                            (found.ty.clone(), id)
                        } else {
                            self.conflict(
                                "field",
                                Ty::Record(Box::new(Row::closed(vec![crate::types::RowField {
                                    name: field.clone(),
                                    ty: Ty::Var(TyVar(0)),
                                }]))),
                                Ty::Record(row),
                                vec![id],
                            );
                            (Ty::Error, id)
                        }
                    }
                    found => {
                        if !matches!(found, Ty::Var(_)) {
                            self.conflict(
                                "field",
                                Ty::Record(Box::new(Row::closed(vec![]))),
                                found,
                                vec![id],
                            );
                        }
                        (Ty::Error, id)
                    }
                }
            }
            ExprKind::If { cond, then, els } => {
                path.push(0);
                let (cond_ty, cond_id) = self.expr(declaration, cond, env, resolver, path);
                path.pop();
                self.unify(Ty::Bool, cond_ty, "if", vec![cond_id]);
                path.push(1);
                let (then_ty, then_id) = self.expr(declaration, then, env, resolver, path);
                path.pop();
                path.push(2);
                let (else_ty, else_id) = self.expr(declaration, els, env, resolver, path);
                path.pop();
                self.unify(then_ty.clone(), else_ty, "if", vec![then_id, else_id]);
                (self.resolve(then_ty), then_id)
            }
            ExprKind::Try { inner, .. } => {
                path.push(0);
                let (inner_ty, id) = self.expr(declaration, inner, env, resolver, path);
                path.pop();
                match self.resolve(inner_ty) {
                    Ty::Result(ok, _) => (*ok, id),
                    found => {
                        if !matches!(found, Ty::Var(_)) {
                            self.conflict(
                                "try",
                                Ty::Result(
                                    Box::new(Ty::Var(TyVar(0))),
                                    Box::new(Ty::Var(TyVar(1))),
                                ),
                                found,
                                vec![id],
                            );
                        }
                        (Ty::Error, id)
                    }
                }
            }
            ExprKind::Comprehension { pattern, yields } => {
                let mut nested = env.clone();
                self.pattern(declaration, pattern, &mut nested, resolver, path);
                let (row, evidence) = match yields {
                    Some(yielded) => {
                        path.push(0);
                        let (ty, evidence) =
                            self.expr(declaration, yielded, &nested, resolver, path);
                        path.pop();
                        match ty {
                            Ty::Record(row) => (*row, evidence),
                            ty => (
                                Row::closed(vec![crate::types::RowField {
                                    name: Ident::new("value"),
                                    ty,
                                }]),
                                evidence,
                            ),
                        }
                    }
                    None => (Row::closed(vec![]), NO_FACT),
                };
                (Ty::rel(row), evidence)
            }
            ExprKind::Call { func, args } => {
                self.call(expr.origin, declaration, func, args, env, resolver, path)
            }
        };
        let ty = self.resolve(ty);
        let id = self.fact(
            Fact::HasType {
                subject,
                ty: ty.clone(),
            },
            vec![because],
        );
        (ty, id)
    }

    #[allow(clippy::too_many_arguments)] // Traversal context is explicit while this prototype stays standalone.
    fn call(
        &mut self,
        origin: ExprOrigin,
        declaration: &Ident,
        func: &QualIdent,
        args: &[Expr],
        env: &Env,
        resolver: &impl SchemaResolver,
        path: &mut Vec<u32>,
    ) -> (Ty, usize) {
        let mut arg_types = Vec::new();
        let mut deps = Vec::new();
        for (ordinal, arg) in args.iter().enumerate() {
            path.push(ordinal as u32);
            let (ty, id) = self.expr(declaration, arg, env, resolver, path);
            path.pop();
            arg_types.push(ty);
            deps.push(id);
        }
        let subject = Subject::Expr { origin };
        let op_fact = self.fact(
            Fact::Applies {
                subject,
                operator: func.to_string(),
            },
            deps.clone(),
        );
        if let Some(op) = func.to_string().strip_prefix("brix.ops.") {
            return (self.operator(op, &arg_types, &deps), op_fact);
        }
        if let Some(sig) = resolver.function(func) {
            if sig.params.len() != arg_types.len() {
                self.conflict(
                    "arity",
                    Ty::Int(IntWidth::Nat),
                    Ty::Int(IntWidth::Nat),
                    deps,
                );
            } else {
                for ((expected, found), dep) in sig.params.iter().zip(arg_types).zip(deps) {
                    self.unify(expected.clone(), found, "call", vec![dep]);
                }
            }
            return (sig.ret.clone(), op_fact);
        }
        (Ty::Error, op_fact)
    }

    /// Dimension-vs-variable ruling: when one side of a same-dimension
    /// operator lacks ground dimensions, [`solve::same_dimension_step`]
    /// solves/unifies it rather than reporting a conflict. Mirrors
    /// `Infer::same_dimension` exactly (down to returning [`Ty::Error`] on
    /// a real conflict, not the stale left-hand operand) — the two must
    /// stay in lockstep since both are "observers" turning the same
    /// [`solve::DimStep`] into their own record of what happened, and this
    /// PR's parity harness (`crates/brix-ir/tests/parity.rs`) asserts
    /// their zonked/mirrored types agree even on a conflicting expression.
    fn same_dimension(&mut self, operation: &str, a: &Ty, b: &Ty, deps: &[usize]) -> Ty {
        match solve::same_dimension_step(a, b) {
            DimStep::Ok(t) => t,
            DimStep::Conflict => {
                self.conflict(operation, a.clone(), b.clone(), deps.to_vec());
                Ty::Error
            }
            DimStep::Solve(x, y) => {
                self.unify(x.clone(), y, operation, deps.to_vec());
                x
            }
        }
    }

    fn operator(&mut self, op: &str, args: &[Ty], deps: &[usize]) -> Ty {
        if args.len() != if matches!(op, "not" | "neg") { 1 } else { 2 } {
            self.conflict(
                "arity",
                Ty::Int(IntWidth::Nat),
                Ty::Int(IntWidth::Nat),
                deps.to_vec(),
            );
            return Ty::Error;
        }
        match op {
            "add" | "sub" | "eq" | "ne" | "lt" | "le" | "gt" | "ge" => {
                let result = self.same_dimension(op, &args[0], &args[1], deps);
                if matches!(op, "eq" | "ne" | "lt" | "le" | "gt" | "ge") {
                    Ty::Bool
                } else {
                    result
                }
            }
            "mul" | "div" => match solve::dimension_binary_step(&args[0], &args[1], op == "mul") {
                DimBinaryStep::Ok(t) => t,
                DimBinaryStep::Conflict => {
                    self.conflict(op, args[0].clone(), args[1].clone(), deps.to_vec());
                    Ty::Error
                }
                DimBinaryStep::Solve(x, y) => {
                    self.unify(x.clone(), y, op, deps.to_vec());
                    x
                }
            },
            "and" | "or" => {
                for (ty, dep) in args.iter().zip(deps) {
                    self.unify(Ty::Bool, ty.clone(), op, vec![*dep]);
                }
                Ty::Bool
            }
            "not" => {
                self.unify(Ty::Bool, args[0].clone(), op, deps.to_vec());
                Ty::Bool
            }
            "neg" => args[0].clone(),
            _ => Ty::Error,
        }
    }
}

fn lit_ty(lit: &Lit) -> Ty {
    match lit {
        Lit::Unit => Ty::Unit,
        Lit::Bool(_) => Ty::Bool,
        Lit::Int(_) => Ty::Int(IntWidth::Int),
        Lit::Str(_) => Ty::Str,
        Lit::F64Bits(_) => Ty::F64,
        Lit::Enum { ty, .. } => Ty::Enum(ty.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Expr, ExprKind, Severity};
    use crate::frontend::FnSignature;
    use crate::infer::{infer_source, TypeError};
    use crate::types::{dimensions_div, money_dimensions, quantity_dimensions};

    fn var(name: &str) -> Expr {
        Expr::new(Ty::Var(TyVar(9)), ExprKind::Var(Ident::new(name)))
    }
    fn op(name: &str, args: Vec<Expr>) -> Expr {
        Expr::new(
            Ty::Var(TyVar(10)),
            ExprKind::Call {
                func: QualIdent::from(format!("brix.ops.{name}").as_str()),
                args,
            },
        )
    }

    #[test]
    fn reflective_pricing_conflict_agrees_with_bootstrap_checker_and_has_provenance() {
        let eur = Ident::new("EUR");
        let km = Ident::new("Kilometre");
        let rate = Ty::Dimensioned(dimensions_div(
            &money_dimensions(&eur),
            &quantity_dimensions(&km),
        ));
        let query = Query {
            name: Ident::new("Price"),
            params: vec![
                (Ident::new("rate"), rate),
                (Ident::new("length"), Ty::Quantity(km)),
                (Ident::new("surcharge"), Ty::Money(eur)),
            ],
            body: Pattern::default(),
            yields: op(
                "add",
                vec![
                    op("div", vec![var("rate"), var("length")]),
                    var("surcharge"),
                ],
            ),
            result: Ty::Var(TyVar(11)),
        };
        let source = FrontendSource {
            rules: vec![],
            constraints: vec![],
            queries: vec![query],
        };
        let report = analyze(&source, &crate::frontend::TableResolver::new());
        let mut bootstrap = source.clone();
        let errors = infer_source(&mut bootstrap, &crate::frontend::TableResolver::new());
        assert_eq!(report.conflicts.len(), 1, "{report:#?}");
        assert_eq!(
            errors
                .iter()
                .filter(|error| matches!(error, TypeError::Dimension { .. }))
                .count(),
            1,
            "{errors:?}"
        );
        assert_eq!(report.conflicts[0].operation, "add");
        assert!(!report.conflicts[0].because.is_empty());
    }

    #[test]
    fn reflective_query_result_and_unknown_field_match_bootstrap_rejections() {
        let record = Ty::record(Row::closed(vec![crate::types::RowField {
            name: Ident::new("present"),
            ty: Ty::Int(IntWidth::Int),
        }]));
        let source = FrontendSource {
            rules: vec![],
            constraints: vec![],
            queries: vec![
                Query {
                    name: Ident::new("BadResult"),
                    params: vec![],
                    body: Pattern::default(),
                    yields: Expr::new(Ty::Int(IntWidth::Int), ExprKind::Lit(Lit::Int(1))),
                    result: Ty::rel(Row::closed(vec![crate::types::RowField {
                        name: Ident::new("value"),
                        ty: Ty::Bool,
                    }])),
                },
                Query {
                    name: Ident::new("MissingField"),
                    params: vec![(Ident::new("record"), record)],
                    body: Pattern::default(),
                    yields: Expr::new(
                        Ty::Var(TyVar(12)),
                        ExprKind::Field {
                            base: var("record"),
                            field: Ident::new("absent"),
                        },
                    ),
                    result: Ty::Var(TyVar(13)),
                },
            ],
        };
        let report = analyze(&source, &crate::frontend::TableResolver::new());
        let mut bootstrap = source.clone();
        let errors = infer_source(&mut bootstrap, &crate::frontend::TableResolver::new());
        assert!(report
            .conflicts
            .iter()
            .any(|conflict| conflict.operation == "query-result"));
        assert!(report
            .conflicts
            .iter()
            .any(|conflict| conflict.operation == "field"));
        assert!(errors
            .iter()
            .any(|error| matches!(error, TypeError::Mismatch { .. })));
        assert!(errors
            .iter()
            .any(|error| matches!(error, TypeError::UnknownField { .. })));
    }

    #[test]
    fn reflective_constraints_comprehensions_and_call_arity_are_checked() {
        let source = FrontendSource {
            rules: vec![],
            constraints: vec![Constraint {
                name: Ident::new("Guard"),
                severity: Severity::Strict,
                body: Pattern::new(vec![Clause::When(Expr::new(
                    Ty::Int(IntWidth::Int),
                    ExprKind::Lit(Lit::Int(1)),
                ))]),
            }],
            queries: vec![
                Query {
                    name: Ident::new("Comp"),
                    params: vec![],
                    body: Pattern::default(),
                    yields: Expr::new(
                        Ty::Var(TyVar(14)),
                        ExprKind::Comprehension {
                            pattern: Pattern::default(),
                            yields: Some(Expr::new(
                                Ty::Int(IntWidth::Int),
                                ExprKind::Lit(Lit::Int(1)),
                            )),
                        },
                    ),
                    result: Ty::Var(TyVar(15)),
                },
                Query {
                    name: Ident::new("Arity"),
                    params: vec![],
                    body: Pattern::default(),
                    yields: Expr::new(
                        Ty::Var(TyVar(16)),
                        ExprKind::Call {
                            func: QualIdent::from("f"),
                            args: vec![],
                        },
                    ),
                    result: Ty::Var(TyVar(17)),
                },
            ],
        };
        let resolver = crate::frontend::TableResolver::new().with_function(FnSignature {
            name: QualIdent::from("f"),
            params: vec![Ty::Int(IntWidth::Int)],
            ret: Ty::Int(IntWidth::Int),
            effects: crate::effects::EffectRow::empty(),
            is_aggregate: false,
            may_diverge: false,
        });
        let report = analyze(&source, &resolver);
        assert!(report
            .conflicts
            .iter()
            .any(|conflict| conflict.operation == "when"));
        assert!(report
            .conflicts
            .iter()
            .any(|conflict| conflict.operation == "arity"));
        assert!(report
            .facts
            .iter()
            .any(|fact| matches!(fact.fact, Fact::HasType { ty: Ty::Rel(_), .. })));
    }

    #[test]
    fn reflective_unifier_solves_variables_detects_cycles_and_admits_open_rows() {
        let variable = TyVar(40);
        let source = FrontendSource {
            rules: vec![],
            constraints: vec![],
            queries: vec![Query {
                name: Ident::new("Solve"),
                params: vec![(Ident::new("x"), Ty::Var(variable))],
                body: Pattern::default(),
                yields: var("x"),
                result: Ty::rel(Row::closed(vec![crate::types::RowField {
                    name: Ident::new("value"),
                    ty: Ty::Int(IntWidth::Int),
                }])),
            }],
        };
        let report = analyze(&source, &crate::frontend::TableResolver::new());
        assert!(report.is_consistent(), "{report:#?}");
        assert!(report.facts.iter().any(|fact| matches!(
            fact.fact,
            Fact::HasType {
                ty: Ty::Int(IntWidth::Int),
                ..
            }
        )));

        let mut cycle = Reflect::default();
        cycle.unify(
            Ty::Var(variable),
            Ty::option(Ty::Var(variable)),
            "test",
            vec![],
        );
        assert!(cycle
            .report
            .conflicts
            .iter()
            .any(|conflict| conflict.operation == "occurs"));

        let mut rows = Reflect::default();
        rows.unify(
            Ty::record(Row::open(
                vec![crate::types::RowField {
                    name: Ident::new("x"),
                    ty: Ty::Int(IntWidth::Int),
                }],
                TyVar(41),
            )),
            Ty::record(Row::closed(vec![
                crate::types::RowField {
                    name: Ident::new("x"),
                    ty: Ty::Int(IntWidth::Int),
                },
                crate::types::RowField {
                    name: Ident::new("y"),
                    ty: Ty::Bool,
                },
            ])),
            "test",
            vec![],
        );
        assert!(rows.report.conflicts.is_empty(), "{:#?}", rows.report);
    }

    /// The Probability↔F64 bridge (ruling: kept in both checkers) used to
    /// be entirely absent from `reflect.rs` — this exercises it directly so
    /// the two checkers cannot silently re-diverge on it.
    #[test]
    fn reflective_probability_f64_bridge_matches_bootstrap_checker() {
        let mut cx = Reflect::default();
        cx.unify(Ty::Probability, Ty::F64, "test", vec![]);
        assert!(cx.report.conflicts.is_empty(), "{:#?}", cx.report);

        let mut cx = Reflect::default();
        cx.unify(Ty::F64, Ty::Probability, "test", vec![]);
        assert!(cx.report.conflicts.is_empty(), "{:#?}", cx.report);
    }

    /// Dimension-vs-variable ruling: a variable side must solve, not
    /// conflict, against a ground-dimensioned side.
    #[test]
    fn reflective_dimension_vs_variable_solves() {
        let km = Ty::Quantity(Ident::new("Kilometre"));
        let source = FrontendSource {
            rules: vec![],
            constraints: vec![],
            queries: vec![Query {
                name: Ident::new("Solve"),
                params: vec![
                    (Ident::new("a"), km.clone()),
                    (Ident::new("b"), Ty::Var(TyVar(50))),
                ],
                body: Pattern::default(),
                yields: op("add", vec![var("a"), var("b")]),
                result: Ty::rel(Row::closed(vec![crate::types::RowField {
                    name: Ident::new("value"),
                    ty: km,
                }])),
            }],
        };
        let report = analyze(&source, &crate::frontend::TableResolver::new());
        assert!(report.is_consistent(), "{report:#?}");
    }

    /// Option/Result descent ruling: `Option<?t> ~ Option<Int>` must solve
    /// `?t := Int`, not report a top-level mismatch.
    #[test]
    fn reflective_option_descent_solves_the_inner_variable() {
        let mut cx = Reflect::default();
        cx.unify(
            Ty::option(Ty::Int(IntWidth::Int)),
            Ty::option(Ty::Var(TyVar(60))),
            "test",
            vec![],
        );
        assert!(cx.report.conflicts.is_empty(), "{:#?}", cx.report);
        assert_eq!(cx.resolve(Ty::Var(TyVar(60))), Ty::Int(IntWidth::Int));
    }
}
