//! ADR-0027 Stage A gates: the v2 profile, its expression IR, and the
//! dependency extraction that makes a derived fact reachable.

use brix_lower::l3_v2::{
    lower_l3_plan_v2, ArithOpV2, CmpOpV2, L3ExprV2, L3PlanItemV2, L3V2LowerError,
    L3_PLAN_FORMAT_V2, L3_PROFILE_MARKER_V2,
};
use brix_syntax::parse;

fn lower(src: &str) -> Result<brix_lower::l3_v2::L3PlanV2, L3V2LowerError> {
    let module = parse(src).expect("fixture parses");
    lower_l3_plan_v2(&module, L3_PROFILE_MARKER_V2)
}

/// **The point of v2.** A rule body may read an earlier rule's committed fact,
/// and the dependency is extracted statically into the plan.
///
/// v1 rejects this outright (`UnclosedReference`), which is why every v1 rule
/// is an independent constant and a run has no computation in it at all.
#[test]
fn a_rule_may_read_an_earlier_rules_fact() {
    let plan = lower("config Z = Hand | Field\nrule a() = Hand\nrule b(a) = a\n")
        .expect("a declared fact dependency is the v2 fragment");

    let rules: Vec<&L3PlanItemV2> = plan
        .items
        .iter()
        .filter(|i| matches!(i, L3PlanItemV2::Rule { .. }))
        .collect();
    assert_eq!(rules.len(), 2);

    match rules[1] {
        L3PlanItemV2::Rule {
            name,
            body,
            depends_on,
            ..
        } => {
            assert_eq!(name, "b");
            assert_eq!(*body, L3ExprV2::RuleFact("a".to_string()));
            assert_eq!(
                depends_on,
                &vec!["a".to_string()],
                "the dependency must be in the plan, not discovered at runtime"
            );
        }
        other => panic!("expected a rule, got {other:?}"),
    }
}

/// A rule cannot read its own fact, nor one from a rule declared later.
///
/// Acyclicity is by construction rather than by a later graph walk: only
/// rules already lowered are in scope, so a forward reference resolves to
/// nothing and a self-reference is refused by name.
#[test]
fn forward_and_self_dependencies_are_refused() {
    // Self-dependency.
    match lower("config Z = Hand\nrule a(a) = a\n") {
        Err(L3V2LowerError::ForwardOrSelfDependency { rule, depends_on }) => {
            assert_eq!(rule, "a");
            assert_eq!(depends_on, "a");
        }
        other => panic!("a self-dependency must be refused, got {other:?}"),
    }
    // Forward dependency: `a` is declared after `b`.
    match lower("config Z = Hand\nrule b(a) = a\nrule a() = Hand\n") {
        Err(L3V2LowerError::UndeclaredDependency { rule, param }) => {
            assert_eq!(rule, "b");
            assert_eq!(param, "a");
        }
        other => panic!("a forward dependency must be refused, got {other:?}"),
    }
}

/// A `let` is a closed static binding and may **not** read a committed fact —
/// otherwise plan construction would depend on run order.
#[test]
fn a_let_may_not_read_a_fact() {
    match lower("config Z = Hand\nrule a() = Hand\nlet x = a\n") {
        Err(L3V2LowerError::UnresolvedReference(name)) => assert_eq!(name, "a"),
        other => panic!("a let must not depend on a committed fact, got {other:?}"),
    }
    // And a `let` has no signature to declare one with.
    assert!(
        lower("rule a() = 1\nlet x = a\n").is_err(),
        "a let can never read a fact"
    );
}

/// The forms v1 rejects by name are the v2 fragment.
#[test]
fn v2_admits_what_v1_rejects() {
    let plan = lower(
        "config Card = MkCard { atk: Int }\n\
         let c = MkCard { atk: 1500 }\n\
         rule strong() = c.atk > 1000\n\
         rule total() = c.atk + 500\n",
    )
    .expect("field access, comparison and arithmetic are the v2 fragment");
    assert_eq!(plan.profile, L3_PROFILE_MARKER_V2);
    assert_eq!(plan.format, L3_PLAN_FORMAT_V2);

    let bodies: Vec<&L3ExprV2> = plan
        .items
        .iter()
        .filter_map(|i| match i {
            L3PlanItemV2::Rule { body, .. } => Some(body),
            _ => None,
        })
        .collect();
    assert!(matches!(bodies[0], L3ExprV2::Cmp(CmpOpV2::Gt, _, _)));
    assert!(matches!(bodies[1], L3ExprV2::Arith(ArithOpV2::Add, _, _)));
}

/// Division is deferred until its fault semantics are pinned, and refused by
/// name rather than lowered to something with unspecified behaviour.
#[test]
fn division_is_refused_by_name() {
    assert_eq!(
        lower("rule r() = 7 - 2\n").map(|_| ()),
        Ok(()),
        "subtraction is admitted"
    );
    match lower("rule r() = 7 / 2\n") {
        Err(L3V2LowerError::DivisionNotAllowed) => {}
        other => panic!("division must be refused by name, got {other:?}"),
    }
}

