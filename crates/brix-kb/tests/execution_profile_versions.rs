use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use brix_kb::world::WorldSession;
use brix_kb::world::{audit, session};

struct TempWorld(PathBuf);

impl TempWorld {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "brix-alpha8-profile-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        Self(path)
    }
}

impl Drop for TempWorld {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn copy_tree(source: &Path, target: &Path) {
    fs::create_dir_all(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let src = entry.path();
        let dst = target.join(entry.file_name());
        if src.is_dir() {
            copy_tree(&src, &dst);
        } else {
            fs::copy(src, dst).unwrap();
        }
    }
}

#[test]
fn alpha9_profiles_are_current_and_alpha8_remains_supported() {
    let current_audit = audit::ExecProfileV1::default();
    assert_eq!(current_audit.crate_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(current_audit.crate_version, "0.1.0-alpha.9");
    assert!(current_audit.is_supported());
    assert!(current_audit.is_supported_for_decision_evidence());

    let mut alpha8_audit = current_audit.clone();
    alpha8_audit.crate_version = "0.1.0-alpha.8".into();
    assert!(alpha8_audit.is_supported());
    assert!(alpha8_audit.is_supported_for_decision_evidence());
    assert_ne!(alpha8_audit.digest(), current_audit.digest());

    let mut unsupported_evidence_audit = current_audit.clone();
    unsupported_evidence_audit.crate_version = "0.1.0-alpha.7".into();
    assert!(unsupported_evidence_audit.is_supported());
    assert!(!unsupported_evidence_audit.is_supported_for_decision_evidence());

    let mut unsupported_audit = current_audit;
    unsupported_audit.crate_version = "0.1.0-alpha.4".into();
    assert!(!unsupported_audit.is_supported());
    assert!(!unsupported_audit.is_supported_for_decision_evidence());

    let current_runtime = session::ExecProfileV1::current_for_network();
    assert_eq!(current_runtime.crate_version, env!("CARGO_PKG_VERSION"));
    assert!(current_runtime.is_supported());
    assert!(session::ExecProfileV1::is_supported_version(
        "0.1.0-alpha.8"
    ));
    assert!(session::ExecProfileV1::is_supported_version(
        "0.1.0-alpha.9"
    ));
    assert!(!session::ExecProfileV1::is_supported_version(
        "0.1.0-alpha.4"
    ));
}

#[test]
fn reopening_alpha8_program_closure_preserves_its_profile_and_genesis() {
    let source_fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/alpha8_golden_genesis");
    let temp = TempWorld::new();
    copy_tree(&source_fixture, &temp.0);

    let profile_before = fs::read(temp.0.join("program.json")).unwrap();
    let head_before = fs::read(temp.0.join("HEAD")).unwrap();
    let genesis_before = fs::read(temp.0.join("revisions/0.json")).unwrap();
    let source = include_str!("fixtures/alpha8_golden_genesis/root.brix").to_string();
    let sources = BTreeMap::from([("root".to_string(), source)]);

    let reopened = WorldSession::from_program_with_sources(&temp.0, "root", &sources).unwrap();
    assert_eq!(
        reopened.exec_profile.as_ref().unwrap().crate_version,
        "0.1.0-alpha.8"
    );
    assert_eq!(
        fs::read(temp.0.join("program.json")).unwrap(),
        profile_before
    );
    assert_eq!(fs::read(temp.0.join("HEAD")).unwrap(), head_before);
    assert_eq!(
        fs::read(temp.0.join("revisions/0.json")).unwrap(),
        genesis_before
    );
    let digest_before = reopened.current_revision_digest;
    drop(reopened);

    let cold = WorldSession::open(&temp.0).unwrap();
    assert_eq!(cold.current_revision_digest, digest_before);
    assert_eq!(
        cold.exec_profile.as_ref().unwrap().crate_version,
        "0.1.0-alpha.8"
    );
    cold.verify_state().unwrap();
}
