//! Shared expression type propagation for world projections, aggregates and decisions.
use crate::module_graph::{LinkedProgram, QualifiedName};
use brix_syntax::ast::{self, Expr, Ty};
use std::collections::BTreeMap;

pub(crate) fn infer(
    expr: &Expr,
    fields: &BTreeMap<String, Ty>,
    module: &str,
    program: &LinkedProgram,
) -> Option<Ty> {
    infer_inner(expr, fields, module, program, &mut Vec::new())
}

/// Reject the two string operations excluded from the release expression
/// profile while all relation schemas and lexical bindings are still known.
/// Helper bodies are checked at each call with their actual argument shapes,
/// including helpers whose parameters have no declared type.
pub(crate) fn validate_string_ops(
    expr: &Expr,
    fields: &BTreeMap<String, Ty>,
    module: &str,
    program: &LinkedProgram,
) -> Result<(), String> {
    validate_string_ops_inner(
        expr,
        fields,
        module,
        program,
        &mut StringAnalysis {
            active: Vec::new(),
            checked: Vec::new(),
            remaining: 100_000,
        },
        0,
    )
}

type StringCall = (String, Vec<Option<Ty>>);

struct StringAnalysis {
    active: Vec<StringCall>,
    checked: Vec<StringCall>,
    remaining: usize,
}

