//! ADR-0043: per-entity `decide` blocks.

use brix_lower::finite_decision::{
    finite_decision_program_id, lower_finite_decision_plan, FiniteDecisionDecideStop,
    FiniteDecisionLowerError, FiniteDecisionPlan, FiniteDecisionRuntime, FiniteDecisionStop,
    FINITE_DECISION_PROFILE,
};
use brix_lower::input::{
    canonicalize_input_shards, decode_input_shard, InputLimits, InputSnapshot,
};
use brix_lower::l3_v2::L3ValueV2;
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

fn snapshot_from_json(json: &str) -> InputSnapshot {
    let limits = InputLimits::default();
    let shard = decode_input_shard(json.as_bytes(), &limits).expect("shard decodes");
    canonicalize_input_shards(vec![shard], &limits).expect("snapshot canonicalizes")
}

const BASIC: &str = r#"
config Order = { units: Int }
config Ship = Ship | Hold

input orders: List<Order> max 8

decide status for o in orders {
  propose ship priority 10 when o.units <= 10 = Ship
  propose hold otherwise = Hold
}

commit noop from (only)
propose only otherwise = Hold
"#;

fn orders_snapshot(units: &[i64]) -> InputSnapshot {
    let items: Vec<String> = units
        .iter()
        .map(|u| {
            format!(
                r#"{{"type":"record","nominal":"Order","fields":[{{"name":"units","value":{{"type":"int","value":"{u}"}}}}]}}"#
            )
        })
        .collect();
    let json = format!(
        r#"{{"schema":"brix.input@3","values":{{"orders":{{"type":"list","items":[{}]}}}}}}"#,
        items.join(",")
    );
    snapshot_from_json(&json)
}

#[test]
fn each_order_gets_its_own_ship_or_hold_decision() {
    let p = plan(BASIC);
    let snapshot = orders_snapshot(&[5, 20, 3]);
    let runtime = FiniteDecisionRuntime::build_with_inputs(&p, &snapshot).expect("runtime builds");
    let run = runtime.run();
    assert_eq!(run.decides.len(), 1);
    let d = &run.decides[0];
    assert_eq!(d.decide, "status");
    assert!(matches!(d.stop, FiniteDecisionDecideStop::Settled));
    assert_eq!(d.instances.len(), 3);
    assert_eq!(d.instances[0].index, 0);
    assert_eq!(d.instances[1].index, 1);
    assert_eq!(d.instances[2].index, 2);
    assert_eq!(d.instances[0].decision.as_ref().unwrap().candidate, "ship");
    assert_eq!(d.instances[1].decision.as_ref().unwrap().candidate, "hold");
    assert_eq!(d.instances[2].decision.as_ref().unwrap().candidate, "ship");
    // Top-level commit pool is unaffected.
    assert!(matches!(run.stop, FiniteDecisionStop::Selected(_)));

    let decided = d.decided_values().expect("all instances selected");
    assert_eq!(decided.len(), 3);
}

#[test]
fn empty_list_gives_zero_instances() {
    let p = plan(BASIC);
    let snapshot = orders_snapshot(&[]);
    let runtime = FiniteDecisionRuntime::build_with_inputs(&p, &snapshot).expect("runtime builds");
    let run = runtime.run();
    let d = &run.decides[0];
    assert!(matches!(d.stop, FiniteDecisionDecideStop::Settled));
    assert!(d.instances.is_empty());
    assert_eq!(d.decided_values(), Some(Vec::new()));
}

#[test]
fn determinism_across_repeated_runs() {
    let p = plan(BASIC);
    let snapshot = orders_snapshot(&[5, 20, 3, 11, 2]);
    let runtime = FiniteDecisionRuntime::build_with_inputs(&p, &snapshot).expect("runtime builds");
    let run1 = runtime.run();
    let run2 = runtime.run();
    let names1: Vec<_> = run1.decides[0]
        .instances
        .iter()
        .map(|i| i.decision.as_ref().unwrap().candidate.clone())
        .collect();
    let names2: Vec<_> = run2.decides[0]
        .instances
        .iter()
        .map(|i| i.decision.as_ref().unwrap().candidate.clone())
        .collect();
    assert_eq!(names1, names2);
    assert_eq!(run1.journal.len(), run2.journal.len());
}

#[test]
fn program_id_is_unaffected_by_reordering_or_reparsing_a_decide_free_program() {
    let without_decide = r#"
config Ship = Ship | Hold
commit noop from (only)
propose only otherwise = Hold
"#;
    let p1 = plan(without_decide);
    let p2 = plan(without_decide);
    assert_eq!(
        finite_decision_program_id(&p1),
        finite_decision_program_id(&p2)
    );
    assert!(p1.decides.is_empty());
}

#[test]
fn decide_and_decide_free_twins_differ_in_program_id() {
    let without_decide = r#"
config Order = { units: Int }
config Ship = Ship | Hold
input orders: List<Order> max 8
commit noop from (only)
propose only otherwise = Hold
"#;
    let id_without = finite_decision_program_id(&plan(without_decide));
    let id_with = finite_decision_program_id(&plan(BASIC));
    assert_ne!(id_without, id_with);
}

