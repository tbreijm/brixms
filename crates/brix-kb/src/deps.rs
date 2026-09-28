//! The plan's dependency graph, for `brix kb diff`'s "why" explanation.
//!
//! For each rule (and each proposal), computes the transitive set of declared
//! **input** names and **rule** names it depends on, from:
//! - a rule's declared `depends_on` (`rule name(dep1, dep2) = …`);
//! - a proposal's declared `deps` (`propose name(dep1, dep2) …`);
//! - every `LetRef`/`RuleFact` identifier actually mentioned in a rule body,
//!   a proposal guard, or a proposal value — because a rule or proposal may
//!   read an input or a `let` binding directly, without it ever appearing in
//!   its declared dependency list (that list only names *other rules*, see
//!   `FiniteDecisionRule::depends_on`'s doc in `plan.rs`).
//!
//! A `let` binding is transparent: it is not itself a fact, so its own
//! transitive input set is inlined wherever it is referenced (a `let` can only
//! read earlier `let`s and inputs, never a rule fact — enforced at lowering).
//!
//! This is a static analysis over one [`FiniteDecisionPlan`]; it says nothing
//! about *values*, only about which names a fact's defining expression can
//! possibly depend on. `diff` then intersects this with what actually
//! changed between two revisions' inputs and facts.

use std::collections::{BTreeMap, BTreeSet};

use brix_lower::finite_decision::{FiniteDecisionPlan, FiniteDecisionRule};
use brix_lower::l3_v2::L3ExprV2;

/// Everything a rule or proposal can reach, in terms of the plan's inputs and
/// other rules.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reach {
    pub inputs: BTreeSet<String>,
    pub rules: BTreeSet<String>,
}

/// The plan's dependency graph: every rule's and proposal's transitive reach.
#[derive(Clone, Debug, Default)]
pub struct DepGraph {
    pub rules: BTreeMap<String, Reach>,
    pub proposals: BTreeMap<String, Reach>,
}

/// Walk an expression, collecting the `LetRef` and `RuleFact` identifiers it
/// mentions anywhere (including inside function-call arguments, record
/// fields, match arms, and both operands of every binary form). Does not
/// recurse into a called function's own body — a function's body is closed
/// over its parameters only (enforced at lowering: a `fn` can read neither an
/// input, a `let`, nor a rule fact), so only its call-site arguments can carry
/// a dependency.
fn collect_refs(
    expr: &L3ExprV2,
    let_refs: &mut BTreeSet<String>,
    rule_refs: &mut BTreeSet<String>,
) {
    match expr {
        L3ExprV2::Int(_) | L3ExprV2::Str(_) | L3ExprV2::Bool(_) => {}
        L3ExprV2::LetRef(name) => {
            let_refs.insert(name.clone());
        }
        L3ExprV2::RuleFact(name) => {
            rule_refs.insert(name.clone());
        }
        L3ExprV2::NullaryVariant { .. } => {}
        L3ExprV2::Ctor { args, .. } => {
            for a in args {
                collect_refs(a, let_refs, rule_refs);
            }
        }
        L3ExprV2::Record { fields, .. } => {
            for (_, v) in fields {
                collect_refs(v, let_refs, rule_refs);
            }
        }
        L3ExprV2::Field(base, _) => collect_refs(base, let_refs, rule_refs),
        L3ExprV2::Arith(_, a, b) | L3ExprV2::Cmp(_, a, b) | L3ExprV2::IntDivMod(_, a, b) => {
            collect_refs(a, let_refs, rule_refs);
            collect_refs(b, let_refs, rule_refs);
        }
        L3ExprV2::Match { scrutinee, arms } => {
            collect_refs(scrutinee, let_refs, rule_refs);
            for (_, body) in arms {
                collect_refs(body, let_refs, rule_refs);
            }
        }
        L3ExprV2::Call { args, .. } => {
            for a in args {
                collect_refs(a, let_refs, rule_refs);
            }
        }
        L3ExprV2::And(a, b) | L3ExprV2::Or(a, b) | L3ExprV2::In(a, b) => {
            collect_refs(a, let_refs, rule_refs);
            collect_refs(b, let_refs, rule_refs);
        }
        L3ExprV2::Not(a) | L3ExprV2::Len(a) | L3ExprV2::Distinct(a) => {
            collect_refs(a, let_refs, rule_refs)
        }
        // A fold/filter/map binder is a hygienic local, not a `let`/rule
        // reference (ADR-0037, ADR-0040) — exactly like a function parameter
        // or match binder above, its own body is walked for *other* names it
        // reads, and the binder itself never becomes a graph node.
        L3ExprV2::Fold { list, body, .. } => {
            collect_refs(list, let_refs, rule_refs);
            collect_refs(body, let_refs, rule_refs);
        }
        L3ExprV2::Filter { list, cond, .. } => {
            collect_refs(list, let_refs, rule_refs);
            collect_refs(cond, let_refs, rule_refs);
        }
        L3ExprV2::Map { list, body, .. } => {
            collect_refs(list, let_refs, rule_refs);
            collect_refs(body, let_refs, rule_refs);
        }
        L3ExprV2::Comprehension {
            generators,
            where_clause,
            yield_expr,
        } => {
            for (_, source) in generators {
                collect_refs(source, let_refs, rule_refs);
            }
            if let Some(w) = where_clause {
                collect_refs(w, let_refs, rule_refs);
            }
            collect_refs(yield_expr, let_refs, rule_refs);
        }
        L3ExprV2::ListLit(items) => {
            for item in items {
                collect_refs(item, let_refs, rule_refs);
            }
        }
    }
}

