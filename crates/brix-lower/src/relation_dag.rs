//! Relational lowering and explicit operator DAG (ADR-0046).
//!
//! Lowers relational syntax (`rel input`, `rel derived`, `select ... from ... where ... group by ...`)
//! into an explicit operator DAG containing:
//! - [`OperatorNode::Scan`]: input and stored relations
//! - [`OperatorNode::Filter`]: selection predicates
//! - [`OperatorNode::Project`]: projections and scalar computations
//! - [`OperatorNode::EquiJoin`]: indexed equality joins
//! - [`OperatorNode::Distinct`]: set semantics / existence
//! - [`OperatorNode::GroupedCount`]: group count aggregations
//!
//! Rejects recursive relation SCCs and unstratified negation with clear typed errors.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use brix_syntax::ast::{self, BinOp, Expr};

use crate::module_graph::LinkedProgram;

/// An index identifying an operator node in [`RelationDag`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OperatorId(pub usize);

impl fmt::Display for OperatorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "op#{}", self.0)
    }
}

/// A field addressed within one lexical query binding. Keeping the binding is
/// essential for self joins and joins with overlapping field names.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct FieldRef {
    pub binding: String,
    pub field: String,
}

/// A grouped projection reads either a materialized group key or its count.
/// Scalar aggregate arithmetic is deliberately not admitted by this version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GroupProjection {
    Key(usize),
    Count,
}

/// An operator node in the relational DAG.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OperatorNode {
    /// Read an input or stored relation.
    Scan {
        relation: String,
        schema: ast::Ty,
        key_fields: Vec<String>,
    },
    /// Introduce a query-local record binding. Each use has its own binding,
    /// including two uses of the same underlying relation in a self join.
    Bind { input: OperatorId, alias: String },
    /// Selection predicate (sigma).
    Filter {
        input: OperatorId,
        predicate: ast::Expr,
    },
    /// Tuple restructuring and scalar computations (pi).
    Project {
        input: OperatorId,
        projections: Vec<(String, ast::Expr)>,
    },
    /// Indexed equality join (bowtie).
    EquiJoin {
        left: OperatorId,
        right: OperatorId,
        left_keys: Vec<FieldRef>,
        right_keys: Vec<FieldRef>,
    },
    /// Distinct / set semantics.
    Distinct { input: OperatorId },
    /// Grouped count aggregation.
    GroupedCount {
        input: OperatorId,
        group_keys: Vec<ast::Expr>,
        projections: Vec<(String, GroupProjection)>,
    },
}

/// The lowered relational operator DAG.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelationDag {
    pub nodes: Vec<OperatorNode>,
    /// Maps each relation name to the OperatorId that produces its tuples.
    pub relation_outputs: BTreeMap<String, OperatorId>,
}

impl RelationDag {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            relation_outputs: BTreeMap::new(),
        }
    }

    pub fn add_node(&mut self, node: OperatorNode) -> OperatorId {
        let id = OperatorId(self.nodes.len());
        self.nodes.push(node);
        id
    }
}

impl Default for RelationDag {
    fn default() -> Self {
        Self::new()
    }
}

/// Errors during relational frontend lowering and validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RelationalLowerError {
    /// Recursive relation cycle / strongly connected component (ADR-0046 §3.5).
    RecursiveRelationCycle { cycle: Vec<String> },
    /// Unstratified negation across relations (ADR-0046 §3.5).
    UnstratifiedNegation {
        relation: String,
        negated_target: String,
    },
    /// Relational negation attempted outside admitted profile operators.
    UnsupportedNegation { relation: String, target: String },
    /// Referenced relation does not exist.
    UnknownRelation(String),
    /// Duplicate relation declared.
    DuplicateRelation(String),
    /// Cartesian product attempted without equijoin equality predicate.
    MissingEquiJoinPredicate { left: String, right: String },
    /// Unsupported expression in query.
    UnsupportedExpression(String),
    /// A `decide` block whose `list` resolves to a relational source has no
    /// explicit `per <field>` entity-identity binding (ADR-0046, decided
    /// 2026-10-04). The world profile requires it; the heuristic that used to
    /// infer an entity field from naming conventions is retired. The legacy
    /// finite-decision profile (ADR-0043) is unaffected: a `decide` whose
    /// `list` is not a relation output never reaches this check.
    DecideMissingEntityField { decide: String },
}

