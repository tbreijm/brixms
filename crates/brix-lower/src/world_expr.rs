//! World expressions use the established scalar lowerer and evaluator.
//! Compilation is separate from evaluation so helper bodies are never rebuilt per row.
use crate::l3_v2::{self, EvalEnv, L3ExprV2, L3FunctionDef, L3Schema, L3SchemaType, L3ValueV2};
use brix_syntax::ast;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct CompiledWorldExpr {
    expr: L3ExprV2,
    functions: Arc<BTreeMap<String, L3FunctionDef>>,
    schemas: Arc<BTreeMap<String, L3Schema>>,
}

/// Rewrite `and` (`ast::BinOp::And`) to logical `&&` (`ast::BinOp::AndAnd`)
/// throughout an expression tree.
///
/// The shared scalar lowerer (`l3_v2::lower_expr_v2`) refuses `BinOp::And`
/// because in the legacy finite-decision profile `and`/`then` are the
/// witness-tensor/sequential composition operators (ADR-0002), not logical
/// conjunction, and that refusal must stay intact for legacy programs.
///
/// The world/relational profile (ADR-0046 §3.5) has no witness-composition
/// surface at all and uses the keyword `and` purely as logical conjunction in
/// `where`/`when` predicates (mirroring `relation_dag::split_and_predicates`,
/// which already folds `and` and `&&` together for join/filter predicates).
/// Guard and value expressions inside `decide ... propose ... when` blocks —
/// and helper function bodies reachable from the world profile — never go
/// through `relation_dag`'s predicate splitting, so they must be normalized
/// here, at the one place all world-profile scalar expressions are compiled.
fn normalize_world_and(expr: &ast::Expr) -> ast::Expr {
    use ast::Expr;
    match expr {
        Expr::Num(_) | Expr::Str(_) | Expr::Bool(_) | Expr::Var(_) => expr.clone(),
        Expr::Record { config, fields } => Expr::Record {
            config: config.clone(),
            fields: fields
                .iter()
                .map(|(n, e)| (n.clone(), normalize_world_and(e)))
                .collect(),
        },
        Expr::Field(base, field) => Expr::Field(Box::new(normalize_world_and(base)), field.clone()),
        Expr::Call { func, args } => Expr::Call {
            func: func.clone(),
            args: args.iter().map(normalize_world_and).collect(),
        },
        Expr::Bin { op, lhs, rhs } => {
            let op = if matches!(op, ast::BinOp::And) {
                ast::BinOp::AndAnd
            } else {
                *op
            };
            Expr::Bin {
                op,
                lhs: Box::new(normalize_world_and(lhs)),
                rhs: Box::new(normalize_world_and(rhs)),
            }
        }
        Expr::Match {
            scrutinee,
            arms,
            proving_exhaustive,
        } => Expr::Match {
            scrutinee: Box::new(normalize_world_and(scrutinee)),
            arms: arms
                .iter()
                .map(|a| ast::MatchArm {
                    pattern: a.pattern.clone(),
                    body: normalize_world_and(&a.body),
                })
                .collect(),
            proving_exhaustive: *proving_exhaustive,
        },
        Expr::Prove(e) => Expr::Prove(Box::new(normalize_world_and(e))),
        Expr::Why(e) => Expr::Why(Box::new(normalize_world_and(e))),
        Expr::Audit(e) => Expr::Audit(Box::new(normalize_world_and(e))),
        Expr::Not(e) => Expr::Not(Box::new(normalize_world_and(e))),
        Expr::Lambda { param, body } => Expr::Lambda {
            param: param.clone(),
            body: Box::new(normalize_world_and(body)),
        },
        Expr::ListLit(items) => Expr::ListLit(items.iter().map(normalize_world_and).collect()),
        Expr::Comprehension {
            generators,
            where_clause,
            yield_expr,
        } => Expr::Comprehension {
            generators: generators
                .iter()
                .map(|(n, e)| (n.clone(), normalize_world_and(e)))
                .collect(),
            where_clause: where_clause
                .as_ref()
                .map(|e| Box::new(normalize_world_and(e))),
            yield_expr: Box::new(normalize_world_and(yield_expr)),
        },
        Expr::AnonRecord(fields) => Expr::AnonRecord(
            fields
                .iter()
                .map(|(n, e)| (n.clone(), normalize_world_and(e)))
                .collect(),
        ),
    }
}

