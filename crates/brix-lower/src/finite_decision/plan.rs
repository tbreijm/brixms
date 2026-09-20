//! Finite-decision alpha plan lowering and canonical program identity (ADR-0030, ADR-0031, ADR-0032).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use brix_canon::{CanonWriter, Canonical, Digest, Domain};
use brix_syntax::ast;

use crate::l3_v2::{
    check_exhaustive_expr, lower_expr_v2, L3ConfigBodyV2, L3ConfigDeclV2, L3ExprV2, L3PatternV2,
    L3V2LowerError, L3ValueType,
};

/// The profile marker for finite-decision alpha (ADR-0030 ⟨D-PROFILE⟩).
pub const FINITE_DECISION_PROFILE: &str = "brix.l3.finite-decision@1";

/// Maximum number of functions declared in a finite-decision module.
pub const MAX_FUNCTION_COUNT: usize = 256;

/// Maximum number of parameters declared by a single function.
pub const MAX_FUNCTION_PARAMS: usize = 32;

/// Maximum nesting depth of expressions.
pub const MAX_EXPR_DEPTH: usize = 128;

/// Maximum number of AST expression nodes per expression.
pub const MAX_EXPR_NODES: usize = 4096;

/// A declared external input in a finite-decision plan (ADR-0031).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FiniteDecisionInput {
    pub ordinal: u64,
    pub name: String,
    pub ty: L3ValueType,
}

/// A declared function parameter or return contract (ADR-0032).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FiniteDecisionContract {
    pub ty: L3ValueType,
    pub grade: Option<ast::Grade>,
}

/// A parameter in a finite-decision function definition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FiniteDecisionFnParam {
    pub name: String,
    pub contract: Option<FiniteDecisionContract>,
}

/// A normalized pure function definition in a finite-decision plan (ADR-0032).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FiniteDecisionFunction {
    pub ordinal: u64,
    pub name: String,
    pub params: Vec<FiniteDecisionFnParam>,
    pub ret_contract: Option<FiniteDecisionContract>,
    pub body: L3ExprV2,
}

/// A normalized rule in a finite-decision plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FiniteDecisionRule {
    pub ordinal: u64,
    pub name: String,
    pub body: L3ExprV2,
    pub depends_on: Vec<String>,
}

/// A candidate proposal in a finite-decision plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FiniteDecisionProposal {
    pub ordinal: u64,
    pub name: String,
    pub deps: Vec<String>,
    pub priority: u64,
    pub guard: L3ExprV2,
    pub value: L3ExprV2,
}

/// The single commit block in a finite-decision plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FiniteDecisionCommit {
    pub name: String,
    pub candidates: Vec<String>,
}

/// A lowered finite-decision plan (ADR-0030, ADR-0031, ADR-0032).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FiniteDecisionPlan {
    pub profile: String,
    pub configs: Vec<L3ConfigDeclV2>,
    pub inputs: Vec<FiniteDecisionInput>,
    pub functions: Vec<FiniteDecisionFunction>,
    pub lets: Vec<(String, L3ExprV2)>,
    pub rules: Vec<FiniteDecisionRule>,
    pub proposals: Vec<FiniteDecisionProposal>,
    pub commit: FiniteDecisionCommit,
    pub shows: Vec<L3ExprV2>,
}

impl FiniteDecisionPlan {
    /// Look up an input by declared name.
    pub fn find_input(&self, name: &str) -> Option<&FiniteDecisionInput> {
        self.inputs.iter().find(|i| i.name == name)
    }

    /// Look up a function by declared name.
    pub fn find_function(&self, name: &str) -> Option<&FiniteDecisionFunction> {
        self.functions.iter().find(|f| f.name == name)
    }

    /// Look up a proposal by candidate name.
    pub fn find_proposal(&self, name: &str) -> Option<&FiniteDecisionProposal> {
        self.proposals.iter().find(|p| p.name == name)
    }
}

/// Errors occurring during finite-decision lowering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FiniteDecisionLowerError {
    ProfileMismatch {
        expected: String,
        found: String,
    },
    ItemNotAllowed(String),
    NoCommit,
    MultipleCommits(usize),
    EmptyCommit(String),
    DuplicateProposalName(String),
    DuplicateInputName(String),
    DuplicateFunctionName(String),
    DuplicateItemName(String),
    DuplicateFunctionParameter {
        func: String,
        param: String,
    },
    TooManyFunctions {
        limit: usize,
        count: usize,
    },
    TooManyFunctionParams {
        func: String,
        limit: usize,
        count: usize,
    },
    FunctionCycle {
        func: String,
        cycle: Vec<String>,
    },
    FunctionArityMismatch {
        func: String,
        expected: usize,
        found: usize,
    },
    RuleFactReadInFunction {
        func: String,
        fact: String,
    },
    InputReadInFunction {
        func: String,
        input: String,
    },
    GlobalLetReadInFunction {
        func: String,
        binding: String,
    },
    UnsupportedContractType {
        ty: String,
        detail: String,
    },
    UnsupportedContractGrade {
        grade: ast::Grade,
        detail: String,
    },
    UnknownContractType {
        name: String,
    },
    ExpressionDepthExceeded {
        limit: usize,
    },
    ExpressionNodeLimitExceeded {
        limit: usize,
    },
    InputNameTooLong {
        limit: usize,
    },
    UnsupportedInputType {
        name: String,
        ty: String,
    },
    UnknownCandidateInCommit {
        commit: String,
        candidate: String,
    },
    DuplicateCandidateInCommit {
        commit: String,
        candidate: String,
    },
    UndeclaredDependency {
        proposal: String,
        dep: String,
    },
    ForwardOrSelfDependency {
        proposal: String,
        dep: String,
    },
    UndeclaredFactRead {
        proposal: String,
        fact: String,
    },
    FunctionConstructorCollision {
        func: String,
        constructor: String,
    },
    DuplicateMatchBinder(String),
    RuleDependencyError(L3V2LowerError),
    ExprError(L3V2LowerError),
    UnresolvedImport(String),
}