impl fmt::Display for RelationalLowerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RecursiveRelationCycle { cycle } => {
                write!(
                    f,
                    "unsupported relational profile: recursive relation cycle detected: {} (recursive relation SCCs are rejected in ADR-0046 v1)",
                    cycle.join(" -> ")
                )
            }
            Self::UnstratifiedNegation {
                relation,
                negated_target,
            } => {
                write!(
                    f,
                    "unsupported relational profile: unstratified negation in relation '{relation}' against '{negated_target}' is rejected in ADR-0046 v1"
                )
            }
            Self::UnsupportedNegation { relation, target } => {
                write!(
                    f,
                    "unsupported relational profile: relational negation of '{target}' in '{relation}' is not admitted in profile brix.world@1"
                )
            }
            Self::UnknownRelation(rel) => write!(f, "unknown relation '{rel}' in relational query"),
            Self::DuplicateRelation(rel) => write!(f, "duplicate relation '{rel}' declared"),
            Self::MissingEquiJoinPredicate { left, right } => {
                write!(
                    f,
                    "join between '{left}' and '{right}' lacks an indexed equality predicate ('==')"
                )
            }
            Self::UnsupportedExpression(msg) => write!(f, "unsupported expression in query: {msg}"),
            Self::DecideMissingEntityField { decide } => {
                write!(
                    f,
                    "decide block '{decide}' decides over a relational source but declares no \
                     'per <field>' entity-identity binding; the world profile (ADR-0046) requires \
                     it explicitly — add 'per <field>' naming the bound row's entity-identifying field"
                )
            }
        }
    }
}

impl std::error::Error for RelationalLowerError {}

/// Lower a linked program's relations into an explicit [`RelationDag`].
pub fn lower_relations(program: &LinkedProgram) -> Result<RelationDag, RelationalLowerError> {
    let mut all_relations: BTreeMap<String, RelationSource> = BTreeMap::new();

    // Collect all rel inputs
    for (qname, decl) in &program.rel_inputs {
        let name = qname.to_string();
        if all_relations.contains_key(&name) {
            return Err(RelationalLowerError::DuplicateRelation(name));
        }
        all_relations.insert(name, RelationSource::Input(decl.clone()));
    }

    // Collect all rel derived
    for (qname, decl) in &program.rel_derived {
        let name = qname.to_string();
        if all_relations.contains_key(&name) {
            return Err(RelationalLowerError::DuplicateRelation(name));
        }
        all_relations.insert(name, RelationSource::Derived(decl.clone()));
    }

    // 1. Build relation dependency graph
    let mut dependencies: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, source) in &all_relations {
        match source {
            RelationSource::Input(_) => {
                dependencies.insert(name.clone(), Vec::new());
            }
            RelationSource::Derived(derived) => {
                let mut deps = Vec::new();
                for binding in &derived.query.from {
                    let dep = resolve_binding_relation(
                        &binding.relation,
                        owner_module(name),
                        &all_relations,
                    )?;
                    deps.push(dep);
                }
                dependencies.insert(name.clone(), deps);
            }
        }
    }

    // 3. Check for unstratified negation (ADR-0046 §3.5)
    for (name, source) in &all_relations {
        if let RelationSource::Derived(derived) = source {
            if let Some(w) = &derived.query.where_clause {
                check_negation(name, w, &dependencies)?;
            }
        }
    }

    // 4. Topological sort
    let order = topological_sort(&dependencies)?;

    // 5. Lower into Operator DAG
    let mut dag = RelationDag::new();

    for rel_name in order {
        let source = &all_relations[&rel_name];
        match source {
            RelationSource::Input(input_decl) => {
                let op = dag.add_node(OperatorNode::Scan {
                    relation: rel_name.clone(),
                    schema: input_decl.ty.clone(),
                    key_fields: input_decl.key_fields.clone(),
                });
                dag.relation_outputs.insert(rel_name, op);
            }
            RelationSource::Derived(derived) => {
                let op = lower_derived_query(
                    &derived.query,
                    &rel_name,
                    &all_relations,
                    program,
                    &mut dag,
                )?;
                dag.relation_outputs.insert(rel_name, op);
            }
        }
    }

    // Entity identity (ADR-0046, decided 2026-10-04): any `decide` block whose
    // `list` names a relation in this DAG is a world-profile decide and must
    // declare `per <field>` explicitly. A `decide` whose `list` is not a known
    // relation belongs to the legacy finite-decision profile (ADR-0043) and is
    // out of scope here — this is the same resolution rule `network.rs` and
    // `reference.rs` use to decide which `decide` blocks they instantiate.
    let schemas = decision_field_schemas(&dag, program);
    for (qname, decl) in &program.decides {
        if let Some(source) = resolve_decide_source_relation(&decl.list, &qname.module, &dag.relation_outputs) {
            let per = decl.per.as_ref().ok_or_else(|| RelationalLowerError::DecideMissingEntityField { decide: qname.to_string() })?;
            let fields = &schemas[dag.relation_outputs[&source].0];
            match fields.get(per) {
                Some(ast::Ty::Named(t)) if matches!(t.as_str(), "Str" | "Int" | "Bool" | "F64" | "Decimal") => {},
                other => return Err(RelationalLowerError::UnsupportedExpression(format!(
                    "decide '{qname}' per field '{per}' must exist and have a scalar type; got {other:?}"
                ))),
            }
        }
    }

    Ok(dag)
}

