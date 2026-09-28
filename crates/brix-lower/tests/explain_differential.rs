//! Differential tests for `FiniteDecisionRuntime::explain_candidate` (ADR-0030).
//!
//! The load-bearing property under test: a trace never disagrees with the
//! real evaluator's own result. For every candidate in every example program
//! (and several hand-written fixtures exercising short-circuiting, `match`
//! arm selection, nested helper calls, and truncation), this asserts that
//! the trace's root guard value equals the runtime's actual admission
//! result, and that every fact/input value the trace reports equals the
//! runtime's own `facts`/`inputs` value for that name.

use std::fs;
use std::path::{Path, PathBuf};

use brix_lower::l3_v2::L3ValueV2;
use brix_lower::{
    canonicalize_input_shards, decode_input_shard, lower_finite_decision_plan, CandidateStatus,
    ExplainOutcome, FactOrigin, FiniteDecisionPlan, FiniteDecisionRun, FiniteDecisionRuntime,
    InputLimits, InputSnapshot, NodeRef, TraceNode, TraceOutcome, FINITE_DECISION_PROFILE,
};

fn repo_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .expect("crates parent")
        .parent()
        .expect("repo root")
        .to_path_buf()
}

fn load_plan_from_source(source: &str) -> FiniteDecisionPlan {
    let mut module = brix_syntax::parse(source).expect("fixture parses");
    // `show <commit-name>` is a display-only surface directive the finite-
    // decision profile does not lower (mirrors the CLI's own
    // `prepare_finite_decision_module`, which strips it before lowering).
    module
        .items
        .retain(|item| !matches!(item, brix_syntax::ast::Item::Show(_)));
    lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).expect("fixture lowers")
}

fn load_plan_from_file(path: &Path) -> FiniteDecisionPlan {
    let source = fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("reading example {}: {e}", path.display()));
    load_plan_from_source(&source)
}

fn load_snapshot_from_file(path: &Path) -> InputSnapshot {
    let limits = InputLimits::default();
    let json = fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("reading input {}: {e}", path.display()));
    let shard = decode_input_shard(json.as_bytes(), &limits).expect("input json decodes");
    canonicalize_input_shards(vec![shard], &limits).expect("input snapshot canonicalizes")
}

/// Recursively assert that every rule-fact or input leaf in `node` carries
/// exactly the value `run` itself already committed for that name — the
/// same evaluator, re-affirmed, never a second opinion.
fn assert_refs_match_run(node: &TraceNode, run: &FiniteDecisionRun) {
    match node.node_ref {
        Some(NodeRef::Rule) => {
            let expected = run
                .facts
                .iter()
                .find(|f| f.rule == node.source)
                .map(|f| f.value.clone());
            assert_eq!(
                node.value().cloned(),
                expected,
                "rule fact '{}' trace value disagrees with run.facts",
                node.source
            );
        }
        Some(NodeRef::Input) => {
            let expected = run
                .inputs
                .iter()
                .find(|i| i.name == node.source)
                .map(|i| i.value.clone());
            assert_eq!(
                node.value().cloned(),
                expected,
                "input '{}' trace value disagrees with run.inputs",
                node.source
            );
        }
        Some(NodeRef::Let) | None => {}
    }
    for child in &node.children {
        assert_refs_match_run(child, run);
    }
}

fn contains_truncated(node: &TraceNode) -> bool {
    matches!(node.outcome, TraceOutcome::Truncated) || node.children.iter().any(contains_truncated)
}

