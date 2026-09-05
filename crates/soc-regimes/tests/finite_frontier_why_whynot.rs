//! Tests for `why` and `why_not` dynamic re-derivation without stored explanations (ADR-0002 §8.1).

use brix_semantic::{ConfigId, RegimeId};
use soc_core::exec::ExecConfig;
use soc_core::intern::Interner;
use soc_core::History;
use soc_regimes::finite_frontier::{
    explain_why, explain_why_not, EvaluationFault, GuardPolicy, NamedCandidate, ReasonCode,
    WhyExplanation, WhyNotExplanation,
};

#[test]
fn explain_why_rederives_selected_admitted_and_not_admitted() {
    let mut interner = Interner::new();
    let regime_id = RegimeId::named("finite-test@1");

    let cfg_a = ConfigId::from_canon(b"cfg-A");
    let cfg_b = ConfigId::from_canon(b"cfg-B");
    let cfg_c = ConfigId::from_canon(b"cfg-C");
    let cfg_d = ConfigId::from_canon(b"cfg-D");

    // c1 has priority 5 (winner)
    let c1 = NamedCandidate::new(
        "winner".to_string(),
        regime_id,
        cfg_a,
        cfg_b,
        5,
        &mut interner,
    );
    // c2 has priority 10 (admitted, but loses to c1)
    let c2 = NamedCandidate::new(
        "runner-up".to_string(),
        regime_id,
        cfg_a,
        cfg_c,
        10,
        &mut interner,
    );
    // c3 has priority 100 (rejected by guard threshold 20)
    let c3 = NamedCandidate::new(
        "rejected".to_string(),
        regime_id,
        cfg_a,
        cfg_d,
        100,
        &mut interner,
    );

    let candidates = vec![c1.clone(), c2.clone(), c3.clone()];
    let policy = GuardPolicy(|_e: &ExecConfig, c: &NamedCandidate| c.priority <= 20);

    let world_handle = interner.intern(cfg_a.digest());
    let policy_handle = interner.intern(ConfigId::from_canon(b"policy").digest());
    let exec_cfg = ExecConfig::new(world_handle, policy_handle, History::empty().digest());

    // 1. Why c1? -> Selected
    let why_c1 = explain_why(&candidates, &policy, &exec_cfg, &c1, &interner);
    match why_c1 {
        WhyExplanation::Selected { key, candidate } => {
            assert_eq!(candidate.name, "winner");
            assert_eq!(key.priority, 5);
        }
        _ => panic!("c1 must be Selected"),
    }

    // 2. Why c2? -> AdmittedNotSelected (overshadowed by c1)
    let why_c2 = explain_why(&candidates, &policy, &exec_cfg, &c2, &interner);
    match why_c2 {
        WhyExplanation::AdmittedNotSelected {
            key,
            candidate,
            selected_key,
            selected_candidate,
        } => {
            assert_eq!(candidate.name, "runner-up");
            assert_eq!(key.priority, 10);
            assert_eq!(selected_candidate.name, "winner");
            assert_eq!(selected_key.priority, 5);
        }
        _ => panic!("c2 must be AdmittedNotSelected"),
    }

    // 3. Why c3? -> NotAdmitted
    let why_c3 = explain_why(&candidates, &policy, &exec_cfg, &c3, &interner);
    match why_c3 {
        WhyExplanation::NotAdmitted { candidate, reason } => {
            assert_eq!(candidate.name, "rejected");
            assert_eq!(reason, ReasonCode::GuardFalse);
        }
        _ => panic!("c3 must be NotAdmitted"),
    }
}

#[test]
fn explanations_reject_a_target_absent_from_the_fresh_candidate_pool() {
    let mut interner = Interner::new();
    let regime_id = RegimeId::named("finite-test@1");
    let src = ConfigId::from_canon(b"source");
    let dst = ConfigId::from_canon(b"successor");
    let absent = NamedCandidate::new("absent".to_string(), regime_id, src, dst, 1, &mut interner);
    let policy_handle = interner.intern(ConfigId::from_canon(b"policy").digest());
    let exec_cfg = ExecConfig::new(absent.src_handle, policy_handle, History::empty().digest());
    let empty = Vec::new();
    let policy = GuardPolicy(|_e: &ExecConfig, _c: &NamedCandidate| true);

    assert!(matches!(
        explain_why(&empty, &policy, &exec_cfg, &absent, &interner),
        WhyExplanation::CandidateNotFound
    ));
    assert!(matches!(
        explain_why_not(&empty, &policy, &exec_cfg, &absent, &interner),
        WhyNotExplanation::CandidateNotFound
    ));
}

