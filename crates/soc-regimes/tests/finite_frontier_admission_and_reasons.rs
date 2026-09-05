//! Tests for first-class admission policies, complete candidate evaluation, and structured reasons (ADR-0002 §5, §5.5).

use brix_semantic::{ConfigId, RegimeId};
use soc_core::adm::Adm;
use soc_core::exec::ExecConfig;
use soc_core::intern::Interner;
use soc_core::History;
use soc_regimes::finite_frontier::{
    AdmissionDecision, AdmissionPolicy, CandidateStatus, DeliberationOutcome, DenyAllPolicy,
    EvaluatedFrontier, EvaluationFault, FnPolicy, GuardPolicy, NamedCandidate, PolicyToAdmAdapter,
    ReasonCode,
};

#[test]
fn complete_evaluation_records_all_admitted_and_rejected_candidates() {
    let mut interner = Interner::new();
    let regime_id = RegimeId::named("finite-test@1");

    let cfg_a = ConfigId::from_canon(b"cfg-A");
    let cfg_b = ConfigId::from_canon(b"cfg-B");
    let cfg_c = ConfigId::from_canon(b"cfg-C");

    let c1 = NamedCandidate::new(
        "allowed-1".to_string(),
        regime_id,
        cfg_a,
        cfg_b,
        10,
        &mut interner,
    );
    let c2 = NamedCandidate::new(
        "disallowed-2".to_string(),
        regime_id,
        cfg_a,
        cfg_c,
        5,
        &mut interner,
    );
    let c3 = NamedCandidate::new(
        "allowed-3".to_string(),
        regime_id,
        cfg_a,
        cfg_b,
        20,
        &mut interner,
    );

    let guard = GuardPolicy(|_e: &ExecConfig, c: &NamedCandidate| c.name != "disallowed-2");

    let world_handle = interner.intern(cfg_a.digest());
    let policy_handle = interner.intern(brix_semantic::ConfigId::from_canon(b"policy").digest());
    let exec_cfg = ExecConfig::new(world_handle, policy_handle, History::empty().digest());

    let res = EvaluatedFrontier::evaluate(
        vec![c1.clone(), c2.clone(), c3.clone()],
        &guard,
        &exec_cfg,
        &interner,
    );

    assert_eq!(res.admitted_count(), 2);
    assert_eq!(res.rejected_count(), 1);
    assert_eq!(res.total_count(), 3);

    // Check rejected set structure
    let rejection_reason = res.rejected.get(&c2).expect("c2 must be in rejected set");
    assert_eq!(*rejection_reason, ReasonCode::GuardFalse);

    // Check statuses
    assert_eq!(res.status_of(&c1), Some(CandidateStatus::Selected));
    assert_eq!(
        res.status_of(&c3),
        Some(CandidateStatus::AdmittedNotSelected)
    );
    assert_eq!(
        res.status_of(&c2),
        Some(CandidateStatus::RejectedGuardFalse)
    );

    // Check selected is the least among admitted (c1 priority 10 vs c3 priority 20)
    let (_, sel_cand) = res.selected.expect("must select");
    assert_eq!(sel_cand.name, "allowed-1");
}

#[test]
fn structured_reason_codes_and_guard_rejection() {
    let mut interner = Interner::new();
    let regime_a = RegimeId::named("regime-A@1");

    let cfg_a = ConfigId::from_canon(b"cfg-A");
    let cfg_b = ConfigId::from_canon(b"cfg-B");

    let c = NamedCandidate::new(
        "candidate".to_string(),
        regime_a,
        cfg_a,
        cfg_b,
        50,
        &mut interner,
    );

    let world_handle = interner.intern(cfg_a.digest());
    let policy_handle = interner.intern(ConfigId::from_canon(b"policy").digest());
    let exec_cfg = ExecConfig::new(world_handle, policy_handle, History::empty().digest());

    // GuardPolicy returning false
    let guard = GuardPolicy(|_e: &ExecConfig, _cand: &NamedCandidate| false);
    let dec = guard.evaluate(&exec_cfg, &c);
    assert_eq!(dec, AdmissionDecision::Rejected(ReasonCode::GuardFalse));
    assert_eq!(ReasonCode::GuardFalse.category(), "guard_false@1");
    assert_eq!(
        format!("{}", ReasonCode::GuardFalse),
        "rejected guard-false"
    );

    // FnPolicy custom rejection preserves structured reason in status_of
    let fn_policy = FnPolicy(|_e: &ExecConfig, cand: &NamedCandidate| {
        if cand.name == "other" {
            AdmissionDecision::Admitted
        } else {
            AdmissionDecision::Rejected(ReasonCode::Custom {
                code: "not_other@1",
                detail: "Name was not other".to_string(),
            })
        }
    });
    let dec_fn = fn_policy.evaluate(&exec_cfg, &c);
    match dec_fn {
        AdmissionDecision::Rejected(ReasonCode::Custom { code, detail }) => {
            assert_eq!(code, "not_other@1");
            assert_eq!(detail, "Name was not other");
        }
        _ => panic!("expected Custom rejection"),
    }

    // Verify EvaluatedFrontier preserves the custom reason and does NOT call it RejectedGuardFalse
    let evaluated = EvaluatedFrontier::evaluate(vec![c.clone()], &fn_policy, &exec_cfg, &interner);
    assert_eq!(evaluated.rejected_count(), 1);
    let expected_reason = ReasonCode::Custom {
        code: "not_other@1",
        detail: "Name was not other".to_string(),
    };
    assert_eq!(
        evaluated.status_of(&c),
        Some(CandidateStatus::Rejected(expected_reason.clone()))
    );
    assert_ne!(
        evaluated.status_of(&c),
        Some(CandidateStatus::RejectedGuardFalse),
        "custom policy rejection must not be collapsed to RejectedGuardFalse"
    );
}

