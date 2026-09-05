//! Tests for canonical key deliberation, order independence, tie-breaking, and key conflicts
//! in the finite named-candidate execution profile (ADR-0002 §1, §8.1).

use brix_semantic::{ConfigId, RegimeId};
use soc_core::exec::ExecConfig;
use soc_core::intern::Interner;
use soc_core::History;
use soc_regimes::finite_frontier::{
    AdmitAllPolicy, DeliberationOutcome, EvaluatedFrontier, EvaluationFault, NamedCandidate,
};

#[test]
fn evaluation_is_strictly_order_independent() {
    let mut interner = Interner::new();
    let regime_id = RegimeId::named("finite-test@1");

    let cfg_a = ConfigId::from_canon(b"cfg-A");
    let cfg_b = ConfigId::from_canon(b"cfg-B");
    let cfg_c = ConfigId::from_canon(b"cfg-C");
    let cfg_d = ConfigId::from_canon(b"cfg-D");

    let c1 = NamedCandidate::new(
        "cand-1".to_string(),
        regime_id,
        cfg_a,
        cfg_b,
        10,
        &mut interner,
    );
    let c2 = NamedCandidate::new(
        "cand-2".to_string(),
        regime_id,
        cfg_a,
        cfg_c,
        5,
        &mut interner,
    );
    let c3 = NamedCandidate::new(
        "cand-3".to_string(),
        regime_id,
        cfg_a,
        cfg_d,
        20,
        &mut interner,
    );

    let world_handle = interner.intern(cfg_a.digest());
    let policy_handle = interner.intern(brix_semantic::ConfigId::from_canon(b"policy").digest());
    let exec_cfg = ExecConfig::new(world_handle, policy_handle, History::empty().digest());

    let policy = AdmitAllPolicy;

    // Permutation 1: c1, c2, c3
    let res1 = EvaluatedFrontier::evaluate(
        vec![c1.clone(), c2.clone(), c3.clone()],
        &policy,
        &exec_cfg,
        &interner,
    );
    // Permutation 2: c3, c2, c1
    let res2 = EvaluatedFrontier::evaluate(
        vec![c3.clone(), c2.clone(), c1.clone()],
        &policy,
        &exec_cfg,
        &interner,
    );
    // Permutation 3: c2, c1, c3
    let res3 = EvaluatedFrontier::evaluate(
        vec![c2.clone(), c1.clone(), c3.clone()],
        &policy,
        &exec_cfg,
        &interner,
    );

    assert_eq!(res1.selected, res2.selected);
    assert_eq!(res1.selected, res3.selected);
    assert_eq!(res1.admitted, res2.admitted);
    assert_eq!(res1.admitted, res3.admitted);
    assert_eq!(res1.admitted.len(), 3);
    assert_eq!(res1.rejected.len(), 0);

    // Selected must be c2 (phase 0, priority 5 beats priority 10 and priority 20)
    let (sel_key, sel_cand) = res1.selected.expect("must have selected candidate");
    assert_eq!(sel_cand.name, "cand-2");
    assert_eq!(sel_key.phase, 0);
    assert_eq!(sel_key.priority, 5);
}

