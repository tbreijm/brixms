//! Static Boolean operand checks, independent of lazy value evaluation.
//!
//! This is shape analysis, not execution: arithmetic faults and branch choices
//! are never computed. Unannotated helpers are checked with abstract parameters
//! and again with their caller's argument shapes. Shared shapes avoid expanding
//! records when a helper duplicates an argument. Work and recursion are bounded
//! separately from the evaluator, including visits through shared shapes.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use super::plan::{FiniteDecisionLowerError as Error, FiniteDecisionPlan, MAX_EXPR_DEPTH};
use crate::l3_v2::{
    ArithOpV2, FoldOpV2, L3ExprV2 as Expr, L3PatternV2, L3ValueType, NumericBuiltinV2,
};
use brix_syntax::ast;

const MAX_ANALYSIS_WORK: usize = 100_000;

type Shape = Arc<Type>;
type Env = BTreeMap<String, Shape>;

enum Type {
    // An unannotated parameter in a declaration, or an expression that cannot
    // produce a value (for example, a missing field). Runtime faults still apply.
    Unknown,
    Known(L3ValueType),
    Record(BTreeMap<String, Shape>),
    Sum {
        nominal: String,
        variant: String,
        args: Vec<Shape>,
    },
    Either(Vec<Shape>),
    /// A list's element shape (ADR-0037, ADR-0040). Kept distinct from
    /// `Known(L3ValueType::List)` — which carries no element shape at all —
    /// so a fold/filter/map/comprehension binder is checked against the
    /// shape its own source expression actually produced wherever that is
    /// known, and only falls back to `Unknown` when it is not (for example,
    /// a bare reference to a declared `List<T>` input, whose runtime type
    /// category is the same flat marker regardless of `T`).
    List(Shape),
}

impl Type {
    fn take_children(&mut self, out: &mut Vec<Shape>) {
        match self {
            Self::Record(fields) => out.extend(std::mem::take(fields).into_values()),
            Self::Sum { args, .. } | Self::Either(args) => out.append(args),
            Self::List(elem) => out.push(std::mem::replace(elem, unknown())),
            Self::Unknown | Self::Known(_) => {}
        }
    }
}

impl Drop for Type {
    fn drop(&mut self) {
        // A sequence of shallow lets can build deeply linked abstract records
        // without deep expression recursion. Releasing those shapes must not
        // turn either success or a resource refusal into a stack overflow.
        let mut pending = Vec::new();
        self.take_children(&mut pending);
        while let Some(shape) = pending.pop() {
            if let Ok(mut ty) = Arc::try_unwrap(shape) {
                ty.take_children(&mut pending);
            }
        }
    }
}

fn known(ty: L3ValueType) -> Shape {
    Arc::new(Type::Known(ty))
}

fn unknown() -> Shape {
    Arc::new(Type::Unknown)
}

struct Checker<'a> {
    plan: &'a FiniteDecisionPlan,
    configs: BTreeMap<&'a str, &'a ast::ConfigBody>,
    remaining: usize,
    /// Helper names whose body is currently being expanded on this path
    /// (ADR-0042). A helper may now recurse, directly or mutually, so
    /// expanding `Expr::Call` by re-walking the callee's body — as this
    /// static pass does — would otherwise unfold a genuinely recursive
    /// helper forever (bounded only by the depth/work charge, and only after
    /// wastefully re-deriving the same shape at every level). A call back
    /// into a helper already on this stack is resolved from its declared
    /// return contract (or `Unknown`) instead of re-entered: this pass is a
    /// static shape approximation, not the evaluator, and termination of the
    /// *program* is the evaluator's call-depth/work budget, not this one.
    active: BTreeSet<String>,
}

