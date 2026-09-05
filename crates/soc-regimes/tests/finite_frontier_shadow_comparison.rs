//! Sharp tests for determinism, canonical tie-breaks, all-rejected quiescence,
//! and publication withholding on evaluation faults (ADR-0002 §1, §8.1).

use brix_semantic::{ConfigId, RegimeId};
use soc_core::exec::ExecConfig;
use soc_core::intern::Interner;
use soc_core::History;
use soc_regimes::finite_frontier::{
    explain_why, explain_why_not, AdmitAllPolicy, CandidateStatus, DeliberationOutcome,
    DenyAllPolicy, EvaluatedFrontier, EvaluationFault, GuardPolicy, NamedCandidate, ReasonCode,
    WhyExplanation, WhyNotExplanation,
};

#[test]
fn determinism_across_all_candidate_permutations() {
    let mut interner = Interner::new();
    let regime_id = RegimeId::named("finite-test@1");

    let cfg_a = ConfigId::from_canon(b"cfg-A");
    let cfg_b = ConfigId::from_canon(b"cfg-B");
    let cfg_c = ConfigId::from_canon(b"cfg-C");
    let cfg_d = ConfigId::from_canon(b"cfg-D");

    let c1 = NamedCandidate::new("cand-1", regime_id, cfg_a, cfg_b, 10, &mut interner);
    let c2 = NamedCandidate::new("cand-2", regime_id, cfg_a, cfg_c, 5, &mut interner);
    let c3 = NamedCandidate::new("cand-3", regime_id, cfg_a, cfg_d, 5, &mut interner);

    let world_handle = interner.intern(cfg_a.digest());
    let policy_handle = interner.intern(ConfigId::from_canon(b"policy").digest());
    let exec_cfg = ExecConfig::new(world_handle, policy_handle, History::empty().digest());

    let permutations = [
        vec![c1.clone(), c2.clone(), c3.clone()],
        vec![c1.clone(), c3.clone(), c2.clone()],
        vec![c2.clone(), c1.clone(), c3.clone()],
        vec![c2.clone(), c3.clone(), c1.clone()],
        vec![c3.clone(), c1.clone(), c2.clone()],
        vec![c3.clone(), c2.clone(), c1.clone()],
    ];

    let first_result = EvaluatedFrontier::evaluate(
        permutations[0].clone(),
        &AdmitAllPolicy,
        &exec_cfg,
        &interner,
    );

    for (idx, p) in permutations.iter().enumerate() {
        let res = EvaluatedFrontier::evaluate(p.clone(), &AdmitAllPolicy, &exec_cfg, &interner);
        assert_eq!(
            res.selected, first_result.selected,
            "Permutation {idx} selected outcome must be strictly identical"
        );
        assert_eq!(
            res.admitted.keys().collect::<Vec<_>>(),
            first_result.admitted.keys().collect::<Vec<_>>(),
            "Permutation {idx} admitted keys order must match exactly"
        );
        assert_eq!(res.admitted.len(), 3);
        assert_eq!(res.rejected.len(), 0);
    }
}

#[test]
fn canonical_tiebreak_deterministic_and_independent_of_display_names() {
    let mut interner = Interner::new();
    let regime_id = RegimeId::named("finite-test@1");

    let cfg_src = ConfigId::from_canon(b"source");
    let cfg_dst_a = ConfigId::from_canon(b"destination-A");
    let cfg_dst_b = ConfigId::from_canon(b"destination-B");

    // Two candidates with equal priority (1) at phase 0
    let c1 = NamedCandidate::new(
        "display-alpha",
        regime_id,
        cfg_src,
        cfg_dst_a,
        1,
        &mut interner,
    );
    let c2 = NamedCandidate::new(
        "display-beta",
        regime_id,
        cfg_src,
        cfg_dst_b,
        1,
        &mut interner,
    );

    let k1 = c1.canonical_key(&interner);
    let k2 = c2.canonical_key(&interner);

    assert_eq!(k1.phase, 0);
    assert_eq!(k2.phase, 0);
    assert_eq!(k1.priority, 1);
    assert_eq!(k2.priority, 1);
    assert_ne!(
        k1.tiebreak, k2.tiebreak,
        "Distinct destinations must produce distinct canonical tiebreak digests"
    );

    let expected_winner = if k1 < k2 { &c1 } else { &c2 };
    let expected_key = if k1 < k2 { k1 } else { k2 };

    let world_handle = interner.intern(cfg_src.digest());
    let policy_handle = interner.intern(ConfigId::from_canon(b"policy").digest());
    let exec_cfg = ExecConfig::new(world_handle, policy_handle, History::empty().digest());

    let res = EvaluatedFrontier::evaluate(
        vec![c1.clone(), c2.clone()],
        &AdmitAllPolicy,
        &exec_cfg,
        &interner,
    );
    let (sel_key, sel_cand) = res.selected.expect("must select candidate");
    assert_eq!(sel_key, expected_key);
    assert_eq!(sel_cand.name, expected_winner.name);
}

