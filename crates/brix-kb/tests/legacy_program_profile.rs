//! Existing executable worlds keep their recorded profile when callers resupply the same
//! program closure under a newer package build.
use brix_kb::world::{TupleRecord, WorldBatch, WorldBatchOp, WorldKey, WorldPaths, WorldSession};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const MODEL: &str = include_str!("fixtures/alpha8_golden_genesis/root.brix");
const PROFILE_FIXTURE: &str = "tests/fixtures/alpha8_golden_genesis";

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "brix-legacy-profile-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn copy_tree(source: &Path, target: &Path) {
    fs::create_dir_all(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = target.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to);
        } else {
            fs::copy(from, to).unwrap();
        }
    }
}

fn source_map(source: &str) -> BTreeMap<String, String> {
    BTreeMap::from([("root".into(), source.into())])
}

fn upsert(relation: &str, key: &str, fields: &[(&str, &str)]) -> WorldBatchOp {
    let mut row = TupleRecord::new();
    for (name, value) in fields {
        row.set_str(*name, value);
    }
    WorldBatchOp::Upsert {
        relation: format!("root::{relation}"),
        key: WorldKey::from_str(key),
        tuple: row.to_tuple(),
    }
}

fn create_populated_alpha8_world(label: &str) -> (TempDir, WorldSession) {
    let dir = TempDir::new(label);
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join(PROFILE_FIXTURE);
    copy_tree(&fixture, &dir.0);
    // Git preserves the frozen files, but not the genesis world's empty directories.
    WorldPaths::new(&dir.0).ensure_dirs().unwrap();
    assert_eq!(
        fs::read_to_string(dir.0.join("root.brix")).unwrap(),
        MODEL,
        "the alpha8 fixture source must be byte-identical to this test's model"
    );
    let mut session = WorldSession::open(&dir.0).expect("open frozen alpha8 genesis");
    assert_eq!(
        session.exec_profile.as_ref().unwrap().crate_version,
        "0.1.0-alpha.8"
    );
    session
        .apply_batch(WorldBatch::new(
            session.current_revision(),
            "populate alpha8 program world",
            vec![
                upsert(
                    "orders",
                    "order-key-1",
                    &[
                        ("id", "order-1"),
                        ("customer", "customer-1"),
                        ("sku", "sku-1"),
                        ("qty", "2"),
                        ("express", "yes"),
                    ],
                ),
                upsert(
                    "inventory",
                    "inventory-key-1",
                    &[("id", "inventory-1"), ("sku", "sku-1"), ("available", "5")],
                ),
                upsert(
                    "shipping",
                    "shipping-key-1",
                    &[
                        ("id", "shipping-1"),
                        ("sku", "sku-1"),
                        ("carrier", "carrier-1"),
                        ("lead_days", "1"),
                    ],
                ),
            ],
        ))
        .expect("append a batch to the alpha8 world");
    session
        .verify_oracle()
        .expect("verify populated alpha8 world");
    (dir, session)
}

fn program_bytes(path: &Path) -> Vec<u8> {
    fs::read(path.join("program.json")).unwrap()
}

fn head_bytes(path: &Path) -> Vec<u8> {
    fs::read(path.join("HEAD")).unwrap()
}

fn profile_binding_snapshot(path: &Path) -> Vec<(&'static str, Vec<u8>)> {
    ["program.json", "HEAD", "revisions/0.json", "world.json"]
        .into_iter()
        .map(|name| (name, fs::read(path.join(name)).unwrap()))
        .collect()
}

#[test]
fn same_program_reopen_preserves_alpha8_profile_binding_and_files() {
    let (dir, session) = create_populated_alpha8_world("same-program");
    let profile_bytes_before = program_bytes(&dir.0);
    let head_before = head_bytes(&dir.0);
    let root_before = session.network_state_root;
    let revision_digest_before = session.current_revision_digest;
    drop(session);

    let reopened = WorldSession::from_program_with_sources(&dir.0, "root", &source_map(MODEL))
        .expect("same source closure should reopen without profile migration");
    assert_eq!(
        reopened.exec_profile.as_ref().unwrap().crate_version,
        "0.1.0-alpha.8"
    );
    assert_eq!(reopened.network_state_root, root_before);
    assert_eq!(reopened.current_revision_digest, revision_digest_before);
    assert_eq!(program_bytes(&dir.0), profile_bytes_before);
    assert_eq!(head_bytes(&dir.0), head_before);
    reopened
        .verify_oracle()
        .expect("verify reopened alpha8 world");
    drop(reopened);

    let cold = WorldSession::open(&dir.0).expect("cold open after same-program reopen");
    assert_eq!(
        cold.exec_profile.as_ref().unwrap().crate_version,
        "0.1.0-alpha.8"
    );
    assert_eq!(cold.network_state_root, root_before);
    assert_eq!(program_bytes(&dir.0), profile_bytes_before);
    assert_eq!(head_bytes(&dir.0), head_before);
    cold.verify_oracle()
        .expect("cold alpha8 world remains valid");
}

#[test]
fn changed_program_is_refused_before_profile_or_head_writes() {
    let (dir, mut session) = create_populated_alpha8_world("changed-program");
    let files_before = profile_binding_snapshot(&dir.0);
    let root_before = session.network_state_root;
    let changed = MODEL.replace("air_express", "air_express_v2");
    let sources = source_map(&changed);

    let error = session
        .save_program_closure("root", &sources)
        .expect_err("a populated world cannot replace its executable program");
    assert!(error
        .to_string()
        .contains("cannot change executable program"));
    assert_eq!(profile_binding_snapshot(&dir.0), files_before);
    assert_eq!(session.network_state_root, root_before);

    assert!(WorldSession::from_program_with_sources(&dir.0, "root", &sources).is_err());
    assert_eq!(profile_binding_snapshot(&dir.0), files_before);
    assert_eq!(session.network_state_root, root_before);
    session
        .verify_oracle()
        .expect("refused change leaves world valid");
}

#[test]
fn changed_program_is_refused_before_empty_world_binding_writes() {
    let dir = TempDir::new("changed-empty-program");
    let mut session = WorldSession::from_program_with_sources(&dir.0, "root", &source_map(MODEL))
        .expect("create current alpha9 empty world");
    let files_before = profile_binding_snapshot(&dir.0);
    let root_before = session.network_state_root;
    let revision_digest_before = session.current_revision_digest;
    let changed = MODEL.replace("air_express", "air_express_v2");

    let error = session
        .save_program_closure("root", &source_map(&changed))
        .expect_err("an existing world cannot replace its genesis-bound program");
    assert!(error.to_string().contains("executable network"));
    assert_eq!(profile_binding_snapshot(&dir.0), files_before);
    assert_eq!(session.network_state_root, root_before);
    assert_eq!(session.current_revision_digest, revision_digest_before);
}
