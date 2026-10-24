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

#[test]
fn review_collect_equal_ordinals_use_child_keys_not_unrelated_fields() {
    let program = r#"
rel input tags: { id: Str, entity: Str, tag: Str, a_label: Str, ordinal: Int } key id
rel derived entity_tags = select {
    entity: t.entity,
    tags: collect(t.tag, t.ordinal, 5)
} from t in tags group by t.entity
decide entity_decide for e in entity_tags per entity {
    propose p priority 1 when true = e.tags
}
"#;
    let dir = TestDir::new("key_tie_breaker");
    let sources = BTreeMap::from([("root".into(), program.into())]);
    let mut session =
        WorldSession::from_program_with_sources(&dir.0, "root", &sources).expect("create session");
    let row = |id: &str, tag: &str, label: &str| {
        let mut record = TupleRecord::new();
        record.set_str("id", id);
        record.set_str("entity", "e1");
        record.set_str("tag", tag);
        record.set_str("a_label", label);
        record.set_str("ordinal", "0");
        WorldBatchOp::Upsert {
            relation: "root::tags".into(),
            key: WorldKey::from_str(id),
            tuple: record.to_tuple(),
        }
    };
    session
        .apply_batch(WorldBatch::new(
            0,
            "seed equal ordinals",
            vec![row("1", "first", "a"), row("2", "second", "b")],
        ))
        .expect("seed");
    let expected = Value::List(vec![
        Value::Str("first".into()),
        Value::Str("second".into()),
    ]);
    assert_eq!(
        session.all_settlements().unwrap()["root::entity_decide"]["e1"].value,
        expected
    );
    session
        .apply_batch(WorldBatch::new(
            1,
            "change unrelated label",
            vec![row("1", "first", "z")],
        ))
        .expect("update");
    assert_eq!(
        session.all_settlements().unwrap()["root::entity_decide"]["e1"].value,
        expected,
        "unchanged ordinal and child key must preserve collection order"
    );
}

#[test]
fn test_grouped_collect_parent_anchored_empty_list_settlement() {
    let program = r#"
config Plan = AskBack(List<Str>) | Done

fn no_slots(): List<Str> = []

rel input parents: { id: Str } key id
rel input slots: { id: Str, parent: Str, slot: Str, prio: Int } key id

rel derived nonempty_slots = select {
    parent: s.parent,
    items: collect(s.slot, s.prio, 5)
} from s in slots group by s.parent

rel derived empty_slots = select {
    parent: p.id,
    items: no_slots()
} from p in parents where not exists s in slots where s.parent == p.id

rel derived parent_slots = union nonempty_slots, empty_slots key parent

decide parent_plan for p in parent_slots per parent {
    propose winner priority 1 when true = Plan::AskBack(p.items)
}
"#;
    let dir = TestDir::new("parent_anchored");
    let sources = BTreeMap::from([("root".into(), program.into())]);
    let mut session =
        WorldSession::from_program_with_sources(&dir.0, "root", &sources).expect("create session");

    let parent_upsert = |id: &str| {
        let mut r = TupleRecord::new();
        r.set_str("id", id);
        WorldBatchOp::Upsert {
            relation: "root::parents".into(),
            key: WorldKey::from_str(id),
            tuple: r.to_tuple(),
        }
    };
    let parent_retract = |id: &str| WorldBatchOp::Remove {
        relation: "root::parents".into(),
        key: WorldKey::from_str(id),
    };

    // 1. Insert parent with no children: settlement produces AskBack([])
    session
        .apply_batch(WorldBatch::new(
            0,
            "create parent",
            vec![parent_upsert("p1")],
        ))
        .expect("batch 0");

    let settlements = session.all_settlements().expect("settlements");
    let plan = &settlements["root::parent_plan"]["p1"].value;
    assert_eq!(
        plan,
        &Value::Variant {
            nominal: "root::Plan".into(),
            variant: "AskBack".into(),
            args: vec![Value::List(vec![])],
        },
        "parent without children produces settlement containing empty list []"
    );

    // 2. Insert children: settlement updates to AskBack(["volume", "customer-grain"])
    session
        .apply_batch(WorldBatch::new(
            1,
            "add children",
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
        "parent with children produces ordered list"
    );

    // 3. Retract one child: settlement updates to AskBack(["customer-grain"])
    session
        .apply_batch(WorldBatch::new(
            2,
            "retract volume child",
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
        "retracting one child updates list"
    );

    // 4. Retract the final child: parent settlement is NOT deleted, but transitions to AskBack([])
    session
        .apply_batch(WorldBatch::new(
            3,
            "retract final child",
            vec![slot_retract("s1")],
        ))
        .expect("batch 3");

    let settlements = session.all_settlements().expect("settlements");
    let plan = &settlements["root::parent_plan"]["p1"].value;
    assert_eq!(
        plan,
        &Value::Variant {
            nominal: "root::Plan".into(),
            variant: "AskBack".into(),
            args: vec![Value::List(vec![])],
        },
        "retracting final child retains parent settlement containing empty list []"
    );

    // 5. Verify state, oracle, and audit bundle
    session.verify_state().expect("state");
    session.verify_oracle().expect("oracle");
    let closure = ProgramClosureV1 {
        root_module: "root".into(),
        sources: vec![("root".into(), program.into())],
        program_manifest_digest: session.manifest().program_digest,
    };
    let bundle = build_genesis_bundle_from_session(&session, closure, ExecProfileV1::default())
        .expect("bundle");
    let limits = WorldAuditDecodeLimits::default();
    let bytes = bundle.encode(&limits).expect("encode");
    let decoded = decode_world_audit_bundle_v1(&bytes, &limits).expect("decode");
    verify_world_audit_bundle(
        &decoded,
        &VerifyOptions {
            expect_head: session.current_revision_digest().expect("head"),
            expect_program: session.manifest().program_digest,
            trust_checkpoint: None,
            limits,
            max_work: None,
        },
    )
    .expect("verify bundle");

    // 6. Retract the parent itself: settlement is now deleted
    session
        .apply_batch(WorldBatch::new(
            4,
            "retract parent",
            vec![parent_retract("p1")],
        ))
        .expect("batch 4");

    let settlements = session.all_settlements().expect("settlements");
    assert!(
        settlements
            .get("root::parent_plan")
            .is_none_or(|m| !m.contains_key("p1")),
        "retracting parent removes settlement"
    );
}
