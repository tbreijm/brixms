//! Static Boolean operand checks, independent of lazy value evaluation.
//!
//! This is shape analysis, not execution: arithmetic faults and branch choices
//! are never computed. Unannotated helpers are checked with abstract parameters
//! and again with their caller's argument shapes. Shared shapes avoid expanding
//! records when a helper duplicates an argument. Work and recursion are bounded
//! separately from the evaluator, including visits through shared shapes.

use std::collections::BTreeMap;
use std::sync::Arc;

use super::plan::{FiniteDecisionLowerError as Error, FiniteDecisionPlan, MAX_EXPR_DEPTH};
use crate::l3_v2::{FoldOpV2, L3ExprV2 as Expr, L3PatternV2, L3ValueType};
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
            Expr::Arith(_, a, b) | Expr::IntDivMod(_, a, b) | Expr::Cmp(_, a, b) => {
                self.expr(a, env, next)?;
                self.expr(b, env, next)?;
                known(if matches!(expr, Expr::Cmp(..)) {
                    L3ValueType::Bool
                } else {
                    L3ValueType::Int
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
                // Lowering has already checked arity, resolution and cycles.
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
                let body = self.expr(&function.body, &locals, next)?;
                function
                    .ret_contract
                    .as_ref()
                    .map(|c| known(c.ty.clone()))
                    .unwrap_or(body)
            }
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
        .chain(plan.shows.iter());
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
        .map(|i| (i.name.clone(), known(i.ty.clone())))
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
            Expr::And(..) | Expr::Or(..) | Expr::Not(..) => return true,
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