/// Run the full differential check for one candidate of an already-built
/// runtime/run pair.
fn check_candidate(runtime: &FiniteDecisionRuntime, run: &FiniteDecisionRun, name: &str) {
    let outcome = runtime
        .explain_candidate(name)
        .unwrap_or_else(|e| panic!("explain_candidate('{name}') faulted: {e}"));
    let ExplainOutcome::Explained(expl) = outcome else {
        panic!("candidate '{name}' unexpectedly not found while explaining");
    };
    assert_eq!(expl.candidate, name);

    // The trace's root guard value equals the runtime's actual admission result.
    let admitted = !matches!(
        run.status_of(name),
        Some(CandidateStatus::RejectedGuardFalse)
    );
    match expl.guard.value() {
        Some(L3ValueV2::Bool(b)) => assert_eq!(
            *b, admitted,
            "candidate '{name}': guard trace value disagrees with actual admission"
        ),
        other => panic!("candidate '{name}': guard trace did not resolve to Bool: {other:?}"),
    }
    assert_refs_match_run(&expl.guard, run);
    assert_refs_match_run(&expl.value, run);

    // Every fact value in the trace equals the runtime's facts value.
    for fact in &expl.facts {
        match &fact.origin {
            FactOrigin::Rule { .. } => {
                let expected = run
                    .facts
                    .iter()
                    .find(|f| f.rule == fact.name)
                    .unwrap_or_else(|| panic!("fact '{}' missing from run.facts", fact.name))
                    .value
                    .clone();
                assert_eq!(fact.value, expected, "fact '{}' value mismatch", fact.name);
            }
            FactOrigin::Input => {
                let expected = run
                    .inputs
                    .iter()
                    .find(|i| i.name == fact.name)
                    .unwrap_or_else(|| panic!("input '{}' missing from run.inputs", fact.name))
                    .value
                    .clone();
                assert_eq!(fact.value, expected, "input '{}' value mismatch", fact.name);
            }
            FactOrigin::Let => {
                // `let` values are not published on `run`; the trace's own
                // root value is asserted against `fact.value` below instead.
            }
        }
        if let Some(trace) = &fact.trace {
            assert_eq!(
                trace.value().cloned(),
                Some(fact.value.clone()),
                "fact '{}' trace root disagrees with its own reported value",
                fact.name
            );
            assert_refs_match_run(trace, run);
        }
    }
}

fn check_example(brix_path: &str, json_path: Option<&str>) {
    let root = repo_root();
    let plan = load_plan_from_file(&root.join(brix_path));
    let runtime = match json_path {
        Some(p) => {
            let snapshot = load_snapshot_from_file(&root.join(p));
            FiniteDecisionRuntime::build_with_inputs(&plan, &snapshot)
                .unwrap_or_else(|e| panic!("{brix_path}: runtime build failed: {e}"))
        }
        None => FiniteDecisionRuntime::build(&plan)
            .unwrap_or_else(|e| panic!("{brix_path}: runtime build failed: {e}")),
    };
    let run = runtime.run();
    assert!(
        !run.is_unknown(),
        "{brix_path}: run unexpectedly Unknown: {:?}",
        run.stop
    );

    for candidate in plan.commit.candidates.clone() {
        check_candidate(&runtime, &run, &candidate);
    }
}

#[test]
fn test_differential_shipping() {
    check_example("examples/shipping.brix", None);
}

#[test]
fn test_differential_shipping_input() {
    check_example(
        "examples/shipping-input.brix",
        Some("examples/shipping-input.json"),
    );
}

#[test]
fn test_differential_shipping_functions() {
    check_example(
        "examples/shipping-functions.brix",
        Some("examples/shipping-functions.json"),
    );
}

#[test]
fn test_differential_allocation() {
    check_example("examples/allocation.brix", Some("examples/allocation.json"));
}

#[test]
fn test_differential_order_policy() {
    check_example(
        "examples/order-policy.brix",
        Some("examples/order-policy.json"),
    );
}

#[test]
fn test_differential_fulfillment() {
    // Exercises the differential property over every list/relational form
    // (ADR-0037, ADR-0040): fold, filter, comprehension, `in`, `len`, and a
    // `match`-guarded `max` — across all four decision outcomes the example
    // suite covers.
    check_example(
        "examples/fulfillment.brix",
        Some("examples/fulfillment.json"),
    );
    check_example(
        "examples/fulfillment.brix",
        Some("examples/tests/fulfillment-ship-all.json"),
    );
    check_example(
        "examples/fulfillment.brix",
        Some("examples/tests/fulfillment-hold-empty.json"),
    );
    check_example(
        "examples/fulfillment.brix",
        Some("examples/tests/fulfillment-hold-all-short.json"),
    );
}