/// Build the full dependency graph for `plan`.
pub fn build_dep_graph(plan: &FiniteDecisionPlan) -> DepGraph {
    let input_names: BTreeSet<String> = plan.inputs.iter().map(|i| i.name.clone()).collect();

    // `let`s can only reference earlier `let`s and inputs (never a rule fact),
    // so a single left-to-right pass over `plan.lets` (already declaration
    // order) resolves every `let`'s own transitive input set.
    let mut let_inputs: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (name, value) in &plan.lets {
        let mut let_refs = BTreeSet::new();
        let mut rule_refs = BTreeSet::new();
        collect_refs(value, &mut let_refs, &mut rule_refs);
        let mut inputs = BTreeSet::new();
        for r in &let_refs {
            if input_names.contains(r) {
                inputs.insert(r.clone());
            } else if let Some(inner) = let_inputs.get(r) {
                inputs.extend(inner.iter().cloned());
            }
        }
        let_inputs.insert(name.clone(), inputs);
    }

    // Rules can only declare a dependency on an *earlier* rule (checked at
    // lowering), so processing `plan.rules` in declaration order lets each
    // rule look up its dependencies' already-computed transitive reach.
    let mut rules: BTreeMap<String, Reach> = BTreeMap::new();
    for rule in &plan.rules {
        rules.insert(
            rule.name.clone(),
            reach_of(
                rule.body_refs(),
                &input_names,
                &let_inputs,
                &rule.depends_on,
                &rules,
            ),
        );
    }

    let mut proposals: BTreeMap<String, Reach> = BTreeMap::new();
    for p in &plan.proposals {
        let mut let_refs = BTreeSet::new();
        let mut rule_refs = BTreeSet::new();
        collect_refs(&p.guard, &mut let_refs, &mut rule_refs);
        collect_refs(&p.value, &mut let_refs, &mut rule_refs);
        proposals.insert(
            p.name.clone(),
            reach_of(
                (let_refs, rule_refs),
                &input_names,
                &let_inputs,
                &p.deps,
                &rules,
            ),
        );
    }

    DepGraph { rules, proposals }
}

