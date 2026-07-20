//! Issue #44: the `brix publish` workflow + registry gates, exercised through
//! the public CLI. The publish/yank/gate legs invoke no `cargo build`, so they
//! run in the default suite; only the full consume-a-published-package
//! round-trip (which shells out to `cargo build`) is `#[ignore]`d.

use std::collections::BTreeMap;
use std::process::Command;

use brixpkg::{version::parse_version, Manifest, PackageName, Registry};
use camino::Utf8PathBuf;

fn tmp_dir(tag: &str) -> Utf8PathBuf {
    let mut p =
        Utf8PathBuf::from_path_buf(std::env::temp_dir()).expect("system temp dir must be UTF-8");
    p.push(format!(
        "brix-publish-cli-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    p
}

fn brix(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_brix"))
        .args(args)
        .output()
        .expect("brix binary must be spawnable")
}

fn name(s: &str) -> PackageName {
    PackageName::parse(s).unwrap()
}

/// `brix new` scaffolds a package that then passes `brix check`.
#[test]
fn new_scaffolds_a_checkable_package() {
    let root = tmp_dir("new");
    let lib = root.join("lib");
    let out = brix(&["new", lib.as_str(), "--name", "lib"]);
    assert!(
        out.status.success(),
        "new: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(lib.join("brix.toml").exists());
    assert!(lib.join("src").join("world.brix").exists());

    let checked = brix(&["check", lib.as_str()]);
    assert!(
        checked.status.success(),
        "scaffolded package must check:\n{}",
        String::from_utf8_lossy(&checked.stderr)
    );
    std::fs::remove_dir_all(&root).ok();
}

/// `brix new` → `brix publish` records the version with compat metadata, and a
/// second identical publish is idempotent (no error, still one entry).
#[test]
fn publish_records_compat_and_is_idempotent() {
    let root = tmp_dir("pub");
    let lib = root.join("lib");
    let registry_root = root.join("registry");
    brix(&["new", lib.as_str(), "--name", "lib"]);

    let first = brix(&[
        "publish",
        lib.as_str(),
        "--registry",
        registry_root.as_str(),
    ]);
    assert!(
        first.status.success(),
        "publish: {}",
        String::from_utf8_lossy(&first.stderr)
    );

    let registry = Registry::open(&registry_root).unwrap();
    let versions = registry.versions(&name("lib")).unwrap();
    assert_eq!(versions.len(), 1, "one published version");
    assert_eq!(versions[0].version, parse_version("0.1.0").unwrap());
    let compat = versions[0]
        .compat
        .as_ref()
        .expect("publish must record compatibility metadata");
    assert!(
        !compat.brixc_version.is_empty() && !compat.rustc_version.is_empty(),
        "compat records brixc + rustc versions: {compat:?}"
    );

    // Idempotent republish of identical content.
    let again = brix(&[
        "publish",
        lib.as_str(),
        "--registry",
        registry_root.as_str(),
    ]);
    assert!(
        again.status.success(),
        "idempotent republish must succeed: {}",
        String::from_utf8_lossy(&again.stderr)
    );
    assert_eq!(
        registry.versions(&name("lib")).unwrap().len(),
        1,
        "republish adds no duplicate entry"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// Two clean publishes of the same package (into two registries) produce a
/// byte-identical archive digest — deterministic artifacts.
#[test]
fn two_clean_publishes_are_byte_identical() {
    let root = tmp_dir("det");
    let lib = root.join("lib");
    brix(&["new", lib.as_str(), "--name", "lib"]);

    let reg_a = root.join("reg-a");
    let reg_b = root.join("reg-b");
    assert!(
        brix(&["publish", lib.as_str(), "--registry", reg_a.as_str()])
            .status
            .success()
    );
    assert!(
        brix(&["publish", lib.as_str(), "--registry", reg_b.as_str()])
            .status
            .success()
    );

    let da = Registry::open(&reg_a)
        .unwrap()
        .versions(&name("lib"))
        .unwrap()[0]
        .content_digest;
    let db = Registry::open(&reg_b)
        .unwrap()
        .versions(&name("lib"))
        .unwrap()[0]
        .content_digest;
    assert_eq!(da, db, "two clean publishes must be byte-identical");
    std::fs::remove_dir_all(&root).ok();
}

/// A committed `brix.lock` that no longer matches a fresh resolution blocks
/// publish (dirty lockfile), leaving the registry unmutated.
#[test]
fn publish_rejects_dirty_lockfile() {
    let root = tmp_dir("dirty");
    let app = root.join("app");
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::write(
        app.join("brix.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\nlib = \"^1.0.0\"\n",
    )
    .unwrap();
    std::fs::write(
        app.join("src").join("world.brix"),
        "package app @ 0.1.0\nuse lib.{Widget}\nrel Out { id: Int } key(id)\n\
         derive R: Out(id: i) from { Widget(id: i, n: _n) }\n",
    )
    .unwrap();

    let registry = Registry::open(app.join(".brix").join("registry")).unwrap();
    let lib_src = "package lib @ VER\nrel Widget { id: Int; n: Int } key(id)\n";
    let publish_lib = |ver: &str| {
        let m =
            Manifest::parse(&format!("[package]\nname = \"lib\"\nversion = \"{ver}\"\n")).unwrap();
        let mut files: BTreeMap<Utf8PathBuf, Vec<u8>> = BTreeMap::new();
        files.insert(
            Utf8PathBuf::from("src/world.brix"),
            lib_src.replace("VER", ver).into_bytes(),
        );
        registry.publish(&m, &files, None).unwrap();
    };

    // Commit a lockfile resolved when only lib@1.0.0 exists.
    publish_lib("1.0.0");
    let locked = brixpkg::resolve(
        &Manifest::parse(&std::fs::read_to_string(app.join("brix.toml")).unwrap()).unwrap(),
        &registry,
        &BTreeMap::new(),
    )
    .unwrap();
    std::fs::write(
        app.join(brixpkg::graph::LOCKFILE_NAME),
        locked.to_toml_string().unwrap(),
    )
    .unwrap();

    // Now a newer compatible version exists → a fresh resolve differs from the
    // committed lock.
    publish_lib("1.0.1");

    let target = root.join("target-registry");
    let out = brix(&["publish", app.as_str(), "--registry", target.as_str()]);
    assert!(
        !out.status.success(),
        "publish with a stale lockfile must fail"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("dirty lockfile")
            || String::from_utf8_lossy(&out.stderr).contains("does not match"),
        "expected a dirty-lockfile error, got: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // The gate ran before any registry write: `app` was never published.
    assert!(
        !target.exists()
            || Registry::open(&target)
                .unwrap()
                .versions(&name("app"))
                .map(|v| v.is_empty())
                .unwrap_or(true),
        "a failed gate must not mutate the target registry"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// `brix yank` excludes a version from fresh resolution: a consumer that
/// resolved fine before now fails closed.
#[test]
fn yank_then_resolve_fails_closed() {
    let root = tmp_dir("yank");
    let registry_root = root.join("registry");
    let registry = Registry::open(&registry_root).unwrap();
    let m = Manifest::parse("[package]\nname = \"lib\"\nversion = \"1.0.0\"\n").unwrap();
    let mut files: BTreeMap<Utf8PathBuf, Vec<u8>> = BTreeMap::new();
    files.insert(
        Utf8PathBuf::from("src/world.brix"),
        b"package lib @ 1.0.0\nrel Widget { id: Int } key(id)\n".to_vec(),
    );
    registry.publish(&m, &files, None).unwrap();

    let app = Manifest::parse(
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\nlib = \"^1.0.0\"\n",
    )
    .unwrap();
    // Resolves before the yank.
    assert!(brixpkg::resolve(&app, &registry, &BTreeMap::new()).is_ok());

    let yanked = brix(&[
        "yank",
        "lib",
        "--at",
        "1.0.0",
        "--registry",
        registry_root.as_str(),
    ]);
    assert!(
        yanked.status.success(),
        "yank: {}",
        String::from_utf8_lossy(&yanked.stderr)
    );

    // A fresh resolve now has no solution.
    assert!(
        brixpkg::resolve(&app, &registry, &BTreeMap::new()).is_err(),
        "a yanked version must be excluded from fresh resolution"
    );
    std::fs::remove_dir_all(&root).ok();
}