/// Resolve a world value or helper contract, including ordered list values.
pub fn contract(ty: &ast::Ty) -> Result<L3SchemaType, String> {
    match ty {
        ast::Ty::Named(name) => Ok(match name.as_str() {
            "Int" => L3SchemaType::Int,
            "Bool" => L3SchemaType::Bool,
            "Str" => L3SchemaType::Str,
            "F64" => L3SchemaType::F64,
            "Decimal" => L3SchemaType::Decimal,
            other => L3SchemaType::Named(other.to_owned()),
        }),
        ast::Ty::App(name, args) if name == "List" && args.len() == 1 => {
            Ok(L3SchemaType::List(Box::new(contract(&args[0])?)))
        }
        ast::Ty::Graded(_, _) => Err("graded helper contracts require grade evidence".into()),
        other => Err(format!("unsupported world helper contract: {other:?}")),
    }
}

use std::sync::atomic::{AtomicUsize, Ordering};

static HELPER_COMPILATION_COUNT: AtomicUsize = AtomicUsize::new(0);
static EXPR_COMPILATION_COUNT: AtomicUsize = AtomicUsize::new(0);

pub fn helper_compilations() -> usize {
    HELPER_COMPILATION_COUNT.load(Ordering::SeqCst)
}

pub fn expr_compilations() -> usize {
    EXPR_COMPILATION_COUNT.load(Ordering::SeqCst)
}

pub fn reset_compilation_counters() {
    HELPER_COMPILATION_COUNT.store(0, Ordering::SeqCst);
    EXPR_COMPILATION_COUNT.store(0, Ordering::SeqCst);
}

/// Convert linked nominal configs into L3 nominal schemas.
pub fn build_nominal_schemas(
    configs: &BTreeMap<crate::module_graph::QualifiedName, ast::ConfigDecl>,
) -> Result<Arc<BTreeMap<String, L3Schema>>, String> {
    use crate::l3_v2::L3SchemaBody;
    let mut schemas = BTreeMap::new();
    for (qname, decl) in configs {
        let body = match &decl.body {
            ast::ConfigBody::Record(fields) => {
                let mut map = BTreeMap::new();
                for f in fields {
                    map.insert(f.name.clone(), contract(&f.ty)?);
                }
                L3SchemaBody::Record(map)
            }
            ast::ConfigBody::Sum(variants) => {
                let mut vars = Vec::new();
                for v in variants {
                    let mut payloads = Vec::new();
                    for p in &v.params {
                        payloads.push(contract(p)?);
                    }
                    vars.push((v.name.clone(), payloads));
                }
                L3SchemaBody::Sum(vars)
            }
        };
        let schema = L3Schema {
            name: qname.to_string(),
            body,
        };
        schemas.insert(qname.to_string(), schema.clone());
    }
    // Bare schema aliases are a convenience for direct callers, never a
    // first-wins nominal identity when packages reuse a local config name.
    for (qname, decl) in configs {
        if configs.keys().filter(|q| q.name == qname.name).count() == 1 {
            schemas.insert(decl.name.clone(), schemas[&qname.to_string()].clone());
        }
    }
    Ok(Arc::new(schemas))
}

/// Build constructor resolution tables from nominal schemas.
/// Returns (nullary_map, variants_of_map, ambiguous_ctors_map).
pub type ConstructorTables = (
    BTreeMap<String, String>,
    BTreeMap<String, String>,
    BTreeMap<String, Vec<String>>,
);

pub fn build_constructor_tables(schemas: &BTreeMap<String, L3Schema>) -> ConstructorTables {
    use crate::l3_v2::L3SchemaBody;
    let mut names: BTreeMap<String, BTreeSet<(String, String, bool)>> = BTreeMap::new();
    for schema in schemas.values() {
        if let L3SchemaBody::Sum(variants) = &schema.body {
            for (variant, payloads) in variants {
                let full = format!("{}::{variant}", schema.name);
                let mut aliases = vec![variant.clone(), full.clone()];
                if let Some((module, local)) = schema.name.rsplit_once("::") {
                    aliases.push(format!("{local}::{variant}"));
                    aliases.push(format!("{module}::{variant}"));
                }
                for alias in aliases {
                    names.entry(alias).or_default().insert((
                        schema.name.clone(),
                        full.clone(),
                        payloads.is_empty(),
                    ));
                }
            }
        }
    }
    let mut nullary = BTreeMap::new();
    let mut variants_of = BTreeMap::new();
    let mut ambiguous = BTreeMap::new();
    for (name, candidates) in names {
        if candidates.len() > 1 {
            ambiguous.insert(
                name,
                candidates.into_iter().map(|(_, full, _)| full).collect(),
            );
        } else if let Some((nominal, _, is_nullary)) = candidates.into_iter().next() {
            if is_nullary {
                nullary.insert(name, nominal);
            } else {
                variants_of.insert(name, nominal);
            }
        }
    }
    (nullary, variants_of, ambiguous)
}

