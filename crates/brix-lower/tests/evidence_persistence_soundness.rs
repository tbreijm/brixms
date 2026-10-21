//! Regression tests for the trust boundaries of persisted evidence.

use brix_lower::evidence::{Authority, History, Snapshot, DISPATCH_POLICY, JOURNAL_PROFILE};

fn history(world: &str, quantity: i64) -> History {
    let mut history = History::default();
    history
        .record_snapshot(Snapshot {
            revision: 42,
            world: world.into(),
            quantities: [("o1".into(), quantity)].into(),
        })
        .expect("snapshot");
    history
}

#[test]
fn journal_cannot_mint_kernel_authority_from_a_label() {
    let journal = format!(
        "{JOURNAL_PROFILE}\nsnapshot 42 digest-42\no1=10\n.\nclaim\nproposition=CanDispatch(o1)\nsubject=o1\ncontext=rev=42;world=digest-42;policy=qty>=1;order=o1;qty=10\nauthority=ProofKernel\nassumptions=\n.\n"
    );
    if let Ok(decoded) = History::decode_journal(&journal) {
        for claim in decoded.claims() {
            let checked = decoded.recheck_historical(claim);
            assert!(
                claim.authority != Authority::ProofKernel || !claim.authentic(),
                "decoding minted an authentic kernel receipt without a certificate: {claim:?}; historical check: {checked:?}"
            );
        }
    }
}

#[test]
fn dispatch_certificate_cannot_move_between_worlds_at_the_same_revision() {
    let mut origin = history("world-A", 10);
    assert!(origin.establish_can_dispatch(42, "o1").authorizes());
    let certificate = origin.export_dispatch_at("o1", 42).unwrap();
    assert!(origin
        .use_dispatch(&certificate, "o1", DISPATCH_POLICY, 42)
        .authorizes());
    let unrelated = history("world-B", 10);
    let result = unrelated.use_dispatch(&certificate, "o1", DISPATCH_POLICY, 42);
    assert!(
        !result.authorizes(),
        "a world-A certificate was accepted against world-B: {result:?}"
    );
}

#[test]
fn recording_a_conflicting_revision_cannot_rewrite_historical_evidence() {
    let mut history = history("world-A", 10);
    assert!(history.establish_can_dispatch(42, "o1").authorizes());
    let receipt = history.claims()[0].clone();
    assert!(history
        .record_snapshot(Snapshot {
            revision: 42,
            world: "replacement-world".into(),
            quantities: [("o1".into(), 0)].into(),
        })
        .is_err());
    let historical = history.recheck_historical(&receipt);
    assert!(
        historical.authorizes(),
        "overwriting a revision destroyed the retained historical claim: {historical:?}"
    );
}

#[test]
fn genuine_historical_claim_can_be_rechecked_after_journal_reload() {
    let mut origin = history("world-A", 10);
    assert!(origin.establish_can_dispatch(42, "o1").authorizes());
    let encoded = origin.encode_journal().unwrap();
    let decoded = History::decode_journal(&encoded).unwrap();
    let claim = &decoded.claims()[0];
    assert!(!claim.authentic(), "loaded claims must start untrusted");
    let checked = decoded.recheck_historical(claim);
    assert!(
        checked.authorizes(),
        "a genuine persisted claim should be independently rechecked: {checked:?}"
    );
}

#[test]
fn historical_recheck_cannot_substitute_another_world() {
    let mut origin = history("world-A", 10);
    assert!(origin.establish_can_dispatch(42, "o1").authorizes());
    let receipt = &origin.claims()[0];
    assert!(origin.recheck_historical(receipt).authorizes());
    let unrelated = history("world-B", 10);
    let checked = unrelated.recheck_historical(receipt);
    assert!(
        !checked.authorizes(),
        "historical rechecking silently rebound world-A to world-B: {checked:?}"
    );
}