fn validate_string_ops_inner(
    expr: &Expr,
    fields: &BTreeMap<String, Ty>,
    module: &str,
    program: &LinkedProgram,
    analysis: &mut StringAnalysis,
    depth: usize,
) -> Result<(), String> {
    if analysis.remaining == 0 {
        return Err("string type analysis work limit exceeded".into());
    }
    analysis.remaining -= 1;
    if depth > 128 {
        return Err("expression nesting exceeds string type analysis limit".into());
    }
    let next = depth + 1;
    let check = |expr: &Expr,
                 fields: &BTreeMap<String, Ty>,
                 analysis: &mut StringAnalysis,
                 depth: usize| {
        validate_string_ops_inner(expr, fields, module, program, analysis, depth)
    };
    match expr {
        Expr::Bin { op, lhs, rhs } => {
            check(lhs, fields, analysis, next)?;
            check(rhs, fields, analysis, next)?;
            if matches!(op, ast::BinOp::Add | ast::BinOp::Lt)
                && expression_may_be_str(lhs, fields, module, program, &mut Vec::new(), next)
                && expression_may_be_str(rhs, fields, module, program, &mut Vec::new(), next)
            {
                return Err(format!(
                    "operator '{}' is unsupported for Str operands in this profile",
                    if *op == ast::BinOp::Add { "+" } else { "<" }
                ));
            }
        }
        Expr::Call { func, args } => {
            // The argument expressions themselves can contain excluded
            // operators, including arguments nested under lambdas.
            for arg in args {
                if let Expr::Lambda { param, body } = arg {
                    let mut scope = fields.clone();
                    if let Some(Ty::App(name, args)) = args.first().and_then(|source| {
                        infer_string_shape(source, fields, module, program, &mut Vec::new())
                    }) {
                        if name == "List" && args.len() == 1 {
                            scope.insert(param.clone(), args[0].clone());
                        }
                    }
                    check(body, &scope, analysis, next)?;
                } else {
                    check(arg, fields, analysis, next)?;
                }
            }
            let default_module = effective_module(module, program);
            let (owner, local) = func.rsplit_once("::").unwrap_or((default_module, func));
            let q = QualifiedName::new(owner, local);
            if let Some(helper) = program.functions.get(&q) {
                let name = q.to_string();
                {
                    let mut scope = fields.clone();
                    for (index, param) in helper.params.iter().enumerate() {
                        let ty = param
                            .ty
                            .as_ref()
                            .map(|ty| qualify_ty(ty, owner, program))
                            .or_else(|| {
                                args.get(index).and_then(|arg| {
                                    infer_string_shape(
                                        arg,
                                        fields,
                                        module,
                                        program,
                                        &mut Vec::new(),
                                    )
                                })
                            });
                        if let Some(ty) = ty {
                            scope.insert(param.name.clone(), ty);
                        }
                    }
                    let call = (
                        name,
                        helper
                            .params
                            .iter()
                            .map(|param| scope.get(&param.name).cloned())
                            .collect(),
                    );
                    if !analysis.active.contains(&call) && !analysis.checked.contains(&call) {
                        analysis.active.push(call.clone());
                        validate_string_ops_inner(
                            &helper.body,
                            &scope,
                            owner,
                            program,
                            analysis,
                            next,
                        )?;
                        analysis.active.pop();
                        analysis.checked.push(call);
                    }
                }
            }
        }
        Expr::Field(base, _)
        | Expr::Not(base)
        | Expr::Prove(base)
        | Expr::Why(base)
        | Expr::Audit(base) => check(base, fields, analysis, next)?,
        Expr::CollectTake { collection, limit } => {
            check(collection, fields, analysis, next)?;
            check(limit, fields, analysis, next)?;
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            check(scrutinee, fields, analysis, next)?;
            let scrutinee_ty =
                infer_string_shape(scrutinee, fields, module, program, &mut Vec::new());
            for arm in arms {
                let mut scope = fields.clone();
                if let Some(ty) = &scrutinee_ty {
                    let _ = bind_pattern(&arm.pattern, ty, &mut scope, module, program);
                }
                check(&arm.body, &scope, analysis, next)?;
            }
        }
        Expr::Record { fields: values, .. } | Expr::AnonRecord(values) => {
            for (_, value) in values {
                check(value, fields, analysis, next)?;
            }
        }
        Expr::ListLit(items) => {
            for item in items {
                check(item, fields, analysis, next)?;
            }
        }
        Expr::Lambda { param, body } => {
            let mut scope = fields.clone();
            scope
                .entry(param.clone())
                .or_insert(Ty::Named("Any".into()));
            check(body, &scope, analysis, next)?;
        }
        Expr::Comprehension {
            generators,
            where_clause,
            yield_expr,
        } => {
            let mut scope = fields.clone();
            for (name, source) in generators {
                check(source, &scope, analysis, next)?;
                if let Some(Ty::App(list, args)) =
                    infer_string_shape(source, &scope, module, program, &mut Vec::new())
                {
                    if list == "List" && args.len() == 1 {
                        scope.insert(name.clone(), args[0].clone());
                    }
                }
            }
            if let Some(predicate) = where_clause {
                check(predicate, &scope, analysis, next)?;
            }
            check(yield_expr, &scope, analysis, next)?;
        }
        Expr::Validate { arg, .. } | Expr::Ascribe { expr: arg, .. } => {
            check(arg, fields, analysis, next)?;
        }
        _ => {}
    }
    Ok(())
}

fn expression_may_be_str(
    expr: &Expr,
    fields: &BTreeMap<String, Ty>,
    module: &str,
    program: &LinkedProgram,
    active: &mut Vec<String>,
    depth: usize,
) -> bool {
    if depth > 128 {
        return false;
    }
    match infer_string_shape(expr, fields, module, program, &mut Vec::new()) {
        Some(Ty::Named(name)) if name == "Str" => return true,
        Some(_) => return false,
        None => {}
    }
    match expr {
        Expr::Match {
            scrutinee, arms, ..
        } => {
            let scrutinee_ty =
                infer_string_shape(scrutinee, fields, module, program, &mut Vec::new());
            arms.iter().any(|arm| {
                let mut scope = fields.clone();
                if let Some(ty) = &scrutinee_ty {
                    let _ = bind_pattern(&arm.pattern, ty, &mut scope, module, program);
                }
                expression_may_be_str(&arm.body, &scope, module, program, active, depth + 1)
            })
        }
        Expr::Call { func, args } => {
            let default_module = effective_module(module, program);
            let (owner, local) = func.rsplit_once("::").unwrap_or((default_module, func));
            let q = QualifiedName::new(owner, local);
            let Some(helper) = program.functions.get(&q) else {
                return false;
            };
            let name = q.to_string();
            if active.len() >= 128 || active.contains(&name) {
                return false;
            }
            let mut scope = fields.clone();
            for (index, param) in helper.params.iter().enumerate() {
                let ty = param
                    .ty
                    .as_ref()
                    .map(|ty| qualify_ty(ty, owner, program))
                    .or_else(|| {
                        args.get(index).and_then(|arg| {
                            infer_string_shape(arg, fields, module, program, &mut Vec::new())
                        })
                    });
                if let Some(ty) = ty {
                    scope.insert(param.name.clone(), ty);
                }
            }
            active.push(name);
            let result =
                expression_may_be_str(&helper.body, &scope, owner, program, active, depth + 1);
            active.pop();
            result
        }
        _ => false,
    }
}