/// Combine one declaration's direct references into its transitive reach,
/// expanding any referenced `let` inline and any referenced rule via its
/// already-computed [`Reach`].
fn reach_of(
    (let_refs, rule_refs): (BTreeSet<String>, BTreeSet<String>),
    input_names: &BTreeSet<String>,
    let_inputs: &BTreeMap<String, BTreeSet<String>>,
    declared_deps: &[String],
    rules_so_far: &BTreeMap<String, Reach>,
) -> Reach {
    let mut reach = Reach::default();
    for r in &let_refs {
        if input_names.contains(r) {
            reach.inputs.insert(r.clone());
        } else if let Some(inner) = let_inputs.get(r) {
            reach.inputs.extend(inner.iter().cloned());
        }
    }
    let mut direct_rules: BTreeSet<String> = rule_refs;
    direct_rules.extend(declared_deps.iter().cloned());
    for dep in &direct_rules {
        reach.rules.insert(dep.clone());
        if let Some(inner) = rules_so_far.get(dep) {
            reach.inputs.extend(inner.inputs.iter().cloned());
            reach.rules.extend(inner.rules.iter().cloned());
        }
    }
    reach
}

/// A rule's direct `LetRef`/`RuleFact` references, collected from its body.
trait RuleBodyRefs {
    fn body_refs(&self) -> (BTreeSet<String>, BTreeSet<String>);
}

impl RuleBodyRefs for FiniteDecisionRule {
    fn body_refs(&self) -> (BTreeSet<String>, BTreeSet<String>) {
        let mut let_refs = BTreeSet::new();
        let mut rule_refs = BTreeSet::new();
        collect_refs(&self.body, &mut let_refs, &mut rule_refs);
        (let_refs, rule_refs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::load_plan;

    fn plan_of(src: &str) -> FiniteDecisionPlan {
        load_plan(src, &[]).expect("plan should lower")
    }

    #[test]
    fn test_shipping_example_dependency_graph() {
        let src = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/shipping-input.brix"
        ))
        .unwrap();
        let plan = plan_of(&src);
        let graph = build_dep_graph(&plan);

        // `threshold` is a bare constant: no input or rule dependencies.
        let threshold = &graph.rules["threshold"];
        assert!(threshold.inputs.is_empty());
        assert!(threshold.rules.is_empty());

        // `destination` reads the `region` input directly.
        let destination = &graph.rules["destination"];
        assert_eq!(destination.inputs, BTreeSet::from(["region".to_string()]));
        assert!(destination.rules.is_empty());

        // `valid_destination` depends on `destination` (declared), which
        // itself transitively depends on `region`.
        let valid_destination = &graph.rules["valid_destination"];
        assert_eq!(
            valid_destination.inputs,
            BTreeSet::from(["region".to_string()])
        );
        assert_eq!(
            valid_destination.rules,
            BTreeSet::from(["destination".to_string()])
        );

        // `can_ship` declares deps on `threshold` and `valid_destination`, and
        // also reads `stock` and `eligible` directly in its body.
        let can_ship = &graph.rules["can_ship"];
        assert_eq!(
            can_ship.inputs,
            BTreeSet::from([
                "stock".to_string(),
                "eligible".to_string(),
                "region".to_string()
            ])
        );
        // Transitive: `valid_destination` itself depends on `destination`.
        assert_eq!(
            can_ship.rules,
            BTreeSet::from([
                "threshold".to_string(),
                "valid_destination".to_string(),
                "destination".to_string(),
            ])
        );
    }

    #[test]
    fn test_allocation_example_transitive_through_two_rules() {
        let src = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../examples/allocation.brix"
        ))
        .unwrap();
        let plan = plan_of(&src);
        let graph = build_dep_graph(&plan);

        let meets_minimum = &graph.rules["meets_minimum"];
        assert_eq!(meets_minimum.inputs, BTreeSet::from(["batch".to_string()]));
        assert_eq!(meets_minimum.rules, BTreeSet::from(["share".to_string()]));
    }
}