impl Checker<'_> {
    fn charge(&mut self, depth: usize) -> Result<(), Error> {
        if depth > MAX_EXPR_DEPTH || self.remaining == 0 {
            return Err(Error::BooleanTypeAnalysisLimit);
        }
        self.remaining -= 1;
        Ok(())
    }

    fn declared_shape(&self, ty: &ast::Ty) -> Shape {
        let ast::Ty::Named(name) = ty else {
            return unknown();
        };
        match name.as_str() {
            "Int" => known(L3ValueType::Int),
            "Bool" => known(L3ValueType::Bool),
            "Str" => known(L3ValueType::Str),
            "F64" => known(L3ValueType::F64),
            "Decimal" => known(L3ValueType::Decimal),
            _ => match self.configs.get(name.as_str()) {
                Some(ast::ConfigBody::Record(_)) => known(L3ValueType::Record(name.clone())),
                Some(ast::ConfigBody::Sum(_)) => known(L3ValueType::Sum(name.clone())),
                None => unknown(),
            },
        }
    }

    fn declared_payload(&self, nominal: Option<&str>, variant: &str, index: usize) -> Shape {
        self.configs
            .iter()
            .filter(|(name, _)| nominal.is_none_or(|nominal| nominal == **name))
            .find_map(|(_, body)| match body {
                ast::ConfigBody::Sum(variants) => variants
                    .iter()
                    .find(|v| v.name == variant)
                    .and_then(|v| v.params.get(index))
                    .map(|ty| self.declared_shape(ty)),
                _ => None,
            })
            .unwrap_or_else(unknown)
    }

    fn require_bool(
        &mut self,
        shape: &Shape,
        operator: &'static str,
        depth: usize,
    ) -> Result<(), Error> {
        self.charge(depth)?;
        let found = match shape.as_ref() {
            Type::Unknown | Type::Known(L3ValueType::Bool) => return Ok(()),
            Type::Either(options) => {
                for option in options {
                    self.require_bool(option, operator, depth + 1)?;
                }
                return Ok(());
            }
            Type::Known(ty) => ty.to_string(),
            Type::Record(_) => "record".to_string(),
            Type::Sum { .. } => "sum".to_string(),
            Type::List(_) => "list".to_string(),
        };
        Err(Error::BooleanOperandType { operator, found })
    }

    fn require_scalar(
        &mut self,
        shape: &Shape,
        operation: &'static str,
        expected: &L3ValueType,
        depth: usize,
    ) -> Result<(), Error> {
        self.charge(depth)?;
        let found = match shape.as_ref() {
            Type::Unknown => return Ok(()),
            Type::Known(ty) if ty == expected => return Ok(()),
            Type::Either(options) => {
                for option in options {
                    self.require_scalar(option, operation, expected, depth + 1)?;
                }
                return Ok(());
            }
            Type::Known(ty) => ty.to_string(),
            Type::Record(_) => "record".into(),
            Type::Sum { .. } => "sum".into(),
            Type::List(_) => "list".into(),
        };
        Err(Error::NumericOperandType {
            operation,
            expected: expected.to_string(),
            found,
        })
    }

    /// The list-form counterpart of [`Self::require_bool`] (ADR-0040,
    /// following the precedent ADR-0034 set for `&&`/`||`/`!`): a `filter`/
    /// `where`/`count`/`all`/`any` condition must be `Bool`, and a `sum`/
    /// `min`/`max` body must be `Int`. Distinct from `require_bool` so the
    /// diagnostic names the list form rather than a logical operator.
    fn require_list_body(
        &mut self,
        shape: &Shape,
        form: &'static str,
        expected: L3ValueType,
        depth: usize,
    ) -> Result<(), Error> {
        self.charge(depth)?;
        let found = match shape.as_ref() {
            Type::Unknown => return Ok(()),
            Type::Known(ty) if *ty == expected => return Ok(()),
            Type::Either(options) => {
                for option in options {
                    self.require_list_body(option, form, expected.clone(), depth + 1)?;
                }
                return Ok(());
            }
            Type::Known(ty) => ty.to_string(),
            Type::Record(_) => "record".to_string(),
            Type::Sum { .. } => "sum".to_string(),
            Type::List(_) => "list".to_string(),
        };
        Err(Error::ListBodyType {
            form,
            expected: match expected {
                L3ValueType::Bool => "Bool",
                L3ValueType::Int => "Int",
                _ => "a scalar",
            },
            found,
        })
    }

    /// The element shape of a list-typed shape, or [`Type::Unknown`] when
    /// `shape` is not statically known to be a list (for example, a bare
    /// reference to a declared `List<T>` input — the runtime type category
    /// there is the flat `L3ValueType::List` marker, which carries no element
    /// shape). Never an error: an unresolved element shape still lets every
    /// downstream check pass permissively, exactly like [`unknown`] elsewhere
    /// in this module, and the evaluator's own [`crate::l3_v2::EvalFault`]
    /// checks remain the defensive boundary.
    fn list_element(&self, shape: &Shape) -> Shape {
        match shape.as_ref() {
            Type::List(elem) => elem.clone(),
            _ => unknown(),
        }
    }

    fn field(&mut self, base: &Shape, field: &str, depth: usize) -> Result<Shape, Error> {
        self.charge(depth)?;
        Ok(match base.as_ref() {
            Type::Record(fields) => fields.get(field).cloned().unwrap_or_else(unknown),
            Type::Known(L3ValueType::Record(name)) => match self.configs.get(name.as_str()) {
                Some(ast::ConfigBody::Record(fields)) => fields
                    .iter()
                    .find(|f| f.name == field)
                    .map(|f| self.declared_shape(&f.ty))
                    .unwrap_or_else(unknown),
                _ => unknown(),
            },
            Type::Either(options) => Arc::new(Type::Either(
                options
                    .iter()
                    .map(|option| self.field(option, field, depth + 1))
                    .collect::<Result<_, _>>()?,
            )),
            _ => unknown(),
        })
    }

    fn payload(
        &mut self,
        base: &Shape,
        variant: &str,
        index: usize,
        depth: usize,
    ) -> Result<Shape, Error> {
        self.charge(depth)?;
        Ok(match base.as_ref() {
            Type::Sum {
                variant: name,
                args,
                ..
            } if name == variant => args.get(index).cloned().unwrap_or_else(unknown),
            Type::Sum { nominal, .. } | Type::Known(L3ValueType::Sum(nominal)) => {
                self.declared_payload(Some(nominal), variant, index)
            }
            Type::Either(options) => Arc::new(Type::Either(
                options
                    .iter()
                    .map(|option| self.payload(option, variant, index, depth + 1))
                    .collect::<Result<_, _>>()?,
            )),
            _ => self.declared_payload(None, variant, index),
        })
    }

    fn expr(&mut self, expr: &Expr, env: &Env, depth: usize) -> Result<Shape, Error> {
        self.charge(depth)?;
        let next = depth + 1;
        Ok(match expr {
            Expr::Int(_) => known(L3ValueType::Int),
            Expr::Bool(_) => known(L3ValueType::Bool),
            Expr::Str(_) => known(L3ValueType::Str),
            Expr::LetRef(name) | Expr::RuleFact(name) => {
                env.get(name).cloned().unwrap_or_else(unknown)
            }
            Expr::NullaryVariant {
                nominal_sum,
                variant,
            } => Arc::new(Type::Sum {
                nominal: nominal_sum.clone(),
                variant: variant.clone(),
                args: vec![],
            }),
            Expr::Ctor {
                nominal_sum,
                variant,
                args,
            } => {
                let args = args
                    .iter()
                    .map(|arg| self.expr(arg, env, next))
                    .collect::<Result<_, _>>()?;
                Arc::new(Type::Sum {
                    nominal: nominal_sum.clone(),
                    variant: variant.clone(),
                    args,
                })
            }
            Expr::Record { fields, .. } => {
                let fields = fields
                    .iter()
                    .map(|(name, value)| Ok((name.clone(), self.expr(value, env, next)?)))
                    .collect::<Result<_, Error>>()?;
                Arc::new(Type::Record(fields))
            }
            Expr::Field(base, field) => {
                let base = self.expr(base, env, next)?;
                self.field(&base, field, next)?
            }
            Expr::Arith(op, a, b) => {
                let lhs = self.expr(a, env, next)?;
                let rhs = self.expr(b, env, next)?;
                if *op == ArithOpV2::Div
                    && matches!(lhs.as_ref(), Type::Known(L3ValueType::Int))
                    && matches!(rhs.as_ref(), Type::Known(L3ValueType::Int))
                {
                    return Err(Error::ExprError(
                        crate::l3_v2::L3V2LowerError::DivisionNotAllowed,
                    ));
                }
                // New numeric domains never mix, including skipped operands.
                for (one, other) in [(&lhs, &rhs), (&rhs, &lhs)] {
                    if let Type::Known(ty @ (L3ValueType::F64 | L3ValueType::Decimal)) =
                        one.as_ref()
                    {
                        self.require_scalar(other, "numeric arithmetic", ty, next)?;
                    }
                }
                // Arithmetic preserves its numeric domain. Unknown arguments
                // remain unknown; the evaluator enforces same-domain operands.
                match lhs.as_ref() {
                    Type::Known(
                        ty @ (L3ValueType::Int | L3ValueType::F64 | L3ValueType::Decimal),
                    ) => known(ty.clone()),
                    _ => unknown(),
                }
            }
            Expr::IntDivMod(_, a, b) | Expr::Cmp(_, a, b) => {
                self.expr(a, env, next)?;
                self.expr(b, env, next)?;
                known(if matches!(expr, Expr::Cmp(..)) {
                    L3ValueType::Bool
                } else {
                    L3ValueType::Int
                })
            }
            Expr::NumericBuiltin(op, args) => {
                let expected: &[L3ValueType] = match op {
                    NumericBuiltinV2::F64 | NumericBuiltinV2::Decimal => &[L3ValueType::Str],
                    NumericBuiltinV2::F64FromInt | NumericBuiltinV2::DecimalFromInt => {
                        &[L3ValueType::Int]
                    }
                    NumericBuiltinV2::F64Neg => &[L3ValueType::F64],
                    NumericBuiltinV2::DecimalNeg => &[L3ValueType::Decimal],
                    NumericBuiltinV2::DecimalDiv => &[
                        L3ValueType::Decimal,
                        L3ValueType::Decimal,
                        L3ValueType::Int,
                        L3ValueType::Str,
                    ],
                };
                for (arg, ty) in args.iter().zip(expected) {
                    let shape = self.expr(arg, env, next)?;
                    self.require_scalar(&shape, op.name(), ty, next)?;
                }
                known(match op {
                    NumericBuiltinV2::F64
                    | NumericBuiltinV2::F64FromInt
                    | NumericBuiltinV2::F64Neg => L3ValueType::F64,
                    NumericBuiltinV2::Decimal
                    | NumericBuiltinV2::DecimalFromInt
                    | NumericBuiltinV2::DecimalDiv
                    | NumericBuiltinV2::DecimalNeg => L3ValueType::Decimal,
                })
            }
            Expr::And(a, b) | Expr::Or(a, b) => {
                let operator = if matches!(expr, Expr::And(..)) {
                    "AND"
                } else {
                    "OR"
                };
                let a = self.expr(a, env, next)?;
                self.require_bool(&a, operator, next)?;
                let b = self.expr(b, env, next)?;
                self.require_bool(&b, operator, next)?;
                known(L3ValueType::Bool)
            }
            Expr::Not(a) => {
                let a = self.expr(a, env, next)?;
                self.require_bool(&a, "NOT", next)?;
                known(L3ValueType::Bool)
            }
            Expr::Match { scrutinee, arms } => {
                let scrutinee = self.expr(scrutinee, env, next)?;
                let mut result = Vec::new();
                for (L3PatternV2::Ctor { variant, binders }, body) in arms {
                    let mut locals = env.clone();
                    for (index, binder) in binders.iter().enumerate() {
                        if let Some(name) = binder {
                            locals.insert(
                                name.clone(),
                                self.payload(&scrutinee, variant, index, next)?,
                            );
                        }
                    }
                    let body = self.expr(body, &locals, next)?;
                    result.push(body);
                }
                Arc::new(Type::Either(result))
            }
            Expr::Call { func, args } => {
                // Lowering has already checked arity and resolution; a cycle
                // in the call graph is admitted (ADR-0042) rather than
                // rejected here.
                let function = self.plan.find_function(func).expect("resolved helper");
                let mut locals = Env::new();
                for (param, arg) in function.params.iter().zip(args) {
                    let shape = self.expr(arg, env, next)?;
                    locals.insert(
                        param.name.clone(),
                        param
                            .contract
                            .as_ref()
                            .map(|c| known(c.ty.clone()))
                            .unwrap_or(shape),
                    );
                }
                // A call back into a helper already being expanded on this
                // path: recursion terminates at runtime (or fails closed to
                // `Unknown` at the evaluator's depth/work bound), not in this
                // static approximation, so it is not re-entered here.
                if !self.active.insert(func.clone()) {
                    return Ok(function
                        .ret_contract
                        .as_ref()
                        .map(|c| known(c.ty.clone()))
                        .unwrap_or_else(unknown));
                }
                let body = self.expr(&function.body, &locals, next);
                self.active.remove(func);
                function
                    .ret_contract
                    .as_ref()
                    .map(|c| known(c.ty.clone()))
                    .unwrap_or(body?)
            }
            Expr::ListLit(_)
            | Expr::Fold { .. }
            | Expr::Filter { .. }
            | Expr::Map { .. }
            | Expr::Comprehension { .. }
            | Expr::In(..)
            | Expr::Len(_)
            | Expr::Distinct(_) => self.list_expr(expr, env, next)?,
        })
    }

    /// The list and relation forms (ADR-0037, ADR-0040), kept out of
    /// [`Self::expr`] so their locals do not widen its frame: `expr` recurses
    /// once per nesting level, and a wider frame lowers the depth a default
    /// thread stack can reach before `MAX_EXPR_DEPTH` is enforced.
    #[inline(never)]
    fn list_expr(&mut self, expr: &Expr, env: &Env, next: usize) -> Result<Shape, Error> {
        Ok(match expr {
            Expr::ListLit(items) => {
                let mut shapes = Vec::with_capacity(items.len());
                for item in items {
                    shapes.push(self.expr(item, env, next)?);
                }
                let elem = match shapes.len() {
                    0 => unknown(),
                    1 => shapes.into_iter().next().expect("checked len == 1"),
                    _ => Arc::new(Type::Either(shapes)),
                };
                Arc::new(Type::List(elem))
            }
            Expr::Fold {
                op,
                list,
                binder,
                body,
            } => {
                let list_shape = self.expr(list, env, next)?;
                let elem_shape = self.list_element(&list_shape);
                let mut locals = env.clone();
                locals.insert(binder.clone(), elem_shape);
                let body_shape = self.expr(body, &locals, next)?;
                match op {
                    FoldOpV2::Sum | FoldOpV2::Min | FoldOpV2::Max => {
                        self.require_list_body(&body_shape, op.name(), L3ValueType::Int, next)?;
                        known(L3ValueType::Int)
                    }
                    FoldOpV2::Count => {
                        self.require_list_body(&body_shape, op.name(), L3ValueType::Bool, next)?;
                        known(L3ValueType::Int)
                    }
                    FoldOpV2::All | FoldOpV2::Any => {
                        self.require_list_body(&body_shape, op.name(), L3ValueType::Bool, next)?;
                        known(L3ValueType::Bool)
                    }
                }
            }
            Expr::Filter { list, binder, cond } => {
                let list_shape = self.expr(list, env, next)?;
                let elem_shape = self.list_element(&list_shape);
                let mut locals = env.clone();
                locals.insert(binder.clone(), elem_shape.clone());
                let cond_shape = self.expr(cond, &locals, next)?;
                self.require_list_body(&cond_shape, "filter", L3ValueType::Bool, next)?;
                Arc::new(Type::List(elem_shape))
            }
            Expr::Map { list, binder, body } => {
                let list_shape = self.expr(list, env, next)?;
                let elem_shape = self.list_element(&list_shape);
                let mut locals = env.clone();
                locals.insert(binder.clone(), elem_shape);
                let body_shape = self.expr(body, &locals, next)?;
                Arc::new(Type::List(body_shape))
            }
            Expr::Comprehension {
                generators,
                where_clause,
                yield_expr,
            } => {
                let mut locals = env.clone();
                for (binder, source) in generators {
                    let source_shape = self.expr(source, &locals, next)?;
                    let elem_shape = self.list_element(&source_shape);
                    locals.insert(binder.clone(), elem_shape);
                }
                if let Some(w) = where_clause {
                    let where_shape = self.expr(w, &locals, next)?;
                    self.require_list_body(&where_shape, "where", L3ValueType::Bool, next)?;
                }
                let yield_shape = self.expr(yield_expr, &locals, next)?;
                Arc::new(Type::List(yield_shape))
            }
            Expr::In(needle, haystack) => {
                self.expr(needle, env, next)?;
                self.expr(haystack, env, next)?;
                known(L3ValueType::Bool)
            }
            Expr::Len(list) => {
                self.expr(list, env, next)?;
                known(L3ValueType::Int)
            }
            Expr::Distinct(list) => {
                let list_shape = self.expr(list, env, next)?;
                let elem_shape = self.list_element(&list_shape);
                Arc::new(Type::List(elem_shape))
            }
            _ => unreachable!("list_expr is only called for list and relation forms"),
        })
    }
}