fn effective_module<'a>(module: &'a str, program: &'a LinkedProgram) -> &'a str {
    if module.is_empty() {
        program.root_module.as_str()
    } else {
        module
    }
}

pub(crate) fn lookup_config<'a>(
    name: &str,
    module: &str,
    program: &'a LinkedProgram,
) -> Option<(&'a str, &'a ast::ConfigDecl)> {
    let default_module = effective_module(module, program);
    let (owner, local) = name.rsplit_once("::").unwrap_or((default_module, name));
    let q = QualifiedName::new(owner, local);
    program
        .available_configs
        .get_key_value(&q)
        .or_else(|| program.configs.get_key_value(&q))
        .map(|(k, v)| (k.module.as_str(), v))
}

pub(crate) fn qualify_ty(ty: &Ty, module: &str, program: &LinkedProgram) -> Ty {
    let default_module = effective_module(module, program);
    match ty {
        Ty::Named(name) => {
            if matches!(
                name.as_str(),
                "Str" | "Int" | "Bool" | "F64" | "Decimal" | "Any"
            ) || name.contains("::")
            {
                Ty::Named(name.clone())
            } else {
                let q = QualifiedName::new(default_module, name);
                if program.available_configs.contains_key(&q) || program.configs.contains_key(&q) {
                    Ty::Named(q.to_string())
                } else {
                    Ty::Named(name.clone())
                }
            }
        }
        Ty::Record(fields) => Ty::Record(
            fields
                .iter()
                .map(|f| ast::FieldDecl {
                    name: f.name.clone(),
                    ty: qualify_ty(&f.ty, default_module, program),
                })
                .collect(),
        ),
        Ty::App(name, args) => Ty::App(
            name.clone(),
            args.iter()
                .map(|a| qualify_ty(a, default_module, program))
                .collect(),
        ),
        Ty::Graded(inner, grade) => {
            Ty::Graded(Box::new(qualify_ty(inner, default_module, program)), *grade)
        }
        other => other.clone(),
    }
}

fn constructor(name: &str, module: &str, program: &LinkedProgram) -> Option<Ty> {
    let default_module = effective_module(module, program);
    let mut seen = std::collections::BTreeSet::new();
    let mut candidates = Vec::new();
    for (q, c) in program
        .available_configs
        .iter()
        .chain(program.configs.iter())
    {
        if !seen.insert(q) {
            continue;
        }
        let ast::ConfigBody::Sum(variants) = &c.body else {
            continue;
        };
        let full = q.to_string();
        if variants.iter().any(|v| {
            let qualified = format!("{full}::{}", v.name);
            name == v.name
                || name == qualified
                || (name.contains("::") && qualified.ends_with(&format!("::{name}")))
        }) {
            candidates.push(full);
        }
    }
    let local: Vec<_> = candidates
        .iter()
        .filter(|name| {
            name.rsplit_once("::")
                .is_some_and(|(owner, _)| owner == default_module)
        })
        .collect();
    match (local.as_slice(), candidates.as_slice()) {
        ([name], _) => Some(Ty::Named((*name).clone())),
        ([], [name]) => Some(Ty::Named(name.clone())),
        _ => None,
    }
}

