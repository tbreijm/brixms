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
            let ty = infer_inner(base, fields, module, program, active)?;
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
                        ty: infer_inner(value, fields, module, program, active)?,
                    })
                })
                .collect::<Option<_>>()?,
        )),
        Expr::ListLit(items) => {
            let element = infer_inner(items.first()?, fields, module, program, active)?;
            for item in &items[1..] {
                if infer_inner(item, fields, module, program, active)? != element {
                    return None;
                }
            }
            Some(Ty::App("List".into(), vec![element]))
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            let scrutinee = infer_inner(scrutinee, fields, module, program, active)?;
            let mut result = None;
            for arm in arms {
                let mut scope = fields.clone();
                bind_pattern(&arm.pattern, &scrutinee, &mut scope, module, program)?;
                let ty = infer_inner(&arm.body, &scope, module, program, active)?;
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
                infer_inner(lhs, fields, module, program, active)
            }
        }
        Expr::Call { func, args } => {
            match func.as_str() {
                "len" => return scalar("Int"),
                "all" | "any" => return scalar("Bool"),
                "distinct" | "filter" => {
                    return infer_inner(args.first()?, fields, module, program, active)
                }
                "map" => {
                    let Ty::App(name, elements) =
                        infer_inner(args.first()?, fields, module, program, active)?
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
                        vec![infer_inner(body, &scope, module, program, active)?],
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
                let scope = helper
                    .params
                    .iter()
                    .map(|p| Some((p.name.clone(), qualify_ty(p.ty.as_ref()?, owner, program))))
                    .collect::<Option<_>>()?;
                active.push(name);
                let result = infer_inner(&helper.body, &scope, owner, program, active);
                active.pop();
                result
            } else {
                constructor(func, module, program)
            }
        }
        Expr::Ascribe { ty, .. } => Some(qualify_ty(ty, module, program)),
        Expr::Prove(inner) | Expr::Why(inner) | Expr::Audit(inner) => {
            infer_inner(inner, fields, module, program, active)
        }
        _ => None,
    }
}