impl fmt::Display for FiniteDecisionLowerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProfileMismatch { expected, found } => {
                write!(
                    f,
                    "profile mismatch: expected '{expected}', found '{found}'"
                )
            }
            Self::ItemNotAllowed(item) => write!(f, "item not allowed in finite-decision: {item}"),
            Self::NoCommit => write!(
                f,
                "finite-decision module must contain exactly one commit declaration, found none"
            ),
            Self::MultipleCommits(count) => write!(
                f,
                "finite-decision module must contain exactly one commit declaration, found {count}"
            ),
            Self::EmptyCommit(name) => {
                write!(f, "commit declaration '{name}' has no candidate members")
            }
            Self::DuplicateProposalName(name) => write!(f, "duplicate proposal name: '{name}'"),
            Self::DuplicateInputName(name) => write!(f, "duplicate input name: '{name}'"),
            Self::DuplicateFunctionName(name) => write!(f, "duplicate function name: '{name}'"),
            Self::DuplicateItemName(name) => write!(f, "duplicate top-level item name: '{name}'"),
            Self::DuplicateFunctionParameter { func, param } => {
                write!(f, "duplicate parameter '{param}' in function '{func}'")
            }
            Self::TooManyFunctions { limit, count } => {
                write!(f, "function count exceeds limit ({count} > {limit})")
            }
            Self::TooManyFunctionParams { func, limit, count } => {
                write!(
                    f,
                    "parameter count in function '{func}' exceeds limit ({count} > {limit})"
                )
            }
            Self::FunctionCycle { func, cycle } => {
                write!(
                    f,
                    "function cycle detected involving '{func}': {}",
                    cycle.join(" -> ")
                )
            }
            Self::FunctionArityMismatch {
                func,
                expected,
                found,
            } => {
                write!(
                    f,
                    "function '{func}' expected {expected} argument(s), found {found}"
                )
            }
            Self::RuleFactReadInFunction { func, fact } => {
                write!(
                    f,
                    "function '{func}' reads rule fact '{fact}'; pure helpers cannot access rule facts"
                )
            }
            Self::InputReadInFunction { func, input } => {
                write!(
                    f,
                    "function '{func}' reads input '{input}'; pure helpers cannot access external inputs directly"
                )
            }
            Self::GlobalLetReadInFunction { func, binding } => {
                write!(
                    f,
                    "function '{func}' reads global binding '{binding}'; pure helpers must take all data as parameters"
                )
            }
            Self::UnsupportedContractType { ty, detail } => {
                write!(f, "unsupported contract type '{ty}': {detail}")
            }
            Self::UnsupportedContractGrade { grade, detail } => {
                write!(f, "unsupported contract grade '{grade:?}': {detail}")
            }
            Self::UnknownContractType { name } => {
                write!(f, "unknown contract type '{name}'")
            }
            Self::ExpressionDepthExceeded { limit } => {
                write!(f, "expression nesting depth exceeds limit ({limit})")
            }
            Self::ExpressionNodeLimitExceeded { limit } => {
                write!(f, "expression node count exceeds limit ({limit})")
            }
            Self::InputNameTooLong { limit } => {
                write!(f, "input name exceeds length limit ({limit} bytes)")
            }
            Self::UnsupportedInputType { name, ty } => {
                write!(f, "unsupported input type for '{name}': '{ty}' (only Int, Bool, Str are supported)")
            }
            Self::UnknownCandidateInCommit { commit, candidate } => {
                write!(
                    f,
                    "commit '{commit}' references undeclared proposal '{candidate}'"
                )
            }
            Self::DuplicateCandidateInCommit { commit, candidate } => {
                write!(
                    f,
                    "commit '{commit}' contains duplicate candidate '{candidate}'"
                )
            }
            Self::UndeclaredDependency { proposal, dep } => {
                write!(f, "proposal '{proposal}' declares dependency '{dep}' which is not an earlier rule")
            }
            Self::ForwardOrSelfDependency { proposal, dep } => {
                write!(
                    f,
                    "proposal '{proposal}' has forward or self-dependency on '{dep}'"
                )
            }
            Self::UndeclaredFactRead { proposal, fact } => {
                write!(f, "proposal '{proposal}' reads fact '{fact}' without declaring it as a dependency")
            }
            Self::FunctionConstructorCollision { func, constructor } => {
                write!(
                    f,
                    "function '{func}' collides with declared constructor '{constructor}'"
                )
            }
            Self::DuplicateMatchBinder(binder) => {
                write!(f, "duplicate binder '{binder}' in match pattern")
            }
            Self::RuleDependencyError(err) => write!(f, "rule dependency error: {err:?}"),
            Self::ExprError(err) => write!(f, "expression lowering error: {err:?}"),
            Self::UnresolvedImport(path) => write!(f, "unresolved import: {path}"),
        }
    }
}

impl std::error::Error for FiniteDecisionLowerError {}

/// Helper function checking that expression nesting depth and node limits are respected.
fn check_expr_bounds(
    e: &ast::Expr,
    depth: usize,
    node_count: &mut usize,
) -> Result<(), FiniteDecisionLowerError> {
    if depth > MAX_EXPR_DEPTH {
        return Err(FiniteDecisionLowerError::ExpressionDepthExceeded {
            limit: MAX_EXPR_DEPTH,
        });
    }
    *node_count += 1;
    if *node_count > MAX_EXPR_NODES {
        return Err(FiniteDecisionLowerError::ExpressionNodeLimitExceeded {
            limit: MAX_EXPR_NODES,
        });
    }
    match e {
        ast::Expr::Num(_) | ast::Expr::Str(_) | ast::Expr::Bool(_) | ast::Expr::Var(_) => Ok(()),
        ast::Expr::Field(base, _)
        | ast::Expr::Prove(base)
        | ast::Expr::Why(base)
        | ast::Expr::Audit(base) => check_expr_bounds(base, depth + 1, node_count),
        ast::Expr::Record { fields, .. } => {
            for (_, val) in fields {
                check_expr_bounds(val, depth + 1, node_count)?;
            }
            Ok(())
        }
        ast::Expr::Call { args, .. } => {
            for a in args {
                check_expr_bounds(a, depth + 1, node_count)?;
            }
            Ok(())
        }
        ast::Expr::Bin { lhs, rhs, .. } => {
            check_expr_bounds(lhs, depth + 1, node_count)?;
            check_expr_bounds(rhs, depth + 1, node_count)
        }
        ast::Expr::Match {
            scrutinee, arms, ..
        } => {
            check_expr_bounds(scrutinee, depth + 1, node_count)?;
            for arm in arms {
                check_expr_bounds(&arm.body, depth + 1, node_count)?;
            }
            Ok(())
        }
    }
}