#[test]
fn tiebreak_uses_canonical_digests_when_phase_and_priority_are_equal() {
    let mut interner = Interner::new();
    let regime_id = RegimeId::named("finite-test@1");

    let cfg_a = ConfigId::from_canon(b"cfg-A");
    let cfg_b = ConfigId::from_canon(b"cfg-B");
    let cfg_c = ConfigId::from_canon(b"cfg-C");

    let c1 = NamedCandidate::new(
        "alpha".to_string(),
        regime_id,
        cfg_a,
        cfg_b,
        10,
        &mut interner,
    );
    let c2 = NamedCandidate::new(
        "beta".to_string(),
        regime_id,
        cfg_a,
        cfg_c,
        10,
        &mut interner,
    );

    let k1 = c1.canonical_key(&interner);
    let k2 = c2.canonical_key(&interner);

    assert_eq!(k1.phase, k2.phase);
    assert_eq!(k1.priority, k2.priority);
    assert_ne!(
        k1.tiebreak, k2.tiebreak,
        "distinct candidates must have distinct canonical tiebreaks"
    );

    let world_handle = interner.intern(cfg_a.digest());
    let policy_handle = interner.intern(brix_semantic::ConfigId::from_canon(b"policy").digest());
    let exec_cfg = ExecConfig::new(world_handle, policy_handle, History::empty().digest());

    let res = EvaluatedFrontier::evaluate(
        vec![c1.clone(), c2.clone()],
        &AdmitAllPolicy,
        &exec_cfg,
        &interner,
    );
    let (sel_key, sel_cand) = res.selected.expect("must select candidate");

    let expected_winner = if k1 < k2 { &c1 } else { &c2 };
    let expected_key = if k1 < k2 { k1 } else { k2 };

    assert_eq!(sel_key, expected_key);
    assert_eq!(sel_cand.name, expected_winner.name);
}

#[test]
fn human_name_is_not_part_of_the_canonical_candidate_identity() {
    let mut interner = Interner::new();
    let regime_id = RegimeId::named("finite-test@1");
    let src = ConfigId::from_canon(b"source");
    let dst = ConfigId::from_canon(b"successor");
    let first = NamedCandidate::new(
        "display-name-a".to_string(),
        regime_id,
        src,
        dst,
        1,
        &mut interner,
    );
    let alias = NamedCandidate::new(
        "display-name-b".to_string(),
        regime_id,
        src,
        dst,
        1,
        &mut interner,
    );

    assert_eq!(
        first.canonical_identity(&interner),
        alias.canonical_identity(&interner)
    );
    assert_eq!(
        first.canonical_key(&interner),
        alias.canonical_key(&interner)
    );
}

#[test]
fn phase_zero_is_strictly_enforced_and_lower_priority_wins() {
    let mut interner = Interner::new();
    let regime_id = RegimeId::named("finite-test@1");

    let cfg_a = ConfigId::from_canon(b"cfg-A");
    let cfg_b = ConfigId::from_canon(b"cfg-B");
    let cfg_c = ConfigId::from_canon(b"cfg-C");

    // Standard constructor enforces phase 0. Lower numerical priority wins.
    let c1 = NamedCandidate::new(
        "prio-5".to_string(),
        regime_id,
        cfg_a,
        cfg_b,
        5,
        &mut interner,
    );
    let c2 = NamedCandidate::new(
        "prio-10".to_string(),
        regime_id,
        cfg_a,
        cfg_c,
        10,
        &mut interner,
    );

    assert_eq!(c1.phase, 0);
    assert_eq!(c2.phase, 0);

    let world_handle = interner.intern(cfg_a.digest());
    let policy_handle = interner.intern(brix_semantic::ConfigId::from_canon(b"policy").digest());
    let exec_cfg = ExecConfig::new(world_handle, policy_handle, History::empty().digest());

    let res = EvaluatedFrontier::evaluate(
        vec![c1.clone(), c2.clone()],
        &AdmitAllPolicy,
        &exec_cfg,
        &interner,
    );
    let (sel_key, sel_cand) = res.selected.expect("must select candidate");

    assert_eq!(sel_key.phase, 0);
    assert_eq!(sel_key.priority, 5);
    assert_eq!(sel_cand.name, "prio-5");

    // Negative coverage: introducing a candidate with non-zero phase triggers EvaluationFault::InvalidPhase
    let bad_phase_cand = NamedCandidate::with_phase_for_test(
        "bad-phase".to_string(),
        regime_id,
        cfg_a,
        cfg_b,
        1,
        0,
        &mut interner,
    );
    let res_bad = EvaluatedFrontier::evaluate(
        vec![c1.clone(), bad_phase_cand.clone()],
        &AdmitAllPolicy,
        &exec_cfg,
        &interner,
    );
    assert!(
        res_bad.selected.is_none(),
        "must fail closed on non-zero phase"
    );
    assert!(matches!(
        res_bad.selection_outcome(),
        Err(EvaluationFault::InvalidPhase { phase: 1, .. })
    ));
}

