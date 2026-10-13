//! Combined Atlas profile qualification through durable world sessions.
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use brix_kb::world::{
    ChunkDecision, ChunkInput, ChunkPolicy, CrashPoint, OpenMode, TupleRecord, WorldBatch,
    WorldBatchOp, WorldError, WorldKey, WorldSession,
};

const SOURCE: &str = include_str!("../../../examples/world-atlas/atlas.brix");
const SEED: &str = include_str!("../../../examples/world-atlas/seed.json");
const RETRACT: &str = include_str!("../../../examples/world-atlas/retract.json");

struct TestDir(PathBuf);
impl TestDir {
    fn new(name: &str) -> Self {
        Self(std::env::temp_dir().join(format!("brix_alpha6_{name}_{}", std::process::id())))
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn create(dir: &TestDir) -> WorldSession {
    WorldSession::from_program_with_sources(
        &dir.0,
        "atlas",
        &BTreeMap::from([("atlas".into(), SOURCE.into())]),
    )
    .unwrap()
}
fn batch(json: &str) -> WorldBatch {
    WorldBatch::from_json(&serde_json::from_str(json).unwrap()).unwrap()
}

#[derive(Debug)]
struct FlushEveryOp;
impl ChunkPolicy for FlushEveryOp {
    fn chunk_plan(&self, _: &ChunkInput) -> ChunkDecision {
        ChunkDecision {
            ops: 1,
            flush_pack: true,
        }
    }
}

#[test]
fn typed_cascade_absence_and_aggregate_crashes_recover_atomically() {
    let clean_dir = TestDir::new("crash_clean");
    let mut clean = create(&clean_dir);
    clean.apply_batch(batch(SEED)).unwrap();
    let old = clean.all_settlements().unwrap();
    clean.apply_batch(batch(RETRACT)).unwrap();
    let new = clean.all_settlements().unwrap();
    assert_ne!(old, new);

    for (i, point) in [
        CrashPoint::AfterChunkPersistedBeforeNextChunk,
        CrashPoint::AfterIntermediatePackFlush,
        CrashPoint::AfterBaseObjectsBeforeNetworkObjects,
        CrashPoint::AfterNetworkObjectsBeforeFlush,
        CrashPoint::BeforeObjectsFsync,
        CrashPoint::AfterObjectsFsyncBeforeRevisionFsync,
        CrashPoint::AfterDecisionDeltaFsyncBeforeRevisionFsync,
        CrashPoint::AfterRevisionFsyncBeforeHeadRename,
        CrashPoint::AfterHeadRenameBeforeDirectoryFsync,
    ]
    .into_iter()
    .enumerate()
    {
        let dir = TestDir::new(&format!("crash_{i}"));
        let mut session = create(&dir);
        session.apply_batch(batch(SEED)).unwrap();
        drop(session);
        let mut session = WorldSession::open(&dir.0).unwrap();
        session.chunk_policy = Arc::new(FlushEveryOp);
        session.set_crash_point(Some(point));
        assert!(matches!(session.apply_batch(batch(RETRACT)),
                         Err(WorldError::InjectedCrash(actual)) if actual == point));
        drop(session);
        let mut recovered = WorldSession::open(&dir.0).unwrap();
        assert_eq!(recovered.open_mode, OpenMode::Restored);
        recovered.verify_state().unwrap();
        match recovered.current_revision() {
            1 => {
                assert_eq!(recovered.all_settlements().unwrap(), old, "{point:?}");
                recovered.apply_batch(batch(RETRACT)).unwrap();
            }
            2 => assert_eq!(recovered.all_settlements().unwrap(), new, "{point:?}"),
            other => panic!("unexpected recovered revision {other} at {point:?}"),
        }
        let retry = recovered.apply_batch(batch(RETRACT)).unwrap();
        assert!(retry.is_idempotent_replay, "{point:?}");
        assert_eq!(recovered.all_settlements().unwrap(), new, "{point:?}");
        recovered.verify_state().unwrap();
    }
}

fn scale(facts: usize) {
    let dir = TestDir::new(&format!("scale_{facts}"));
    let mut session = create(&dir);
    let mut ops = Vec::with_capacity(facts);
    for i in 0..facts / 2 {
        let id = format!("o{i}");
        let mut parent = TupleRecord::new();
        parent.set_str("id", &id);
        ops.push(WorldBatchOp::Upsert {
            relation: "atlas::orders".into(),
            key: WorldKey::from_str(&id),
            tuple: parent.to_tuple(),
        });
        let mut line = TupleRecord::new();
        line.set_str("id", &id);
        line.set_str("order_id", &id);
        line.set_str("amount", "12");
        ops.push(WorldBatchOp::Upsert {
            relation: "atlas::lines".into(),
            key: WorldKey::from_str(&id),
            tuple: line.to_tuple(),
        });
    }
    let started = Instant::now();
    session
        .apply_batch(WorldBatch::new(0, "seed", ops))
        .unwrap();
    let ingestion = started.elapsed();
    drop(session);
    let started = Instant::now();
    session = WorldSession::open(&dir.0).unwrap();
    let reopen = started.elapsed();
    assert_eq!(session.open_mode, OpenMode::Restored);
    let before = session
        .network
        .as_ref()
        .unwrap()
        .get_settlement("atlas::dispatch", "o1")
        .unwrap()
        .unwrap();
    let started = Instant::now();
    let receipt = session
        .apply_batch(WorldBatch::new(
            1,
            "remove-one-line",
            vec![WorldBatchOp::Remove {
                relation: "atlas::lines".into(),
                key: WorldKey::from_str("o0"),
            }],
        ))
        .unwrap();
    let edit = started.elapsed();
    assert!(session
        .network
        .as_ref()
        .unwrap()
        .get_settlement("atlas::dispatch", "o0")
        .unwrap()
        .is_none());
    assert_eq!(
        session
            .network
            .as_ref()
            .unwrap()
            .get_settlement("atlas::dispatch", "o1")
            .unwrap()
            .unwrap(),
        before
    );
    assert_eq!(
        session
            .network
            .as_ref()
            .unwrap()
            .get_settlement("atlas::availability", "o0")
            .unwrap()
            .unwrap()
            .candidate_name,
        "missing"
    );
    drop(session);
    let reopened = WorldSession::open(&dir.0).unwrap();
    assert_eq!(reopened.open_mode, OpenMode::Restored);
    reopened.verify_state().unwrap();
    println!("ATLAS facts={facts} ingest_ms={} cold_restore_ms={} durable_retraction_ms={} objects_written={}",
        ingestion.as_millis(), reopen.as_millis(), edit.as_millis(), receipt.objects_written);
}

#[test]
#[ignore = "release-build Atlas qualification at 10k resident facts"]
fn atlas_scale_10k() {
    scale(10_000);
}

#[test]
#[ignore = "release-build Atlas qualification at 100k resident facts"]
fn atlas_scale_100k() {
    scale(100_000);
}