// ---------------------------------------------------------------------------
// Hand-written fixtures: short-circuit, match arm selection, nested helpers,
// and truncation.
// ---------------------------------------------------------------------------

const AND_SHORT_CIRCUIT_UNEVEN: &str = r#"
config Decision = Balanced | Insufficient | Uneven

config Allocation = { price_cents: Int, car_count: Int }

input batch: Allocation

fn per_car_cents(a: Allocation): Int = div_floor(a.price_cents, a.car_count)
fn leftover_cents(a: Allocation): Int = mod_euclid(a.price_cents, a.car_count)

rule share() = per_car_cents(batch)
rule leftover() = leftover_cents(batch)
rule evenly_split(leftover) = leftover == 0
rule meets_minimum(share) = share >= 500

propose balanced(evenly_split, meets_minimum) priority 10 when evenly_split && meets_minimum = Balanced
propose insufficient(evenly_split, meets_minimum) priority 20 when evenly_split && !meets_minimum = Insufficient
propose uneven() priority 100 when true = Uneven

commit decision from (balanced, insufficient, uneven)
"#;

fn uneven_snapshot() -> InputSnapshot {
    // price_cents=12001, car_count=5: not evenly divisible, so
    // `evenly_split` is false and every guard's right `&&` operand is
    // short-circuited.
    load_snapshot_from_source(
        r#"{
            "schema": "brix.input@2",
            "values": {
                "batch": {
                    "type": "record",
                    "nominal": "Allocation",
                    "fields": [
                        {"name": "price_cents", "value": {"type": "int", "value": "12001"}},
                        {"name": "car_count", "value": {"type": "int", "value": "5"}}
                    ]
                }
            }
        }"#,
    )
}

fn load_snapshot_from_source(json: &str) -> InputSnapshot {
    let limits = InputLimits::default();
    let shard = decode_input_shard(json.as_bytes(), &limits).expect("input json decodes");
    canonicalize_input_shards(vec![shard], &limits).expect("input snapshot canonicalizes")
}

#[test]
fn test_and_short_circuit_marks_right_operand_not_evaluated() {
    let plan = load_plan_from_source(AND_SHORT_CIRCUIT_UNEVEN);
    let snapshot = uneven_snapshot();
    let runtime = FiniteDecisionRuntime::build_with_inputs(&plan, &snapshot).expect("builds");
    let run = runtime.run();
    assert!(!run.is_unknown());

    // Differential property holds for every candidate here too.
    for candidate in plan.commit.candidates.clone() {
        check_candidate(&runtime, &run, &candidate);
    }

    // `balanced`'s guard is `evenly_split && meets_minimum`; `evenly_split`
    // is false, so `meets_minimum` must be reported not evaluated.
    let ExplainOutcome::Explained(balanced) = runtime.explain_candidate("balanced").unwrap() else {
        panic!("balanced not found");
    };
    assert_eq!(balanced.guard.value(), Some(&L3ValueV2::Bool(false)));
    assert_eq!(balanced.guard.children.len(), 2);
    assert_eq!(
        balanced.guard.children[0].value(),
        Some(&L3ValueV2::Bool(false))
    );
    assert_eq!(
        balanced.guard.children[1].outcome,
        TraceOutcome::NotEvaluated,
        "meets_minimum must not be evaluated once evenly_split is false"
    );

    // `insufficient`'s guard is `evenly_split && !meets_minimum`; the whole
    // `!meets_minimum` subtree is short-circuited, not partially evaluated.
    let ExplainOutcome::Explained(insufficient) =
        runtime.explain_candidate("insufficient").unwrap()
    else {
        panic!("insufficient not found");
    };
    assert_eq!(insufficient.guard.children.len(), 2);
    assert_eq!(
        insufficient.guard.children[1].outcome,
        TraceOutcome::NotEvaluated
    );
    assert!(
        insufficient.guard.children[1].children.is_empty(),
        "a not-evaluated subtree must not recurse into its own children"
    );
}

