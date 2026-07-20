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

use std::collections::BTreeMap;
use std::fmt;

use brix_ast::{parse_file, File};
use brixpkg::{
    hydrate, resolve, version::parse_version, HydrateError, Lockfile, Manifest, ManifestError,
    PackageName, Registry, RegistryError, ResolveError,
};
use camino::{Utf8Path, Utf8PathBuf};

/// A located package: its manifest (real or synthesized), the entry source
/// file to compile, its local submodule sources (issue #42 multi-file
/// packages: `src/<name>.brix` siblings of the entry), the directory build
/// artifacts live under (`<pkg_root>/.brix-cache/...`), and — for a package
/// with dependencies — the resolved dependency graph plus its lockfile.
pub struct LocatedPackage {
    pub manifest: Manifest,
    pub source_path: Utf8PathBuf,
    pub pkg_root: Utf8PathBuf,
    /// Whether `manifest` was loaded from an on-disk `brix.toml` (as opposed
    /// to synthesized from the source `package` declaration).
    pub explicit_manifest: bool,
    /// Local submodule sources discovered alongside `source_path` (issue
    /// #42 multi-file packages). Empty for a single-file package. Sorted by
    /// qualifier for determinism, independent of directory enumeration
    /// order.
    pub submodules: Vec<Submodule>,
    /// Resolved dependency graph: each dependency's package-name segments and
    /// its entry source. Empty for a dependency-free package.
    pub deps: Vec<GraphDep>,
    /// The resolved lockfile — real when `deps` is non-empty (it feeds the
    /// build cache key); `None` for a bare file / dependency-free package.
    pub lockfile: Option<Lockfile>,
}

/// One local submodule source (issue #42): its relative path (`src/order.brix`),
/// its module qualifier (the file stem, `"order"`), and its text.
pub struct Submodule {
    pub path: Utf8PathBuf,
    pub qualifier: String,
    pub source: String,
}

/// One resolved dependency's entry source, plus its own local submodules if
/// it is itself a multi-file package (issue #42).
pub struct GraphDep {
    pub name_segments: Vec<String>,
    pub source: String,
    pub submodules: Vec<Submodule>,
}

#[derive(Debug)]
pub enum LocateError {
    NotFound(Utf8PathBuf),
    Io(std::io::Error),
    MissingEntrySource(Utf8PathBuf),
    ManifestParse(ManifestError),
    NoPackageDecl(Utf8PathBuf),
    BadPackageDecl { reason: String },
    Registry(RegistryError),
    Resolve(ResolveError),
    Hydrate(HydrateError),
    NonUtf8Source(PackageName),
    NonUtf8Path(std::path::PathBuf),
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
            LocateError::Registry(e) => write!(f, "opening the local registry: {e}"),
            LocateError::Resolve(e) => write!(f, "resolving dependencies: {e:?}"),
            LocateError::Hydrate(e) => write!(f, "loading the dependency graph: {e}"),
            LocateError::NonUtf8Source(name) => {
                write!(f, "dependency `{name}`'s entry source is not valid UTF-8")
            }
            LocateError::NonUtf8Path(p) => {
                write!(f, "{} is not a valid UTF-8 path", p.display())
            }
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
        return finish_located(manifest, source_path, pkg_root, true);
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

    finish_located(manifest, source_path, pkg_root, explicit_manifest)
}

/// Assemble a [`LocatedPackage`], resolving + hydrating the dependency graph
/// (issue #42) when the manifest declares dependencies. A dependency-free
/// package carries an empty graph and no lockfile — the pre-#42 behavior.
fn finish_located(
    manifest: Manifest,
    source_path: Utf8PathBuf,
    pkg_root: Utf8PathBuf,
    explicit_manifest: bool,
) -> Result<LocatedPackage, LocateError> {
    let (deps, lockfile) = if manifest.dependencies.is_empty() {
        (Vec::new(), None)
    } else {
        let (deps, lockfile) = load_graph(&manifest, &pkg_root)?;
        (deps, Some(lockfile))
    };
    let submodules = discover_submodules(&pkg_root, &source_path)?;
    Ok(LocatedPackage {
        manifest,
        source_path,
        pkg_root,
        explicit_manifest,
        submodules,
        deps,
        lockfile,
    })
}