/// Resolve the relation that a `decide` block's `list` expression names, if
/// any. Returns `None` when `list` is not a bare `Var` naming a relation
/// output in `relation_outputs` — such a `decide` is out of scope for the
/// world profile (it belongs to the legacy finite-decision profile, ADR-0043,
/// whose `list` is an ordinary expression, not a relation reference).
///
/// Shared by [`lower_relations`]'s entity-identity check and by the two
/// independent evaluators (`brix_kb::world::network`, `brix_kb::world::reference`)
/// that each instantiate `decide` blocks over the same lowered DAG — a single
/// resolution rule, reused as the *problem statement*, not as shared
/// evaluation logic (see `reference.rs`'s module docs on what independence
/// does and does not require).
pub fn resolve_decide_source_relation(
    list: &Expr,
    root_module: &str,
    relation_outputs: &BTreeMap<String, OperatorId>,
) -> Option<String> {
    let Expr::Var(v) = list else {
        return None;
    };
    if relation_outputs.contains_key(v) {
        return Some(v.clone());
    }
    let qualified = format!("{root_module}::{v}");
    if relation_outputs.contains_key(&qualified) {
        return Some(qualified);
    }
    None
}

/// Propagate output field types through the acyclic DAG for entity-field validation.
fn decision_field_schemas(dag: &RelationDag, program: &LinkedProgram) -> Vec<BTreeMap<String, ast::Ty>> {
    fn expr_ty(e: &Expr, fields: &BTreeMap<String, ast::Ty>, program: &LinkedProgram) -> Option<ast::Ty> {
        match e {
            Expr::Var(v) => fields.get(v).cloned(),
            Expr::Field(base, field) => match base.as_ref() {
                Expr::Var(v) => fields.get(&format!("{v}.{field}")).cloned(),
                _ => None,
            },
            Expr::Str(_) => Some(ast::Ty::Named("Str".into())),
            Expr::Bool(_) | Expr::Not(_) => Some(ast::Ty::Named("Bool".into())),
            Expr::Num(n) => Some(ast::Ty::Named(if n.contains('.') { "F64" } else { "Int" }.into())),
            Expr::Bin { op, lhs, .. } => match op {
                BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => expr_ty(lhs, fields, program),
                _ => Some(ast::Ty::Named("Bool".into())),
            },
            Expr::Call { func, .. } => program.functions.iter().find(|(k,_)| k.to_string() == *func).and_then(|(_, f)| f.ret.clone()),
            _ => None,
        }
    }
    let mut schemas: Vec<BTreeMap<String, ast::Ty>> = Vec::new();
    for node in &dag.nodes {
        let fields = match node {
            OperatorNode::Scan { relation, schema, .. } => {
                let fields = match schema {
                    ast::Ty::Record(f) => Some(f),
                    ast::Ty::Named(name) => {
                        let owner = owner_module(relation);
                        let (module, local) = name.rsplit_once("::").unwrap_or((owner, name));
                        program.configs.get(&crate::module_graph::QualifiedName::new(module, local)).and_then(|c| match &c.body { ast::ConfigBody::Record(f) => Some(f), _ => None })
                    },
                    _ => None,
                };
                fields.into_iter().flatten().map(|f| (f.name.clone(), f.ty.clone())).collect()
            },
            OperatorNode::Bind { input, alias } => schemas[input.0].iter().map(|(k,v)| (format!("{alias}.{k}"), v.clone())).collect(),
            OperatorNode::Filter { input, .. } | OperatorNode::Distinct { input } => schemas[input.0].clone(),
            OperatorNode::EquiJoin { left, right, .. } => { let mut f = schemas[left.0].clone(); f.extend(schemas[right.0].clone()); f },
            OperatorNode::Project { input, projections } => projections.iter().filter_map(|(k,e)| expr_ty(e, &schemas[input.0], program).map(|t| (k.clone(),t))).collect(),
            OperatorNode::GroupedCount { input, group_keys, projections } => projections.iter().filter_map(|(k,p)| match p {
                GroupProjection::Count => Some((k.clone(), ast::Ty::Named("Int".into()))),
                GroupProjection::Key(i) => expr_ty(&group_keys[*i], &schemas[input.0], program).map(|t| (k.clone(), t)),
            }).collect(),
        };
        schemas.push(fields);
    }
    schemas
}