const MATCH_ARM_SELECTION: &str = r#"
config Decision = Expedite | Ship | Hold

input stock: Int
input eligible: Bool
input region: Str

rule threshold() = 10
rule destination() = region
rule valid_destination(destination) = destination == "EU-NORTH"

rule can_ship(threshold, valid_destination) = match eligible {
  true => match valid_destination {
    true => stock >= threshold
    false => false
  }
  false => false
}

propose expedite() priority 5 when stock >= 50 = Expedite
propose ship(can_ship) priority 10 when can_ship == true = Ship
propose hold() priority 100 when true = Hold

commit shipping from (expedite, ship, hold)
"#;

#[test]
fn test_match_untaken_arm_marked_not_evaluated() {
    let plan = load_plan_from_source(MATCH_ARM_SELECTION);
    let snapshot = load_snapshot_from_source(
        r#"{
            "schema": "brix.input@1",
            "values": {
                "stock": {"type": "int", "value": "12"},
                "eligible": {"type": "bool", "value": false},
                "region": {"type": "string", "value": "EU-NORTH"}
            }
        }"#,
    );
    let runtime = FiniteDecisionRuntime::build_with_inputs(&plan, &snapshot).expect("builds");
    let run = runtime.run();
    assert!(!run.is_unknown());
    for candidate in plan.commit.candidates.clone() {
        check_candidate(&runtime, &run, &candidate);
    }

    let ExplainOutcome::Explained(ship) = runtime.explain_candidate("ship").unwrap() else {
        panic!("ship not found");
    };
    let can_ship_fact = ship
        .facts
        .iter()
        .find(|f| f.name == "can_ship")
        .expect("can_ship read");
    assert_eq!(can_ship_fact.value, L3ValueV2::Bool(false));
    let trace = can_ship_fact.trace.as_ref().expect("rule trace present");
    // Root is the outer match; children[0] is the scrutinee `eligible`,
    // children[1] the `true` arm (untaken, since eligible is false),
    // children[2] the `false` arm (taken).
    assert_eq!(trace.children.len(), 3);
    assert_eq!(trace.children[1].outcome, TraceOutcome::NotEvaluated);
    assert!(trace.children[1].children.is_empty());
    assert_eq!(trace.children[2].value(), Some(&L3ValueV2::Bool(false)));
}

const NESTED_HELPER: &str = r#"
config Decision = Yes | No

input x: Int

fn inner(n: Int): Bool = n >= 5
fn outer(n: Int): Bool = inner(n)

propose p() priority 10 when outer(x) = Yes
propose q() priority 100 when true = No

commit c from (p, q)
"#;

#[test]
fn test_helper_body_expands_at_most_one_level() {
    let plan = load_plan_from_source(NESTED_HELPER);
    let snapshot = load_snapshot_from_source(
        r#"{"schema": "brix.input@1", "values": {"x": {"type": "int", "value": "7"}}}"#,
    );
    let runtime = FiniteDecisionRuntime::build_with_inputs(&plan, &snapshot).expect("builds");
    let run = runtime.run();
    assert!(!run.is_unknown());
    for candidate in plan.commit.candidates.clone() {
        check_candidate(&runtime, &run, &candidate);
    }

    let ExplainOutcome::Explained(p) = runtime.explain_candidate("p").unwrap() else {
        panic!("p not found");
    };
    // `outer(x)`: one argument trace, plus one expanded body trace (the
    // `inner(n)` call).
    assert_eq!(p.guard.value(), Some(&L3ValueV2::Bool(true)));
    assert_eq!(p.guard.children.len(), 2);
    let body_trace = &p.guard.children[1];
    assert_eq!(body_trace.source, "inner(n)");
    assert_eq!(body_trace.value(), Some(&L3ValueV2::Bool(true)));
    // `inner(n)` itself is a call one level deeper: its own argument is
    // shown, but its body is *not* expanded again.
    assert_eq!(
        body_trace.children.len(),
        1,
        "a call reached while already inside an expanded body must not expand its own body"
    );
}