/// Local submodule discovery (issue #42): every `src/<name>.brix` sibling of
/// the entry source, sorted by qualifier (file stem) — deterministic
/// regardless of how the OS enumerated the directory. Returns an empty list
/// when there is no `src` directory at all (a bare single-file operand with
/// no manifest, the pre-#42 case) or the entry itself is not laid out under
/// a `src/` directory (nested submodules under `src/` are the only shape
/// this slice supports; see `packages/STATUS.md`).
fn discover_submodules(
    pkg_root: &Utf8Path,
    source_path: &Utf8Path,
) -> Result<Vec<Submodule>, LocateError> {
    let src_dir = pkg_root.join("src");
    if !src_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut found = BTreeMap::new();
    for entry in std::fs::read_dir(&src_dir)? {
        let entry = entry?;
        let raw = entry.path();
        let path = Utf8PathBuf::try_from(raw.clone()).map_err(|_| LocateError::NonUtf8Path(raw))?;
        // Exclude the entry by *file name*, not a full-path string compare:
        // `pkg_root`/`source_path` can be reached through different (but
        // filesystem-equivalent, e.g. containing a literal `..`) spellings
        // depending on how the operand was resolved, so comparing the raw
        // path strings can miss the match even when it's the same file.
        // File names are unique within one directory, so this is exact.
        if path.file_name() == source_path.file_name() || path.extension() != Some("brix") {
            continue;
        }
        let Some(stem) = path.file_stem().map(str::to_string) else {
            continue;
        };
        let source = std::fs::read_to_string(&path)?;
        found.insert(
            stem.clone(),
            Submodule {
                path,
                qualifier: stem,
                source,
            },
        );
    }
    Ok(found.into_values().collect())
}

/// Resolve `manifest`'s dependencies against the package-local registry
/// (`<pkg_root>/.brix/registry`) and hydrate every locked package's entry
/// source — a fully offline, deterministic load (issue #42). Registry
/// dependencies only in this slice; a path dependency surfaces as a clear
/// hydrate error.
fn load_graph(
    manifest: &Manifest,
    pkg_root: &Utf8Path,
) -> Result<(Vec<GraphDep>, Lockfile), LocateError> {
    let registry_root = pkg_root.join(".brix").join("registry");
    let registry = Registry::open(&registry_root).map_err(LocateError::Registry)?;
    // Path dependencies (caller-located, pre-digested) are deferred; register
    // deps resolve against the local registry with no network access.
    let path_deps = BTreeMap::new();
    let lockfile = resolve(manifest, &registry, &path_deps).map_err(LocateError::Resolve)?;
    let graph = hydrate(&lockfile, &registry, pkg_root).map_err(LocateError::Hydrate)?;

    let entry = Utf8Path::new("src/world.brix");
    let mut deps = Vec::new();
    for (name, files) in &graph {
        let bytes = files
            .get(entry)
            .ok_or_else(|| LocateError::MissingEntrySource(Utf8PathBuf::from(name.to_string())))?;
        let source = String::from_utf8(bytes.clone())
            .map_err(|_| LocateError::NonUtf8Source(name.clone()))?;
        // Issue #42: a dependency may itself be a multi-file package — carry
        // every other `src/*.brix` sibling of its entry too, not just the
        // entry, so `lower_program` can merge its submodules the same way it
        // would for the root package.
        let mut submodules = Vec::new();
        for (path, sub_bytes) in files {
            if path.file_name() == entry.file_name()
                || path.extension() != Some("brix")
                || path.parent() != entry.parent()
            {
                continue;
            }
            let Some(stem) = path.file_stem() else {
                continue;
            };
            let sub_source = String::from_utf8(sub_bytes.clone())
                .map_err(|_| LocateError::NonUtf8Source(name.clone()))?;
            submodules.push(Submodule {
                path: path.clone(),
                qualifier: stem.to_string(),
                source: sub_source,
            });
        }
        deps.push(GraphDep {
            name_segments: name.to_string().split('.').map(str::to_string).collect(),
            source,
            submodules,
        });
    }
    Ok((deps, lockfile))
}

fn load_manifest(manifest_path: &Utf8Path) -> Result<Manifest, LocateError> {
    let text = std::fs::read_to_string(manifest_path)?;
    Manifest::parse(&text).map_err(LocateError::ManifestParse)
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