/// Immutable compiled program environment containing compiled helper functions,
/// arities, and linked nominal schemas.
/// Built once per linked program and shared via `Arc`.
#[derive(Clone, Debug)]
pub struct CompiledProgramEnv {
    pub arities: Arc<BTreeMap<String, usize>>,
    pub functions: Arc<BTreeMap<String, L3FunctionDef>>,
    pub schemas: Arc<BTreeMap<String, L3Schema>>,
    pub nullary: Arc<BTreeMap<String, String>>,
    pub variants_of: Arc<BTreeMap<String, String>>,
    pub ambiguous_ctors: Arc<BTreeMap<String, Vec<String>>>,
}

impl CompiledProgramEnv {
    pub fn new(
        helpers: &BTreeMap<String, ast::Callable>,
        schemas: Arc<BTreeMap<String, L3Schema>>,
    ) -> Result<Self, String> {
        let arities: BTreeMap<String, usize> = helpers
            .iter()
            .map(|(name, f)| (name.clone(), f.params.len()))
            .collect();
        let arities_arc = Arc::new(arities);
        let (nullary, variants_of, ambiguous_ctors) = build_constructor_tables(&schemas);
        let nullary_arc = Arc::new(nullary);
        let variants_of_arc = Arc::new(variants_of);
        let ambiguous_ctors_arc = Arc::new(ambiguous_ctors);

        let empty_set = BTreeSet::new();
        let nullary_ref = nullary_arc.clone();
        let variants_ref = variants_of_arc.clone();
        let ambig_ref = ambiguous_ctors_arc.clone();
        let arities_ref = arities_arc.clone();
        let lower = move |expr: &ast::Expr, names: &BTreeSet<String>| {
            let normalized = normalize_world_and(expr);
            l3_v2::lower_expr_v2_with_ambiguous(
                &normalized,
                names,
                names,
                &empty_set,
                &nullary_ref,
                &variants_ref,
                &arities_ref,
                false,
                true,
                &ambig_ref,
            )
            .map_err(|e| format!("world expression lowering: {e}"))
        };
        let mut functions = BTreeMap::new();
        for (name, f) in helpers {
            HELPER_COMPILATION_COUNT.fetch_add(1, Ordering::SeqCst);
            let names = f.params.iter().map(|p| p.name.clone()).collect();
            let params = f
                .params
                .iter()
                .map(|p| Ok((p.name.clone(), p.ty.as_ref().map(contract).transpose()?)))
                .collect::<Result<Vec<_>, String>>()?;
            functions.insert(
                name.clone(),
                L3FunctionDef {
                    name: name.clone(),
                    params,
                    ret_contract: f.ret.as_ref().map(contract).transpose()?,
                    schemas: schemas.clone(),
                    body: lower(&f.body, &names)?,
                },
            );
        }
        Ok(Self {
            arities: arities_arc,
            functions: Arc::new(functions),
            schemas,
            nullary: nullary_arc,
            variants_of: variants_of_arc,
            ambiguous_ctors: ambiguous_ctors_arc,
        })
    }

    /// Construct the compiled environment directly from a linked program.
    pub fn from_linked_program(
        program: &crate::module_graph::LinkedProgram,
    ) -> Result<Arc<Self>, String> {
        let schemas = build_nominal_schemas(&program.available_configs)?;
        let mut helpers = BTreeMap::new();
        for (qname, callable) in &program.functions {
            helpers.insert(qname.to_string(), callable.clone());
        }
        let env = Self::new(&helpers, schemas)?;
        Ok(Arc::new(env))
    }

    /// Compile an individual expression in this environment.
    /// Does not recompile helpers!
    pub fn compile_expr(
        &self,
        expr: &ast::Expr,
        bindings: &BTreeSet<String>,
    ) -> Result<CompiledWorldExpr, String> {
        EXPR_COMPILATION_COUNT.fetch_add(1, Ordering::SeqCst);
        let empty_set = BTreeSet::new();
        let normalized = normalize_world_and(expr);
        let lowered = l3_v2::lower_expr_v2_with_ambiguous(
            &normalized,
            bindings,
            bindings,
            &empty_set,
            &self.nullary,
            &self.variants_of,
            &self.arities,
            false,
            true,
            &self.ambiguous_ctors,
        )
        .map_err(|e| format!("world expression lowering: {e}"))?;

        Ok(CompiledWorldExpr {
            expr: lowered,
            functions: self.functions.clone(),
            schemas: self.schemas.clone(),
        })
    }
}

