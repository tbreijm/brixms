//! Dogfood test (Track A slice B): `packages/brix.type` is registered as a
//! real, locatable package via the standard `brix.toml` + `src/world.brix`
//! layout — not just a bare source file. Confirms `brix_cli::package::locate`
//! finds it through the real package-directory path, its `brix.toml`
//! manifest matches the in-source `package NAME @ VERSION` declaration, and
//! the full `brix check` pipeline (parse -> lower -> phase-assign) passes on
//! it end to end, exactly as `brix check packages/brix.type` would from the
//! CLI.

use std::path::PathBuf;

use brix_ast::parse_file;

/// `packages/brix.type`, resolved from this crate's manifest dir so the test
/// works regardless of the workspace-relative CWD a test runner uses.
fn pkg_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("packages")
        .join("brix.type")
}

#[test]
fn brix_type_locates_via_standard_package_layout() {
    let root = pkg_root();
    let located = brix_cli::package::locate(root.to_str().expect("pkg root must be UTF-8"))
        .expect("packages/brix.type must locate via brix.toml + src/world.brix");

    assert_eq!(located.manifest.name.as_str(), "brix.type");
    assert_eq!(located.manifest.version.to_string(), "0.1.0");
    assert!(
        located.explicit_manifest,
        "brix.toml must be picked up, not synthesized from the source decl"
    );
    assert!(
        located.source_path.as_str().ends_with("src/world.brix"),
        "entry source must resolve to src/world.brix, got {}",
        located.source_path
    );
    assert!(
        located.deps.is_empty() && located.lockfile.is_none(),
        "brix.type declares zero dependencies; no lockfile should be needed"
    );

    // The registration proof: the manifest must agree with the in-source
    // `package brix.type @ 0.1.0` declaration (Manifest::check_matches_source_decl).
    let source = std::fs::read_to_string(&located.source_path).expect("world.brix must read");
    let (file, parse_diags) = parse_file(&source);
    assert!(!parse_diags.has_errors(), "world.brix must parse cleanly");
    let decl = file
        .package
        .as_ref()
        .expect("world.brix must declare a package");
    let source_name = decl
        .name
        .segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<Vec<_>>()
        .join(".");
    located
        .manifest
        .check_matches_source_decl(&source_name, &decl.version.text)
        .expect("brix.toml must match the in-source package declaration");
}

#[test]
fn brix_type_checks_cleanly_through_the_real_locate_and_check_path() {
    let root = pkg_root();
    let outcome = brix_cli::build::check(root.to_str().expect("pkg root must be UTF-8"));
    match outcome {
        Ok(checked) => assert!(checked.source_path.as_str().ends_with("src/world.brix")),
        Err(e) => panic!("brix check packages/brix.type failed end to end: {e}"),
    }
}