pub(super) fn check(plan: &FiniteDecisionPlan, module: &ast::Module) -> Result<(), Error> {
    let roots = plan
        .functions
        .iter()
        .map(|f| &f.body)
        .chain(plan.lets.iter().map(|(_, e)| e))
        .chain(plan.rules.iter().map(|r| &r.body))
        .chain(plan.proposals.iter().flat_map(|p| [&p.guard, &p.value]))
        .chain(plan.shows.iter())
        .chain(plan.decides.iter().flat_map(|d| {
            std::iter::once(&d.list).chain(d.proposals.iter().flat_map(|p| [&p.guard, &p.value]))
        }));
    // Preserve the existing acceptance of programs without Boolean operators
    // or list/relational forms.
    if !needs_shape_check(roots.collect()) {
        return Ok(());
    }
    let configs = module
        .items
        .iter()
        .filter_map(|item| match item {
            ast::Item::Config(c) => Some((c.name.as_str(), &c.body)),
            _ => None,
        })
        .collect();
    let mut checker = Checker {
        plan,
        configs,
        remaining: MAX_ANALYSIS_WORK,
        active: BTreeSet::new(),
    };
    for function in &plan.functions {
        let locals = function
            .params
            .iter()
            .map(|p| {
                (
                    p.name.clone(),
                    p.contract
                        .as_ref()
                        .map(|c| known(c.ty.clone()))
                        .unwrap_or_else(unknown),
                )
            })
            .collect();
        checker.expr(&function.body, &locals, 0)?;
    }
    let mut env: Env = plan
        .inputs
        .iter()
        .map(|i| {
            let shape = if let Some(list) = &i.list {
                Arc::new(Type::List(
                    checker.declared_shape(&ast::Ty::Named(list.element.display_name())),
                ))
            } else {
                known(i.ty.clone())
            };
            (i.name.clone(), shape)
        })
        .collect();
    for (name, expr) in &plan.lets {
        let shape = checker.expr(expr, &env, 0)?;
        env.insert(name.clone(), shape);
    }
    for rule in &plan.rules {
        let shape = checker.expr(&rule.body, &env, 0)?;
        env.insert(rule.name.clone(), shape);
    }
    for proposal in &plan.proposals {
        checker.expr(&proposal.guard, &env, 0)?;
        checker.expr(&proposal.value, &env, 0)?;
    }
    for decide in &plan.decides {
        let list_shape = checker.expr(&decide.list, &env, 0)?;
        let mut locals = env.clone();
        locals.insert(decide.binder.clone(), checker.list_element(&list_shape));
        for proposal in &decide.proposals {
            checker.expr(&proposal.guard, &locals, 0)?;
            checker.expr(&proposal.value, &locals, 0)?;
        }
    }
    for expr in &plan.shows {
        checker.expr(expr, &env, 0)?;
    }
    Ok(())
}