enum RelationSource {
    Input(ast::RelInputDecl),
    Derived(ast::RelDerivedDecl),
}

fn owner_module(qualified_name: &str) -> &str {
    qualified_name
        .rsplit_once("::")
        .map_or("", |(module, _)| module)
}

fn check_negation(
    rel_name: &str,
    expr: &Expr,
    deps: &BTreeMap<String, Vec<String>>,
) -> Result<(), RelationalLowerError> {
    match expr {
        Expr::Not(inner) => {
            if let Some(target) = find_relation_reference(inner, deps) {
                let resolved_target = if deps.contains_key(&target) {
                    target.clone()
                } else if let Some(k) = deps.keys().find(|k| k.ends_with(&format!("::{target}"))) {
                    k.clone()
                } else {
                    target.clone()
                };

                if path_exists(deps, &resolved_target, rel_name) {
                    return Err(RelationalLowerError::UnstratifiedNegation {
                        relation: rel_name.to_string(),
                        negated_target: target,
                    });
                } else {
                    return Err(RelationalLowerError::UnsupportedNegation {
                        relation: rel_name.to_string(),
                        target,
                    });
                }
            }
        }
        Expr::Bin { lhs, rhs, .. } => {
            check_negation(rel_name, lhs, deps)?;
            check_negation(rel_name, rhs, deps)?;
        }
        _ => {}
    }
    Ok(())
}

fn path_exists(deps: &BTreeMap<String, Vec<String>>, from: &str, to: &str) -> bool {
    let mut visited = BTreeSet::new();
    let mut queue = vec![from.to_string()];
    while let Some(curr) = queue.pop() {
        if curr == to {
            return true;
        }
        if visited.insert(curr.clone()) {
            if let Some(neighbors) = deps.get(&curr) {
                for n in neighbors {
                    queue.push(n.clone());
                }
            }
        }
    }
    false
}