/// Parse and validate a contract type and optional grade annotation (ADR-0032).
fn parse_contract(
    ty: &ast::Ty,
    declared_sum_configs: &BTreeSet<String>,
    declared_record_configs: &BTreeSet<String>,
) -> Result<FiniteDecisionContract, FiniteDecisionLowerError> {
    let (inner_ty, grade) = match ty {
        ast::Ty::Graded(inner, g) => match g {
            ast::Grade::Derived => (inner.as_ref(), Some(ast::Grade::Derived)),
            ast::Grade::Proven | ast::Grade::Audited => {
                return Err(FiniteDecisionLowerError::UnsupportedContractGrade {
                    grade: *g,
                    detail: "helpers cannot declare or elevate to @Proven or @Audited".to_string(),
                });
            }
        },
        other => (other, None),
    };

    let value_type = match inner_ty {
        ast::Ty::Named(name) => match name.as_str() {
            "Int" => L3ValueType::Int,
            "Bool" => L3ValueType::Bool,
            "Str" => L3ValueType::Str,
            "Float" => {
                return Err(FiniteDecisionLowerError::UnsupportedContractType {
                    ty: "Float".to_string(),
                    detail: "Float is not supported in finite-decision".to_string(),
                });
            }
            n if declared_sum_configs.contains(n) || declared_record_configs.contains(n) => {
                return Err(FiniteDecisionLowerError::UnsupportedContractType {
                    ty: n.to_string(),
                    detail: "sum and record contract annotations are not supported in finite-decision alpha (only Int, Bool, Str are supported)".to_string(),
                });
            }
            n => {
                return Err(FiniteDecisionLowerError::UnknownContractType {
                    name: n.to_string(),
                });
            }
        },
        ast::Ty::Record(_) => {
            return Err(FiniteDecisionLowerError::UnsupportedContractType {
                ty: "anonymous record".to_string(),
                detail: "composite types are not supported in function contracts in this slice"
                    .to_string(),
            });
        }
        ast::Ty::App(name, _) => {
            return Err(FiniteDecisionLowerError::UnsupportedContractType {
                ty: format!("{name}<...>"),
                detail: "generic/parameterized types are not supported in finite-decision"
                    .to_string(),
            });
        }
        ast::Ty::Graded(_, _) => {
            return Err(FiniteDecisionLowerError::UnsupportedContractType {
                ty: "nested graded type".to_string(),
                detail: "nested grade annotations are not supported".to_string(),
            });
        }
    };

    Ok(FiniteDecisionContract {
        ty: value_type,
        grade,
    })
}

/// Collect function calls from an L3 expression for cycle detection.
fn collect_function_calls(e: &L3ExprV2, calls: &mut BTreeSet<String>) {
    match e {
        L3ExprV2::Call { func, args } => {
            calls.insert(func.clone());
            for a in args {
                collect_function_calls(a, calls);
            }
        }
        L3ExprV2::Ctor { args, .. } => {
            for a in args {
                collect_function_calls(a, calls);
            }
        }
        L3ExprV2::Record { fields, .. } => {
            for (_, v) in fields {
                collect_function_calls(v, calls);
            }
        }
        L3ExprV2::Field(base, _) => collect_function_calls(base, calls),
        L3ExprV2::Arith(_, a, b) | L3ExprV2::Cmp(_, a, b) => {
            collect_function_calls(a, calls);
            collect_function_calls(b, calls);
        }
        L3ExprV2::Match { scrutinee, arms } => {
            collect_function_calls(scrutinee, calls);
            for (_, body) in arms {
                collect_function_calls(body, calls);
            }
        }
        L3ExprV2::Int(_)
        | L3ExprV2::Str(_)
        | L3ExprV2::Bool(_)
        | L3ExprV2::LetRef(_)
        | L3ExprV2::RuleFact(_)
        | L3ExprV2::NullaryVariant { .. } => {}
    }
}

enum DfsMark {
    Visiting,
    Visited,
}

/// Detect direct and mutual function recursion cycles across all declared functions.
fn detect_cycles(
    call_graph: &BTreeMap<String, BTreeSet<String>>,
) -> Result<(), FiniteDecisionLowerError> {
    let mut marks: BTreeMap<&str, DfsMark> = BTreeMap::new();
    let mut path: Vec<&str> = Vec::new();

    for start in call_graph.keys() {
        if !marks.contains_key(start.as_str()) {
            dfs_cycle(start.as_str(), call_graph, &mut marks, &mut path)?;
        }
    }
    Ok(())
}

fn dfs_cycle<'a>(
    node: &'a str,
    call_graph: &'a BTreeMap<String, BTreeSet<String>>,
    marks: &mut BTreeMap<&'a str, DfsMark>,
    path: &mut Vec<&'a str>,
) -> Result<(), FiniteDecisionLowerError> {
    marks.insert(node, DfsMark::Visiting);
    path.push(node);

    if let Some(neighbors) = call_graph.get(node) {
        for next in neighbors {
            match marks.get(next.as_str()) {
                Some(DfsMark::Visiting) => {
                    let cycle_start = path.iter().position(|&x| x == next.as_str()).unwrap_or(0);
                    let mut cycle: Vec<String> =
                        path[cycle_start..].iter().map(|s| s.to_string()).collect();
                    cycle.push(next.clone());
                    return Err(FiniteDecisionLowerError::FunctionCycle {
                        func: node.to_string(),
                        cycle,
                    });
                }
                Some(DfsMark::Visited) => {}
                None => {
                    dfs_cycle(next.as_str(), call_graph, marks, path)?;
                }
            }
        }
    }

    path.pop();
    marks.insert(node, DfsMark::Visited);
    Ok(())
}

