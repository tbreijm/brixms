//! Checkpoint 6 (R10): Incremental grouped collection contract and parent workflow tests.
use brix_kb::world::audit::{
    build_genesis_bundle_from_session, decode_world_audit_bundle_v1, ExecProfileV1,
    ProgramClosureV1, WorldAuditDecodeLimits,
};
use brix_kb::world::decision_codec::Value;
use brix_kb::world::verify::{verify_world_audit_bundle, VerifyOptions};
use brix_kb::world::{TupleRecord, WorldBatch, WorldBatchOp, WorldError, WorldKey, WorldSession};
use std::collections::BTreeMap;
use std::path::PathBuf;

struct TestDir(PathBuf);
impl TestDir {
    fn new(label: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "brix-collect-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const PROGRAM: &str = r#"
config Plan = AskBack(List<Str>) | Done

rel input slots: { id: Str, parent: Str, slot: Str, prio: Int } key id

rel derived parent_slots = select {
    parent: s.parent,
    items: collect(s.slot, s.prio, 2)
} from s in slots group by s.parent

decide parent_plan for p in parent_slots per parent {
    propose winner priority 1 when true = Plan::AskBack(p.items)
}
"#;

fn slot_upsert(id: &str, parent: &str, slot: &str, prio: i64) -> WorldBatchOp {
    let mut row = TupleRecord::new();
    row.set_str("id", id);
    row.set_str("parent", parent);
    row.set_str("slot", slot);
    row.set_str("prio", &prio.to_string());
    WorldBatchOp::Upsert {
        relation: "root::slots".into(),
        key: WorldKey::from_str(id),
        tuple: row.to_tuple(),
    }
}

fn slot_retract(id: &str) -> WorldBatchOp {
    WorldBatchOp::Remove {
        relation: "root::slots".into(),
        key: WorldKey::from_str(id),
    }
}

#[test]
fn test_grouped_collect_incremental_workflow_and_retraction() {
    let dir = TestDir::new("workflow");
    let sources = BTreeMap::from([("root".into(), PROGRAM.into())]);
    let mut session =
        WorldSession::from_program_with_sources(&dir.0, "root", &sources).expect("create session");

    // 1. Insert ordered slots: "customer-grain" (prio 20) and "volume" (prio 10)
    session
        .apply_batch(WorldBatch::new(
            0,
            "add two slots",
            vec![
                slot_upsert("s1", "p1", "customer-grain", 20),
                slot_upsert("s2", "p1", "volume", 10),
            ],
        ))
        .expect("batch 1");

    let settlements = session.all_settlements().expect("settlements");
    let plan = &settlements["root::parent_plan"]["p1"].value;
    assert_eq!(
        plan,
        &Value::Variant {
            nominal: "root::Plan".into(),
            variant: "AskBack".into(),
            args: vec![Value::List(vec![
                Value::Str("volume".into()),
                Value::Str("customer-grain".into()),
            ])],
        },
        "slots must be ordered ascending by prio (10 < 20)"
    );

    // 2. Retract one slot ("volume"): settlement updates to AskBack(["customer-grain"])
    session
        .apply_batch(WorldBatch::new(
            1,
            "retract volume slot",
            vec![slot_retract("s2")],
        ))
        .expect("batch 2");

    let settlements = session.all_settlements().expect("settlements");
    let plan = &settlements["root::parent_plan"]["p1"].value;
    assert_eq!(
        plan,
        &Value::Variant {
            nominal: "root::Plan".into(),
            variant: "AskBack".into(),
            args: vec![Value::List(vec![Value::Str("customer-grain".into())])],
        },
        "retracting one slot updates settlement"
    );

    // 3. Retract the last slot ("customer-grain"): group becomes empty and settlement is retracted
    session
        .apply_batch(WorldBatch::new(
            2,
            "retract last slot",
            vec![slot_retract("s1")],
        ))
        .expect("batch 3");

    let settlements = session.all_settlements().expect("settlements");
    assert!(
        settlements
            .get("root::parent_plan")
            .is_none_or(|m| !m.contains_key("p1")),
        "retracting last child retracts group and removes settlement"
    );

    // 4. Multiplicity: distinct children with identical values
    session
        .apply_batch(WorldBatch::new(
            3,
            "insert duplicate slot values",
            vec![
                slot_upsert("s3", "p1", "volume", 10),
                slot_upsert("s4", "p1", "volume", 10),
            ],
        ))
        .expect("batch 4");

    let settlements = session.all_settlements().expect("settlements");
    let plan = &settlements["root::parent_plan"]["p1"].value;
    assert_eq!(
        plan,
        &Value::Variant {
            nominal: "root::Plan".into(),
            variant: "AskBack".into(),
            args: vec![Value::List(vec![
                Value::Str("volume".into()),
                Value::Str("volume".into()),
            ])],
        },
        "multiplicity preserved for distinct child rows"
    );

    // 5. Cardinality overflow: limit is 2; adding a 3rd slot aborts transaction leaving HEAD unchanged
    let head_before = session.current_revision_digest().expect("head");
    let rev_before = session.current_revision();
    let overflow_err = session
        .apply_batch(WorldBatch::new(
            4,
            "overflow collect limit",
            vec![slot_upsert("s5", "p1", "extra", 5)],
        ))
        .unwrap_err();

    assert!(
        matches!(overflow_err, WorldError::NetworkError(_)),
        "exceeding limit must return NetworkError: {overflow_err:?}"
    );
    assert_eq!(
        session.current_revision_digest().expect("head"),
        head_before,
        "overflow aborts transaction and leaves HEAD digest unchanged"
    );
    assert_eq!(
        session.current_revision(),
        rev_before,
        "overflow aborts transaction and leaves revision number unchanged"
    );

    // 6. Persistence & Cold Reopen Restoration
    drop(session);
    let mut reopened = WorldSession::open(&dir.0).expect("reopen session");
    let settlements = reopened.all_settlements().expect("settlements");
    let plan = &settlements["root::parent_plan"]["p1"].value;
    assert_eq!(
        plan,
        &Value::Variant {
            nominal: "root::Plan".into(),
            variant: "AskBack".into(),
            args: vec![Value::List(vec![
                Value::Str("volume".into()),
                Value::Str("volume".into()),
            ])],
        },
        "settlement restored identically after cold reopen"
    );

    // Reopened session can continue mutating: retract s3
    let rev = reopened.current_revision();
    reopened
        .apply_batch(WorldBatch::new(
            rev,
            "retract s3 on reopened session",
            vec![slot_retract("s3")],
        ))
        .expect("batch after reopen");

    let settlements = reopened.all_settlements().expect("settlements");
    let plan = &settlements["root::parent_plan"]["p1"].value;
    assert_eq!(
        plan,
        &Value::Variant {
            nominal: "root::Plan".into(),
            variant: "AskBack".into(),
            args: vec![Value::List(vec![Value::Str("volume".into())])],
        },
        "mutating restored accumulator updates settlement correctly"
    );

    // 7. Audit bundle verification
    let closure = ProgramClosureV1 {
        root_module: "root".into(),
        sources: vec![("root".into(), PROGRAM.into())],
        program_manifest_digest: reopened.manifest().program_digest,
    };
    let bundle =
        build_genesis_bundle_from_session(&reopened, closure, ExecProfileV1::default()).unwrap();
    let limits = WorldAuditDecodeLimits::default();
    let bytes = bundle.encode(&limits).unwrap();
    let decoded = decode_world_audit_bundle_v1(&bytes, &limits).unwrap();
    verify_world_audit_bundle(
        &decoded,
        &VerifyOptions {
            expect_head: reopened.current_revision_digest().unwrap(),
            expect_program: reopened.manifest().program_digest,
            trust_checkpoint: None,
            limits,
            max_work: None,
        },
    )
    .expect("verify audit bundle");
}

#[test]
fn test_grouped_collect_default_ordering_and_multi_group() {
    let prog = r#"
rel input tags: { id: Str, entity: Str, tag: Str } key id

rel derived entity_tags = select {
    entity: t.entity,
    tags: collect(t.tag, 5)
} from t in tags group by t.entity

decide entity_decide for e in entity_tags per entity {
    propose p priority 1 when true = e.tags
}
"#;
    let dir = TestDir::new("default_order");
    let sources = BTreeMap::from([("root".into(), prog.into())]);
    let mut session =
        WorldSession::from_program_with_sources(&dir.0, "root", &sources).expect("create session");

    let row = |id: &str, entity: &str, tag: &str| {
        let mut r = TupleRecord::new();
        r.set_str("id", id);
        r.set_str("entity", entity);
        r.set_str("tag", tag);
        WorldBatchOp::Upsert {
            relation: "root::tags".into(),
            key: WorldKey::from_str(id),
            tuple: r.to_tuple(),
        }
    };

    session
        .apply_batch(WorldBatch::new(
            0,
            "multi group insert",
            vec![
                row("1", "e1", "zebra"),
                row("2", "e1", "apple"),
                row("3", "e2", "moon"),
                row("4", "e1", "mango"),
                row("5", "e2", "sun"),
            ],
        ))
        .expect("batch");

    let settlements = session.all_settlements().expect("settlements");
    let e1_tags = &settlements["root::entity_decide"]["e1"].value;
    let e2_tags = &settlements["root::entity_decide"]["e2"].value;

    assert_eq!(
        e1_tags,
        &Value::List(vec![
            Value::Str("apple".into()),
            Value::Str("mango".into()),
            Value::Str("zebra".into()),
        ]),
        "omitted order_by orders by item expression itself"
    );

    assert_eq!(
        e2_tags,
        &Value::List(vec![Value::Str("moon".into()), Value::Str("sun".into()),]),
        "omitted order_by orders by item expression itself"
    );
}