fn find_relation_reference(expr: &Expr, deps: &BTreeMap<String, Vec<String>>) -> Option<String> {
    match expr {
        Expr::Var(v) => {
            if deps.contains_key(v) || deps.keys().any(|k| k.ends_with(&format!("::{v}"))) {
                Some(v.clone())
            } else {
                None
            }
        }
        Expr::Field(base, _) => find_relation_reference(base, deps),
        Expr::Bin { lhs, rhs, .. } => {
            find_relation_reference(lhs, deps).or_else(|| find_relation_reference(rhs, deps))
        }
        Expr::Not(inner) => find_relation_reference(inner, deps),
        _ => None,
    }
}

fn topological_sort(
    deps: &BTreeMap<String, Vec<String>>,
) -> Result<Vec<String>, RelationalLowerError> {
    let mut in_degree: BTreeMap<String, usize> = BTreeMap::new();
    let mut reverse_graph: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for k in deps.keys() {
        in_degree.insert(k.clone(), 0);
    }

    for (k, children) in deps {
        for child in children {
            if deps.contains_key(child) {
                *in_degree.entry(k.clone()).or_default() += 1;
                reverse_graph
                    .entry(child.clone())
                    .or_default()
                    .push(k.clone());
            }
        }
    }

    let mut ready: Vec<String> = in_degree
        .iter()
        .filter(|(_, &deg)| deg == 0)
        .map(|(k, _)| k.clone())
        .collect();

    let mut result = Vec::new();
    while let Some(node) = ready.pop() {
        result.push(node.clone());
        if let Some(parents) = reverse_graph.get(&node) {
            for parent in parents {
                let deg = in_degree.get_mut(parent).expect("in_degree exists");
                *deg -= 1;
                if *deg == 0 {
                    ready.push(parent.clone());
                }
            }
        }
    }

    if result.len() != deps.len() {
        // Follow dependencies within the unresolved subgraph iteratively. This
        // finds a concrete cycle without recursion or rewalking diamond DAGs.
        let mut current = in_degree
            .iter()
            .find(|(_, degree)| **degree > 0)
            .expect("unresolved node")
            .0
            .clone();
        let mut path = Vec::new();
        let mut positions = BTreeMap::new();
        loop {
            if let Some(start) = positions.insert(current.clone(), path.len()) {
                let mut cycle = path[start..].to_vec();
                cycle.push(current);
                return Err(RelationalLowerError::RecursiveRelationCycle { cycle });
            }
            path.push(current.clone());
            current = deps[&current]
                .iter()
                .find(|child| in_degree.get(*child).copied().unwrap_or(0) > 0)
                .expect("unresolved node depends on unresolved node")
                .clone();
        }
    }
    Ok(result)
}

