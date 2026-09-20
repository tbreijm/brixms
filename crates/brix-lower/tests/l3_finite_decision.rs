//! Finite-decision alpha acceptance tests and specification verifications (ADR-0030).

use brix_lower::{
    finite_decision_program_id, lower_finite_decision_plan, lower_l3_plan,
    run_finite_decision_plan, run_l3_plan, CandidateStatus, FiniteDecisionLowerError,
    FiniteDecisionPlan, FiniteDecisionRuntime, FiniteDecisionStop, FiniteDecisionUnknownReason,
    L3AdmChoice, Outcome, PlanLimitsV1, FINITE_DECISION_PROFILE, L3_PROFILE_MARKER_V1,
};
use brix_syntax::parse;
use soc_core::audit::AuditResult;
use soc_core::saturate::SaturationBudget;
use soc_regimes::finite_frontier::{EvaluationFault, WhyExplanation, WhyNotExplanation};

const SHIPPING_EXAMPLE: &str = r#"
config Decision = Expedite | Ship | Hold

rule stock() = 12
rule threshold() = 10

propose expedite(stock) priority 5 when stock >= 50 = Expedite
propose ship(stock, threshold) priority 10 when stock >= threshold = Ship
propose hold() priority 100 when true = Hold

commit shipping_decision from (expedite, ship, hold)
"#;

const TWO_CANDIDATES: &str = r#"
config Arrangement = A | B

rule base() = 1

propose opt_a(base) priority 10 when base == 1 = A
propose opt_b(base) priority 20 when base == 1 = B

commit pick from (opt_a, opt_b)
"#;

fn plan(source: &str) -> FiniteDecisionPlan {
    let module = parse(source).expect("fixture parses");
    lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).expect("fixture lowers")
}

// ---------------------------------------------------------------------------
// Ported Acceptance Gates (ADR-0029 -> ADR-0030)
// ---------------------------------------------------------------------------

#[test]
fn candidates_coexist_before_selection() {
    let runtime = FiniteDecisionRuntime::build(&plan(TWO_CANDIDATES)).expect("runtime builds");
    let candidates = runtime.candidates_at_initial();
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].0, "opt_a");
    assert_eq!(candidates[0].1, 10);
    assert_eq!(candidates[1].0, "opt_b");
    assert_eq!(candidates[1].1, 20);
    assert_ne!(candidates[0].2, candidates[1].2);
}

#[test]
fn keyed_frontier_commits_exactly_one_decision() {
    let runtime = FiniteDecisionRuntime::build(&plan(TWO_CANDIDATES)).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_selected(), "expected clean selection");
    let decision = run.decision.expect("decision selected");
    assert_eq!(decision.candidate, "opt_a", "lower priority wins");
    assert_eq!(decision.priority, 10);
    assert_eq!(decision.grade, Outcome::Derived);

    assert_eq!(run.journal.len(), 1, "exactly one committed step");
    assert_eq!(run.journal.steps()[0].src, runtime.initial_world);
    assert_eq!(run.journal.steps()[0].dst, run.final_world);
}

#[test]
fn repeated_runs_have_identical_selection_journal_and_world() {
    let p = plan(TWO_CANDIDATES);
    let first_runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let first = first_runtime.run();

    for _ in 0..8 {
        let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
        let run = runtime.run();
        assert_eq!(run.decision, first.decision);
        assert_eq!(run.final_world, first.final_world);
        assert_eq!(run.journal.step_digests(), first.journal.step_digests());
    }
}

#[test]
fn audit_is_explicit_and_rederives_the_selected_decision() {
    let runtime = FiniteDecisionRuntime::build(&plan(TWO_CANDIDATES)).expect("runtime builds");
    let run = runtime.run();
    assert_eq!(run.journal.len(), 1);
    let audit_results = runtime.audit(&run.journal);
    assert!(
        matches!(audit_results.as_slice(), [AuditResult::Audited(_)]),
        "audit produces separate Audited artifacts"
    );
}

