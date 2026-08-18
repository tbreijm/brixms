//! Witness-frontier acceptance gates over ordinary source rules.

use brix_lower::{
    lower_l3_plan, lower_witness_frontier_plan, run_l3_plan, run_witness_frontier_once,
    witness_frontier_program_id, L3AdmChoice, PlanLimitsV1, WitnessFrontierRuntime,
    L3_PROFILE_MARKER_V1, L3_WITNESS_FRONTIER_PROFILE,
};
use brix_syntax::parse;
use soc_core::audit::AuditResult;
use soc_core::saturate::SaturationBudget;

const TWO_ARRANGEMENTS: &str =
    "config Arrangement = A | B\nrule arrangement_a() = A\nrule arrangement_b() = B\n";

fn plan(source: &str) -> brix_lower::WitnessFrontierPlan {
    let module = parse(source).expect("fixture parses");
    lower_witness_frontier_plan(
        &module,
        L3_WITNESS_FRONTIER_PROFILE,
        &PlanLimitsV1::generous(),
    )
    .expect("fixture lowers")
}

#[test]
fn both_rule_witnesses_coexist_before_selection() {
    let runtime = WitnessFrontierRuntime::build(&plan(TWO_ARRANGEMENTS));
    let candidates = runtime.candidates_at_initial();
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].ordinal, 0);
    assert_eq!(candidates[1].ordinal, 1);
    assert_ne!(candidates[0].rule, candidates[1].rule);
}

#[test]
fn keyed_frontier_commits_exactly_one_rule_witness() {
    let runtime = WitnessFrontierRuntime::build(&plan(TWO_ARRANGEMENTS));
    let run = run_witness_frontier_once(&runtime).expect("one witness commits");
    assert_eq!(
        run.selected.ordinal, 0,
        "source order is deterministic priority"
    );
    assert_eq!(run.journal.len(), 1);
    assert_eq!(run.journal.steps()[0].src, runtime.initial_world);
    assert_eq!(run.journal.steps()[0].dst, run.final_world);
}

#[test]
fn repeated_runs_have_identical_selection_journal_and_world() {
    let first_runtime = WitnessFrontierRuntime::build(&plan(TWO_ARRANGEMENTS));
    let first = run_witness_frontier_once(&first_runtime).expect("first run");
    for _ in 0..8 {
        let runtime = WitnessFrontierRuntime::build(&plan(TWO_ARRANGEMENTS));
        let run = run_witness_frontier_once(&runtime).expect("repeat run");
        assert_eq!(run.selected, first.selected);
        assert_eq!(run.final_world, first.final_world);
        assert_eq!(run.journal.step_digests(), first.journal.step_digests());
    }
}

#[test]
fn audit_is_explicit_and_rederives_the_selected_witness() {
    let runtime = WitnessFrontierRuntime::build(&plan(TWO_ARRANGEMENTS));
    let run = run_witness_frontier_once(&runtime).expect("one witness commits");
    assert!(matches!(
        runtime.audit(&run.journal).as_slice(),
        [AuditResult::Audited(_)]
    ));
}

#[test]
fn identity_is_canonical_and_limits_are_semantic() {
    let a = plan(TWO_ARRANGEMENTS);
    let b = plan("config Arrangement=A|B\nrule arrangement_a()=A\nrule arrangement_b()=B\n");
    assert_eq!(
        witness_frontier_program_id(&a),
        witness_frontier_program_id(&b)
    );
    let module = parse(TWO_ARRANGEMENTS).expect("fixture parses");
    let mut limits = PlanLimitsV1::generous();
    limits.max_selected_rules = 1;
    assert!(lower_witness_frontier_plan(&module, L3_WITNESS_FRONTIER_PROFILE, &limits).is_err());
}

#[test]
fn v1_remains_the_serial_rule_agenda() {
    let module = parse(TWO_ARRANGEMENTS).expect("fixture parses");
    let v1 = lower_l3_plan(&module, L3_PROFILE_MARKER_V1, &PlanLimitsV1::generous())
        .expect("v1 remains valid");
    let report = run_l3_plan(&v1, L3AdmChoice::Compiled, SaturationBudget::uniform(32));
    assert_eq!(report.journal.len(), 2, "v1 serially commits both rules");
}