fn lower_derived_query(
    query: &ast::RelQuery,
    rel_name: &str,
    all_relations: &BTreeMap<String, RelationSource>,
    program: &LinkedProgram,
    dag: &mut RelationDag,
) -> Result<OperatorId, RelationalLowerError> {
    if query.from.is_empty() {
        return Err(RelationalLowerError::UnsupportedExpression(
            "relational query must have at least one 'from' source".to_string(),
        ));
    }

    let owner = owner_module(rel_name);
    let mut aliases = BTreeSet::new();
    for binding in &query.from {
        if !aliases.insert(binding.var.clone()) {
            return Err(RelationalLowerError::UnsupportedExpression(format!(
                "duplicate query binding '{}'",
                binding.var
            )));
        }
    }
    let mut query = query.clone();
    qualify_query_expr(&mut query.select, owner, program)?;
    if let Some(predicate) = &mut query.where_clause {
        qualify_query_expr(predicate, owner, program)?;
    }
    for key in &mut query.group_by {
        qualify_query_expr(key, owner, program)?;
    }
    for expression in std::iter::once(&query.select)
        .chain(query.where_clause.iter())
        .chain(query.group_by.iter())
    {
        validate_binding_fields(expression, &query.from, owner, all_relations, program)?;
    }
    // 1. Resolve and bind input relations
    let mut current_op: OperatorId;
    let mut var_to_relation: BTreeMap<String, String> = BTreeMap::new();

    let first_binding = &query.from[0];
    let first_rel = resolve_binding_relation(&first_binding.relation, owner, all_relations)?;
    current_op = *dag
        .relation_outputs
        .get(&first_rel)
        .ok_or_else(|| RelationalLowerError::UnknownRelation(first_rel.clone()))?;
    current_op = dag.add_node(OperatorNode::Bind {
        input: current_op,
        alias: first_binding.var.clone(),
    });
    var_to_relation.insert(first_binding.var.clone(), first_rel);

    // 2. Joins
    let mut split_predicates = if let Some(w) = &query.where_clause {
        split_and_predicates(w)
    } else {
        Vec::new()
    };

    for binding in &query.from[1..] {
        let next_rel = resolve_binding_relation(&binding.relation, owner, all_relations)?;
        let next_op = *dag
            .relation_outputs
            .get(&next_rel)
            .ok_or_else(|| RelationalLowerError::UnknownRelation(next_rel.clone()))?;
        let next_op = dag.add_node(OperatorNode::Bind {
            input: next_op,
            alias: binding.var.clone(),
        });

        // Find equijoin predicate matching current relation(s) and next relation
        let mut left_keys = Vec::new();
        let mut right_keys = Vec::new();
        let mut remaining_predicates = Vec::new();

        for pred in split_predicates {
            if let Some((l_key, r_key)) =
                match_equijoin_predicate(&pred, &var_to_relation, &binding.var)
            {
                left_keys.push(l_key);
                right_keys.push(r_key);
            } else {
                remaining_predicates.push(pred);
            }
        }

        if left_keys.is_empty() {
            return Err(RelationalLowerError::MissingEquiJoinPredicate {
                left: query.from[0].relation.clone(),
                right: next_rel,
            });
        }

        current_op = dag.add_node(OperatorNode::EquiJoin {
            left: current_op,
            right: next_op,
            left_keys,
            right_keys,
        });

        var_to_relation.insert(binding.var.clone(), next_rel);
        split_predicates = remaining_predicates;
    }

    // 3. Selection Filter for remaining predicates
    if !split_predicates.is_empty() {
        let combined_filter = combine_predicates(split_predicates);
        current_op = dag.add_node(OperatorNode::Filter {
            input: current_op,
            predicate: combined_filter,
        });
    }

    // 4. Compile aggregate projections explicitly. The downstream evaluator
    // never receives a raw count() call or a dangling pre-group row binding.
    let projections = match &query.select {
        Expr::AnonRecord(fields) => fields.clone(),
        single => vec![("value".to_string(), single.clone())],
    };
    let mut projection_names = BTreeSet::new();
    for (name, _) in &projections {
        if !projection_names.insert(name) {
            return Err(RelationalLowerError::UnsupportedExpression(format!(
                "duplicate projection field '{name}'"
            )));
        }
    }
    let has_count = expr_contains_count(&query.select);
    if !query.group_by.is_empty() || has_count {
        let mut grouped_projections = Vec::new();
        for (name, expression) in projections {
            let projection = match &expression {
                Expr::Call { func, args } if func == "count" && args.is_empty() => {
                    GroupProjection::Count
                }
                _ => match query.group_by.iter().position(|key| key == &expression) {
                    Some(index) => GroupProjection::Key(index),
                    None => {
                        return Err(RelationalLowerError::UnsupportedExpression(format!(
                            "grouped projection '{name}' must be an exact group key or count()"
                        )))
                    }
                },
            };
            grouped_projections.push((name, projection));
        }
        current_op = dag.add_node(OperatorNode::GroupedCount {
            input: current_op,
            group_keys: query.group_by.clone(),
            projections: grouped_projections,
        });
    } else {
        current_op = dag.add_node(OperatorNode::Project {
            input: current_op,
            projections,
        });
    }

    // 6. Distinct set semantics
    current_op = dag.add_node(OperatorNode::Distinct { input: current_op });

    Ok(current_op)
}