/// Lower a parsed syntax module into a validated [`FiniteDecisionPlan`].
pub fn lower_finite_decision_plan(
    module: &ast::Module,
    profile: &str,
) -> Result<FiniteDecisionPlan, FiniteDecisionLowerError> {
    if profile != FINITE_DECISION_PROFILE {
        return Err(FiniteDecisionLowerError::ProfileMismatch {
            expected: FINITE_DECISION_PROFILE.to_string(),
            found: profile.to_string(),
        });
    }

    // Pass 0: check item admissibility.
    let mut commit_items: Vec<&ast::CommitDecl> = Vec::new();
    let mut fn_count = 0;
    for item in &module.items {
        match item {
            ast::Item::Config(_)
            | ast::Item::Let(_)
            | ast::Item::Rule(_)
            | ast::Item::Propose(_)
            | ast::Item::Show(_)
            | ast::Item::Input(_) => {}
            ast::Item::Fn(_) => {
                fn_count += 1;
            }
            ast::Item::Commit(c) => {
                commit_items.push(c);
            }
            ast::Item::Regime(r) => {
                return Err(FiniteDecisionLowerError::ItemNotAllowed(format!(
                    "regime {}",
                    r.name
                )));
            }
            ast::Item::Witness { name, .. } => {
                return Err(FiniteDecisionLowerError::ItemNotAllowed(format!(
                    "witness {name}"
                )));
            }
            ast::Item::Use(path) => {
                return Err(FiniteDecisionLowerError::UnresolvedImport(path.clone()));
            }
        }
    }

    let has_functions = fn_count > 0;

    if fn_count > MAX_FUNCTION_COUNT {
        return Err(FiniteDecisionLowerError::TooManyFunctions {
            limit: MAX_FUNCTION_COUNT,
            count: fn_count,
        });
    }

    // Exactly one nonempty commit.
    if commit_items.is_empty() {
        return Err(FiniteDecisionLowerError::NoCommit);
    }
    if commit_items.len() > 1 {
        return Err(FiniteDecisionLowerError::MultipleCommits(
            commit_items.len(),
        ));
    }
    let commit_decl = commit_items[0];
    if commit_decl.candidates.is_empty() {
        return Err(FiniteDecisionLowerError::EmptyCommit(
            commit_decl.name.clone(),
        ));
    }
    let mut commit_candidates_seen = BTreeSet::new();
    for cand in &commit_decl.candidates {
        if !commit_candidates_seen.insert(cand.clone()) {
            return Err(FiniteDecisionLowerError::DuplicateCandidateInCommit {
                commit: commit_decl.name.clone(),
                candidate: cand.clone(),
            });
        }
    }

    // Scopes and discovery.
    let mut all_input_names: BTreeSet<String> = BTreeSet::new();
    let mut all_rule_names: BTreeSet<String> = BTreeSet::new();
    let mut all_let_names: BTreeSet<String> = BTreeSet::new();
    let mut sum_configs: BTreeSet<String> = BTreeSet::new();
    let mut record_configs: BTreeSet<String> = BTreeSet::new();
    let mut variants_of: BTreeMap<String, String> = BTreeMap::new();
    let mut nullary: BTreeMap<String, String> = BTreeMap::new();
    let mut sum_of_variant: BTreeMap<String, String> = BTreeMap::new();
    let mut variants_of_sum: BTreeMap<String, Vec<String>> = BTreeMap::new();

    // Pre-scan configs and top-level item names.
    for item in &module.items {
        match item {
            ast::Item::Input(inp) => {
                all_input_names.insert(inp.name.clone());
            }
            ast::Item::Rule(r) => {
                all_rule_names.insert(r.name.clone());
            }
            ast::Item::Let(l) => {
                all_let_names.insert(l.name.clone());
            }
            ast::Item::Config(c) => match &c.body {
                ast::ConfigBody::Sum(variants) => {
                    sum_configs.insert(c.name.clone());
                    let mut var_names = Vec::new();
                    for v in variants {
                        variants_of.insert(v.name.clone(), c.name.clone());
                        sum_of_variant.insert(v.name.clone(), c.name.clone());
                        var_names.push(v.name.clone());
                        if v.params.is_empty() {
                            nullary.insert(v.name.clone(), c.name.clone());
                        }
                    }
                    variants_of_sum.insert(c.name.clone(), var_names);
                }
                ast::ConfigBody::Record(_) => {
                    record_configs.insert(c.name.clone());
                }
            },
            _ => {}
        }
    }

    // Builtin Bool sum for exhaustiveness checking.
    variants_of_sum.insert("Bool".to_string(), vec!["false".into(), "true".into()]);
    sum_of_variant.insert("true".to_string(), "Bool".to_string());
    sum_of_variant.insert("false".to_string(), "Bool".to_string());

    // Pre-scan functions: validate parameters, contracts, arities, and uniqueness.
    let mut fn_names: BTreeSet<String> = BTreeSet::new();
    let mut function_arities: BTreeMap<String, usize> = BTreeMap::new();
    let mut parsed_signatures: BTreeMap<
        String,
        (Vec<FiniteDecisionFnParam>, Option<FiniteDecisionContract>),
    > = BTreeMap::new();

    for item in &module.items {
        if let ast::Item::Fn(f) = item {
            if variants_of.contains_key(&f.name) || f.name == "true" || f.name == "false" {
                return Err(FiniteDecisionLowerError::FunctionConstructorCollision {
                    func: f.name.clone(),
                    constructor: f.name.clone(),
                });
            }
            if !fn_names.insert(f.name.clone()) {
                return Err(FiniteDecisionLowerError::DuplicateFunctionName(
                    f.name.clone(),
                ));
            }
            if f.params.len() > MAX_FUNCTION_PARAMS {
                return Err(FiniteDecisionLowerError::TooManyFunctionParams {
                    func: f.name.clone(),
                    limit: MAX_FUNCTION_PARAMS,
                    count: f.params.len(),
                });
            }
            let mut param_names = BTreeSet::new();
            let mut params = Vec::with_capacity(f.params.len());
            for p in &f.params {
                if !param_names.insert(p.name.clone()) {
                    return Err(FiniteDecisionLowerError::DuplicateFunctionParameter {
                        func: f.name.clone(),
                        param: p.name.clone(),
                    });
                }
                let contract = if let Some(ty) = &p.ty {
                    Some(parse_contract(ty, &sum_configs, &record_configs)?)
                } else {
                    None
                };
                params.push(FiniteDecisionFnParam {
                    name: p.name.clone(),
                    contract,
                });
            }

            let ret_contract = if let Some(ret_ty) = &f.ret {
                Some(parse_contract(ret_ty, &sum_configs, &record_configs)?)
            } else {
                None
            };

            function_arities.insert(f.name.clone(), f.params.len());
            parsed_signatures.insert(f.name.clone(), (params, ret_contract));
        }
    }

    // For function-free modules, restore source-order config visibility to preserve
    // backward compatibility with alpha.2 and alpha.3.
    if !has_functions {
        variants_of.clear();
        nullary.clear();
    }

    // Lowering state.
    let mut input_names: BTreeSet<String> = BTreeSet::new();
    let mut let_names: BTreeSet<String> = BTreeSet::new();
    let mut rule_names: BTreeSet<String> = BTreeSet::new();
    let mut proposal_names: BTreeSet<String> = BTreeSet::new();
    let mut all_top_level_names: BTreeSet<String> = BTreeSet::new();

    let mut configs = Vec::new();
    let mut inputs = Vec::new();
    let mut functions = Vec::new();
    let mut lets = Vec::new();
    let mut rules = Vec::new();
    let mut proposals = Vec::new();
    let mut shows = Vec::new();

    let map_lower_err = |e: L3V2LowerError| match e {
        L3V2LowerError::FunctionArityMismatch {
            func,
            expected,
            found,
        } => FiniteDecisionLowerError::FunctionArityMismatch {
            func,
            expected,
            found,
        },
        L3V2LowerError::DuplicateMatchBinder(binder) => {
            FiniteDecisionLowerError::DuplicateMatchBinder(binder)
        }
        other => FiniteDecisionLowerError::ExprError(other),
    };

    for item in &module.items {
        match item {
            ast::Item::Input(inp) => {
                if inp.name.len() > crate::input::MAX_INPUT_NAME_BYTES {
                    return Err(FiniteDecisionLowerError::InputNameTooLong {
                        limit: crate::input::MAX_INPUT_NAME_BYTES,
                    });
                }
                if !input_names.insert(inp.name.clone()) {
                    return Err(FiniteDecisionLowerError::DuplicateInputName(
                        inp.name.clone(),
                    ));
                }
                if !all_top_level_names.insert(inp.name.clone()) {
                    return Err(FiniteDecisionLowerError::DuplicateItemName(
                        inp.name.clone(),
                    ));
                }
                let ty = match &inp.ty {
                    ast::Ty::Named(n) => match n.as_str() {
                        "Int" => L3ValueType::Int,
                        "Bool" => L3ValueType::Bool,
                        "Str" => L3ValueType::Str,
                        other => {
                            return Err(FiniteDecisionLowerError::UnsupportedInputType {
                                name: inp.name.clone(),
                                ty: other.to_string(),
                            });
                        }
                    },
                    other => {
                        return Err(FiniteDecisionLowerError::UnsupportedInputType {
                            name: inp.name.clone(),
                            ty: format!("{other:?}"),
                        });
                    }
                };
                inputs.push(FiniteDecisionInput {
                    ordinal: inputs.len() as u64,
                    name: inp.name.clone(),
                    ty,
                });
            }
            ast::Item::Config(c) => {
                if !all_top_level_names.insert(c.name.clone()) {
                    return Err(FiniteDecisionLowerError::DuplicateItemName(c.name.clone()));
                }
                if !has_functions {
                    if let ast::ConfigBody::Sum(variants) = &c.body {
                        for v in variants {
                            variants_of.insert(v.name.clone(), c.name.clone());
                            if v.params.is_empty() {
                                nullary.insert(v.name.clone(), c.name.clone());
                            }
                        }
                    }
                }
                let body = match &c.body {
                    ast::ConfigBody::Sum(variants) => L3ConfigBodyV2::Sum(
                        variants
                            .iter()
                            .map(|v| (v.name.clone(), v.params.len()))
                            .collect(),
                    ),
                    ast::ConfigBody::Record(fields) => {
                        L3ConfigBodyV2::Record(fields.iter().map(|f| f.name.clone()).collect())
                    }
                };
                configs.push(L3ConfigDeclV2 {
                    name: c.name.clone(),
                    body,
                });
            }
            ast::Item::Fn(f) => {
                if !all_top_level_names.insert(f.name.clone()) {
                    return Err(FiniteDecisionLowerError::DuplicateItemName(f.name.clone()));
                }
                let mut node_count = 0;
                check_expr_bounds(&f.body, 0, &mut node_count)?;

                let param_set: BTreeSet<String> = f.params.iter().map(|p| p.name.clone()).collect();
                let body = lower_expr_v2(
                    &f.body,
                    &param_set,
                    &param_set,
                    &BTreeSet::new(),
                    &nullary,
                    &variants_of,
                    &function_arities,
                    false,
                    true,
                )
                .map_err(|e| match e {
                    L3V2LowerError::UnresolvedReference(n) if all_input_names.contains(&n) => {
                        FiniteDecisionLowerError::InputReadInFunction {
                            func: f.name.clone(),
                            input: n,
                        }
                    }
                    L3V2LowerError::UnresolvedReference(n) if all_rule_names.contains(&n) => {
                        FiniteDecisionLowerError::RuleFactReadInFunction {
                            func: f.name.clone(),
                            fact: n,
                        }
                    }
                    L3V2LowerError::UnresolvedReference(n) if all_let_names.contains(&n) => {
                        FiniteDecisionLowerError::GlobalLetReadInFunction {
                            func: f.name.clone(),
                            binding: n,
                        }
                    }
                    L3V2LowerError::FunctionArityMismatch {
                        func,
                        expected,
                        found,
                    } => FiniteDecisionLowerError::FunctionArityMismatch {
                        func,
                        expected,
                        found,
                    },
                    L3V2LowerError::DuplicateMatchBinder(binder) => {
                        FiniteDecisionLowerError::DuplicateMatchBinder(binder)
                    }
                    other => FiniteDecisionLowerError::ExprError(other),
                })?;

                let (params, ret_contract) = parsed_signatures.remove(&f.name).unwrap();
                functions.push(FiniteDecisionFunction {
                    ordinal: functions.len() as u64,
                    name: f.name.clone(),
                    params,
                    ret_contract,
                    body,
                });
            }
            ast::Item::Let(l) => {
                if !all_top_level_names.insert(l.name.clone()) {
                    return Err(FiniteDecisionLowerError::DuplicateItemName(l.name.clone()));
                }
                if has_functions {
                    let mut node_count = 0;
                    check_expr_bounds(&l.value, 0, &mut node_count)?;
                }

                let mut visible_bindings = let_names.clone();
                visible_bindings.extend(input_names.iter().cloned());
                let value = lower_expr_v2(
                    &l.value,
                    &visible_bindings,
                    &BTreeSet::new(),
                    &BTreeSet::new(),
                    &nullary,
                    &variants_of,
                    &function_arities,
                    false,
                    has_functions,
                )
                .map_err(map_lower_err)?;

                if has_functions {
                    check_exhaustive_expr(&value, &sum_of_variant, &variants_of_sum)
                        .map_err(FiniteDecisionLowerError::ExprError)?;
                }

                let_names.insert(l.name.clone());
                lets.push((l.name.clone(), value));
            }
            ast::Item::Rule(r) => {
                if !all_top_level_names.insert(r.name.clone()) {
                    return Err(FiniteDecisionLowerError::DuplicateItemName(r.name.clone()));
                }
                if has_functions {
                    let mut node_count = 0;
                    check_expr_bounds(&r.body, 0, &mut node_count)?;
                }

                let mut depends_on: Vec<String> = Vec::new();
                for param in &r.params {
                    if param.name == r.name {
                        return Err(FiniteDecisionLowerError::RuleDependencyError(
                            L3V2LowerError::ForwardOrSelfDependency {
                                rule: r.name.clone(),
                                depends_on: param.name.clone(),
                            },
                        ));
                    }
                    if !rule_names.contains(&param.name) {
                        return Err(FiniteDecisionLowerError::RuleDependencyError(
                            L3V2LowerError::UndeclaredDependency {
                                rule: r.name.clone(),
                                param: param.name.clone(),
                            },
                        ));
                    }
                    if !depends_on.contains(&param.name) {
                        depends_on.push(param.name.clone());
                    }
                }
                let readable: BTreeSet<String> = depends_on.iter().cloned().collect();
                let mut visible_bindings = let_names.clone();
                visible_bindings.extend(input_names.iter().cloned());
                let body = lower_expr_v2(
                    &r.body,
                    &visible_bindings,
                    &BTreeSet::new(),
                    &readable,
                    &nullary,
                    &variants_of,
                    &function_arities,
                    true,
                    has_functions,
                )
                .map_err(|e| match e {
                    L3V2LowerError::UnresolvedReference(n) if rule_names.contains(&n) => {
                        FiniteDecisionLowerError::RuleDependencyError(
                            L3V2LowerError::UndeclaredFactRead {
                                rule: r.name.clone(),
                                fact: n,
                            },
                        )
                    }
                    L3V2LowerError::FunctionArityMismatch {
                        func,
                        expected,
                        found,
                    } => FiniteDecisionLowerError::FunctionArityMismatch {
                        func,
                        expected,
                        found,
                    },
                    L3V2LowerError::DuplicateMatchBinder(binder) => {
                        FiniteDecisionLowerError::DuplicateMatchBinder(binder)
                    }
                    other => FiniteDecisionLowerError::ExprError(other),
                })?;

                if has_functions {
                    check_exhaustive_expr(&body, &sum_of_variant, &variants_of_sum)
                        .map_err(FiniteDecisionLowerError::ExprError)?;
                }

                rule_names.insert(r.name.clone());
                rules.push(FiniteDecisionRule {
                    ordinal: rules.len() as u64,
                    name: r.name.clone(),
                    body,
                    depends_on,
                });
            }
            ast::Item::Propose(p) => {
                if !proposal_names.insert(p.name.clone()) {
                    return Err(FiniteDecisionLowerError::DuplicateProposalName(
                        p.name.clone(),
                    ));
                }
                if !all_top_level_names.insert(p.name.clone()) {
                    return Err(FiniteDecisionLowerError::DuplicateItemName(p.name.clone()));
                }
                if has_functions {
                    let mut node_count = 0;
                    check_expr_bounds(&p.guard, 0, &mut node_count)?;
                    node_count = 0;
                    check_expr_bounds(&p.value, 0, &mut node_count)?;
                }

                let mut deps: Vec<String> = Vec::new();
                for dep in &p.deps {
                    if dep == &p.name {
                        return Err(FiniteDecisionLowerError::ForwardOrSelfDependency {
                            proposal: p.name.clone(),
                            dep: dep.clone(),
                        });
                    }
                    if !rule_names.contains(dep) {
                        return Err(FiniteDecisionLowerError::UndeclaredDependency {
                            proposal: p.name.clone(),
                            dep: dep.clone(),
                        });
                    }
                    if !deps.contains(dep) {
                        deps.push(dep.clone());
                    }
                }
                let readable: BTreeSet<String> = deps.iter().cloned().collect();
                let mut visible_bindings = let_names.clone();
                visible_bindings.extend(input_names.iter().cloned());
                let guard = lower_expr_v2(
                    &p.guard,
                    &visible_bindings,
                    &BTreeSet::new(),
                    &readable,
                    &nullary,
                    &variants_of,
                    &function_arities,
                    true,
                    has_functions,
                )
                .map_err(|e| match e {
                    L3V2LowerError::UnresolvedReference(n) if rule_names.contains(&n) => {
                        FiniteDecisionLowerError::UndeclaredFactRead {
                            proposal: p.name.clone(),
                            fact: n,
                        }
                    }
                    L3V2LowerError::FunctionArityMismatch {
                        func,
                        expected,
                        found,
                    } => FiniteDecisionLowerError::FunctionArityMismatch {
                        func,
                        expected,
                        found,
                    },
                    L3V2LowerError::DuplicateMatchBinder(binder) => {
                        FiniteDecisionLowerError::DuplicateMatchBinder(binder)
                    }
                    other => FiniteDecisionLowerError::ExprError(other),
                })?;

                if has_functions {
                    check_exhaustive_expr(&guard, &sum_of_variant, &variants_of_sum)
                        .map_err(FiniteDecisionLowerError::ExprError)?;
                }

                let value = lower_expr_v2(
                    &p.value,
                    &visible_bindings,
                    &BTreeSet::new(),
                    &readable,
                    &nullary,
                    &variants_of,
                    &function_arities,
                    true,
                    has_functions,
                )
                .map_err(|e| match e {
                    L3V2LowerError::UnresolvedReference(n) if rule_names.contains(&n) => {
                        FiniteDecisionLowerError::UndeclaredFactRead {
                            proposal: p.name.clone(),
                            fact: n,
                        }
                    }
                    L3V2LowerError::FunctionArityMismatch {
                        func,
                        expected,
                        found,
                    } => FiniteDecisionLowerError::FunctionArityMismatch {
                        func,
                        expected,
                        found,
                    },
                    L3V2LowerError::DuplicateMatchBinder(binder) => {
                        FiniteDecisionLowerError::DuplicateMatchBinder(binder)
                    }
                    other => FiniteDecisionLowerError::ExprError(other),
                })?;

                if has_functions {
                    check_exhaustive_expr(&value, &sum_of_variant, &variants_of_sum)
                        .map_err(FiniteDecisionLowerError::ExprError)?;
                }

                proposals.push(FiniteDecisionProposal {
                    ordinal: proposals.len() as u64,
                    name: p.name.clone(),
                    deps,
                    priority: p.priority,
                    guard,
                    value,
                });
            }
            ast::Item::Show(expr) => {
                if has_functions {
                    let mut node_count = 0;
                    check_expr_bounds(expr, 0, &mut node_count)?;
                }

                let mut visible_bindings = let_names.clone();
                visible_bindings.extend(input_names.iter().cloned());
                let show = lower_expr_v2(
                    expr,
                    &visible_bindings,
                    &BTreeSet::new(),
                    &rule_names,
                    &nullary,
                    &variants_of,
                    &function_arities,
                    true,
                    has_functions,
                )
                .map_err(map_lower_err)?;

                if has_functions {
                    check_exhaustive_expr(&show, &sum_of_variant, &variants_of_sum)
                        .map_err(FiniteDecisionLowerError::ExprError)?;
                }

                shows.push(show);
            }
            ast::Item::Commit(_) => {
                // Handled via commit_decl.
            }
            _ => unreachable!("pass 0 filtered unexpected items"),
        }
    }

    // Cycle detection across all declared functions.
    let mut call_graph: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for f in &functions {
        let mut calls = BTreeSet::new();
        collect_function_calls(&f.body, &mut calls);
        call_graph.insert(f.name.clone(), calls);
    }
    detect_cycles(&call_graph)?;

    // Exhaustiveness check over all function bodies.
    for f in &functions {
        check_exhaustive_expr(&f.body, &sum_of_variant, &variants_of_sum)
            .map_err(FiniteDecisionLowerError::ExprError)?;
    }

    // Every candidate in commit must reference a declared proposal.
    for cand in &commit_decl.candidates {
        if !proposal_names.contains(cand) {
            return Err(FiniteDecisionLowerError::UnknownCandidateInCommit {
                commit: commit_decl.name.clone(),
                candidate: cand.clone(),
            });
        }
    }

    let commit = FiniteDecisionCommit {
        name: commit_decl.name.clone(),
        candidates: commit_decl.candidates.clone(),
    };

    Ok(FiniteDecisionPlan {
        profile: FINITE_DECISION_PROFILE.to_string(),
        configs,
        inputs,
        functions,
        lets,
        rules,
        proposals,
        commit,
        shows,
    })
}

