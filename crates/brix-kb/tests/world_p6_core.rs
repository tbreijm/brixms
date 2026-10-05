//! Adversarial qualification for P6's persisted settlement evidence.
use std::{collections::BTreeMap, fs, path::PathBuf};

use brix_canon::{Digest, Domain};
use brix_kb::world::{
    decision_codec::{
        decision_tuple, decode_decision_delta, decode_settled_decision, encode_decision_delta,
    },
    CrashPoint, TupleRecord, Value, WorldBatch, WorldBatchOp, WorldError, WorldKey, WorldNetwork,
    WorldSession,
};
use brix_lower::module_graph::{ModuleGraph, ModuleLoaderLimits};

const SOURCE: &str = r#"
rel input rows: { id: Str, owner: Str, amount: Int } key id
rel derived eligible = select { owner: r.owner, amount: r.amount } from r in rows where r.amount > 0
decide alpha for r in eligible per owner { propose allow priority 1 when true = r.amount }
decide omega for r in eligible per owner { propose label priority 2 when true = r.owner }
"#;

struct TestDir(PathBuf);
impl TestDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("brix_p6_core_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        Self(path)
    }
    fn session(&self) -> WorldSession {
        let manifest = brix_kb::world::WorldManifest::new(
            "p6-core",
            "2026-10-05",
            Digest::of(Domain::Value, b"initial"),
            vec![brix_kb::world::RelationDecl::new(
                "root::rows",
                vec!["id".into()],
                vec!["owner".into(), "amount".into()],
                vec!["owner".into()],
            )],
        );
        let mut world = WorldSession::create(&self.0, manifest).unwrap();
        world.set_network(network(SOURCE).unwrap()).unwrap();
        world
            .save_program_closure("root", &BTreeMap::from([("root".into(), SOURCE.into())]))
            .unwrap();
        world
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn upsert(id: &str, amount: i64) -> WorldBatchOp {
    let mut record = TupleRecord::new();
    record.set_str("id", id);
    record.set_str("owner", id);
    record.set_str("amount", &amount.to_string());
    WorldBatchOp::Upsert {
        relation: "root::rows".into(),
        key: WorldKey::from_str(id),
        tuple: record.to_tuple(),
    }
}
fn batch(seq: u64, ops: Vec<WorldBatchOp>) -> WorldBatch {
    WorldBatch::new(seq, format!("batch-{seq}"), ops)
}
fn network(source: &str) -> Result<WorldNetwork, WorldError> {
    let graph = ModuleGraph::load(
        "root",
        &|_: &str| Some(source.to_owned()),
        ModuleLoaderLimits::default(),
    )
    .unwrap();
    WorldNetwork::from_program(&graph.link().unwrap())
}

#[test]
fn per_is_required_and_resolves_a_typed_field() {
    assert!(network(&SOURCE.replace(" per owner", "")).is_err());
    assert!(network(&SOURCE.replace(" per owner", " per absent")).is_err());
    assert!(network(SOURCE).is_ok());
    assert!(network(&SOURCE.replace("per owner", "per amount")).is_ok());
    // A contextual `per` stays legal as a relation, field, binder and candidate.
    assert!(network(
        r#"
rel input per: { per: Str } key per
decide selected for per in per per per { propose per priority 1 when true = per.per }
"#
    )
    .is_ok());
}

#[test]
fn historical_decisions_and_exact_deltas_survive_updates_retractions_and_reopen() {
    let dir = TestDir::new("history");
    let mut world = dir.session();
    world
        .apply_batch(batch(0, vec![upsert("a", 10), upsert("b", 20)]))
        .unwrap();
    let original = world.all_settlements();
    assert_eq!(original.len(), 2);
    let delta = world.decision_delta(1).unwrap();
    assert_eq!(
        delta.len(),
        4,
        "every entity in every decide must be persisted"
    );
    for (decide, entities) in &original {
        for (entity, settlement) in entities {
            assert_eq!(
                delta[&(decide.clone(), entity.clone())].as_ref(),
                Some(settlement)
            );
        }
    }
    world.apply_batch(batch(1, vec![upsert("a", 11)])).unwrap();
    let changed = world.decision_delta(2).unwrap();
    assert_eq!(
        changed.len(),
        1,
        "unchanged omega settlement must not be traced"
    );
    assert_eq!(
        changed[&("root::alpha".into(), "a".into())]
            .as_ref()
            .unwrap()
            .value,
        Value::Int(11)
    );
    world.apply_batch(batch(2, vec![upsert("a", 0)])).unwrap();
    let removed = world.decision_delta(3).unwrap();
    assert_eq!(
        removed,
        BTreeMap::from([
            (("root::alpha".into(), "a".into()), None),
            (("root::omega".into(), "a".into()), None),
        ])
    );
    world.apply_batch(batch(3, vec![])).unwrap();
    assert!(world.decision_delta(4).unwrap().is_empty());
    drop(world);
    let world = WorldSession::open(&dir.0).unwrap();
    for (decide, entities) in original {
        for (entity, settlement) in entities {
            assert_eq!(
                world
                    .historical_settlement(1, Some(&decide), &entity)
                    .unwrap(),
                Some((decide.clone(), settlement))
            );
        }
    }
    assert!(world
        .historical_settlement(3, Some("alpha"), "a")
        .unwrap()
        .is_none());
    assert_eq!(
        world
            .historical_settlement(2, Some("alpha"), "a")
            .unwrap()
            .unwrap()
            .1
            .value,
        Value::Int(11)
    );
}

#[test]
fn revision_binds_profile_and_delta_and_rejects_tampered_bodies() {
    let dir = TestDir::new("bindings");
    let mut world = dir.session();
    world.apply_batch(batch(0, vec![upsert("a", 10)])).unwrap();
    let revision = world.pin_revision(1).unwrap().revision;
    assert_eq!(revision.schema, "brix.world.revision@2");
    assert_eq!(
        revision.exec_profile_digest,
        Some(world.exec_profile.as_ref().unwrap().digest())
    );
    let delta_path = world.paths.decision_delta_file(1);
    let body = fs::read(&delta_path).unwrap();
    assert_eq!(
        revision.decision_delta_digest,
        Some(Digest::of(Domain::Value, &body))
    );
    for field in ["decision_delta_digest", "exec_profile_digest"] {
        let mut json = revision.to_json();
        json[field] = Digest::of(Domain::Value, b"forged").to_hex().into();
        assert!(brix_kb::world::WorldRevision::from_json(&json).is_err());
    }
    fs::write(&delta_path, b"forged").unwrap();
    assert!(world.decision_delta(1).is_err());
    assert!(WorldSession::open(&dir.0).is_err());
    fs::write(&delta_path, body).unwrap();
    let program_path = dir.0.join("program.json");
    let mut program: serde_json::Value =
        serde_json::from_slice(&fs::read(&program_path).unwrap()).unwrap();
    program["exec_profile"]["crate_version"] = "forged".into();
    fs::write(program_path, serde_json::to_vec(&program).unwrap()).unwrap();
    assert!(WorldSession::open(&dir.0).is_err());
}

#[test]
fn historical_reads_reject_valid_revision_file_substitution() {
    let dir = TestDir::new("substitution");
    let mut world = dir.session();
    world.apply_batch(batch(0, vec![upsert("a", 10)])).unwrap();
    world.apply_batch(batch(1, vec![upsert("a", 20)])).unwrap();
    fs::copy(world.paths.revision_file(2), world.paths.revision_file(1)).unwrap();
    assert!(world.pin_revision(1).is_err());
    assert!(world.historical_settlement(1, Some("alpha"), "a").is_err());
    assert!(world.decision_delta(1).is_err());
}

#[test]
fn decision_codec_round_trips_every_scalar_and_refuses_malformed_evidence() {
    let dir = TestDir::new("codec");
    let mut world = dir.session();
    world.apply_batch(batch(0, vec![upsert("a", 10)])).unwrap();
    let mut decision = world.all_settlements()["root::alpha"]["a"].clone();
    for value in [
        Value::Int(i64::MIN),
        Value::Str("nul\0é".into()),
        Value::Bool(true),
        Value::F64("0.125".parse().unwrap()),
        Value::Decimal(brix_canon::decimal_parse("123.450").unwrap()),
    ] {
        decision.value = value;
        let bytes = decision_tuple(&decision);
        assert_eq!(
            decode_settled_decision("a", bytes.as_bytes()).unwrap(),
            decision
        );
        for end in 0..bytes.as_bytes().len() {
            assert!(decode_settled_decision("a", &bytes.as_bytes()[..end]).is_err());
        }
        let mut trailing = bytes.as_bytes().to_vec();
        trailing.push(0);
        assert!(decode_settled_decision("a", &trailing).is_err());
    }
    let added = BTreeMap::from([(
        "root::alpha".into(),
        BTreeMap::from([("a".into(), decision)]),
    )]);
    let bytes = encode_decision_delta(&added, &BTreeMap::new());
    assert_eq!(decode_decision_delta(&bytes, 1).unwrap().len(), 1);
    assert!(decode_decision_delta(&bytes, 0).is_err());
    for end in 0..bytes.len() {
        assert!(decode_decision_delta(&bytes[..end], 1).is_err());
    }
}

#[test]
fn every_crash_boundary_preserves_atomic_relation_index_and_decision_evidence() {
    for (index, point) in [
        CrashPoint::BeforeObjectsFsync,
        CrashPoint::AfterObjectsFsyncBeforeRevisionFsync,
        CrashPoint::AfterDecisionDeltaFsyncBeforeRevisionFsync,
        CrashPoint::AfterRevisionFsyncBeforeHeadRename,
        CrashPoint::AfterHeadRenameBeforeDirectoryFsync,
    ]
    .into_iter()
    .enumerate()
    {
        let dir = TestDir::new(&format!("crash_{index}"));
        let mut world = dir.session();
        world.apply_batch(batch(0, vec![upsert("a", 10)])).unwrap();
        world.set_crash_point(Some(point));
        let next = batch(1, vec![upsert("a", 20), upsert("b", 30)]);
        assert!(matches!(
            world.apply_batch(next.clone()),
            Err(WorldError::InjectedCrash(_))
        ));
        drop(world);
        let mut world = WorldSession::open(&dir.0).unwrap();
        let published = point == CrashPoint::AfterHeadRenameBeforeDirectoryFsync;
        assert_eq!(world.current_revision(), if published { 2 } else { 1 });
        assert_eq!(
            world
                .historical_settlement(1, Some("alpha"), "a")
                .unwrap()
                .unwrap()
                .1
                .value,
            Value::Int(10)
        );
        if !published {
            assert!(world.pin_revision(2).is_err());
        }
        let receipt = world.apply_batch(next).unwrap();
        assert_eq!(receipt.is_idempotent_replay, published);
        drop(world);
        let world = WorldSession::open(&dir.0).unwrap();
        assert_eq!(world.current_revision(), 2);
        assert_eq!(
            world
                .historical_settlement(2, Some("alpha"), "a")
                .unwrap()
                .unwrap()
                .1
                .value,
            Value::Int(20)
        );
        assert_eq!(world.decision_delta(2).unwrap().len(), 3);
        assert!(world
            .get("root::rows", &WorldKey::from_str("b"))
            .unwrap()
            .is_some());
        assert_eq!(
            world
                .query_secondary_index("root::rows", "owner", &WorldKey::from_str("b"))
                .unwrap(),
            vec![WorldKey::from_str("b")]
        );
    }
}

#[test]
fn v1_history_and_program_without_exec_profile_remain_readable() {
    let dir = TestDir::new("legacy");
    let mut world = WorldSession::from_program_with_sources(
        &dir.0,
        "root",
        &BTreeMap::from([(
            "root".into(),
            "rel input rows: { id: Str, owner: Str, amount: Int } key id".into(),
        )]),
    )
    .unwrap();
    world.apply_batch(batch(0, vec![upsert("a", 10)])).unwrap();
    // Reconstruct the pre-P6 on-disk shape, including absent additive keys.
    let mut previous = None;
    for seq in 0..=1 {
        let mut revision = world.pin_revision(seq).unwrap().revision;
        revision.schema = "brix.world.revision@1".into();
        revision.previous_revision_digest = previous;
        revision.decision_delta_digest = None;
        revision = revision.bind_exec_profile(None);
        previous = Some(revision.revision_digest);
        let mut json = revision.to_json();
        json.as_object_mut()
            .unwrap()
            .remove("decision_delta_digest");
        json.as_object_mut().unwrap().remove("exec_profile_digest");
        fs::write(
            world.paths.revision_file(seq),
            serde_json::to_vec(&json).unwrap(),
        )
        .unwrap();
    }
    fs::write(
        world.paths.head(),
        format!("1 {}\n", previous.unwrap().to_hex()),
    )
    .unwrap();
    let path = dir.0.join("program.json");
    let mut program: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    program.as_object_mut().unwrap().remove("exec_profile");
    fs::write(path, serde_json::to_vec(&program).unwrap()).unwrap();
    drop(world);
    let world = WorldSession::open(&dir.0).unwrap();
    assert!(world.exec_profile.is_none());
    assert!(world
        .get("root::rows", &WorldKey::from_str("a"))
        .unwrap()
        .is_some());
    assert!(world
        .decision_delta(1)
        .unwrap_err()
        .to_string()
        .contains("legacy-history-unavailable"));
    assert!(world
        .historical_settlement(1, None, "a")
        .unwrap_err()
        .to_string()
        .contains("legacy-history-unavailable"));
}

#[test]
fn conflicting_support_values_refuse_atomically_in_both_orders() {
    for reverse in [false, true] {
        let dir = TestDir::new(if reverse {
            "conflict_reverse"
        } else {
            "conflict_forward"
        });
        let mut world = dir.session();
        world.apply_batch(batch(0, vec![upsert("a", 10)])).unwrap();
        let head = world.current_revision_digest();
        let mut record = TupleRecord::new();
        record.set_str("id", "other");
        record.set_str("owner", "a");
        record.set_str("amount", "99");
        let conflicting = WorldBatchOp::Upsert {
            relation: "root::rows".into(),
            key: WorldKey::from_str("other"),
            tuple: record.to_tuple(),
        };
        let mut ops = vec![upsert("b", 20), conflicting];
        if reverse {
            ops.reverse();
        }
        assert!(world
            .apply_batch(batch(1, ops))
            .unwrap_err()
            .to_string()
            .contains("conflicting support values"));
        assert_eq!(world.current_revision_digest(), head);
        assert!(!world.paths.revision_file(2).exists());
        assert!(!world.paths.decision_delta_file(2).exists());
        drop(world);
        let world = WorldSession::open(&dir.0).unwrap();
        assert_eq!(world.current_revision_digest(), head);
        assert!(world
            .get("root::rows", &WorldKey::from_str("b"))
            .unwrap()
            .is_none());
        assert_eq!(world.all_settlements()["root::alpha"].len(), 1);
    }
}
