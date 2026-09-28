//! ADR-0038: inferred rule/proposal dependencies and the `otherwise` fallback.

use brix_lower::{
    finite_decision_program_id, lower_finite_decision_plan, FiniteDecisionLowerError,
    FiniteDecisionPlan, FiniteDecisionRuntime, FiniteDecisionStop, FINITE_DECISION_PROFILE,
};
use brix_syntax::parse;

fn plan(source: &str) -> FiniteDecisionPlan {
    let module = parse(source).expect("fixture parses");
    lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).expect("fixture lowers")
}

fn lower_err(source: &str) -> FiniteDecisionLowerError {
    let module = parse(source).expect("fixture parses");
    lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE)
        .expect_err("fixture must be rejected")
}

// ---------------------------------------------------------------------------
// Inference equivalence: an explicit-list program and its inferred twin have
// the same program id.
// ---------------------------------------------------------------------------

const EXPLICIT: &str = r#"
config Decision = Ship | Hold

rule stock() = 12
rule threshold() = 10
rule can_ship(stock, threshold) = stock >= threshold

propose ship(can_ship) priority 10 when can_ship = Ship
propose hold() priority 100 when true = Hold

commit shipping from (ship, hold)
"#;

const INFERRED: &str = r#"
config Decision = Ship | Hold

rule stock = 12
rule threshold = 10
rule can_ship = stock >= threshold

propose ship priority 10 when can_ship = Ship
propose hold priority 100 when true = Hold

commit shipping from (ship, hold)
"#;

#[test]
fn inferred_and_explicit_twins_have_the_same_program_id() {
    let explicit = plan(EXPLICIT);
    let inferred = plan(INFERRED);
    assert_eq!(explicit.rules[2].depends_on, vec!["stock", "threshold"]);
    assert_eq!(inferred.rules[2].depends_on, vec!["stock", "threshold"]);
    assert_eq!(explicit.proposals[0].deps, vec!["can_ship"]);
    assert_eq!(inferred.proposals[0].deps, vec!["can_ship"]);
    assert_eq!(
        finite_decision_program_id(&explicit),
        finite_decision_program_id(&inferred)
    );
}

#[test]
fn inference_dependency_order_is_rule_declaration_order_not_expression_order() {
    // The body reads `b` before `a` syntactically; the canonical order must
    // still be declaration order (a, then b).
    let src = r#"
rule a = 1
rule b = 2
rule c = b + a

propose only priority 0 when true = c
commit pick from (only)
"#;
    let p = plan(src);
    let c = p.rules.iter().find(|r| r.name == "c").unwrap();
    assert_eq!(c.depends_on, vec!["a", "b"]);
}

#[test]
fn inferred_dependencies_run_and_select_identically_to_explicit() {
    let explicit_run = FiniteDecisionRuntime::build(&plan(EXPLICIT))
        .expect("runtime builds")
        .run();
    let inferred_run = FiniteDecisionRuntime::build(&plan(INFERRED))
        .expect("runtime builds")
        .run();
    assert_eq!(explicit_run.decision, inferred_run.decision);
    assert_eq!(
        explicit_run.journal.step_digests(),
        inferred_run.journal.step_digests()
    );
}

// ---------------------------------------------------------------------------
// Explicit lists keep their current (strict) meaning.
// ---------------------------------------------------------------------------