fn resolve_binding_relation(
    rel: &str,
    owner: &str,
    all_relations: &BTreeMap<String, RelationSource>,
) -> Result<String, RelationalLowerError> {
    let qualified = if rel.contains("::") {
        rel.to_string()
    } else {
        format!("{owner}::{rel}")
    };
    if all_relations.contains_key(&qualified) {
        return Ok(qualified);
    }
    Err(RelationalLowerError::UnknownRelation(qualified))
}

/// Resolve scalar helper names in their defining module. Builtins keep their
/// reserved spelling; lexical row variables never become global symbols.
fn qualify_query_expr(
    expr: &mut Expr,
    owner: &str,
    program: &LinkedProgram,
) -> Result<(), RelationalLowerError> {
    match expr {
        Expr::Call { func, args } => {
            let qualified = if let Some((module, name)) = func.rsplit_once("::") {
                crate::module_graph::QualifiedName::new(module, name)
            } else {
                crate::module_graph::QualifiedName::new(owner, &*func)
            };
            if program.functions.contains_key(&qualified) {
                *func = qualified.to_string();
            }
            for arg in args {
                qualify_query_expr(arg, owner, program)?;
            }
        }
        Expr::Record { fields, .. } | Expr::AnonRecord(fields) => {
            for (_, value) in fields {
                qualify_query_expr(value, owner, program)?;
            }
        }
        Expr::Field(base, _)
        | Expr::Not(base)
        | Expr::Prove(base)
        | Expr::Why(base)
        | Expr::Audit(base) => qualify_query_expr(base, owner, program)?,
        Expr::Bin { lhs, rhs, .. } => {
            qualify_query_expr(lhs, owner, program)?;
            qualify_query_expr(rhs, owner, program)?;
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            qualify_query_expr(scrutinee, owner, program)?;
            for arm in arms {
                qualify_query_expr(&mut arm.body, owner, program)?;
            }
        }
        Expr::Lambda { body, .. } => qualify_query_expr(body, owner, program)?,
        Expr::ListLit(items) => {
            for item in items {
                qualify_query_expr(item, owner, program)?;
            }
        }
        Expr::Comprehension {
            generators,
            where_clause,
            yield_expr,
        } => {
            for (_, source) in generators {
                qualify_query_expr(source, owner, program)?;
            }
            if let Some(predicate) = where_clause {
                qualify_query_expr(predicate, owner, program)?;
            }
            qualify_query_expr(yield_expr, owner, program)?;
        }
        Expr::Num(_) | Expr::Str(_) | Expr::Bool(_) | Expr::Var(_) => {}
    }
    Ok(())
}

