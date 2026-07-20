//! `brix publish` — the gated package-publication workflow (issue #44).
//!
//! Every gate runs *before* the single [`brixpkg::Registry::publish`] call, so a
//! failed gate can never mutate registry state. In order: parse/check the
//! package, verify a clean committed lockfile, assemble the canonical archive
//! from **declared files only**, then publish — recording the producing
//! toolchain as compatibility metadata. Path-dependency and immutable-version
//! rejection are enforced by `Registry::publish` itself.

use std::collections::BTreeMap;
use std::fmt;

use brix_diag::DiagnosticFormat;
use brixpkg::{graph::LOCKFILE_NAME, Compat, ContentDigest, Lockfile, Registry};
use camino::{Utf8Path, Utf8PathBuf};

use crate::build::BuildError;
use crate::package;
use crate::toolchain;

/// A successful publication.
pub struct PublishOutcome {
    pub name: String,
    pub version: String,
    pub digest: ContentDigest,
}

#[derive(Debug)]
pub enum PublishError {
    Locate(package::LocateError),
    /// The parse/check gate failed — the package does not compile.
    Check(BuildError),
    /// The package declares dependencies but has no committed `brix.lock`.
    NoLockfile(Utf8PathBuf),
    /// The committed `brix.lock` does not match a fresh resolution.
    DirtyLockfile,
    Toolchain(toolchain::ToolchainError),
    Registry(brixpkg::RegistryError),
    Lock(String),
    Io(std::io::Error),
    /// `publish` needs a package directory (a `brix.toml` + `src/`), not a bare
    /// source file.
    NotAPackage(Utf8PathBuf),
}

impl fmt::Display for PublishError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PublishError::Locate(e) => write!(f, "{e}"),
            PublishError::Check(e) => write!(f, "package does not check: {e}"),
            PublishError::NoLockfile(p) => write!(
                f,
                "no committed lockfile at {p} — run `brix build` first to lock dependencies"
            ),
            PublishError::DirtyLockfile => write!(
                f,
                "committed brix.lock does not match a fresh resolution (dirty lockfile) — \
                 run `brix build` to update it, then commit"
            ),
            PublishError::Toolchain(e) => write!(f, "{e}"),
            PublishError::Registry(e) => write!(f, "{e}"),
            PublishError::Lock(e) => write!(f, "lockfile error: {e}"),
            PublishError::Io(e) => write!(f, "I/O error: {e}"),
            PublishError::NotAPackage(p) => {
                write!(
                    f,
                    "{p} is not a package directory (needs brix.toml and src/)"
                )
            }
        }
    }
}

impl std::error::Error for PublishError {}

impl PublishError {
    /// Machine-facing rendering: only the check gate carries structured
    /// diagnostics; every other publish error is operational (human `Display`).
    pub fn render(&self, format: DiagnosticFormat) -> String {
        match self {
            PublishError::Check(e) => e.render(format),
            other => other.to_string(),
        }
    }
}

