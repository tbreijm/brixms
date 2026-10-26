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
config CertifiedAnswer = { path: Plan, caller: Str, release_id: Str, grants: List<Str> }

rel input slots: { id: Str, parent: Str, slot: Str, prio: Int } key id
rel input claim_meta: { parent: Str, caller: Str, release_id: Str } key parent
rel input claim_grants: { id: Str, parent: Str, grant: Str, ord: Int } key id

rel derived parent_slots = select {
    parent: s.parent,
    items: collect(s.slot, s.prio, 10)
} from s in slots group by s.parent

rel derived parent_grants = select {
    parent: g.parent,
    grants: collect(g.grant, g.ord, 10)
} from g in claim_grants group by g.parent

rel derived parent_context = select {
    parent: p.parent,
    items: p.items,
    caller: m.caller,
    release_id: m.release_id,
    grants: g.grants
} from p in parent_slots, m in claim_meta, g in parent_grants
where p.parent == m.parent && p.parent == g.parent

decide parent_plan for p in parent_context per parent {
    propose ask priority 1 when true = CertifiedAnswer {
        path: Plan::AskBack(p.items),
        caller: p.caller,
        release_id: p.release_id,
        grants: p.grants
    }
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

fn claim_meta_op(parent: &str, caller: &str, release_id: &str) -> WorldBatchOp {
    let mut row = TupleRecord::new();
    row.set_str("parent", parent);
    row.set_str("caller", caller);
    row.set_str("release_id", release_id);
    WorldBatchOp::Upsert {
        relation: "root::claim_meta".into(),
        key: WorldKey::from_str(parent),
        tuple: row.to_tuple(),
    }
}

fn claim_grant_op(id: &str, parent: &str, grant: &str, ord: i64) -> WorldBatchOp {
    let mut row = TupleRecord::new();
    row.set_str("id", id);
    row.set_str("parent", parent);
    row.set_str("grant", grant);
    row.set_str("ord", &ord.to_string());
    WorldBatchOp::Upsert {
        relation: "root::claim_grants".into(),
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
            vec![
                slot("s1", "p1", "volume", 10),
                claim_meta_op("p1", "service:atlas", "atlas-2026.10"),
                claim_grant_op("g1", "p1", "read:atlas", 10),
            ],
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
fn review_finite_runtime_validation_checks_full_revision_identity() {
    use brix_lower::l3_v2::{eval, DocumentaryRegistry, EvalEnv, L3ExprV2, L3ValueV2};
    let (_dir, _session, evidence) = review_fixture("finite-revision-pins");
    let target = evidence.target.clone();
    let verified = verify_decision_evidence(&evidence, &target).expect("real independent replay");
    let mut registry = DocumentaryRegistry::new();
    verified.admit_into(&mut registry);
    let env = EvalEnv::new().with_documentary_registry(std::sync::Arc::new(registry));
    let text = |s: String| L3ExprV2::Str(s);
    let fields = vec![
        ("decision_name".into(), text(target.decide.clone())),
        ("entity".into(), text(target.entity.clone())),
        ("world".into(), text(target.world.to_hex())),
        ("program".into(), text(target.program.to_hex())),
        ("revision".into(), L3ExprV2::Int(target.revision as i64)),
        (
            "revision_digest".into(),
            text(target.revision_digest.to_hex()),
        ),
        ("candidate".into(), text(target.candidate.clone())),
        ("result".into(), text(target.value_digest.to_hex())),
        (
            "caller".into(),
            text(target.caller.clone().unwrap_or_default()),
        ),
        ("release".into(), text(target.release.clone())),
        (
            "grants".into(),
            L3ExprV2::ListLit(target.grants.iter().cloned().map(text).collect()),
        ),
    ];
    let validate = |fields| {
        eval(
            &L3ExprV2::Validate {
                target_type: "RuntimeOutcome".into(),
                arg: Box::new(L3ExprV2::Record {
                    nominal_config: "RuntimeClaim".into(),
                    fields,
                }),
            },
            &env,
        )
        .unwrap()
    };
    assert!(
        matches!(validate(fields.clone()), L3ValueV2::Ctor { variant, .. } if variant == "Satisfied")
    );
    for component in ["revision_digest", "program"] {
        let mut changed = fields.clone();
        changed
            .iter_mut()
            .find(|(name, _)| name == component)
            .unwrap()
            .1 =
            text(Digest::of(Domain::Value, b"different independently expected context").to_hex());
        let outcome = validate(changed);
        assert!(
            !matches!(&outcome, L3ValueV2::Ctor { variant, .. } if variant == "Satisfied"),
            "changing {component} must refuse reuse: {outcome:?}"
        );
    }
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
fn review_runtime_query_cannot_omit_program_and_revision_digest() {
    use brix_lower::l3_v2::{DocumentaryRegistry, RuntimeClaimQuery};
    let (_dir, _session, evidence) = review_fixture("missing-runtime-pins");
    let t = evidence.target.clone();
    let verified = verify_decision_evidence(&evidence, &t).unwrap();
    let mut registry = DocumentaryRegistry::new();
    verified.admit_into(&mut registry);
    let query = RuntimeClaimQuery {
        decide: t.decide,
        entity: t.entity,
        world: t.world,
        program: None,
        revision: t.revision,
        revision_digest: None,
        candidate: t.candidate,
        result: t.value_digest,
        caller: t.caller,
        release: t.release,
        grants: t.grants,
    };
    assert!(
        registry.lookup_runtime_claim(&query).is_err(),
        "omitting expected program/revision pins must not bypass applicability checks"
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
fn review_rewriting_bundle_header_cannot_grant_unmodeled_authority() {
    use brix_canon::{CanonReader, CanonWriter};
    use brix_kb::world::decision_evidence::DECISION_CLAIM_BUNDLE_PREFIX;
    let (_dir, _session, mut evidence) = review_fixture("rewritten-header");
    let bundle = evidence.bundle.as_ref().unwrap();
    let mut reader = CanonReader::new(&bundle[DECISION_CLAIM_BUNDLE_PREFIX.len()..]);
    if reader.read_uint().unwrap() == 1 {
        reader.read_bytes().unwrap();
    }
    reader.read_bytes().unwrap(); // Release
    let count = reader.read_uint().unwrap();
    for _ in 0..count {
        reader.read_bytes().unwrap();
    }
    let raw_audit = reader.read_bytes().unwrap().to_vec();

    let mut expected = evidence.target.clone();
    expected.caller = Some("attacker".into());
    expected.grants = vec!["admin:all".into()];
    let mut writer = CanonWriter::new();
    writer.write_uint(1);
    writer.write_str("attacker");
    writer.write_str(&expected.release);
    writer.write_uint(1);
    writer.write_str("admin:all");
    writer.write_bytes(&raw_audit);
    let mut rewritten = DECISION_CLAIM_BUNDLE_PREFIX.to_vec();
    rewritten.extend_from_slice(&writer.finish());
    evidence.target = expected.clone();
    evidence.bundle = Some(rewritten);
    let decoded = DecisionEvidence::decode(&evidence.encode().unwrap()).unwrap();
    assert!(
        verify_decision_evidence(&decoded, &expected).is_err(),
        "duplicated attacker-controlled headers are not revision-bound grants or caller evidence"
    );
}

#[test]
fn review_claim_header_grant_count_is_bounded_before_allocation() {
    use brix_canon::CanonWriter;
    use brix_kb::world::decision_evidence::DECISION_CLAIM_BUNDLE_PREFIX;
    let (_dir, _session, mut evidence) = review_fixture("oversized-count");
    let expected = evidence.target.clone();
    let mut writer = CanonWriter::new();
    writer.write_uint(0);
    writer.write_str(&expected.release);
    writer.write_uint(u64::MAX);
    let mut malformed = DECISION_CLAIM_BUNDLE_PREFIX.to_vec();
    malformed.extend_from_slice(&writer.finish());
    evidence.bundle = Some(malformed);
    let decoded = DecisionEvidence::decode(&evidence.encode().unwrap()).unwrap();
    let result = std::panic::catch_unwind(|| verify_decision_evidence(&decoded, &expected));
    assert!(
        result.is_ok(),
        "a short hostile artifact must return an error, never panic"
    );
    assert!(result.unwrap().is_err());
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
                claim_meta_op("p1", "service:atlas", "atlas-2026.10"),
                claim_grant_op("g1", "p1", "read:atlas", 10),
                claim_grant_op("g2", "p1", "org:brixms", 20),
            ],
        ))
        .expect("batch 1");

    let rev = session.current_revision();
    let rev_digest = session.current_revision_digest().expect("rev digest");
    let mut grants = vec!["read:atlas".to_string(), "org:brixms".to_string()];
    grants.sort();

    // 1. Request decision evidence with embedded audit replay bundle
    let evidence = session
        .request_decision_evidence(rev, "root::parent_plan", "p1", grants.clone(), true)
        .expect("request evidence");

    assert_eq!(evidence.target.revision, 1);
    assert_eq!(evidence.target.revision_digest, rev_digest);
    assert_eq!(evidence.target.decide, "root::parent_plan");
    assert_eq!(evidence.target.entity, "p1");
    assert_eq!(evidence.target.candidate, "ask");
    assert_eq!(evidence.target.caller.as_deref(), Some("service:atlas"));
    assert_eq!(evidence.target.release, "atlas-2026.10");
    assert_eq!(evidence.target.grants, grants);
    assert!(evidence.bundle.is_some(), "audit bundle must be included");

    let expected_val = Value::Record {
        nominal: "root::CertifiedAnswer".into(),
        fields: BTreeMap::from([
            ("caller".into(), Value::Str("service:atlas".into())),
            (
                "grants".into(),
                Value::List(vec![
                    Value::Str("read:atlas".into()),
                    Value::Str("org:brixms".into()),
                ]),
            ),
            (
                "path".into(),
                Value::Variant {
                    nominal: "root::Plan".into(),
                    variant: "AskBack".into(),
                    args: vec![Value::List(vec![
                        Value::Str("volume".into()),
                        Value::Str("customer-grain".into()),
                    ])],
                },
            ),
            ("release_id".into(), Value::Str("atlas-2026.10".into())),
        ]),
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
    drop(session);
    drop(dir);
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
    use brix_kb::world::policy_evidence::PolicyError;
    use brix_lower::evidence::{Disposition, Reason};

    let (dir, mut session, evidence) = review_fixture("modeled-grants");

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
        "requesting a grant not present in the revision's CertifiedAnswer must fail"
    );

    // Requesting with empty grants resolves the revision's CertifiedAnswer grants, caller, and release_id
    let resolved = session
        .request_decision_evidence_with_caller(1, "root::parent_plan", "p1", None, vec![], true)
        .expect("request evidence with modeled CertifiedAnswer");

    assert_eq!(resolved.target.caller.as_deref(), Some("service:atlas"));
    assert_eq!(resolved.target.grants, vec!["read:atlas".to_string()]);
    assert_eq!(resolved.target.release, "atlas-2026.10");

    let expected = evidence.target.clone();
    drop(session);
    drop(dir);

    let receipt = verify_decision_evidence(&evidence, &expected).expect("verify modeled evidence");
    assert_eq!(receipt.authority, Authority::RuntimeCheck);
    assert_eq!(receipt.assumptions, vec!["read:atlas".to_string()]);
    assert_eq!(receipt.claim().release, "atlas-2026.10");

    // Tampering with caller or catalogue release_id produces component-specific ContextMismatch
    let mut bad_caller = expected.clone();
    bad_caller.caller = Some("service:other".into());
    match verify_decision_evidence(&evidence, &bad_caller) {
        Err(PolicyError::Refused(Disposition::Unknown {
            reason: Reason::ContextMismatch { expected, found },
        })) => {
            assert!(expected.contains("caller="));
            assert!(found.contains("caller="));
        }
        other => panic!("expected caller ContextMismatch, got {other:?}"),
    }

    let mut bad_release = expected.clone();
    bad_release.release = "atlas-0.0.0-forged".into();
    match verify_decision_evidence(&evidence, &bad_release) {
        Err(PolicyError::Refused(Disposition::Unknown {
            reason: Reason::ContextMismatch { expected, found },
        })) => {
            assert!(expected.contains("release="));
            assert!(found.contains("release"));
        }
        other => panic!("expected release ContextMismatch, got {other:?}"),
    }

    // Even if both target and BRIXDCB1 header are rewritten for release_id, replay catches it with release= mismatch
    use brix_canon::{CanonReader, CanonWriter};
    use brix_kb::world::decision_evidence::DECISION_CLAIM_BUNDLE_PREFIX;
    let bundle = evidence.bundle.as_ref().unwrap();
    let mut reader = CanonReader::new(&bundle[DECISION_CLAIM_BUNDLE_PREFIX.len()..]);
    if reader.read_uint().unwrap() == 1 {
        reader.read_bytes().unwrap();
    }
    reader.read_bytes().unwrap();
    let count = reader.read_uint().unwrap();
    for _ in 0..count {
        reader.read_bytes().unwrap();
    }
    let raw_audit = reader.read_bytes().unwrap().to_vec();

    let mut writer = CanonWriter::new();
    writer.write_uint(1);
    writer.write_str("service:atlas");
    writer.write_str("atlas-0.0.0-forged");
    writer.write_uint(1);
    writer.write_str("read:atlas");
    writer.write_bytes(&raw_audit);
    let mut rewritten_release_bundle = DECISION_CLAIM_BUNDLE_PREFIX.to_vec();
    rewritten_release_bundle.extend_from_slice(&writer.finish());
    let mut forged_release_ev = evidence.clone();
    forged_release_ev.target = bad_release.clone();
    forged_release_ev.bundle = Some(rewritten_release_bundle);
    match verify_decision_evidence(&forged_release_ev, &bad_release) {
        Err(PolicyError::Refused(Disposition::Unknown {
            reason: Reason::ContextMismatch { expected, found },
        })) => {
            assert!(expected.contains("release="));
            assert!(found.contains("release"));
        }
        other => panic!("expected replay release ContextMismatch, got {other:?}"),
    }
}

#[test]
fn test_deleting_context_declaration_or_unrelated_permission_rows_cannot_authorize_grants() {
    let dir = TestDir::new("settlement-only");
    let program_without_certified_answer = r#"
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
    let sources = BTreeMap::from([("root".into(), program_without_certified_answer.into())]);
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
            "seed",
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

    // 1. Unrelated `grants` relation cannot authorize grants when CertifiedAnswer is absent
    assert!(
        session
            .request_decision_evidence(
                1,
                "root::parent_plan",
                "p1",
                vec!["read:atlas".into()],
                true
            )
            .is_err(),
        "unrelated permission rows must not authorize grants without CertifiedAnswer"
    );
    assert!(
        session
            .request_decision_evidence_with_caller(
                1,
                "root::parent_plan",
                "p1",
                Some("service:atlas".into()),
                vec![],
                true,
            )
            .is_err(),
        "settlement-only decisions must refuse caller authority"
    );

    // 2. Settlement-only request (no caller, no grants) succeeds and verifies only empty context
    let settlement_only = session
        .request_decision_evidence(1, "root::parent_plan", "p1", vec![], true)
        .expect("settlement-only evidence");
    assert!(settlement_only.target.caller.is_none());
    assert!(settlement_only.target.grants.is_empty());
    assert!(settlement_only.target.release.is_empty());

    let expected = settlement_only.target.clone();
    verify_decision_evidence(&settlement_only, &expected).expect("settlement-only verifies");

    let mut forged_expected = expected.clone();
    forged_expected.grants = vec!["read:atlas".into()];
    let mut forged_evidence = settlement_only.clone();
    forged_evidence.target = forged_expected.clone();
    assert!(
        verify_decision_evidence(&forged_evidence, &forged_expected).is_err(),
        "settlement-only replay must reject claimed grants"
    );
}

#[test]
fn test_decoder_bounds_and_replay_work_budget() {
    use brix_canon::CanonWriter;
    use brix_kb::world::decision_evidence::{
        verify_decision_evidence_with_limits, DecisionEvidenceDecodeLimits,
        DECISION_CLAIM_BUNDLE_PREFIX,
    };

    let (_dir, _session, evidence) = review_fixture("bounds");
    let expected = evidence.target.clone();

    // 1. Invalid has_caller tag (2)
    let mut writer = CanonWriter::new();
    writer.write_uint(2);
    let mut bad_caller_tag = DECISION_CLAIM_BUNDLE_PREFIX.to_vec();
    bad_caller_tag.extend_from_slice(&writer.finish());
    let mut ev = evidence.clone();
    ev.bundle = Some(bad_caller_tag);
    assert!(verify_decision_evidence(&ev, &expected).is_err());

    // 2. Duplicate grants in header
    let mut writer = CanonWriter::new();
    writer.write_uint(1);
    writer.write_str("service:atlas");
    writer.write_str("atlas-2026.10");
    writer.write_uint(2);
    writer.write_str("read:atlas");
    writer.write_str("read:atlas");
    writer.write_bytes(b"dummy");
    let mut dup_grants = DECISION_CLAIM_BUNDLE_PREFIX.to_vec();
    dup_grants.extend_from_slice(&writer.finish());
    let mut ev = evidence.clone();
    ev.bundle = Some(dup_grants);
    assert!(verify_decision_evidence(&ev, &expected).is_err());

    // 3. Trailing bytes after claim bundle
    let mut trailing = evidence.bundle.clone().unwrap();
    trailing.push(0x00);
    let mut ev = evidence.clone();
    ev.bundle = Some(trailing);
    assert!(verify_decision_evidence(&ev, &expected).is_err());

    // 4. Oversized string rejected by custom decode limits
    let encoded = evidence.encode().unwrap();
    let tight_str_limits = DecisionEvidenceDecodeLimits {
        max_string_bytes: 4,
        ..DecisionEvidenceDecodeLimits::default()
    };
    assert!(DecisionEvidence::decode_with_limits(&encoded, &tight_str_limits).is_err());

    // 5. Replay work budget exhaustion aborts verification cleanly
    let tight_work_limits = DecisionEvidenceDecodeLimits {
        max_replay_work: 1,
        ..DecisionEvidenceDecodeLimits::default()
    };
    assert!(
        verify_decision_evidence_with_limits(&evidence, &expected, &tight_work_limits).is_err(),
        "replay exceeding max_replay_work must abort with an error"
    );
}