#[test]
fn duplicate_decide_name_is_rejected() {
    let src = r#"
config Order = { units: Int }
config Ship = Ship | Hold
input orders: List<Order> max 8
decide status for o in orders {
  propose ship otherwise = Ship
}
decide status for o in orders {
  propose hold otherwise = Hold
}
commit noop from (only)
propose only otherwise = Hold
"#;
    assert!(matches!(
        lower_err(src),
        FiniteDecisionLowerError::DuplicateDecideName(name) if name == "status"
    ));
}

#[test]
fn empty_decide_block_is_rejected() {
    let src = r#"
config Order = { units: Int }
config Ship = Ship | Hold
input orders: List<Order> max 8
decide status for o in orders {
}
commit noop from (only)
propose only otherwise = Hold
"#;
    assert!(matches!(
        lower_err(src),
        FiniteDecisionLowerError::EmptyDecide(name) if name == "status"
    ));
}

#[test]
fn candidate_owned_by_decide_cannot_also_be_a_commit_candidate() {
    let src = r#"
config Order = { units: Int }
config Ship = Ship | Hold
input orders: List<Order> max 8
decide status for o in orders {
  propose ship otherwise = Ship
}
commit noop from (ship)
"#;
    assert!(matches!(
        lower_err(src),
        FiniteDecisionLowerError::CandidateOwnedByDecide { .. }
    ));
}

#[test]
fn binder_shadows_an_outer_let_of_the_same_name() {
    let src = r#"
config Order = { units: Int }
config Ship = Ship | Hold
input orders: List<Order> max 8
let o = 999
decide status for o in orders {
  propose ship priority 1 when o.units > 0 = Ship
  propose hold otherwise = Hold
}
commit noop from (only)
propose only otherwise = Hold
"#;
    let p = plan(src);
    let snapshot = orders_snapshot(&[7]);
    let runtime = FiniteDecisionRuntime::build_with_inputs(&p, &snapshot).expect("builds");
    let run = runtime.run();
    assert_eq!(
        run.decides[0].instances[0]
            .decision
            .as_ref()
            .unwrap()
            .candidate,
        "ship"
    );
}

#[test]
fn show_of_a_decide_name_is_the_list_of_decided_values_in_order() {
    let src = r#"
config Order = { units: Int }
config Ship = Ship | Hold
input orders: List<Order> max 8
decide status for o in orders {
  propose ship priority 10 when o.units <= 10 = Ship
  propose hold otherwise = Hold
}
commit noop from (only)
propose only otherwise = Hold
show status
"#;
    let module = parse(src).expect("parses");
    let p = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).expect("lowers");
    let snapshot = orders_snapshot(&[5, 20]);
    let runtime = FiniteDecisionRuntime::build_with_inputs(&p, &snapshot).expect("builds");
    let run = runtime.run();
    let results = runtime.evaluate_shows(&run).expect("shows evaluate");
    assert_eq!(results.len(), 1);
    match &results[0] {
        L3ValueV2::List(items) => assert_eq!(items.len(), 2),
        other => panic!("expected a list, found {other:?}"),
    }
}

#[test]
fn exceeding_the_program_wide_instance_cap_is_a_typed_unknown() {
    use brix_lower::FiniteDecisionRuntime;

    // MAX_TOTAL_DECIDE_INSTANCES is 4096; two decide blocks each over a
    // 2049-element literal list (each under MAX_DERIVED_LIST_LEN = 4096 on
    // its own, so each block's own list evaluates cleanly) together exceed
    // the program-wide cap.
    let elems: String = std::iter::repeat_n("1", 2049).collect::<Vec<_>>().join(",");
    let src = format!(
        r#"
config Ship = Ship | Hold
let xs = [{elems}]
let ys = [{elems}]
decide a for x in xs {{
  propose sa priority 1 when x > 0 = Ship
  propose ha otherwise = Hold
}}
decide b for y in ys {{
  propose sb priority 1 when y > 0 = Ship
  propose hb otherwise = Hold
}}
commit noop from (only)
propose only otherwise = Hold
"#
    );
    let p = plan(&src);
    let runtime = FiniteDecisionRuntime::build(&p).expect("builds with no inputs");
    let run = runtime.run();
    assert_eq!(run.decides.len(), 2);
    assert!(run.decides[0].is_unknown());
    assert!(run.decides[1].is_unknown());
    // An unrelated commit pool is unaffected by the decide-block cap fault.
    assert!(matches!(run.stop, FiniteDecisionStop::Selected(_)));
}

#[test]
fn a_fault_in_one_instance_makes_the_whole_decide_block_unknown() {
    // Neither proposal's guard admits when `units == 0` and there is no
    // `otherwise`, so the deliberation frontier for that one instance
    // quiesces with no fault — use a genuine fault instead: a guard that
    // divides so a specific element trips `EvalFault::DivisionByZero`.
    let src = r#"
config Order = { units: Int }
config Ship = Ship | Hold
input orders: List<Order> max 8
decide status for o in orders {
  propose ship priority 1 when div_floor(10, o.units) > 0 = Ship
  propose hold otherwise = Hold
}
commit noop from (only)
propose only otherwise = Hold
"#;
    let p = plan(src);
    let snapshot = orders_snapshot(&[5, 0, 3]);
    let runtime = FiniteDecisionRuntime::build_with_inputs(&p, &snapshot).expect("builds");
    let run = runtime.run();
    assert!(run.decides[0].is_unknown());
    assert!(run.decides[0].instances.is_empty());
    // The unrelated commit pool still settles.
    assert!(matches!(run.stop, FiniteDecisionStop::Selected(_)));
}