// ---------------------------------------------------------------------------
// Canonical Program Identity (ADR-0030 ⟨D-PROGID⟩)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FiniteDecisionProgramId(pub Digest);

impl FiniteDecisionProgramId {
    pub fn digest(&self) -> Digest {
        self.0
    }

    pub fn to_hex(&self) -> String {
        self.0.to_hex()
    }

    pub fn from_canon(payload: &[u8]) -> Self {
        FiniteDecisionProgramId(Digest::of(Domain::Value, payload))
    }
}

impl Canonical for FiniteDecisionProgramId {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_bytes(self.0.as_bytes());
    }
}

fn encode_config_body_v2(w: &mut CanonWriter, body: &L3ConfigBodyV2) {
    match body {
        L3ConfigBodyV2::Sum(variants) => {
            w.write_enum(0, |w| {
                w.write_uint(variants.len() as u64);
                for (name, arity) in variants {
                    w.write_ident(name);
                    w.write_uint(*arity as u64);
                }
            });
        }
        L3ConfigBodyV2::Record(fields) => {
            w.write_enum(1, |w| {
                w.write_uint(fields.len() as u64);
                for f in fields {
                    w.write_ident(f);
                }
            });
        }
    }
}

fn encode_pattern_v2(w: &mut CanonWriter, pat: &L3PatternV2) {
    match pat {
        L3PatternV2::Ctor { variant, binders } => {
            w.write_ident(variant);
            w.write_uint(binders.len() as u64);
            for b in binders {
                match b {
                    None => w.write_enum(0, |_| {}),
                    Some(name) => w.write_enum(1, |w| w.write_ident(name)),
                }
            }
        }
    }
}