#[test]
fn identity_is_canonical_and_binds_full_specification() {
    let a = plan(TWO_CANDIDATES);
    let b_source = "config Arrangement=A|B\nrule base()=1\npropose opt_a(base) priority 10 when base == 1 = A\npropose opt_b(base) priority 20 when base == 1 = B\ncommit pick from (opt_a, opt_b)";
    let b = plan(b_source);
    assert_eq!(
        finite_decision_program_id(&a),
        finite_decision_program_id(&b),
        "insignificant syntactic formatting does not alter program identity"
    );

    // Modifying priorities produces distinct program ID.
    let changed_prio = plan("config Arrangement=A|B\nrule base()=1\npropose opt_a(base) priority 15 when base == 1 = A\npropose opt_b(base) priority 20 when base == 1 = B\ncommit pick from (opt_a, opt_b)");
    assert_ne!(
        finite_decision_program_id(&a),
        finite_decision_program_id(&changed_prio)
    );

    // Modifying commit order produces distinct program ID.
    let changed_order = plan("config Arrangement=A|B\nrule base()=1\npropose opt_a(base) priority 10 when base == 1 = A\npropose opt_b(base) priority 20 when base == 1 = B\ncommit pick from (opt_b, opt_a)");
    assert_ne!(
        finite_decision_program_id(&a),
        finite_decision_program_id(&changed_order)
    );
}

#[test]
fn v1_remains_the_serial_rule_agenda() {
    let source = "config Arrangement = A | B\nrule arrangement_a() = A\nrule arrangement_b() = B\n";
    let module = parse(source).expect("fixture parses");
    let v1 = lower_l3_plan(&module, L3_PROFILE_MARKER_V1, &PlanLimitsV1::generous())
        .expect("v1 remains valid");
    let report = run_l3_plan(&v1, L3AdmChoice::Compiled, SaturationBudget::uniform(32));
    assert_eq!(report.journal.len(), 2, "v1 serially commits both rules");
}

// ---------------------------------------------------------------------------
// Shipping Example (Normative User Requirement)
// ---------------------------------------------------------------------------

#[test]
fn shipping_example_deliberation_and_dispositions() {
    let p = plan(SHIPPING_EXAMPLE);
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();

    // 1. Facts are Derived.
    assert_eq!(run.facts.len(), 2);
    assert_eq!(run.facts[0].rule, "stock");
    assert_eq!(run.facts[0].value, brix_lower::l3_v2::L3ValueV2::Int(12));
    assert_eq!(run.facts[0].grade, Outcome::Derived);
    assert_eq!(run.facts[1].rule, "threshold");
    assert_eq!(run.facts[1].value, brix_lower::l3_v2::L3ValueV2::Int(10));
    assert_eq!(run.facts[1].grade, Outcome::Derived);

    // 2. Structured dispositions:
    // expedite guard stock >= 50 rejected guard-false
    assert_eq!(
        run.status_of("expedite"),
        Some(CandidateStatus::RejectedGuardFalse)
    );

    // ship priority 10 selected
    assert_eq!(run.status_of("ship"), Some(CandidateStatus::Selected));

    // hold priority 100 admitted-not-selected
    assert_eq!(
        run.status_of("hold"),
        Some(CandidateStatus::AdmittedNotSelected)
    );

    // 3. Decision Ship at Derived.
    assert!(run.is_selected());
    let decision = run.decision.expect("decision present");
    assert_eq!(decision.candidate, "ship");
    assert_eq!(decision.priority, 10);
    assert_eq!(
        decision.value,
        brix_lower::l3_v2::L3ValueV2::Ctor {
            nominal_sum: "Decision".to_string(),
            variant: "Ship".to_string(),
            args: Vec::new(),
        }
    );
    assert_eq!(decision.grade, Outcome::Derived);

    // 4. Exactly one step in journal at Derived.
    assert_eq!(run.journal.len(), 1);
    assert_eq!(
        run.journal.steps()[0].observation.outcome_class,
        Outcome::Derived
    );

    // 5. Audit produces Audited artifact without mutating runtime Derived grade.
    let audit = runtime.audit(&run.journal);
    assert!(matches!(audit.as_slice(), [AuditResult::Audited(_)]));

    // 6. Dynamic explanations:
    let why_ship = runtime.explain_why("ship").expect("explain why ship");
    assert!(matches!(why_ship, WhyExplanation::Selected { .. }));

    let why_hold = runtime.explain_why("hold").expect("explain why hold");
    assert!(matches!(
        why_hold,
        WhyExplanation::AdmittedNotSelected { .. }
    ));

    let why_expedite = runtime
        .explain_why("expedite")
        .expect("explain why expedite");
    assert!(matches!(why_expedite, WhyExplanation::NotAdmitted { .. }));
}