#[test]
fn admission_error_causes_evaluation_fault_and_fails_closed() {
    let mut interner = Interner::new();
    let regime_id = RegimeId::named("finite-test@1");
    let cfg_a = ConfigId::from_canon(b"cfg-A");
    let cfg_b = ConfigId::from_canon(b"cfg-B");

    let c = NamedCandidate::new("faulty", regime_id, cfg_a, cfg_b, 10, &mut interner);
    let world_handle = interner.intern(cfg_a.digest());
    let policy_handle = interner.intern(ConfigId::from_canon(b"policy").digest());
    let exec_cfg = ExecConfig::new(world_handle, policy_handle, History::empty().digest());

    let error_policy = FnPolicy(|_e: &ExecConfig, _c: &NamedCandidate| {
        AdmissionDecision::Error("simulated arithmetic overflow in guard".to_string())
    });

    let res = EvaluatedFrontier::evaluate(vec![c.clone()], &error_policy, &exec_cfg, &interner);
    assert!(res.selected.is_none(), "admission error must fail closed");
    assert!(
        !res.is_quiescent(),
        "admission error is a fault, not quiescence"
    );
    assert!(matches!(
        res.fault(),
        Some(EvaluationFault::AdmissionError { detail, .. }) if detail.contains("arithmetic overflow")
    ));
    assert!(matches!(
        res.selection_outcome(),
        Err(EvaluationFault::AdmissionError { .. })
    ));
    assert!(matches!(
        res.deliberation_outcome(),
        Err(EvaluationFault::AdmissionError { .. })
    ));
}

#[test]
fn all_rejected_is_valid_quiescence() {
    let mut interner = Interner::new();
    let regime_id = RegimeId::named("finite-test@1");

    let cfg_a = ConfigId::from_canon(b"cfg-A");
    let cfg_b = ConfigId::from_canon(b"cfg-B");

    let c1 = NamedCandidate::new("c1", regime_id, cfg_a, cfg_b, 10, &mut interner);
    let c2 = NamedCandidate::new("c2", regime_id, cfg_a, cfg_b, 20, &mut interner);

    let world_handle = interner.intern(cfg_a.digest());
    let policy_handle = interner.intern(ConfigId::from_canon(b"policy").digest());
    let exec_cfg = ExecConfig::new(world_handle, policy_handle, History::empty().digest());

    let policy = DenyAllPolicy;
    let res =
        EvaluatedFrontier::evaluate(vec![c1.clone(), c2.clone()], &policy, &exec_cfg, &interner);

    assert_eq!(res.admitted_count(), 0);
    assert_eq!(res.rejected_count(), 2);
    assert_eq!(res.selected, None);
    assert!(res.is_quiescent());
    assert!(res.fault().is_none());

    // Valid quiescence returns Ok(None) and Ok(DeliberationOutcome::Quiescent)
    assert_eq!(res.selection_outcome(), Ok(None));
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
fn policy_to_adm_adapter_interop() {
    let mut interner = Interner::new();
    let regime_id = RegimeId::named("finite-test@1");
    let cfg_a = ConfigId::from_canon(b"cfg-A");
    let cfg_b = ConfigId::from_canon(b"cfg-B");

    let c = NamedCandidate::new("c", regime_id, cfg_a, cfg_b, 10, &mut interner);
    let world_handle = interner.intern(cfg_a.digest());
    let policy_handle = interner.intern(ConfigId::from_canon(b"policy").digest());
    let exec_cfg = ExecConfig::new(world_handle, policy_handle, History::empty().digest());

    // Adapt structured GuardPolicy to boolean Adm
    let guard = GuardPolicy(|_e: &ExecConfig, cand: &NamedCandidate| cand.name == "c");
    let adm_adapter = PolicyToAdmAdapter::new(&guard, vec![c.clone()]);

    let raw_cand = c.to_candidate();
    assert!(adm_adapter.admits(&exec_cfg, &raw_cand));

    let unknown_cand = soc_core::witness_provider::Candidate {
        witness: interner.intern(ConfigId::from_canon(b"unknown-wit").digest()),
        successor: interner.intern(ConfigId::from_canon(b"unknown-succ").digest()),
    };
    assert!(!adm_adapter.admits(&exec_cfg, &unknown_cand));
}