#[test]
fn test_truncation_marker_on_oversized_rule_body() {
    fn balanced_sum(n: usize) -> String {
        if n <= 1 {
            "1".to_string()
        } else {
            let left = n / 2;
            let right = n - left;
            format!("({} + {})", balanced_sum(left), balanced_sum(right))
        }
    }

    // 300 leaves => 599 AST nodes for this one rule body, comfortably over
    // the 512-node explanation budget while staying well under the plan's
    // own expression node/depth limits.
    let expr = balanced_sum(300);
    let source = format!(
        r#"
config Decision = Yes | No

rule big() = {expr}

propose p(big) priority 10 when big >= 0 = Yes
propose q() priority 100 when true = No

commit c from (p, q)
"#
    );
    let plan = load_plan_from_source(&source);
    let runtime = FiniteDecisionRuntime::build(&plan).expect("builds");
    let run = runtime.run();
    assert!(!run.is_unknown());
    check_candidate(&runtime, &run, "p");
    check_candidate(&runtime, &run, "q");

    let ExplainOutcome::Explained(p) = runtime.explain_candidate("p").unwrap() else {
        panic!("p not found");
    };
    assert!(p.truncated, "explanation must report truncation");
    let big_fact = p.facts.iter().find(|f| f.name == "big").expect("big read");
    assert_eq!(big_fact.value, L3ValueV2::Int(300));
    let trace = big_fact.trace.as_ref().expect("rule trace present");
    assert!(
        contains_truncated(trace),
        "the oversized rule body's trace must contain an explicit truncation marker"
    );
}

// ---------------------------------------------------------------------------
// Bounded list-form summaries (ADR-0040).
// ---------------------------------------------------------------------------

fn contains_summarized(node: &TraceNode) -> bool {
    matches!(node.outcome, TraceOutcome::Summarized { .. })
        || node.children.iter().any(contains_summarized)
}

const LIST_FOLD_OVER_CAP: &str = r#"
config Decision = Yes | No

input xs: List<Int> max 16

rule total() = sum(xs, x => x)

propose p(total) priority 10 when total >= 0 = Yes
propose q() priority 100 when true = No

commit c from (p, q)
"#;

#[test]
fn test_fold_over_trace_cap_reports_summarized_marker() {
    // 10 elements: over `MAX_LIST_TRACE_ELEMENTS` (5), under `max 16`.
    let items = (1..=10)
        .map(|n| format!(r#"{{"type":"int","value":"{n}"}}"#))
        .collect::<Vec<_>>()
        .join(",");
    let json = format!(
        r#"{{"schema":"brix.input@3","values":{{"xs":{{"type":"list","items":[{items}]}}}}}}"#
    );

    let plan = load_plan_from_source(LIST_FOLD_OVER_CAP);
    let snapshot = load_snapshot_from_source(&json);
    let runtime = FiniteDecisionRuntime::build_with_inputs(&plan, &snapshot).expect("builds");
    let run = runtime.run();
    assert!(!run.is_unknown());
    for candidate in plan.commit.candidates.clone() {
        check_candidate(&runtime, &run, &candidate);
    }

    let ExplainOutcome::Explained(p) = runtime.explain_candidate("p").unwrap() else {
        panic!("p not found");
    };
    let total_fact = p
        .facts
        .iter()
        .find(|f| f.name == "total")
        .expect("total read");
    assert_eq!(total_fact.value, L3ValueV2::Int(55));
    let trace = total_fact.trace.as_ref().expect("rule trace present");
    assert!(
        contains_summarized(trace),
        "a fold over more elements than the trace cap must report an explicit Summarized marker, \
         even though `total`'s own value (55) is the real evaluator's sum over all 10 elements"
    );
}
