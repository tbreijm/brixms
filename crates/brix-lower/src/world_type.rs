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

fn config<'a>(name: &str, module: &str, program: &'a LinkedProgram) -> Option<&'a ast::ConfigDecl> {
    let (owner, local) = name.rsplit_once("::").unwrap_or((module, name));
    program
        .available_configs
        .get(&QualifiedName::new(owner, local))
}

fn constructor(name: &str, module: &str, program: &LinkedProgram) -> Option<Ty> {
    let candidates: Vec<_> = program
        .available_configs
        .iter()
        .filter_map(|(q, c)| {
            let ast::ConfigBody::Sum(variants) = &c.body else {
                return None;
            };
            let full = q.to_string();
            variants
                .iter()
                .any(|v| {
                    let qualified = format!("{full}::{}", v.name);
                    name == v.name
                        || name == qualified
                        || (name.contains("::") && qualified.ends_with(&format!("::{name}")))
                })
                .then_some(full)
        })
        .collect();
    let local: Vec<_> = candidates
        .iter()
        .filter(|name| {
            name.rsplit_once("::")
                .is_some_and(|(owner, _)| owner == module)
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
            fields.insert(name.clone(), ty.clone());
        }
        ast::Pattern::Wildcard => {}
        ast::Pattern::Ctor { name, args } if !args.is_empty() => {
            let Ty::Named(nominal) = ty else {
                return None;
            };
            let ast::ConfigBody::Sum(variants) = &config(nominal, module, program)?.body else {
                return None;
            };
            let variant = name.rsplit("::").next()?;
            let types = &variants.iter().find(|v| v.name == variant)?.params;
            if types.len() != args.len() {
                return None;
            }
            for (arg, ty) in args.iter().zip(types) {
                bind_pattern(arg, ty, fields, module, program)?;
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
                    return Some(ty.clone());
                }
            }
            let ty = infer_inner(base, fields, module, program, active)?;
            let record = match &ty {
                Ty::Record(fields) => fields,
                Ty::Named(name) => match &config(name, module, program)?.body {
                    ast::ConfigBody::Record(fields) => fields,
                    _ => return None,
                },
                _ => return None,
            };
            record
                .iter()
                .find(|f| f.name == *field)
                .map(|f| f.ty.clone())
        }
        Expr::Record { config, .. } => Some(Ty::Named(if config.contains("::") {
            config.clone()
        } else {
            format!("{module}::{config}")
        })),
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
            let (owner, local) = func.rsplit_once("::").unwrap_or((module, func));
            let q = QualifiedName::new(owner, local);
            if let Some(helper) = program.functions.get(&q) {
                if let Some(ret) = &helper.ret {
                    return Some(ret.clone());
                }
                // Recursive helpers need an annotation; never recurse through a cycle.
                let name = q.to_string();
                if active.contains(&name) || active.len() >= 64 {
                    return None;
                }
                let scope = helper
                    .params
                    .iter()
                    .map(|p| Some((p.name.clone(), p.ty.clone()?)))
                    .collect::<Option<_>>()?;
                active.push(name);
                let result = infer_inner(&helper.body, &scope, owner, program, active);
                active.pop();
                result
            } else {
                constructor(func, module, program)
            }
        }
        Expr::Prove(inner) | Expr::Why(inner) | Expr::Audit(inner) => {
            infer_inner(inner, fields, module, program, active)
        }
        _ => None,
    }
}
