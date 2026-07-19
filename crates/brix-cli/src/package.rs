//! Locate the package a `brix build`/`brix run` operand names.
//!
//! `brixpkg::resolve` deliberately does not walk the filesystem itself (see
//! its own docs: "brix-cli does not itself walk the filesystem... that is
//! brix-cli's job") — this module is that job, scoped to what issue #9
//! needs: a bare `.brix` file (the acceptance-tested case) or a package
//! directory, with an opportunistic `brix.toml` and a synthesized
//! zero-dependency manifest as the fallback. A manifest with any declared
//! dependency is a clear, explicit error — full `brixpkg::resolve`/registry
//! wiring is separable, sizeable work outside this issue's scope, and a
//! documented gap here beats a silent partial resolution.

use std::fmt;

use brix_ast::{parse_file, File};
use brixpkg::{version::parse_version, Manifest, ManifestError, PackageName};
use camino::{Utf8Path, Utf8PathBuf};

/// A located package: its manifest (real or synthesized), the entry source
/// file to compile, and the directory build artifacts live under
/// (`<pkg_root>/.brix-cache/...`).
pub struct LocatedPackage {
    pub manifest: Manifest,
    pub source_path: Utf8PathBuf,
    pub pkg_root: Utf8PathBuf,
    /// Whether `manifest` was loaded from an on-disk `brix.toml` (as opposed
    /// to synthesized from the source `package` declaration).
    pub explicit_manifest: bool,
}

#[derive(Debug)]
pub enum LocateError {
    NotFound(Utf8PathBuf),
    Io(std::io::Error),
    MissingEntrySource(Utf8PathBuf),
    ManifestParse(ManifestError),
    DependenciesNotSupported,
    NoPackageDecl(Utf8PathBuf),
    BadPackageDecl { reason: String },
}

impl fmt::Display for LocateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LocateError::NotFound(p) => write!(f, "{p} not found"),
            LocateError::Io(e) => write!(f, "I/O error: {e}"),
            LocateError::MissingEntrySource(p) => {
                write!(f, "package directory has no entry source file at {p}")
            }
            LocateError::ManifestParse(e) => write!(f, "malformed brix.toml: {e}"),
            LocateError::DependenciesNotSupported => write!(
                f,
                "dependencies are not yet supported by `brix build` (brix.toml declares at \
                 least one) — build a dependency-free package for now"
            ),
            LocateError::NoPackageDecl(p) => {
                write!(f, "{p} has no `package NAME @ VERSION` declaration and no brix.toml was found alongside it — cannot name the generated crate")
            }
            LocateError::BadPackageDecl { reason } => {
                write!(f, "source's package declaration is invalid: {reason}")
            }
        }
    }
}

impl std::error::Error for LocateError {}

impl From<std::io::Error> for LocateError {
    fn from(e: std::io::Error) -> Self {
        LocateError::Io(e)
    }
}

/// Locate and load the package named by `operand` (a file or directory
/// path, as given on the `brix build`/`brix run` command line).
pub fn locate(operand: &str) -> Result<LocatedPackage, LocateError> {
    let path = Utf8Path::new(operand);
    if !path.exists() {
        return Err(LocateError::NotFound(path.to_path_buf()));
    }

    if path.is_dir() {
        let pkg_root = path.to_path_buf();
        let manifest_path = pkg_root.join("brix.toml");
        let source_path = pkg_root.join("src").join("world.brix");
        if !manifest_path.exists() {
            return Err(LocateError::NotFound(manifest_path));
        }
        if !source_path.exists() {
            return Err(LocateError::MissingEntrySource(source_path));
        }
        let manifest = load_manifest(&manifest_path)?;
        return Ok(LocatedPackage {
            manifest,
            source_path,
            pkg_root,
            explicit_manifest: true,
        });
    }

    let source_path = path.to_path_buf();
    let file_dir = source_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| Utf8PathBuf::from("."));

    // Opportunistic manifest discovery: <dir>/brix.toml, then the layout
    // scaffold.rs itself produces, <dir>/../brix.toml (for src/world.brix).
    let candidates = [
        file_dir.join("brix.toml"),
        file_dir.join("..").join("brix.toml"),
    ];
    let found = candidates.into_iter().find(|c| c.exists());

    let (manifest, pkg_root, explicit_manifest) = match found {
        Some(manifest_path) => {
            let manifest = load_manifest(&manifest_path)?;
            let pkg_root = manifest_path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| Utf8PathBuf::from("."));
            (manifest, pkg_root, true)
        }
        None => {
            let src = std::fs::read_to_string(&source_path)?;
            let (file, _parse_diags) = parse_file(&src);
            let manifest = synthesize_manifest(&file, &source_path)?;
            (manifest, file_dir, false)
        }
    };

    Ok(LocatedPackage {
        manifest,
        source_path,
        pkg_root,
        explicit_manifest,
    })
}

fn load_manifest(manifest_path: &Utf8Path) -> Result<Manifest, LocateError> {
    let text = std::fs::read_to_string(manifest_path)?;
    let manifest = Manifest::parse(&text).map_err(LocateError::ManifestParse)?;
    if !manifest.dependencies.is_empty() {
        return Err(LocateError::DependenciesNotSupported);
    }
    Ok(manifest)
}

/// Synthesize an in-memory, zero-dependency manifest from the source
/// file's own `package NAME @ VERSION` declaration (Appendix D
/// `PackageDecl`) — package identity is normative source-level state, so
/// this is a faithful, not a guessed, stand-in for a missing `brix.toml`.
fn synthesize_manifest(file: &File, source_path: &Utf8Path) -> Result<Manifest, LocateError> {
    let decl = file
        .package
        .as_ref()
        .ok_or_else(|| LocateError::NoPackageDecl(source_path.to_path_buf()))?;
    let name_text = decl
        .name
        .segments
        .iter()
        .map(|s| s.text.as_str())
        .collect::<Vec<_>>()
        .join(".");
    let name = PackageName::parse(&name_text).map_err(|e| LocateError::BadPackageDecl {
        reason: e.to_string(),
    })?;
    let version = parse_version(&decl.version.text).map_err(|e| LocateError::BadPackageDecl {
        reason: e.to_string(),
    })?;
    Ok(Manifest {
        name,
        version,
        authors: Vec::new(),
        dependencies: Default::default(),
    })
}