fn bind_pattern(
    pattern: &ast::Pattern,
    ty: &Ty,
    fields: &mut BTreeMap<String, Ty>,
    module: &str,
    program: &LinkedProgram,
) -> Option<()> {
    match pattern {
        ast::Pattern::Var(name) => {
            fields.insert(name.clone(), qualify_ty(ty, module, program));
        }
        ast::Pattern::Wildcard => {}
        ast::Pattern::Ctor { name, args } if !args.is_empty() => {
            let Ty::Named(nominal) = ty else {
                return None;
            };
            let (cfg_module, cfg) = lookup_config(nominal, module, program)?;
            let ast::ConfigBody::Sum(variants) = &cfg.body else {
                return None;
            };
            let variant = name.rsplit("::").next()?;
            let types = &variants.iter().find(|v| v.name == variant)?.params;
            if types.len() != args.len() {
                return None;
            }
            for (arg, ty) in args.iter().zip(types) {
                let qualified_ty = qualify_ty(ty, cfg_module, program);
                bind_pattern(arg, &qualified_ty, fields, cfg_module, program)?;
            }
        }
        ast::Pattern::Ctor { .. } => {}
    }
    Some(())
}

fn infer_inner(
    expr: &Expr,
    fields: &BTreeMap<String, Ty>,
    module: &str,
    program: &LinkedProgram,
    active: &mut Vec<String>,
) -> Option<Ty> {
    infer_inner_with_callsite(expr, fields, module, program, active, false)
}

// Callsite inference is restricted to admission checks. Persisted result
// schemas retain the established annotation-based inference contract.
fn infer_string_shape(
    expr: &Expr,
    fields: &BTreeMap<String, Ty>,
    module: &str,
    program: &LinkedProgram,
    active: &mut Vec<String>,
) -> Option<Ty> {
    infer_inner_with_callsite(expr, fields, module, program, active, true)
}