fn encode_expr_v2(w: &mut CanonWriter, e: &L3ExprV2) {
    match e {
        L3ExprV2::Int(n) => w.write_enum(0, |w| w.write_int(*n)),
        L3ExprV2::Str(s) => w.write_enum(1, |w| w.write_str(s)),
        L3ExprV2::Bool(b) => w.write_enum(2, |w| w.write_bool(*b)),
        L3ExprV2::LetRef(name) => w.write_enum(3, |w| w.write_ident(name)),
        L3ExprV2::RuleFact(rule) => w.write_enum(4, |w| w.write_ident(rule)),
        L3ExprV2::NullaryVariant {
            nominal_sum,
            variant,
        } => w.write_enum(5, |w| {
            w.write_ident(nominal_sum);
            w.write_ident(variant);
        }),
        L3ExprV2::Ctor {
            nominal_sum,
            variant,
            args,
        } => w.write_enum(6, |w| {
            w.write_ident(nominal_sum);
            w.write_ident(variant);
            w.write_uint(args.len() as u64);
            for a in args {
                encode_expr_v2(w, a);
            }
        }),
        L3ExprV2::Record {
            nominal_config,
            fields,
        } => w.write_enum(7, |w| {
            w.write_ident(nominal_config);
            w.write_uint(fields.len() as u64);
            for (name, value) in fields {
                w.write_ident(name);
                encode_expr_v2(w, value);
            }
        }),
        L3ExprV2::Field(base, field) => w.write_enum(8, |w| {
            encode_expr_v2(w, base);
            w.write_ident(field);
        }),
        L3ExprV2::Arith(op, a, b) => w.write_enum(9, |w| {
            w.write_uint(op.ordinal());
            encode_expr_v2(w, a);
            encode_expr_v2(w, b);
        }),
        L3ExprV2::Cmp(op, a, b) => w.write_enum(10, |w| {
            w.write_uint(op.ordinal());
            encode_expr_v2(w, a);
            encode_expr_v2(w, b);
        }),
        L3ExprV2::Match { scrutinee, arms } => w.write_enum(11, |w| {
            encode_expr_v2(w, scrutinee);
            w.write_uint(arms.len() as u64);
            for (pat, body) in arms {
                encode_pattern_v2(w, pat);
                encode_expr_v2(w, body);
            }
        }),
        L3ExprV2::Call { func, args } => w.write_enum(12, |w| {
            w.write_ident(func);
            w.write_uint(args.len() as u64);
            for a in args {
                encode_expr_v2(w, a);
            }
        }),
    }
}