fn validate_binding_fields(
    expr: &Expr,
    bindings: &[ast::RelBinding],
    owner: &str,
    relations: &BTreeMap<String, RelationSource>,
    program: &LinkedProgram,
) -> Result<(), RelationalLowerError> {
    if let Expr::Field(base, field) = expr {
        if let Expr::Var(alias) = base.as_ref() {
            let binding = bindings
                .iter()
                .find(|binding| &binding.var == alias)
                .ok_or_else(|| {
                    RelationalLowerError::UnsupportedExpression(format!(
                        "unknown query binding '{alias}'"
                    ))
                })?;
            let relation = resolve_binding_relation(&binding.relation, owner, relations)?;
            let fields = match &relations[&relation] {
                RelationSource::Input(input) => match &input.ty {
                    ast::Ty::Record(fields) => {
                        Some(fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>())
                    }
                    ast::Ty::Named(name) => {
                        let module = owner_module(&relation);
                        let q = if let Some((module, name)) = name.rsplit_once("::") {
                            crate::module_graph::QualifiedName::new(module, name)
                        } else {
                            crate::module_graph::QualifiedName::new(module, name)
                        };
                        match program.configs.get(&q).map(|config| &config.body) {
                            Some(ast::ConfigBody::Record(fields)) => {
                                Some(fields.iter().map(|f| f.name.as_str()).collect())
                            }
                            _ => None,
                        }
                    }
                    _ => None,
                },
                RelationSource::Derived(derived) => match &derived.query.select {
                    Expr::AnonRecord(fields) => {
                        Some(fields.iter().map(|(name, _)| name.as_str()).collect())
                    }
                    _ => Some(vec!["value"]),
                },
            };
            if let Some(fields) = fields {
                if !fields.contains(&field.as_str()) {
                    return Err(RelationalLowerError::UnsupportedExpression(format!(
                        "unknown field '{alias}.{field}'"
                    )));
                }
            }
            return Ok(());
        }
    }
    match expr {
        Expr::Call { args, .. } | Expr::ListLit(args) => {
            for arg in args {
                validate_binding_fields(arg, bindings, owner, relations, program)?;
            }
        }
        Expr::Record { fields, .. } | Expr::AnonRecord(fields) => {
            for (_, value) in fields {
                validate_binding_fields(value, bindings, owner, relations, program)?;
            }
        }
        Expr::Field(base, _)
        | Expr::Not(base)
        | Expr::Prove(base)
        | Expr::Why(base)
        | Expr::Audit(base) => validate_binding_fields(base, bindings, owner, relations, program)?,
        Expr::Bin { lhs, rhs, .. } => {
            validate_binding_fields(lhs, bindings, owner, relations, program)?;
            validate_binding_fields(rhs, bindings, owner, relations, program)?;
        }
        // Complex lexical binders are validated by scalar lowering, which knows
        // their local scope. The DAG does not reinterpret those binders.
        _ => {}
    }
    Ok(())
}

fn split_and_predicates(expr: &Expr) -> Vec<Expr> {
    let mut preds = Vec::new();
    match expr {
        Expr::Bin {
            op: BinOp::And | BinOp::AndAnd,
            lhs,
            rhs,
        } => {
            preds.extend(split_and_predicates(lhs));
            preds.extend(split_and_predicates(rhs));
        }
        other => preds.push(other.clone()),
    }
    preds
}

fn combine_predicates(mut preds: Vec<Expr>) -> Expr {
    let mut out = preds.pop().expect("nonempty predicates");
    while let Some(prev) = preds.pop() {
        out = Expr::Bin {
            op: BinOp::AndAnd,
            lhs: Box::new(prev),
            rhs: Box::new(out),
        };
    }
    out
}

fn match_equijoin_predicate(
    expr: &Expr,
    var_to_rel: &BTreeMap<String, String>,
    right_var: &str,
) -> Option<(FieldRef, FieldRef)> {
    if let Expr::Bin {
        op: BinOp::Eq,
        lhs,
        rhs,
    } = expr
    {
        if let (Expr::Field(l_base, l_field), Expr::Field(r_base, r_field)) =
            (lhs.as_ref(), rhs.as_ref())
        {
            if let (Expr::Var(l_var), Expr::Var(r_var)) = (l_base.as_ref(), r_base.as_ref()) {
                if r_var == right_var && l_var != right_var && var_to_rel.contains_key(l_var) {
                    return Some((
                        FieldRef {
                            binding: l_var.clone(),
                            field: l_field.clone(),
                        },
                        FieldRef {
                            binding: r_var.clone(),
                            field: r_field.clone(),
                        },
                    ));
                } else if l_var == right_var && r_var != right_var && var_to_rel.contains_key(r_var)
                {
                    return Some((
                        FieldRef {
                            binding: r_var.clone(),
                            field: r_field.clone(),
                        },
                        FieldRef {
                            binding: l_var.clone(),
                            field: l_field.clone(),
                        },
                    ));
                }
            }
        }
    }
    None
}

fn expr_contains_count(expr: &Expr) -> bool {
    match expr {
        Expr::Call { func, .. } if func == "count" => true,
        Expr::AnonRecord(fields) => fields.iter().any(|(_, e)| expr_contains_count(e)),
        Expr::Field(base, _) => expr_contains_count(base),
        Expr::Bin { lhs, rhs, .. } => expr_contains_count(lhs) || expr_contains_count(rhs),
        _ => false,
    }
}
