//! Differential parity tests between IncrementalEngine and naive oracle recomputation (ADR-0002 §9.2).

use std::collections::BTreeSet;

use brix_semantic::{ConfigId, ContextId, GeneratorId, Outcome};
use soc_core::adm::AdmAll;
use soc_core::calendar::Key;
use soc_core::commit::{commit_tick, Committed, SettlementWitnessProvider};
use soc_core::delta::Delta;
use soc_core::engine::{naive_view_over, IncrementalEngine};
use soc_core::exec::ExecConfig;
use soc_core::intern::Interner;
use soc_core::journal::Journal;
use soc_core::witness_provider::{Candidate, WitnessProvider};
use soc_core::History;
use soc_regimes::finite_frontier::{FiniteCandidateRegime, FINITE_FRONTIER_GENERATOR_NAME};

#[test]
fn differential_parity_incremental_view_equals_naive_recompute_over_delta_stream() {
    let mut interner = Interner::new();
    let mut regime = FiniteCandidateRegime::default_regime(&mut interner);

    let cfgs: Vec<ConfigId> = (0..6)
        .map(|i| ConfigId::from_canon(format!("world-{i}").as_bytes()))
        .collect();

    let mut handles = Vec::new();
    for cfg in &cfgs {
        handles.push(regime.register_config(&mut interner, *cfg));
    }

    // Register transitions:
    // world-0 -> world-1
    // world-1 -> world-2
    // world-2 -> world-3
    // world-3 -> world-4
    // world-4 -> world-5
    for i in 0..5 {
        regime.register_named_candidate(
            &mut interner,
            format!("step-{i}"),
            cfgs[i],
            cfgs[i + 1],
            i as u64,
        );
    }

    let policy_handle = interner.intern(ConfigId::from_canon(b"policy").digest());
    let history_digest = History::empty().digest();

    let mut engine = IncrementalEngine::new(vec![Box::new(regime.clone())]);
    let mut present = BTreeSet::new();

    let stream = vec![
        Delta::of_added([handles[0], handles[2]]),
        Delta::of_added([handles[4]]),
        Delta::of_removed([handles[0]]),
        Delta::of_added([handles[1], handles[3]]),
        Delta::of_removed([handles[2], handles[4]]),
        Delta::of_added([handles[0]]),
        Delta::of_removed([handles[1], handles[3], handles[0]]),
    ];

    let naive_providers: Vec<&dyn WitnessProvider> = vec![&regime];

    for (step_idx, delta) in stream.into_iter().enumerate() {
        for h in &delta.added {
            present.insert(*h);
        }
        for h in &delta.removed {
            present.remove(h);
        }

        let report = engine.step(&delta);
        let expected_view = naive_view_over(
            &naive_providers,
            &AdmAll,
            &present,
            policy_handle,
            history_digest,
        );

        assert_eq!(
            engine.view(),
            &expected_view,
            "Delta step {step_idx}: incremental view must match naive recompute exactly"
        );
        assert_eq!(
            report
                .candidate_delta
                .added
                .intersection(&report.candidate_delta.removed)
                .count(),
            0,
            "Added and removed candidate sets in delta report must be disjoint"
        );
    }
}

#[test]
fn try_decompose_produces_valid_recorded_decomposition() {
    let mut interner = Interner::new();
    let mut regime = FiniteCandidateRegime::default_regime(&mut interner);

    let cfg_src = ConfigId::from_canon(b"src-cfg");
    let cfg_dst = ConfigId::from_canon(b"dst-cfg");

    let candidate = regime.register_named_candidate(
        &mut interner,
        "transition".to_string(),
        cfg_src,
        cfg_dst,
        1,
    );

    let world_handle = candidate.src_handle;
    let policy_handle = interner.intern(ConfigId::from_canon(b"policy").digest());
    let exec_cfg = ExecConfig::new(world_handle, policy_handle, History::empty().digest());

    let raw_candidate = candidate.to_candidate();
    let decomp = regime
        .try_decompose(&exec_cfg, &raw_candidate)
        .expect("must decompose");

    assert_eq!(decomp.generators().len(), 1);
    assert_eq!(
        decomp.generators()[0],
        GeneratorId::named(FINITE_FRONTIER_GENERATOR_NAME)
    );
    assert_eq!(decomp.configs().len(), 2);
    assert_eq!(decomp.configs()[0], cfg_src);
    assert_eq!(decomp.configs()[1], cfg_dst);

    let unrelated = Candidate {
        witness: candidate.witness_handle,
        successor: candidate.src_handle,
    };
    assert_eq!(
        regime.try_decompose(&exec_cfg, &unrelated),
        Err(soc_core::commit::CommitError::CandidateMismatch),
        "decomposition must only accept a candidate this regime actually enumerated"
    );
}

#[test]
fn full_soc_commit_loop_integration() {
    let mut interner = Interner::new();
    let mut regime = FiniteCandidateRegime::default_regime(&mut interner);

    let cfg_0 = ConfigId::from_canon(b"state-0");
    let cfg_1 = ConfigId::from_canon(b"state-1");

    let candidate =
        regime.register_named_candidate(&mut interner, "step-0-1".to_string(), cfg_0, cfg_1, 5);

    let world_handle = candidate.src_handle;
    let policy_handle = interner.intern(ConfigId::from_canon(b"policy").digest());
    let exec_cfg = ExecConfig::new(world_handle, policy_handle, History::empty().digest());

    let mut journal = Journal::new();
    let providers: Vec<&dyn SettlementWitnessProvider> = vec![&regime];

    // Keyer function assigning canonical key
    let mut keyer =
        |c: &Candidate, phase: u64| -> Key { Key::new(phase, 5, interner.resolve(c.witness)) };

    let (committed, step_opt, _cost) = commit_tick(
        &providers,
        &AdmAll,
        &interner,
        &exec_cfg,
        ContextId::root(),
        0,
        &mut keyer,
    );

    match committed {
        Committed::Step {
            observation,
            successor,
        } => {
            assert_eq!(observation.outcome_class, Outcome::Derived);
            assert_eq!(successor.world, candidate.successor_handle);
            assert_eq!(successor.policy, policy_handle);
            assert_ne!(successor.history, exec_cfg.history);
        }
        Committed::Quiescent => panic!("expected committed Step, got Quiescent"),
    }

    if let Some(step) = step_opt {
        journal.append(step);
    }
    assert_eq!(journal.len(), 1);
}