fn encode_input_type(w: &mut CanonWriter, ty: &L3ValueType) {
    match ty {
        L3ValueType::Int => w.write_enum(0, |_| {}),
        L3ValueType::Bool => w.write_enum(1, |_| {}),
        L3ValueType::Str => w.write_enum(2, |_| {}),
        other => panic!("non-scalar input type in plan: {other:?}"),
    }
}

fn encode_value_type(w: &mut CanonWriter, ty: &L3ValueType) {
    match ty {
        L3ValueType::Int => w.write_enum(0, |_| {}),
        L3ValueType::Bool => w.write_enum(1, |_| {}),
        L3ValueType::Str => w.write_enum(2, |_| {}),
        L3ValueType::Sum(nominal) => w.write_enum(3, |w| w.write_ident(nominal)),
        L3ValueType::Record(nominal) => w.write_enum(4, |w| w.write_ident(nominal)),
    }
}

fn encode_grade(w: &mut CanonWriter, grade: ast::Grade) {
    match grade {
        ast::Grade::Derived => w.write_enum(0, |_| {}),
        ast::Grade::Audited => w.write_enum(1, |_| {}),
        ast::Grade::Proven => w.write_enum(2, |_| {}),
    }
}

fn encode_contract(w: &mut CanonWriter, contract: &FiniteDecisionContract) {
    encode_value_type(w, &contract.ty);
    match contract.grade {
        None => w.write_enum(0, |_| {}),
        Some(g) => w.write_enum(1, |w| encode_grade(w, g)),
    }
}