#[test]
fn duplicate_key_conflict_is_detected_and_recorded() {
    let mut interner = Interner::new();
    let regime_id = RegimeId::named("finite-test@1");

    let cfg_a = ConfigId::from_canon(b"cfg-A");
    let cfg_b = ConfigId::from_canon(b"cfg-B");
    let cfg_c = ConfigId::from_canon(b"cfg-C");

    let c1 = NamedCandidate::new(
        "same-name".to_string(),
        regime_id,
        cfg_a,
        cfg_b,
        5,
        &mut interner,
    );
    // Manually build c2 with differing successor handle but same phase, priority, and simulated tiebreak conflict
    let mut c2 = NamedCandidate::new(
        "same-name".to_string(),
        regime_id,
        cfg_a,
        cfg_c,
        5,
        &mut interner,
    );
    // Force c2 to carry c1's handles and name so tiebreak matches, but with a different destination config
    c2.src_handle = c1.src_handle;
    c2.witness_handle = c1.witness_handle;
    c2.successor_handle = c1.successor_handle;
    c2.dst = cfg_c; // Different dst -> different candidate value

    let world_handle = interner.intern(cfg_a.digest());
    let policy_handle = interner.intern(brix_semantic::ConfigId::from_canon(b"policy").digest());
    let exec_cfg = ExecConfig::new(world_handle, policy_handle, History::empty().digest());

    let res = EvaluatedFrontier::evaluate(
        vec![c1.clone(), c2.clone()],
        &AdmitAllPolicy,
        &exec_cfg,
        &interner,
    );

    assert_eq!(
        res.key_conflicts.len(),
        1,
        "must detect key conflict under B^uk discipline"
    );
    assert_eq!(res.key_conflicts[0].existing, c1);
    assert_eq!(res.key_conflicts[0].attempted, c2);
    assert!(
        res.selected.is_none(),
        "an invalid unique-key frontier must fail closed"
    );
    assert!(res.fault().is_some());
    assert!(matches!(
        res.selection_outcome(),
        Err(EvaluationFault::KeyConflict(_))
    ));
    assert!(matches!(
        res.deliberation_outcome(),
        Err(EvaluationFault::KeyConflict(_))
    ));
    assert!(matches!(
        EvaluatedFrontier::try_evaluate(
            vec![c1.clone(), c2.clone()],
            &AdmitAllPolicy,
            &exec_cfg,
            &interner
        ),
        Err(EvaluationFault::KeyConflict(_))
    ));
}

#[test]
fn empty_candidate_pool_yields_quiescence() {
    let mut interner = Interner::new();
    let cfg_a = ConfigId::from_canon(b"cfg-A");
    let world_handle = interner.intern(cfg_a.digest());
    let policy_handle = interner.intern(brix_semantic::ConfigId::from_canon(b"policy").digest());
    let exec_cfg = ExecConfig::new(world_handle, policy_handle, History::empty().digest());

    let candidates: Vec<NamedCandidate> = Vec::new();
    let res = EvaluatedFrontier::evaluate(candidates, &AdmitAllPolicy, &exec_cfg, &interner);

    assert!(res.is_quiescent());
    assert_eq!(res.selected, None);
    assert_eq!(res.admitted_count(), 0);
    assert_eq!(res.rejected_count(), 0);
    assert_eq!(
        res.deliberation_outcome(),
        Ok(DeliberationOutcome::Quiescent)
    );
    assert_eq!(res.selection_outcome(), Ok(None));
}