/// A default arm is refused, so malformed input cannot be swallowed by a
/// catch-all (ADR-0027 §5).
#[test]
fn a_default_match_arm_is_refused() {
    match lower("config Z = Hand | Field\nlet z = Hand\nrule r() = match z { Hand => 1  _ => 2 }\n")
    {
        Err(L3V2LowerError::DefaultArmNotAllowed) => {}
        other => panic!("a default arm must be refused, got {other:?}"),
    }
}

/// The profile marker is checked before anything else, and the v1 marker is
/// not accepted — a v2 plan can never be mistaken for a v1 plan.
#[test]
fn the_v1_marker_is_refused() {
    let module = parse("rule a() = 1\n").expect("parses");
    match lower_l3_plan_v2(&module, brix_lower::L3_PROFILE_MARKER_V1) {
        Err(L3V2LowerError::ProfileMismatch { expected, found }) => {
            assert_eq!(expected, L3_PROFILE_MARKER_V2);
            assert_eq!(found, brix_lower::L3_PROFILE_MARKER_V1);
        }
        other => panic!("the v1 marker must be refused, got {other:?}"),
    }
    assert_ne!(
        L3_PROFILE_MARKER_V2,
        brix_lower::L3_PROFILE_MARKER_V1,
        "the two profiles must be distinguishable by identity"
    );
}

/// Every top-level item outside `{config, let, rule}` is named, never dropped.
#[test]
fn disallowed_items_are_named() {
    for (src, needle) in [
        ("fn f(x) = x\n", "fn f"),
        ("regime r { gen g() = 1 }\n", "regime r"),
        ("show 1\n", "show"),
        ("witness w = 1\n", "witness w"),
    ] {
        match lower(src) {
            Err(L3V2LowerError::ItemNotAllowed(what)) => {
                assert!(what.contains(needle), "expected {needle}, got {what}")
            }
            other => panic!("{src}: expected ItemNotAllowed, got {other:?}"),
        }
    }
}

/// A rule parameter **is** a declared dependency: it names an earlier rule and
/// binds that rule's committed fact.
///
/// This is not the schema case ADR-0027 §7 defers — a parameter here binds
/// exactly one fact, so there is no quantification domain to supply. A
/// parameter naming something that is not an earlier rule is refused.
#[test]
fn a_rule_parameter_is_a_declared_dependency() {
    let plan = lower("rule base() = 1500\nrule boosted(base) = base + 500\n")
        .expect("a parameter naming an earlier rule is the v2 fragment");

    match plan
        .items
        .iter()
        .find(|i| matches!(i, L3PlanItemV2::Rule { name, .. } if name == "boosted"))
        .expect("rule present")
    {
        L3PlanItemV2::Rule { depends_on, .. } => {
            assert_eq!(
                depends_on,
                &vec!["base".to_string()],
                "the dependency is DECLARED by the signature, not extracted from the body"
            );
        }
        other => panic!("expected a rule, got {other:?}"),
    }

    // A parameter that names nothing declared earlier.
    match lower("rule r(nope) = 1\n") {
        Err(L3V2LowerError::UndeclaredDependency { rule, param }) => {
            assert_eq!(rule, "r");
            assert_eq!(param, "nope");
        }
        other => panic!("expected UndeclaredDependency, got {other:?}"),
    }
}

/// A body may read only what its signature declares, which is what keeps the
/// plan's dependency list from drifting from what the body actually does.
#[test]
fn a_body_may_not_read_an_undeclared_fact() {
    match lower("rule base() = 1\nrule other() = 2\nrule r(base) = base + other\n") {
        Err(L3V2LowerError::UndeclaredFactRead { rule, fact }) => {
            assert_eq!(rule, "r");
            assert_eq!(fact, "other");
        }
        other => panic!("expected UndeclaredFactRead, got {other:?}"),
    }
}

/// The plan carries the declaration's real shape, not a placeholder.
///
/// An empty or filled-in stub would make the plan silently disagree with the
/// module it claims to be a lowering of, and Stage B's exhaustiveness check
/// reads exactly this.
#[test]
fn the_plan_carries_real_config_shapes() {
    use brix_lower::l3_v2::{L3ConfigBodyV2, L3ConfigDeclV2};

    let plan = lower(
        "config Z = Hand | Field\n\
         config Card = MkCard(Int, Int)\n\
         config Stats = { atk: Int, def: Int }\n",
    )
    .expect("declarations lower");

    let configs: Vec<&L3ConfigDeclV2> = plan
        .items
        .iter()
        .filter_map(|i| match i {
            brix_lower::l3_v2::L3PlanItemV2::Config(c) => Some(c),
            _ => None,
        })
        .collect();
    assert_eq!(configs.len(), 3);

    assert_eq!(
        configs[0].body,
        L3ConfigBodyV2::Sum(vec![("Hand".into(), 0), ("Field".into(), 0)])
    );
    assert_eq!(
        configs[1].body,
        L3ConfigBodyV2::Sum(vec![("MkCard".into(), 2)]),
        "variant arity is carried, so a payload constructor is distinguishable"
    );
    assert_eq!(
        configs[2].body,
        L3ConfigBodyV2::Record(vec!["atk".into(), "def".into()])
    );
}