#[test]
fn all_rejected_is_valid_quiescence_without_fault() {
    let mut interner = Interner::new();
    let regime_id = RegimeId::named("finite-test@1");

    let cfg_a = ConfigId::from_canon(b"cfg-A");
    let cfg_b = ConfigId::from_canon(b"cfg-B");
    let cfg_c = ConfigId::from_canon(b"cfg-C");

    let c1 = NamedCandidate::new("c1", regime_id, cfg_a, cfg_b, 10, &mut interner);
    let c2 = NamedCandidate::new("c2", regime_id, cfg_a, cfg_c, 20, &mut interner);

    let world_handle = interner.intern(cfg_a.digest());
    let policy_handle = interner.intern(ConfigId::from_canon(b"policy").digest());
    let exec_cfg = ExecConfig::new(world_handle, policy_handle, History::empty().digest());

    // DenyAllPolicy rejects everything as guard-false
    let res = EvaluatedFrontier::evaluate(
        vec![c1.clone(), c2.clone()],
        &DenyAllPolicy,
        &exec_cfg,
        &interner,
    );

    assert_eq!(res.admitted_count(), 0);
    assert_eq!(res.rejected_count(), 2);
    assert_eq!(res.selected, None);
    assert!(res.is_quiescent());
    assert!(res.fault().is_none());

    // Decision publication can be cleanly withheld on valid quiescence
    assert_eq!(res.selection_outcome(), Ok(None));
    assert_eq!(res.try_selected(), Ok(None));
    assert_eq!(
        res.deliberation_outcome(),
        Ok(DeliberationOutcome::Quiescent)
    );

    assert_eq!(
        res.status_of(&c1),
        Some(CandidateStatus::RejectedGuardFalse)
    );
    assert_eq!(
        res.status_of(&c2),
        Some(CandidateStatus::RejectedGuardFalse)
    );
}

#[test]
fn evaluation_fault_withholds_decision_publication_and_is_not_quiescent() {
    let mut interner = Interner::new();
    let regime_id = RegimeId::named("finite-test@1");

    let cfg_a = ConfigId::from_canon(b"cfg-A");
    let cfg_b = ConfigId::from_canon(b"cfg-B");
    let cfg_c = ConfigId::from_canon(b"cfg-C");

    let c1 = NamedCandidate::new("cand", regime_id, cfg_a, cfg_b, 5, &mut interner);
    // Duplicate candidate: same canonical identity (same handles) but different destination metadata
    let mut c2 = NamedCandidate::new("cand", regime_id, cfg_a, cfg_c, 5, &mut interner);
    c2.src_handle = c1.src_handle;
    c2.witness_handle = c1.witness_handle;
    c2.successor_handle = c1.successor_handle;
    c2.dst = cfg_c;

    let world_handle = interner.intern(cfg_a.digest());
    let policy_handle = interner.intern(ConfigId::from_canon(b"policy").digest());
    let exec_cfg = ExecConfig::new(world_handle, policy_handle, History::empty().digest());

    let res = EvaluatedFrontier::evaluate(
        vec![c1.clone(), c2.clone()],
        &AdmitAllPolicy,
        &exec_cfg,
        &interner,
    );

    // Key conflict must fail closed
    assert_eq!(res.key_conflicts.len(), 1);
    assert_eq!(res.selected, None);

    // Fault must NOT be confused with valid quiescence:
    assert!(
        !res.is_quiescent(),
        "An integrity fault must not be reported as quiescence"
    );
    assert!(res.fault().is_some());

    // Caller-visible error path withholds publication
    let outcome = res.selection_outcome();
    assert!(
        matches!(outcome, Err(EvaluationFault::KeyConflict(_))),
        "selection_outcome must return Err(EvaluationFault) on key conflict"
    );
    assert!(
        matches!(
            res.deliberation_outcome(),
            Err(EvaluationFault::KeyConflict(_))
        ),
        "deliberation_outcome must return Err(EvaluationFault) on key conflict"
    );

    // try_evaluate fails closed with the same fault
    let try_eval = EvaluatedFrontier::try_evaluate(
        vec![c1.clone(), c2.clone()],
        &AdmitAllPolicy,
        &exec_cfg,
        &interner,
    );
    assert!(matches!(try_eval, Err(EvaluationFault::KeyConflict(_))));
}