fn encode_contract_opt(w: &mut CanonWriter, contract_opt: &Option<FiniteDecisionContract>) {
    match contract_opt {
        None => w.write_enum(0, |_| {}),
        Some(c) => w.write_enum(1, |w| encode_contract(w, c)),
    }
}

/// The canonical program preimage uniquely binding normalized rules, proposals,
/// guards, values, priorities, commit membership and order, show directives,
/// and profile marker (ADR-0030 ⟨D-PROGID⟩, ADR-0031 ⟨D-IDENTITY⟩, ADR-0032 ⟨D-IDENTITY⟩).
pub fn finite_decision_program_preimage(plan: &FiniteDecisionPlan) -> Vec<u8> {
    let mut w = CanonWriter::new();
    w.write_tag("brix.l3.finite-decision.program@1");
    w.write_str(&plan.profile);

    // Configs
    w.write_uint(plan.configs.len() as u64);
    for c in &plan.configs {
        w.write_ident(&c.name);
        encode_config_body_v2(&mut w, &c.body);
    }

    // Inputs (ADR-0031):
    // Bound into program identity when present. When empty, omitted to maintain
    // byte-for-byte preimage and ProgramId compatibility with alpha.2 programs.
    if !plan.inputs.is_empty() {
        w.write_tag("brix.l3.finite-decision.inputs@1");
        w.write_uint(plan.inputs.len() as u64);
        for inp in &plan.inputs {
            w.write_uint(inp.ordinal);
            w.write_ident(&inp.name);
            encode_input_type(&mut w, &inp.ty);
        }
    }

    // Functions (ADR-0032):
    // Bound into program identity when present. When empty, omitted to maintain
    // byte-for-byte preimage and ProgramId compatibility with alpha.2 and alpha.3 function-free programs.
    if !plan.functions.is_empty() {
        w.write_tag("brix.l3.finite-decision.functions@1");
        w.write_uint(plan.functions.len() as u64);
        for f in &plan.functions {
            w.write_uint(f.ordinal);
            w.write_ident(&f.name);
            w.write_uint(f.params.len() as u64);
            for p in &f.params {
                w.write_ident(&p.name);
                encode_contract_opt(&mut w, &p.contract);
            }
            encode_contract_opt(&mut w, &f.ret_contract);
            encode_expr_v2(&mut w, &f.body);
        }
    }

    // Lets
    w.write_uint(plan.lets.len() as u64);
    for (name, expr) in &plan.lets {
        w.write_ident(name);
        encode_expr_v2(&mut w, expr);
    }

    // Rules
    w.write_uint(plan.rules.len() as u64);
    for r in &plan.rules {
        w.write_uint(r.ordinal);
        w.write_ident(&r.name);
        w.write_uint(r.depends_on.len() as u64);
        for d in &r.depends_on {
            w.write_ident(d);
        }
        encode_expr_v2(&mut w, &r.body);
    }

    // Proposals
    w.write_uint(plan.proposals.len() as u64);
    for p in &plan.proposals {
        w.write_uint(p.ordinal);
        w.write_ident(&p.name);
        w.write_uint(p.deps.len() as u64);
        for d in &p.deps {
            w.write_ident(d);
        }
        w.write_uint(p.priority);
        encode_expr_v2(&mut w, &p.guard);
        encode_expr_v2(&mut w, &p.value);
    }

    // Commit membership and order
    w.write_ident(&plan.commit.name);
    w.write_uint(plan.commit.candidates.len() as u64);
    for cand in &plan.commit.candidates {
        w.write_ident(cand);
    }

    // Show directives
    w.write_uint(plan.shows.len() as u64);
    for s in &plan.shows {
        encode_expr_v2(&mut w, s);
    }

    w.finish()
}

/// Compute the canonical program identity for a finite-decision plan.
pub fn finite_decision_program_id(plan: &FiniteDecisionPlan) -> FiniteDecisionProgramId {
    FiniteDecisionProgramId::from_canon(&finite_decision_program_preimage(plan))
}