fn infer_inner_with_callsite(
    expr: &Expr,
    fields: &BTreeMap<String, Ty>,
    module: &str,
    program: &LinkedProgram,
    active: &mut Vec<String>,
    infer_args: bool,
) -> Option<Ty> {
    let scalar = |name: &str| Some(Ty::Named(name.into()));
    match expr {
        Expr::Str(_) => scalar("Str"),
        Expr::Bool(_) | Expr::Not(_) => scalar("Bool"),
        Expr::Num(n) => scalar(if n.contains('.') { "F64" } else { "Int" }),
        Expr::Var(name) => fields
            .get(name)
            .cloned()
            .or_else(|| constructor(name, module, program)),
        Expr::Field(base, field) => {
            if let Expr::Var(alias) = base.as_ref() {
                if let Some(ty) = fields.get(&format!("{alias}.{field}")) {
                    return Some(qualify_ty(ty, module, program));
                }
            }
            let ty = infer_inner_with_callsite(base, fields, module, program, active, infer_args)?;
            let (cfg_module, record) = match &ty {
                Ty::Record(fields) => (effective_module(module, program), fields),
                Ty::Named(name) => {
                    let (cfg_module, cfg) = lookup_config(name, module, program)?;
                    match &cfg.body {
                        ast::ConfigBody::Record(fields) => (cfg_module, fields),
                        _ => return None,
                    }
                }
                _ => return None,
            };
            record
                .iter()
                .find(|f| f.name == *field)
                .map(|f| qualify_ty(&f.ty, cfg_module, program))
        }
        Expr::Record { config, .. } => {
            let default_module = effective_module(module, program);
            Some(Ty::Named(if config.contains("::") {
                config.clone()
            } else {
                format!("{default_module}::{config}")
            }))
        }
        Expr::AnonRecord(items) => Some(Ty::Record(
            items
                .iter()
                .map(|(name, value)| {
                    Some(ast::FieldDecl {
                        name: name.clone(),
                        ty: infer_inner_with_callsite(
                            value, fields, module, program, active, infer_args,
                        )?,
                    })
                })
                .collect::<Option<_>>()?,
        )),
        Expr::ListLit(items) => {
            let element = infer_inner_with_callsite(
                items.first()?,
                fields,
                module,
                program,
                active,
                infer_args,
            )?;
            for item in &items[1..] {
                if infer_inner_with_callsite(item, fields, module, program, active, infer_args)?
                    != element
                {
                    return None;
                }
            }
            Some(Ty::App("List".into(), vec![element]))
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            let scrutinee =
                infer_inner_with_callsite(scrutinee, fields, module, program, active, infer_args)?;
            let mut result = None;
            for arm in arms {
                let mut scope = fields.clone();
                bind_pattern(&arm.pattern, &scrutinee, &mut scope, module, program)?;
                let ty = infer_inner_with_callsite(
                    &arm.body, &scope, module, program, active, infer_args,
                )?;
                if result.as_ref().is_some_and(|existing| *existing != ty) {
                    return None;
                }
                result = Some(ty);
            }
            result
        }
        Expr::Bin { op, lhs, .. } => {
            if op.is_comparison() || op.is_logical() {
                scalar("Bool")
            } else {
                infer_inner_with_callsite(lhs, fields, module, program, active, infer_args)
            }
        }
        Expr::Call { func, args } => {
            match func.as_str() {
                "len" => return scalar("Int"),
                "all" | "any" => return scalar("Bool"),
                "distinct" | "filter" => {
                    return infer_inner_with_callsite(
                        args.first()?,
                        fields,
                        module,
                        program,
                        active,
                        infer_args,
                    )
                }
                "map" => {
                    let Ty::App(name, elements) = infer_inner_with_callsite(
                        args.first()?,
                        fields,
                        module,
                        program,
                        active,
                        infer_args,
                    )?
                    else {
                        return None;
                    };
                    if name != "List" || elements.len() != 1 {
                        return None;
                    }
                    let Expr::Lambda { param, body } = args.get(1)? else {
                        return None;
                    };
                    let mut scope = fields.clone();
                    scope.insert(param.clone(), elements[0].clone());
                    return Some(Ty::App(
                        "List".into(),
                        vec![infer_inner_with_callsite(
                            body, &scope, module, program, active, infer_args,
                        )?],
                    ));
                }
                _ => {}
            }
            let default_module = effective_module(module, program);
            let (owner, local) = func.rsplit_once("::").unwrap_or((default_module, func));
            let q = QualifiedName::new(owner, local);
            if let Some(helper) = program.functions.get(&q) {
                if let Some(ret) = &helper.ret {
                    return Some(qualify_ty(ret, owner, program));
                }
                // Recursive helpers need an annotation; never recurse through a cycle.
                let name = q.to_string();
                if active.contains(&name) || active.len() >= 64 {
                    return None;
                }
                let mut scope = BTreeMap::new();
                for (index, param) in helper.params.iter().enumerate() {
                    let ty = param
                        .ty
                        .as_ref()
                        .map(|ty| qualify_ty(ty, owner, program))
                        .or_else(|| {
                            if !infer_args {
                                return None;
                            }
                            args.get(index).and_then(|arg| {
                                infer_inner_with_callsite(
                                    arg, fields, module, program, active, infer_args,
                                )
                            })
                        })?;
                    scope.insert(param.name.clone(), ty);
                }
                active.push(name);
                let result = infer_inner_with_callsite(
                    &helper.body,
                    &scope,
                    owner,
                    program,
                    active,
                    infer_args,
                );
                active.pop();
                result
            } else {
                constructor(func, module, program)
            }
        }
        Expr::Ascribe { ty, .. } => Some(qualify_ty(ty, module, program)),
        Expr::Prove(inner) | Expr::Why(inner) | Expr::Audit(inner) => {
            infer_inner_with_callsite(inner, fields, module, program, active, infer_args)
        }
        _ => None,
    }
}