impl CompiledWorldExpr {
    pub fn new(
        expr: &ast::Expr,
        helpers: &BTreeMap<String, ast::Callable>,
        bindings: &BTreeSet<String>,
    ) -> Result<Self, String> {
        Self::with_schemas(expr, helpers, bindings, Arc::new(BTreeMap::new()))
    }

    /// Supply linked nominal schemas when helpers accept or return records.
    pub fn with_schemas(
        expr: &ast::Expr,
        helpers: &BTreeMap<String, ast::Callable>,
        bindings: &BTreeSet<String>,
        schemas: Arc<BTreeMap<String, L3Schema>>,
    ) -> Result<Self, String> {
        let env = CompiledProgramEnv::new(helpers, schemas)?;
        env.compile_expr(expr, bindings)
    }

    /// Compile an expression using a precompiled program environment.
    pub fn compile(
        env: &CompiledProgramEnv,
        expr: &ast::Expr,
        bindings: &BTreeSet<String>,
    ) -> Result<Self, String> {
        env.compile_expr(expr, bindings)
    }

    pub fn eval_with_budget(
        &self,
        bindings: &BTreeMap<String, L3ValueV2>,
        max_steps: Option<usize>,
    ) -> Result<(L3ValueV2, usize), String> {
        let mut env = EvalEnv::new()
            .with_functions(self.functions.clone())
            .with_schemas(self.schemas.clone())
            .with_checked_constructors();
        for (name, value) in bindings {
            env = env.with_let(name, value.clone());
        }
        let (value, steps) = l3_v2::eval_with_budget(&self.expr, &env, max_steps)
            .map_err(|fault| format!("Unknown(EvaluationFault): {fault}"))?;
        let mut remaining = crate::input::MAX_INPUT_VALUE_NODES;
        validate_value_bounds(&value, 0, &mut remaining)?;
        validate_world_result(&value, &self.schemas)?;
        Ok((value, steps))
    }

    pub fn eval(&self, bindings: &BTreeMap<String, L3ValueV2>) -> Result<L3ValueV2, String> {
        self.eval_with_budget(bindings, None).map(|(val, _)| val)
    }
}

// Apply the brix.input@2 value budget to the complete result, before it can
// enter persistent state. Lists preserve expression order and duplicates.
fn validate_value_bounds(
    value: &L3ValueV2,
    depth: usize,
    remaining: &mut usize,
) -> Result<(), String> {
    use crate::input::{MAX_INPUT_CONTAINER_WIDTH, MAX_INPUT_VALUE_DEPTH};
    if depth >= MAX_INPUT_VALUE_DEPTH || *remaining == 0 {
        return Err("world value (including list values) exceeds depth or node limit".into());
    }
    *remaining -= 1;
    let children: Vec<&L3ValueV2> = match value {
        L3ValueV2::List(items) => items.iter().collect(),
        L3ValueV2::Record { fields, .. } => fields.iter().map(|(_, v)| v).collect(),
        L3ValueV2::Ctor { args, .. } => args.iter().collect(),
        _ => return Ok(()),
    };
    if children.len() > MAX_INPUT_CONTAINER_WIDTH || children.len() > *remaining {
        let kind = if matches!(value, L3ValueV2::List(_)) {
            "list"
        } else {
            "structured"
        };
        return Err(format!(
            "{kind} value exceeds container width ({MAX_INPUT_CONTAINER_WIDTH}) or node limit"
        ));
    }
    for child in children {
        validate_value_bounds(child, depth + 1, remaining)?;
    }
    Ok(())
}

/// Validate every structured result, including structured fields projected in
/// anonymous relation tuples, before it enters settlement or persistent state.
fn validate_world_result(
    value: &L3ValueV2,
    schemas: &BTreeMap<String, L3Schema>,
) -> Result<(), String> {
    match value {
        L3ValueV2::List(items) => {
            for item in items.iter() {
                validate_world_result(item, schemas)?;
            }
            Ok(())
        }
        L3ValueV2::Ctor { nominal_sum, .. } => l3_v2::validate_l3_value(
            value,
            &L3SchemaType::Named(nominal_sum.clone()),
            schemas,
            "<result>",
        ),
        L3ValueV2::Record { nominal_config, .. } if !nominal_config.is_empty() => {
            l3_v2::validate_l3_value(
                value,
                &L3SchemaType::Named(nominal_config.clone()),
                schemas,
                "<result>",
            )
        }
        L3ValueV2::Record { fields, .. } => {
            for (_, field) in fields {
                validate_world_result(field, schemas)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