#[test]
fn explanations_reflect_statuses_and_guard_rejections() {
    let mut interner = Interner::new();
    let regime_id = RegimeId::named("finite-test@1");

    let cfg_src = ConfigId::from_canon(b"source");
    let cfg_dst1 = ConfigId::from_canon(b"dest-1");
    let cfg_dst2 = ConfigId::from_canon(b"dest-2");
    let cfg_dst3 = ConfigId::from_canon(b"dest-3");

    let winner = NamedCandidate::new("winner", regime_id, cfg_src, cfg_dst1, 10, &mut interner);
    let runner_up =
        NamedCandidate::new("runner-up", regime_id, cfg_src, cfg_dst2, 20, &mut interner);
    let blocked = NamedCandidate::new("blocked", regime_id, cfg_src, cfg_dst3, 5, &mut interner);

    let candidates = vec![winner.clone(), runner_up.clone(), blocked.clone()];
    let guard = GuardPolicy(|_e: &ExecConfig, c: &NamedCandidate| c.name != "blocked");

    let world_handle = interner.intern(cfg_src.digest());
    let policy_handle = interner.intern(ConfigId::from_canon(b"policy").digest());
    let exec_cfg = ExecConfig::new(world_handle, policy_handle, History::empty().digest());

    // why winner -> Selected
    assert!(matches!(
        explain_why(&candidates, &guard, &exec_cfg, &winner, &interner),
        WhyExplanation::Selected { candidate, .. } if candidate.name == "winner"
    ));

    // why runner_up -> AdmittedNotSelected
    assert!(matches!(
        explain_why(&candidates, &guard, &exec_cfg, &runner_up, &interner),
        WhyExplanation::AdmittedNotSelected {
            candidate,
            selected_candidate,
            ..
        } if candidate.name == "runner-up" && selected_candidate.name == "winner"
    ));

    // why blocked -> NotAdmitted with ReasonCode::GuardFalse
    assert!(matches!(
        explain_why(&candidates, &guard, &exec_cfg, &blocked, &interner),
        WhyExplanation::NotAdmitted {
            candidate,
            reason: ReasonCode::GuardFalse,
        } if candidate.name == "blocked"
    ));

    // why_not blocked -> RejectedByPolicy
    assert!(matches!(
        explain_why_not(&candidates, &guard, &exec_cfg, &blocked, &interner),
        WhyNotExplanation::RejectedByPolicy {
            candidate,
            reason: ReasonCode::GuardFalse,
        } if candidate.name == "blocked"
    ));

    // why_not runner_up -> Overshadowed
    assert!(matches!(
        explain_why_not(&candidates, &guard, &exec_cfg, &runner_up, &interner),
        WhyNotExplanation::Overshadowed {
            candidate,
            selected_candidate,
            ..
        } if candidate.name == "runner-up" && selected_candidate.name == "winner"
    ));

    // why_not winner -> ActuallySelected
    assert!(matches!(
        explain_why_not(&candidates, &guard, &exec_cfg, &winner, &interner),
        WhyNotExplanation::ActuallySelected { candidate, .. } if candidate.name == "winner"
    ));
}