/// Whether any reachable expression needs this module's shape analysis:
/// a Boolean operator (ADR-0034) or a list/relational form whose body has a
/// required type (ADR-0037, ADR-0040: `fold`/`filter`/`map`/comprehension —
/// `distinct`/`len`/`in`/a list literal impose no requirement of their own,
/// but are still walked so a form nested inside one is found).
fn needs_shape_check(mut pending: Vec<&Expr>) -> bool {
    while let Some(expr) = pending.pop() {
        match expr {
            Expr::And(..)
            | Expr::Or(..)
            | Expr::Not(..)
            | Expr::NumericBuiltin(..)
            | Expr::Arith(ArithOpV2::Div, ..) => return true,
            Expr::Fold { .. } | Expr::Filter { .. } | Expr::Map { .. } => return true,
            Expr::Arith(_, a, b) | Expr::IntDivMod(_, a, b) | Expr::Cmp(_, a, b) => {
                pending.extend([a.as_ref(), b.as_ref()]);
            }
            Expr::Field(base, _) => pending.push(base),
            Expr::Call { args, .. } | Expr::Ctor { args, .. } => pending.extend(args),
            Expr::Record { fields, .. } => pending.extend(fields.iter().map(|(_, e)| e)),
            Expr::Match { scrutinee, arms } => {
                pending.push(scrutinee);
                pending.extend(arms.iter().map(|(_, body)| body));
            }
            Expr::Comprehension {
                generators,
                where_clause,
                yield_expr,
            } => {
                // A comprehension's `where` needs the same Bool check a
                // `filter` condition does; a comprehension with no `where`
                // still needs walking for a form nested in a generator or the
                // `yield` expression.
                if where_clause.is_some() {
                    return true;
                }
                pending.extend(generators.iter().map(|(_, source)| source));
                pending.push(yield_expr);
            }
            Expr::ListLit(items) => pending.extend(items),
            Expr::In(a, b) => pending.extend([a.as_ref(), b.as_ref()]),
            Expr::Len(a) | Expr::Distinct(a) => pending.push(a),
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_shared_shapes_are_released_without_recursing() {
        let mut shape = known(L3ValueType::Bool);
        for _ in 0..20_000 {
            shape = Arc::new(Type::Record(BTreeMap::from([("value".into(), shape)])));
        }
        drop(shape);
    }
}
