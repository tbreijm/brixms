//! Checkpoint 7 (R11): Portable settled decision evidence tests (`brix.world.decision-evidence@1`).
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use brix_canon::{Digest, Domain};
use brix_kb::world::decision_codec::Value;
use brix_kb::world::decision_evidence::{verify_decision_evidence, DecisionEvidence};
use brix_kb::world::{TupleRecord, WorldBatch, WorldBatchOp, WorldKey, WorldSession};
use brix_lower::evidence::Authority;

struct TestDir(PathBuf);
impl TestDir {
    fn new(label: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "brix-decision-evidence-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const PROGRAM: &str = r#"
config Plan = AskBack(List<Str>) | Done

rel input slots: { id: Str, parent: Str, slot: Str, prio: Int } key id

rel derived parent_slots = select {
    parent: s.parent,
    items: collect(s.slot, s.prio, 10)
} from s in slots group by s.parent

decide parent_plan for p in parent_slots per parent {
    propose ask priority 1 when true = Plan::AskBack(p.items)
}
"#;

fn slot(id: &str, parent: &str, slot_name: &str, prio: i64) -> WorldBatchOp {
    let mut row = TupleRecord::new();
    row.set_str("id", id);
    row.set_str("parent", parent);
    row.set_str("slot", slot_name);
    row.set_str("prio", &prio.to_string());
    WorldBatchOp::Upsert {
        relation: "root::slots".into(),
        key: WorldKey::from_str(id),
        tuple: row.to_tuple(),
    }
}

fn review_fixture(label: &str) -> (TestDir, WorldSession, DecisionEvidence) {
    let dir = TestDir::new(label);
    let sources = BTreeMap::from([("root".into(), PROGRAM.into())]);
    let mut session =
        WorldSession::from_program_with_sources(&dir.0, "root", &sources).expect("create session");
    session
        .apply_batch(WorldBatch::new(
            0,
            "seed",
            vec![slot("s1", "p1", "volume", 10)],
        ))
        .expect("seed revision");
    let evidence = session
        .request_decision_evidence(
            1,
            "root::parent_plan",
            "p1",
            vec!["read:atlas".into()],
            true,
        )
        .expect("request valid evidence");
    (dir, session, evidence)
}

#[test]
fn review_runtime_receipt_requires_independent_replay() {
    let (_dir, _session, mut evidence) = review_fixture("no-replay");
    let expected = evidence.target.clone();
    evidence.bundle = None;
    assert!(
        verify_decision_evidence(&evidence, &expected).is_err(),
        "a cached settlement Merkle proof alone must not issue runtime authority"
    );
}

#[test]
fn review_manifest_and_program_cannot_be_relabelled_without_replay() {
    let (_dir, _session, mut evidence) = review_fixture("relabelled");
    let mut expected = evidence.target.clone();
    expected.world = Digest::of(Domain::Value, b"different trusted world");
    expected.program = Digest::of(Domain::Value, b"different trusted program");
    evidence.target = expected.clone();
    evidence.bundle = None;
    assert!(
        verify_decision_evidence(&evidence, &expected).is_err(),
        "rewriting the envelope must not satisfy independent world/program pins"
    );
}

#[test]
fn review_grants_must_be_bound_to_replayed_context() {
    let (_dir, _session, mut evidence) = review_fixture("grants");
    let mut expected = evidence.target.clone();
    expected.grants = vec!["admin:all".into()];
    evidence.target.grants = expected.grants.clone();
    assert!(
        verify_decision_evidence(&evidence, &expected).is_err(),
        "editing envelope grants must not change the authority of the same replay"
    );
}

#[test]
fn review_historical_request_replays_the_requested_revision() {
    let (dir, mut session, original) = review_fixture("historical");
    fs::remove_file(session.decision_evidence_path(&original.target))
        .expect("remove only the test fixture's cached evidence");
    session
        .apply_batch(WorldBatch::new(
            1,
            "later revision",
            vec![slot("s2", "p1", "customer-grain", 20)],
        ))
        .expect("advance world");
    let evidence = session
        .request_decision_evidence(
            1,
            "root::parent_plan",
            "p1",
            vec!["read:atlas".into()],
            true,
        )
        .expect("historical evidence remains requestable after HEAD advances");
    let expected = evidence.target.clone();
    drop(session);
    drop(dir);
    verify_decision_evidence(&evidence, &expected).expect("portable historical replay");
}

#[test]
fn test_decision_evidence_request_verify_and_independent_replay() {
    let dir = TestDir::new("full");
    let sources = BTreeMap::from([("root".into(), PROGRAM.into())]);
    let mut session =
        WorldSession::from_program_with_sources(&dir.0, "root", &sources).expect("create session");

    // Commit revision 1
    session
        .apply_batch(WorldBatch::new(
            0,
            "batch 1",
            vec![
                slot("s1", "p1", "volume", 10),
                slot("s2", "p1", "customer-grain", 20),
            ],
        ))
        .expect("batch 1");

    let rev = session.current_revision();
    let rev_digest = session.current_revision_digest().expect("rev digest");
    let grants = vec!["read:atlas".to_string(), "org:brixms".to_string()];

    // 1. Request decision evidence with embedded audit replay bundle
    let evidence = session
        .request_decision_evidence(rev, "root::parent_plan", "p1", grants.clone(), true)
        .expect("request evidence");

    assert_eq!(evidence.target.revision, 1);
    assert_eq!(evidence.target.revision_digest, rev_digest);
    assert_eq!(evidence.target.decide, "root::parent_plan");
    assert_eq!(evidence.target.entity, "p1");
    assert_eq!(evidence.target.candidate, "ask");
    assert_eq!(evidence.target.grants, grants);
    assert!(evidence.bundle.is_some(), "audit bundle must be included");

    let expected_val = Value::Variant {
        nominal: "root::Plan".into(),
        variant: "AskBack".into(),
        args: vec![Value::List(vec![
            Value::Str("volume".into()),
            Value::Str("customer-grain".into()),
        ])],
    };
    assert_eq!(evidence.settlement.value, expected_val);

    // 2. Independently verify decision evidence
    let expected_target = session
        .decision_target(rev, "root::parent_plan", "p1", grants.clone())
        .expect("expected target");
    let receipt = verify_decision_evidence(&evidence, &expected_target).expect("verify");
    assert_eq!(receipt.authority, Authority::RuntimeCheck);
    assert_eq!(receipt.subject, "p1");
    assert_eq!(receipt.assumptions, grants);
    assert!(receipt.authentic());

    // 3. Encode & Decode roundtrip
    let encoded = evidence.encode().expect("encode");
    let decoded = DecisionEvidence::decode(&encoded).expect("decode");
    let receipt2 = verify_decision_evidence(&decoded, &expected_target).expect("verify decoded");
    assert_eq!(receipt2, receipt);

    // 4. Persistence & reload check
    let reloaded = session
        .load_decision_evidence(&expected_target)
        .expect("load from disk");
    assert_eq!(reloaded.target, expected_target);

    // 5. Fresh process isolation check: verify from bytes only, without world session
    let isolated_receipt =
        verify_decision_evidence(&decoded, &expected_target).expect("isolated verify");
    assert_eq!(isolated_receipt.context, rev_digest.to_hex());

    // 6. Negative controls & tamper detection
    // A: Tampered candidate
    let mut bad_target = expected_target.clone();
    bad_target.candidate = "other".into();
    assert!(
        verify_decision_evidence(&evidence, &bad_target).is_err(),
        "tampered candidate must fail"
    );

    // B: Tampered entity
    let mut bad_target = expected_target.clone();
    bad_target.entity = "p2".into();
    assert!(
        verify_decision_evidence(&evidence, &bad_target).is_err(),
        "tampered entity must fail"
    );

    // C: Tampered revision digest
    let mut bad_target = expected_target.clone();
    bad_target.revision_digest = Digest::of(Domain::Value, b"fake-digest");
    assert!(
        verify_decision_evidence(&evidence, &bad_target).is_err(),
        "tampered rev digest must fail"
    );

    // D: Tampered grants
    let mut bad_target = expected_target.clone();
    bad_target.grants = vec!["forged:grant".into()];
    assert!(
        verify_decision_evidence(&evidence, &bad_target).is_err(),
        "tampered grants must fail"
    );

    // E: Tampered proof node
    let mut bad_evidence = evidence.clone();
    if let Some((_, bytes)) = bad_evidence.nodes.iter_mut().next() {
        if !bytes.is_empty() {
            bytes[0] ^= 0xff;
        }
    }
    assert!(
        verify_decision_evidence(&bad_evidence, &expected_target).is_err(),
        "tampered proof node must fail Merkle check"
    );
}

#[test]
fn test_modeled_revision_grants_caller_and_release_binding() {
    let dir = TestDir::new("modeled-grants");
    let program_with_grants = r#"
config Plan = AskBack(List<Str>) | Done

rel input slots: { id: Str, parent: Str, slot: Str, prio: Int } key id
rel input grants: { id: Str, caller: Str, entity: Str, grant: Str } key id

rel derived parent_slots = select {
    parent: s.parent,
    items: collect(s.slot, s.prio, 10)
} from s in slots group by s.parent

decide parent_plan for p in parent_slots per parent {
    propose ask priority 1 when true = Plan::AskBack(p.items)
}
"#;
    let sources = BTreeMap::from([("root".into(), program_with_grants.into())]);
    let mut session =
        WorldSession::from_program_with_sources(&dir.0, "root", &sources).expect("create session");

    let mut grant_row = TupleRecord::new();
    grant_row.set_str("id", "g1");
    grant_row.set_str("caller", "service:atlas");
    grant_row.set_str("entity", "p1");
    grant_row.set_str("grant", "read:atlas");

    session
        .apply_batch(WorldBatch::new(
            0,
            "seed-with-grants",
            vec![
                slot("s1", "p1", "volume", 10),
                WorldBatchOp::Upsert {
                    relation: "root::grants".into(),
                    key: WorldKey::from_str("g1"),
                    tuple: grant_row.to_tuple(),
                },
            ],
        ))
        .expect("seed revision");

    // Requesting an unmodeled grant ("admin:all") on a revision that models grants must fail
    assert!(
        session
            .request_decision_evidence_with_caller(
                1,
                "root::parent_plan",
                "p1",
                Some("service:atlas".into()),
                vec!["admin:all".into()],
                true,
            )
            .is_err(),
        "requesting a grant not present in the revision's modeled grants relation must fail"
    );

    // Requesting with empty grants resolves the revision's modeled grants and caller
    let evidence = session
        .request_decision_evidence_with_caller(
            1,
            "root::parent_plan",
            "p1",
            Some("service:atlas".into()),
            vec![],
            true,
        )
        .expect("request evidence with modeled grants");

    assert_eq!(evidence.target.caller.as_deref(), Some("service:atlas"));
    assert_eq!(evidence.target.grants, vec!["read:atlas".to_string()]);
    assert_eq!(evidence.target.release, env!("CARGO_PKG_VERSION"));

    let expected = evidence.target.clone();
    let receipt = verify_decision_evidence(&evidence, &expected).expect("verify modeled evidence");
    assert_eq!(receipt.authority, Authority::RuntimeCheck);
    assert_eq!(receipt.assumptions, vec!["read:atlas".to_string()]);

    // Tampering with caller or release must fail verification
    let mut bad_caller = expected.clone();
    bad_caller.caller = Some("service:other".into());
    assert!(verify_decision_evidence(&evidence, &bad_caller).is_err());

    let mut bad_release = expected.clone();
    bad_release.release = "0.0.0-forged".into();
    assert!(verify_decision_evidence(&evidence, &bad_release).is_err());
}