#[test]
fn explicit_empty_dependency_list_still_forbids_reading_a_rule() {
    let err = lower_err(
        r#"
rule a() = 1
rule b() = a
propose only priority 0 when true = b
commit pick from (only)
"#,
    );
    match err {
        FiniteDecisionLowerError::RuleDependencyError(_) => {}
        other => panic!("expected RuleDependencyError, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Forward references.
// ---------------------------------------------------------------------------

#[test]
fn inferred_rule_reading_a_later_rule_is_a_forward_reference_error() {
    let err = lower_err(
        r#"
rule a = b
rule b = 1
propose only priority 0 when true = a
commit pick from (only)
"#,
    );
    match &err {
        FiniteDecisionLowerError::ForwardRuleRead {
            reader_kind,
            reader,
            dep,
        } => {
            assert_eq!(*reader_kind, "rule");
            assert_eq!(reader, "a");
            assert_eq!(dep, "b");
        }
        other => panic!("expected ForwardRuleRead, got {other:?}"),
    }
    let msg = err.to_string();
    assert!(msg.contains("rule 'a' reads rule 'b'"));
    assert!(msg.contains("declared below it"));
    assert!(msg.contains("rules can only read rules declared above them"));
}

#[test]
fn inferred_rule_reading_itself_is_a_forward_reference_error() {
    let err = lower_err(
        r#"
rule a = a
propose only priority 0 when true = 1
commit pick from (only)
"#,
    );
    match &err {
        FiniteDecisionLowerError::ForwardRuleRead { reader, dep, .. } => {
            assert_eq!(reader, "a");
            assert_eq!(dep, "a");
        }
        other => panic!("expected ForwardRuleRead, got {other:?}"),
    }
    assert!(err.to_string().contains("reads itself"));
}

#[test]
fn inferred_proposal_reading_a_later_rule_is_a_forward_reference_error() {
    let err = lower_err(
        r#"
rule a = 1
propose only priority 0 when true = b
rule b = 2
commit pick from (only)
"#,
    );
    match &err {
        FiniteDecisionLowerError::ForwardRuleRead {
            reader_kind,
            reader,
            dep,
        } => {
            assert_eq!(*reader_kind, "proposal");
            assert_eq!(reader, "only");
            assert_eq!(dep, "b");
        }
        other => panic!("expected ForwardRuleRead, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// `otherwise`.
// ---------------------------------------------------------------------------

const OTHERWISE_SUGAR: &str = r#"
config Decision = Ship | Hold
rule stock = 4
propose ship priority 10 when stock >= 10 = Ship
propose hold otherwise = Hold
commit shipping from (ship, hold)
"#;

const OTHERWISE_EXPLICIT: &str = r#"
config Decision = Ship | Hold
rule stock = 4
propose ship priority 10 when stock >= 10 = Ship
propose hold priority 18446744073709551615 when true = Hold
commit shipping from (ship, hold)
"#;

#[test]
fn otherwise_desugars_to_the_identical_explicit_plan_and_program_id() {
    let sugar = plan(OTHERWISE_SUGAR);
    let explicit = plan(OTHERWISE_EXPLICIT);
    assert_eq!(
        finite_decision_program_id(&sugar),
        finite_decision_program_id(&explicit)
    );
    let hold = sugar.proposals.iter().find(|p| p.name == "hold").unwrap();
    assert_eq!(hold.priority, u64::MAX);
}

#[test]
fn otherwise_is_selected_only_when_it_is_the_sole_admitted_candidate() {
    let run = FiniteDecisionRuntime::build(&plan(OTHERWISE_SUGAR))
        .expect("runtime builds")
        .run();
    match run.stop {
        FiniteDecisionStop::Selected(ref d) => assert_eq!(d.candidate, "hold"),
        other => panic!("expected hold selected, got {other:?}"),
    }
}

#[test]
fn multiple_otherwise_in_one_commit_pool_is_rejected() {
    let err = lower_err(
        r#"
config Decision = Ship | Hold
propose a otherwise = Hold
propose b otherwise = Hold
commit pick from (a, b)
"#,
    );
    match err {
        FiniteDecisionLowerError::MultipleOtherwiseInCommit { commit } => {
            assert_eq!(commit, "pick");
        }
        other => panic!("expected MultipleOtherwiseInCommit, got {other:?}"),
    }
}

#[test]
fn otherwise_alongside_explicit_max_priority_in_the_same_pool_is_ambiguous() {
    let err = lower_err(
        r#"
config Decision = Ship | Hold
propose a otherwise = Hold
propose b priority 18446744073709551615 when true = Hold
commit pick from (a, b)
"#,
    );
    match err {
        FiniteDecisionLowerError::AmbiguousFallbackPriority {
            commit,
            otherwise,
            explicit,
        } => {
            assert_eq!(commit, "pick");
            assert_eq!(otherwise, "a");
            assert_eq!(explicit, "b");
        }
        other => panic!("expected AmbiguousFallbackPriority, got {other:?}"),
    }
}

#[test]
fn otherwise_with_explicit_dependencies_still_infers_nothing_extra() {
    let p = plan(
        r#"
config Decision = Ship | Hold
rule base = 3
propose a(base) otherwise = base
commit pick from (a)
"#,
    );
    let a = p.proposals.iter().find(|p| p.name == "a").unwrap();
    assert_eq!(a.deps, vec!["base"]);
    assert_eq!(a.priority, u64::MAX);
}
