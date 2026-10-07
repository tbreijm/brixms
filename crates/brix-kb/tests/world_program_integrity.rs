use std::collections::BTreeMap;
use std::fs;

use brix_kb::world::{WorldBatch, WorldSession};

const SOURCE: &str = "rel input orders: { id: Str, item: Str } key id";

fn sources() -> BTreeMap<String, String> {
    BTreeMap::from([("root".to_owned(), SOURCE.to_owned())])
}

fn test_dir(name: &str) -> std::path::PathBuf {
    let path =
        std::env::temp_dir().join(format!("brix_world_program_{name}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn executable_world_binds_closure_in_manifest_and_each_revision() {
    let dir = test_dir("revision_binding");
    let source_map = sources();
    let session = WorldSession::from_program_with_sources(&dir, "root", &source_map).unwrap();
    let digest = session.manifest().program_digest;
    assert!(session.manifest().program_required);
    drop(session);

    let opened = WorldSession::open(&dir).unwrap();
    assert_eq!(opened.manifest().program_digest, digest);
    assert!(opened.network.is_some());
    let revision: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join("revisions/0.json")).unwrap()).unwrap();
    assert_eq!(revision["program_digest"], digest.to_hex());
    drop(opened);

    let mut world = WorldSession::open(&dir).unwrap();
    world
        .apply_batch(WorldBatch::new(0, "one", Vec::new()))
        .unwrap();
    drop(world);
    let revision: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join("revisions/1.json")).unwrap()).unwrap();
    assert_eq!(revision["program_digest"], digest.to_hex());
    let reopened = WorldSession::open(&dir).unwrap();
    assert_eq!(
        reopened.pin_revision(0).unwrap().revision.program_digest,
        Some(digest)
    );
    assert_eq!(
        reopened.pin_revision(1).unwrap().revision.program_digest,
        Some(digest)
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn executable_program_cannot_be_rebound_after_a_commit() {
    let dir = test_dir("no_rebind");
    let source_map = sources();
    let mut session = WorldSession::from_program_with_sources(&dir, "root", &source_map).unwrap();
    session
        .apply_batch(WorldBatch::new(0, "one", Vec::new()))
        .unwrap();
    let changed = BTreeMap::from([(
        "root".to_owned(),
        "rel input orders: { id: Str, item: Int } key id".to_owned(),
    )]);
    assert!(session.save_program_closure("root", &changed).is_err());
    drop(session);
    assert!(WorldSession::open(&dir).is_ok());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn executable_world_fails_closed_for_missing_or_tampered_closure() {
    let dir = test_dir("tampered");
    let source_map = sources();
    let session = WorldSession::from_program_with_sources(&dir, "root", &source_map).unwrap();
    drop(session);

    let closure_path = dir.join("program.json");
    let mut closure: serde_json::Value =
        serde_json::from_slice(&fs::read(&closure_path).unwrap()).unwrap();
    closure["sources"]["root"] = "rel input orders: { id: Str, item: Int } key id".into();
    fs::write(&closure_path, serde_json::to_vec(&closure).unwrap()).unwrap();
    assert!(
        WorldSession::open(&dir).is_err(),
        "tampered executable closure must be rejected"
    );

    fs::remove_file(&closure_path).unwrap();
    assert!(
        WorldSession::open(&dir).is_err(),
        "missing executable closure must be rejected"
    );
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn storage_only_world_remains_valid_without_source_closure() {
    let dir = test_dir("storage_only");
    let manifest = brix_kb::world::WorldManifest::new(
        "storage-only",
        "2026-10-04T00:00:00Z",
        brix_canon::Digest::of(brix_canon::Domain::Value, b"storage-only"),
        vec![],
    );
    WorldSession::create(&dir, manifest).unwrap();
    assert!(
        !WorldSession::open(&dir)
            .unwrap()
            .manifest()
            .program_required
    );
    fs::remove_dir_all(dir).unwrap();
}