// ---------------------------------------------------------------------------
// Certified Quiescence
// ---------------------------------------------------------------------------

#[test]
fn all_candidates_rejected_yields_certified_quiescence_with_decision_none() {
    let source = r#"
config Flag = Off

rule limit() = 5

propose opt_a(limit) priority 10 when limit > 100 = Off
propose opt_b(limit) priority 20 when limit > 200 = Off

commit pick from (opt_a, opt_b)
"#;
    let p = plan(source);
    let run = run_finite_decision_plan(&p).expect("runs successfully");

    assert!(run.is_quiescent());
    assert_eq!(run.decision, None, "decision None upon quiescence");
    assert_eq!(run.journal.len(), 0, "no step committed to journal");
    assert_eq!(
        run.status_of("opt_a"),
        Some(CandidateStatus::RejectedGuardFalse)
    );
    assert_eq!(
        run.status_of("opt_b"),
        Some(CandidateStatus::RejectedGuardFalse)
    );

    match run.stop {
        FiniteDecisionStop::Quiescent { certificate } => {
            // A quiescent result must carry a verified QuiescenceCertificateId directly (never Option / None).
            let _ = certificate;
        }
        other => panic!("expected Quiescent stop, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Equal Priority Deterministic Canonical Tie-Break
// ---------------------------------------------------------------------------

#[test]
fn equal_priority_breaks_by_canonical_candidate_digest() {
    let source = r#"
config Value = Left | Right

rule seed() = 0

propose a(seed) priority 10 when seed == 0 = Left
propose b(seed) priority 10 when seed == 0 = Right

commit pick from (a, b)
"#;
    let p = plan(source);
    let run1 = run_finite_decision_plan(&p).expect("run 1 succeeds");
    let run2 = run_finite_decision_plan(&p).expect("run 2 succeeds");

    assert!(run1.is_selected());
    assert_eq!(run1.decision, run2.decision);
    let dec = run1.decision.as_ref().unwrap();
    assert_eq!(dec.priority, 10);
    // Deterministic tiebreak on canonical candidate digest
    let winner = dec.candidate.clone();
    let loser = if winner == "a" { "b" } else { "a" };
    assert_eq!(run1.status_of(&winner), Some(CandidateStatus::Selected));
    assert_eq!(
        run1.status_of(loser),
        Some(CandidateStatus::AdmittedNotSelected)
    );
}

// ---------------------------------------------------------------------------
// Negative Lowering Tests
// ---------------------------------------------------------------------------

#[test]
fn negative_lowering_profile_mismatch() {
    let module = parse(TWO_CANDIDATES).expect("parses");
    let err = lower_finite_decision_plan(&module, "wrong.profile@1").unwrap_err();
    assert!(matches!(
        err,
        FiniteDecisionLowerError::ProfileMismatch { .. }
    ));
}

#[test]
fn negative_lowering_missing_commit() {
    let source = "config A = X\nrule r() = X\npropose p(r) priority 1 when true = X";
    let module = parse(source).expect("parses");
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    assert_eq!(err, FiniteDecisionLowerError::NoCommit);
}

#[test]
fn negative_lowering_multiple_commits() {
    let source = "config A = X\nrule r() = X\npropose p(r) priority 1 when true = X\ncommit c1 from (p)\ncommit c2 from (p)";
    let module = parse(source).expect("parses");
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    assert_eq!(err, FiniteDecisionLowerError::MultipleCommits(2));
}

#[test]
fn negative_lowering_empty_commit() {
    let source =
        "config A = X\nrule r() = X\npropose p(r) priority 1 when true = X\ncommit c from (p)";
    let mut module = parse(source).expect("parses");
    for item in &mut module.items {
        if let brix_syntax::ast::Item::Commit(ref mut c) = item {
            c.candidates.clear();
        }
    }
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    assert_eq!(err, FiniteDecisionLowerError::EmptyCommit("c".to_string()));
}

#[test]
fn negative_lowering_duplicate_proposal_name() {
    let source = "config A = X\nrule r() = X\npropose p(r) priority 1 when true = X\npropose p(r) priority 2 when true = X\ncommit c from (p)";
    let module = parse(source).expect("parses");
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    assert_eq!(
        err,
        FiniteDecisionLowerError::DuplicateProposalName("p".to_string())
    );
}

#[test]
fn negative_lowering_duplicate_candidate_in_commit() {
    let source =
        "config A = X\nrule r() = X\npropose p(r) priority 1 when true = X\ncommit c from (p, p)";
    let module = parse(source).expect("parses");
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(matches!(
        err,
        FiniteDecisionLowerError::DuplicateCandidateInCommit { .. }
    ));
}

#[test]
fn negative_lowering_unknown_candidate_in_commit() {
    let source = "config A = X\nrule r() = X\npropose p(r) priority 1 when true = X\ncommit c from (nonexistent)";
    let module = parse(source).expect("parses");
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(matches!(
        err,
        FiniteDecisionLowerError::UnknownCandidateInCommit { .. }
    ));
}

#[test]
fn negative_lowering_undeclared_proposal_dependency() {
    let source =
        "config A = X\npropose p(missing_rule) priority 1 when true = X\ncommit c from (p)";
    let module = parse(source).expect("parses");
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(matches!(
        err,
        FiniteDecisionLowerError::UndeclaredDependency { .. }
    ));
}

#[test]
fn negative_lowering_forward_proposal_dependency() {
    let source =
        "config A = X\npropose p(r) priority 1 when true = X\nrule r() = X\ncommit c from (p)";
    let module = parse(source).expect("parses");
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(matches!(
        err,
        FiniteDecisionLowerError::UndeclaredDependency { .. }
    ));
}

#[test]
fn negative_lowering_undeclared_fact_read_in_guard() {
    let source =
        "config A = X\nrule r() = 5\npropose p() priority 1 when r > 0 = X\ncommit c from (p)";
    let module = parse(source).expect("parses");
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(matches!(
        err,
        FiniteDecisionLowerError::UndeclaredFactRead { .. }
    ));
}

#[test]
fn negative_lowering_disallowed_items() {
    let source_witness = "witness W = 1\ncommit c from (p)";
    let module = parse(source_witness).expect("parses");
    assert!(matches!(
        lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE),
        Err(FiniteDecisionLowerError::ItemNotAllowed(_))
    ));

    let source_regime = "regime R { gen g() = 1 }\ncommit c from (p)";
    let module = parse(source_regime).expect("parses");
    assert!(matches!(
        lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE),
        Err(FiniteDecisionLowerError::ItemNotAllowed(_))
    ));
}

// ---------------------------------------------------------------------------
// Runtime Fault Tests (Fail-Closed to Unknown)
// ---------------------------------------------------------------------------

#[test]
fn runtime_fault_rule_eval_overflow_returns_unknown() {
    let source = r#"
config Val = Num

rule overflow() = 9223372036854775807 + 1

propose p(overflow) priority 1 when true = Num

commit pick from (p)
"#;
    let p = plan(source);
    let run = run_finite_decision_plan(&p).expect("runtime builds successfully");
    assert!(run.is_unknown(), "arithmetic overflow must halt at Unknown");
    assert_eq!(run.decision, None, "publishes no decision");
    assert_eq!(run.journal.len(), 0);
}

#[test]
fn runtime_fault_guard_not_bool_returns_unknown() {
    let source = r#"
config Val = Num

rule number() = 42

propose p(number) priority 1 when number = Num

commit pick from (p)
"#;
    let p = plan(source);
    let run = run_finite_decision_plan(&p).expect("runtime builds successfully");
    assert!(run.is_unknown(), "non-Bool guard must halt at Unknown");
    assert_eq!(run.decision, None, "publishes no decision");
    match run.stop {
        FiniteDecisionStop::Unknown(FiniteDecisionUnknownReason::TypeFault { .. }) => {}
        other => panic!("expected TypeFault, got {other:?}"),
    }
}

#[test]
fn runtime_fault_proposal_value_types_mismatch_returns_unknown() {
    let source = r#"
config Val = Num

rule seed() = 1

propose p1(seed) priority 1 when seed == 1 = 42
propose p2(seed) priority 2 when seed == 1 = "string_value"

commit pick from (p1, p2)
"#;
    let p = plan(source);
    let run = run_finite_decision_plan(&p).expect("runtime builds successfully");
    assert!(
        run.is_unknown(),
        "differing proposal value types must halt at Unknown"
    );
    assert_eq!(run.decision, None, "publishes no decision");
    match run.stop {
        FiniteDecisionStop::Unknown(FiniteDecisionUnknownReason::TypeFault { .. }) => {}
        other => panic!("expected TypeFault, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Remediated Fail-Closed and Explanation API Regression Tests
// ---------------------------------------------------------------------------

#[test]
fn explain_why_and_explain_why_not_surface_unknown_on_expression_fault() {
    let source = r#"
config Val = Num

rule overflow() = 9223372036854775807 + 1

propose p(overflow) priority 1 when true = Num

commit pick from (p)
"#;
    let p = plan(source);
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_unknown(), "arithmetic overflow must halt at Unknown");
    assert_eq!(run.decision, None);

    // Explain why on candidate in plan returns Err(UnknownReason).
    let why_res = runtime.explain_why("p");
    assert!(
        why_res.is_err(),
        "explain_why on Unknown run must return Err"
    );
    let why_err = why_res.unwrap_err();
    assert!(
        why_err.is_expression_fault(),
        "expected expression fault, got {why_err:?}"
    );

    // Explain why-not on candidate in plan returns Err(UnknownReason).
    let whynot_res = runtime.explain_why_not("p");
    assert!(
        whynot_res.is_err(),
        "explain_why_not on Unknown run must return Err"
    );
    let whynot_err = whynot_res.unwrap_err();
    assert!(
        whynot_err.is_expression_fault(),
        "expected expression fault, got {whynot_err:?}"
    );

    // Explain why on NONEXISTENT candidate must ALSO return Err(UnknownReason),
    // NEVER CandidateNotFound!
    let why_missing = runtime.explain_why("nonexistent");
    assert!(
        why_missing.is_err(),
        "explain_why on Unknown run must surface Unknown even for nonexistent target"
    );
    assert!(why_missing.unwrap_err().is_expression_fault());

    let whynot_missing = runtime.explain_why_not("nonexistent");
    assert!(
        whynot_missing.is_err(),
        "explain_why_not on Unknown run must surface Unknown even for nonexistent target"
    );
    assert!(whynot_missing.unwrap_err().is_expression_fault());
}

#[test]
fn explain_why_and_explain_why_not_surface_unknown_on_type_fault_and_checkability() {
    let source = r#"
config Val = Num

rule number() = 42

propose p(number) priority 1 when number = Num

commit pick from (p)
"#;
    let p = plan(source);
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_unknown());
    assert_eq!(run.decision, None);

    // Checkability of runtime type faults
    match &run.stop {
        FiniteDecisionStop::Unknown(reason) => {
            assert!(reason.is_type_fault());
            assert!(!reason.is_expression_fault());
            assert!(!reason.is_frontier_fault());
            assert!(!reason.is_commit_fault());
            match reason {
                FiniteDecisionUnknownReason::TypeFault { context, detail } => {
                    assert!(context.contains("guard"));
                    assert!(detail.contains("Bool"));
                }
                other => panic!("expected TypeFault variant, got {other:?}"),
            }
        }
        other => panic!("expected Unknown stop, got {other:?}"),
    }

    // Explanation API returns Err(TypeFault)
    let why_res = runtime.explain_why("p");
    assert!(why_res.is_err());
    assert!(why_res.unwrap_err().is_type_fault());

    let whynot_res = runtime.explain_why_not("p");
    assert!(whynot_res.is_err());
    assert!(whynot_res.unwrap_err().is_type_fault());

    let why_missing = runtime.explain_why("nonexistent");
    assert!(why_missing.is_err());
    assert!(why_missing.unwrap_err().is_type_fault());

    // Second type fault: proposal values mismatch
    let source_mismatch = r#"
config Val = Num

rule seed() = 1

propose p1(seed) priority 1 when seed == 1 = 42
propose p2(seed) priority 2 when seed == 1 = "string_value"

commit pick from (p1, p2)
"#;
    let p_mismatch = plan(source_mismatch);
    let runtime_mismatch = FiniteDecisionRuntime::build(&p_mismatch).expect("runtime builds");
    let run_mismatch = runtime_mismatch.run();
    assert!(run_mismatch.is_unknown());
    let why_mismatch = runtime_mismatch.explain_why("p1");
    assert!(why_mismatch.is_err());
    let err = why_mismatch.unwrap_err();
    assert!(err.is_type_fault());
    match &err {
        FiniteDecisionUnknownReason::TypeFault { context, detail } => {
            assert!(context.contains("proposal values"));
            assert!(detail.contains("does not match"));
        }
        other => panic!("expected TypeFault, got {other:?}"),
    }
}

#[test]
fn explain_why_successful_run_reports_candidate_not_found_when_absent() {
    let p = plan(SHIPPING_EXAMPLE);
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();
    assert!(run.is_selected());

    let why_missing = runtime
        .explain_why("nonexistent")
        .expect("explain succeeds");
    assert_eq!(why_missing, WhyExplanation::CandidateNotFound);

    let whynot_missing = runtime
        .explain_why_not("nonexistent")
        .expect("explain_why_not succeeds");
    assert_eq!(whynot_missing, WhyNotExplanation::CandidateNotFound);
}

#[test]
fn frontier_fault_reasons_preserved_and_not_mislabeled() {
    use brix_semantic::{ConfigId, RegimeId};
    use soc_core::calendar::Key;
    use soc_core::intern::Interner;
    use soc_regimes::finite_frontier::NamedCandidate;

    let mut interner = Interner::new();
    let r = RegimeId::named("test@1");
    let c1 = ConfigId::from_canon(b"c1");
    let c2 = ConfigId::from_canon(b"c2");
    let cand = NamedCandidate::new("test_cand", r, c1, c2, 10, &mut interner);

    // 1. AdmissionError
    let admission_fault = EvaluationFault::admission_error(cand.clone(), "custom admission error");
    let reason = FiniteDecisionUnknownReason::from(admission_fault);
    assert!(reason.is_frontier_fault());
    assert!(!reason.is_type_fault());
    assert!(!reason.is_expression_fault());
    match &reason {
        FiniteDecisionUnknownReason::AdmissionError { candidate, detail } => {
            assert_eq!(candidate, "test_cand");
            assert_eq!(detail, "custom admission error");
        }
        other => panic!("expected AdmissionError, got {other:?}"),
    }
    assert!(
        format!("{reason}").contains("deliberation frontier admission error"),
        "display must reflect admission error"
    );

    // 2. EvaluationError
    let eval_fault = EvaluationFault::evaluation_error("eval failed");
    let reason = FiniteDecisionUnknownReason::from(eval_fault);
    assert!(reason.is_frontier_fault());
    match &reason {
        FiniteDecisionUnknownReason::EvaluationError { detail } => {
            assert_eq!(detail, "eval failed");
        }
        other => panic!("expected EvaluationError, got {other:?}"),
    }
    assert!(format!("{reason}").contains("deliberation frontier evaluation error"));

    // 3. InvalidPhase
    let phase_fault = EvaluationFault::invalid_phase(cand.clone(), 5);
    let reason = FiniteDecisionUnknownReason::from(phase_fault);
    assert!(reason.is_frontier_fault());
    match &reason {
        FiniteDecisionUnknownReason::InvalidPhase { candidate, phase } => {
            assert_eq!(candidate, "test_cand");
            assert_eq!(*phase, 5);
        }
        other => panic!("expected InvalidPhase, got {other:?}"),
    }
    assert!(format!("{reason}").contains("invalid non-zero phase 5"));

    // 4. KeyConflict under B^uk discipline
    let cand2 = NamedCandidate::new("test_cand_2", r, c1, c2, 10, &mut interner);
    let key = Key::new(
        0,
        10,
        brix_canon::Digest::of(brix_canon::Domain::Value, b"tie"),
    );
    let key_conflict = soc_core::calendar::KeyConflict {
        key,
        existing: cand.clone(),
        attempted: cand2,
    };
    let conflict_fault = EvaluationFault::KeyConflict(key_conflict);
    let reason = FiniteDecisionUnknownReason::from(conflict_fault);
    assert!(reason.is_frontier_fault());
    match &reason {
        FiniteDecisionUnknownReason::DecisionKeyConflict { detail } => {
            assert!(detail.contains("Key conflict"));
        }
        other => panic!("expected DecisionKeyConflict, got {other:?}"),
    }
}

#[test]
fn quiescence_requires_verified_certificate_id() {
    let source = r#"
config Flag = Off

rule limit() = 5

propose opt_a(limit) priority 10 when limit > 100 = Off
propose opt_b(limit) priority 20 when limit > 200 = Off

commit pick from (opt_a, opt_b)
"#;
    let p = plan(source);
    let runtime = FiniteDecisionRuntime::build(&p).expect("runtime builds");
    let run = runtime.run();

    assert!(run.is_quiescent());
    assert_eq!(run.decision, None);
    assert_eq!(run.journal.len(), 0);

    // Stop must carry QuiescenceCertificateId directly (never None)
    match run.stop {
        FiniteDecisionStop::Quiescent { certificate } => {
            let _ = certificate;
        }
        other => panic!("expected Quiescent stop, got {other:?}"),
    }

    // QuiescenceVerificationFault predicate check
    let fault_reason = FiniteDecisionUnknownReason::QuiescenceVerificationFault {
        detail: "verification failed".to_string(),
    };
    assert!(fault_reason.is_commit_fault());
    assert!(!fault_reason.is_frontier_fault());
    assert!(format!("{fault_reason}").contains("quiescence verification fault"));
}

#[test]
fn invariant_violations_and_commit_faults_checkability() {
    let commit_err = FiniteDecisionUnknownReason::CommitTickError {
        detail: "tick error".to_string(),
    };
    assert!(commit_err.is_commit_fault());
    assert!(!commit_err.is_frontier_fault());
    assert!(format!("{commit_err}").contains("commit tick error"));

    let inv_err = FiniteDecisionUnknownReason::InvariantViolation {
        detail: "bad grade".to_string(),
    };
    assert!(inv_err.is_commit_fault());
    assert!(format!("{inv_err}").contains("invariant violation"));

    let dep_err = FiniteDecisionUnknownReason::DependencyFault {
        context: "ctx".to_string(),
        detail: "det".to_string(),
    };
    assert!(dep_err.is_dependency_fault());
    assert!(!dep_err.is_type_fault());
    assert!(format!("{dep_err}").contains("dependency fault"));
}