#[test]
fn explain_why_not_rederives_rejection_and_overshadowing() {
    let mut interner = Interner::new();
    let regime_id = RegimeId::named("finite-test@1");

    let cfg_a = ConfigId::from_canon(b"cfg-A");
    let cfg_b = ConfigId::from_canon(b"cfg-B");
    let cfg_c = ConfigId::from_canon(b"cfg-C");

    let c1 = NamedCandidate::new("top".to_string(), regime_id, cfg_a, cfg_b, 1, &mut interner);
    let c2 = NamedCandidate::new(
        "second".to_string(),
        regime_id,
        cfg_a,
        cfg_c,
        2,
        &mut interner,
    );
    let c3 = NamedCandidate::new(
        "unallowed".to_string(),
        regime_id,
        cfg_a,
        cfg_c,
        1,
        &mut interner,
    );

    let candidates = vec![c1.clone(), c2.clone(), c3.clone()];
    let policy = GuardPolicy(|_e: &ExecConfig, c: &NamedCandidate| c.name != "unallowed");

    let world_handle = interner.intern(cfg_a.digest());
    let policy_handle = interner.intern(ConfigId::from_canon(b"policy").digest());
    let exec_cfg = ExecConfig::new(world_handle, policy_handle, History::empty().digest());

    // 1. Why not c3? -> RejectedByPolicy
    let why_not_c3 = explain_why_not(&candidates, &policy, &exec_cfg, &c3, &interner);
    match why_not_c3 {
        WhyNotExplanation::RejectedByPolicy { candidate, reason } => {
            assert_eq!(candidate.name, "unallowed");
            assert_eq!(reason, ReasonCode::GuardFalse);
        }
        _ => panic!("c3 must be RejectedByPolicy"),
    }

    // 2. Why not c2? -> Overshadowed by c1
    let why_not_c2 = explain_why_not(&candidates, &policy, &exec_cfg, &c2, &interner);
    match why_not_c2 {
        WhyNotExplanation::Overshadowed {
            candidate,
            key,
            selected_key,
            selected_candidate,
        } => {
            assert_eq!(candidate.name, "second");
            assert_eq!(key.priority, 2);
            assert_eq!(selected_candidate.name, "top");
            assert_eq!(selected_key.priority, 1);
        }
        _ => panic!("c2 must be Overshadowed"),
    }

    // 3. Why not c1? -> ActuallySelected (it was not rejected!)
    let why_not_c1 = explain_why_not(&candidates, &policy, &exec_cfg, &c1, &interner);
    match why_not_c1 {
        WhyNotExplanation::ActuallySelected { candidate, key } => {
            assert_eq!(candidate.name, "top");
            assert_eq!(key.priority, 1);
        }
        _ => panic!("c1 must be ActuallySelected"),
    }
}

#[test]
fn why_rederivation_is_dynamic_across_policy_updates() {
    let mut interner = Interner::new();
    let regime_id = RegimeId::named("finite-test@1");

    let cfg_a = ConfigId::from_canon(b"cfg-A");
    let cfg_b = ConfigId::from_canon(b"cfg-B");

    let c = NamedCandidate::new("c".to_string(), regime_id, cfg_a, cfg_b, 15, &mut interner);
    let candidates = vec![c.clone()];

    let world_handle = interner.intern(cfg_a.digest());
    let policy_handle = interner.intern(ConfigId::from_canon(b"policy").digest());
    let exec_cfg = ExecConfig::new(world_handle, policy_handle, History::empty().digest());

    // Under tight threshold (<= 10), c is rejected
    let strict_policy = GuardPolicy(|_e: &ExecConfig, cand: &NamedCandidate| cand.priority <= 10);
    let why_strict = explain_why(&candidates, &strict_policy, &exec_cfg, &c, &interner);
    assert!(matches!(why_strict, WhyExplanation::NotAdmitted { .. }));

    // Under relaxed threshold (<= 20), c is immediately re-derived as selected
    let relaxed_policy = GuardPolicy(|_e: &ExecConfig, cand: &NamedCandidate| cand.priority <= 20);
    let why_relaxed = explain_why(&candidates, &relaxed_policy, &exec_cfg, &c, &interner);
    assert!(matches!(why_relaxed, WhyExplanation::Selected { .. }));
}

#[test]
fn explain_why_and_why_not_fail_closed_on_key_conflict_and_never_report_selected_or_not_found() {
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

    let candidates = vec![c1.clone(), c2.clone()];
    let policy = GuardPolicy(|_e: &ExecConfig, _c: &NamedCandidate| true);

    // explain_why must return EvaluationFaulted, NEVER Selected, NEVER CandidateNotFound
    let why_res = explain_why(&candidates, &policy, &exec_cfg, &c1, &interner);
    match why_res {
        WhyExplanation::EvaluationFaulted { candidate, fault } => {
            assert_eq!(candidate, c1);
            assert!(matches!(fault, EvaluationFault::KeyConflict(_)));
        }
        WhyExplanation::Selected { .. } => {
            panic!("explain_why must NEVER report Selected after a key conflict")
        }
        WhyExplanation::CandidateNotFound => {
            panic!("explain_why must NEVER report CandidateNotFound for an existing candidate")
        }
        other => panic!("expected EvaluationFaulted, got {other:?}"),
    }

    // explain_why_not must return EvaluationFaulted, NEVER ActuallySelected, NEVER CandidateNotFound
    let why_not_res = explain_why_not(&candidates, &policy, &exec_cfg, &c1, &interner);
    match why_not_res {
        WhyNotExplanation::EvaluationFaulted { candidate, fault } => {
            assert_eq!(candidate, c1);
            assert!(matches!(fault, EvaluationFault::KeyConflict(_)));
        }
        WhyNotExplanation::ActuallySelected { .. } => {
            panic!("explain_why_not must NEVER report ActuallySelected after a key conflict")
        }
        WhyNotExplanation::CandidateNotFound => {
            panic!("explain_why_not must NEVER report CandidateNotFound for an existing candidate")
        }
        other => panic!("expected EvaluationFaulted, got {other:?}"),
    }
}