/// Publish the package named by `operand` (a package directory) to `registry`
/// (defaulting to the package-local `<pkg_root>/.brix/registry`).
pub fn publish(
    operand: &str,
    registry_override: Option<&str>,
) -> Result<PublishOutcome, PublishError> {
    // Gate 1 — parse/check. A package that does not compile is never published.
    crate::build::check(operand).map_err(PublishError::Check)?;

    let located = package::locate(operand).map_err(PublishError::Locate)?;
    let pkg_root = located.pkg_root.clone();
    let manifest = located.manifest.clone();

    // publish operates on a real package directory (brix.toml on disk).
    let manifest_path = pkg_root.join("brix.toml");
    if !manifest_path.exists() {
        return Err(PublishError::NotAPackage(pkg_root));
    }

    // Gate 2 — clean lockfile. A package with dependencies must have a committed
    // brix.lock that matches a fresh resolution (reuses locate's resolve).
    if let Some(fresh) = &located.lockfile {
        let committed_path = pkg_root.join(LOCKFILE_NAME);
        if !committed_path.exists() {
            return Err(PublishError::NoLockfile(committed_path));
        }
        let committed_text = std::fs::read_to_string(&committed_path).map_err(PublishError::Io)?;
        let committed =
            Lockfile::parse(&committed_text).map_err(|e| PublishError::Lock(e.to_string()))?;
        if committed.digest() != fresh.digest() {
            return Err(PublishError::DirtyLockfile);
        }
    }

    // Gate 3 — assemble the canonical archive from DECLARED files only:
    // brix.toml + everything under src/ (+ OWNER.md if present). Undeclared
    // paths cannot enter the archive by construction.
    let files = declared_files(&pkg_root, &manifest_path)?;

    // Compatibility metadata: the toolchain that produced this package.
    let tc = toolchain::detect().map_err(PublishError::Toolchain)?;
    let compat = Compat {
        brixc_version: tc.brixc_version,
        rustc_version: tc.rustc_version,
        target: tc.target,
    };

    // Gate 4 (+ publish) — path-dependency and immutable-version rejection are
    // enforced inside Registry::publish; a gate failure here leaves the registry
    // untouched because this is the only mutating call.
    let registry_root = match registry_override {
        Some(r) => Utf8PathBuf::from(r),
        None => pkg_root.join(".brix").join("registry"),
    };
    let registry = Registry::open(&registry_root).map_err(PublishError::Registry)?;
    let digest = registry
        .publish(&manifest, &files, Some(compat))
        .map_err(PublishError::Registry)?;

    Ok(PublishOutcome {
        name: manifest.name.to_string(),
        version: manifest.version.to_string(),
        digest,
    })
}

/// The declared file set of a package: `brix.toml`, `OWNER.md` (if present), and
/// every file under `src/`, keyed by path relative to `pkg_root` in `BTreeMap`
/// order — so two clean publishes produce a byte-identical archive.
fn declared_files(
    pkg_root: &Utf8Path,
    manifest_path: &Utf8Path,
) -> Result<BTreeMap<Utf8PathBuf, Vec<u8>>, PublishError> {
    let mut files = BTreeMap::new();
    files.insert(
        Utf8PathBuf::from("brix.toml"),
        std::fs::read(manifest_path).map_err(PublishError::Io)?,
    );
    let owner = pkg_root.join("OWNER.md");
    if owner.exists() {
        files.insert(
            Utf8PathBuf::from("OWNER.md"),
            std::fs::read(&owner).map_err(PublishError::Io)?,
        );
    }
    let src = pkg_root.join("src");
    if src.is_dir() {
        collect_dir(&src, pkg_root, &mut files)?;
    }
    Ok(files)
}

/// Recursively collect every file under `dir` into `files`, keyed by its path
/// relative to `base` (sorted, deterministic — no filesystem-order leakage).
fn collect_dir(
    dir: &Utf8Path,
    base: &Utf8Path,
    files: &mut BTreeMap<Utf8PathBuf, Vec<u8>>,
) -> Result<(), PublishError> {
    let mut entries: Vec<Utf8PathBuf> = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(PublishError::Io)? {
        let entry = entry.map_err(PublishError::Io)?;
        let path = Utf8PathBuf::from_path_buf(entry.path())
            .map_err(|_| PublishError::Io(std::io::Error::other("non-UTF-8 path")))?;
        entries.push(path);
    }
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_dir(&path, base, files)?;
        } else {
            let rel = path
                .strip_prefix(base)
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|_| path.clone());
            files.insert(rel, std::fs::read(&path).map_err(PublishError::Io)?);
        }
    }
    Ok(())
}

/// `brix yank <pkg> --at <version> [--registry <path>]`.
pub fn yank(name: &str, version: &str, registry_root: &str) -> Result<(), PublishError> {
    let registry = Registry::open(registry_root).map_err(PublishError::Registry)?;
    let pkg = brixpkg::PackageName::parse(name)
        .map_err(|e| PublishError::Lock(format!("invalid package name `{name}`: {e}")))?;
    let ver = brixpkg::version::parse_version(version)
        .map_err(|e| PublishError::Lock(format!("invalid version `{version}`: {e}")))?;
    registry.yank(&pkg, &ver).map_err(PublishError::Registry)
}
