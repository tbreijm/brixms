//! Hydrate a resolved package graph into in-memory source trees (issue #42).
//!
//! `resolve` produces a [`Lockfile`] naming every package in the transitive
//! graph by content digest; a compiler front end then needs the actual source
//! bytes of each. [`hydrate`] reads them — registry entries from the
//! content-addressed [`Registry`], path entries from disk — and **verifies
//! every tree against its locked content digest**, so a tampered store or a
//! mutated path package fails closed rather than compiling silently.
//!
//! Determinism: the result is keyed by [`PackageName`] in a `BTreeMap` (canon
//! order), independent of resolution or filesystem enumeration order.

use std::collections::BTreeMap;
use std::fmt;

use camino::{Utf8Path, Utf8PathBuf};

use crate::digest::tree_digest;
use crate::lock::{LockSource, Lockfile};
use crate::registry::{Registry, RegistryError};
use crate::version::PackageName;
use crate::ContentDigest;

/// The committed lockfile's filename at a package root.
pub const LOCKFILE_NAME: &str = "brix.lock";

/// One package's file tree: relative path -> bytes.
pub type PackageFiles = BTreeMap<Utf8PathBuf, Vec<u8>>;

/// The hydrated transitive graph: every locked package's verified source tree.
pub type PackageGraph = BTreeMap<PackageName, PackageFiles>;

#[derive(Debug)]
pub enum HydrateError {
    /// A registry fetch failed (missing/corrupt store blob).
    Registry(PackageName, RegistryError),
    /// A path-dependency file could not be read from disk.
    Io {
        package: PackageName,
        path: Utf8PathBuf,
        error: std::io::Error,
    },
    /// A package's hydrated content does not match its locked digest — a
    /// tampered store, index, or path package. Fail closed.
    Tampered {
        package: PackageName,
        expected: ContentDigest,
        actual: ContentDigest,
    },
    /// Path-dependency hydration is not implemented in this slice (registry
    /// dependencies only for now).
    PathUnsupported(PackageName, Utf8PathBuf),
}

impl fmt::Display for HydrateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HydrateError::Registry(pkg, e) => {
                write!(f, "fetching `{pkg}` from the registry: {e}")
            }
            HydrateError::Io {
                package,
                path,
                error,
            } => write!(f, "reading path package `{package}` at {path}: {error}"),
            HydrateError::Tampered {
                package,
                expected,
                actual,
            } => write!(
                f,
                "package `{package}` content digest mismatch (tampered): locked {}, got {}",
                expected.to_hex(),
                actual.to_hex()
            ),
            HydrateError::PathUnsupported(pkg, path) => write!(
                f,
                "path dependency `{pkg}` at {path} is not yet hydrated (registry dependencies only)"
            ),
        }
    }
}

impl std::error::Error for HydrateError {}

/// Read and verify every package's source tree named by `lockfile`.
///
/// The root package is **not** an entry in the lockfile (only its
/// dependencies are), so the caller supplies the root's files separately; this
/// hydrates the dependency graph. Registry entries are fetched from
/// `registry`; path entries are read relative to `base_dir`. Every tree is
/// checked against its `content_digest` — a mismatch is [`HydrateError::Tampered`].
pub fn hydrate(
    lockfile: &Lockfile,
    registry: &Registry,
    base_dir: &Utf8Path,
) -> Result<PackageGraph, HydrateError> {
    let mut out = PackageGraph::new();
    for (name, entry) in &lockfile.entries {
        let files = match &entry.source {
            LockSource::Registry => registry
                .fetch(&entry.content_digest)
                .map_err(|e| HydrateError::Registry(name.clone(), e))?,
            LockSource::Path(rel) => {
                // Path-dependency hydration (walking a sibling package's
                // `src/` off disk) is deferred; registry deps cover the
                // Slice-1 graph. Documented gap, not a silent skip.
                return Err(HydrateError::PathUnsupported(
                    name.clone(),
                    base_dir.join(rel),
                ));
            }
        };
        let actual = tree_digest(&files);
        if actual != entry.content_digest {
            return Err(HydrateError::Tampered {
                package: name.clone(),
                expected: entry.content_digest,
                actual,
            });
        }
        out.insert(name.clone(), files);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Manifest;
    use crate::resolve::resolve;
    use crate::version::Version;

    fn tmp_dir(tag: &str) -> Utf8PathBuf {
        let mut p = Utf8PathBuf::from_path_buf(std::env::temp_dir()).unwrap();
        p.push(format!(
            "brixpkg-graph-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        p
    }

    fn manifest(toml: &str) -> Manifest {
        Manifest::parse(toml).expect("manifest parses")
    }

    /// Publish `lib@1.0.0` (one source file) to a fresh registry, resolve a
    /// root that depends on it, and hydrate — the tree round-trips and
    /// verifies against its locked digest.
    #[test]
    fn hydrate_round_trips_a_registry_dependency() {
        let root = tmp_dir("rt");
        let registry = Registry::open(&root).expect("registry opens");
        let lib_manifest = manifest("[package]\nname = \"lib\"\nversion = \"1.0.0\"\n");
        let mut lib_files = PackageFiles::new();
        lib_files.insert(
            Utf8PathBuf::from("src/world.brix"),
            b"package lib @ 1.0.0\nrel Widget { id: I64 } key(id)\n".to_vec(),
        );
        registry
            .publish(&lib_manifest, &lib_files, None)
            .expect("publish lib");

        let app = manifest(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\nlib = \"^1.0.0\"\n",
        );
        let lockfile = resolve(&app, &registry, &BTreeMap::new()).expect("resolve");

        let graph = hydrate(&lockfile, &registry, &root).expect("hydrate");
        let lib_name = PackageName::parse("lib").unwrap();
        assert_eq!(graph.len(), 1);
        assert_eq!(graph[&lib_name], lib_files);
    }

    /// A store blob that no longer matches its locked digest fails closed.
    #[test]
    fn tampered_content_fails_closed() {
        let root = tmp_dir("tamper");
        let registry = Registry::open(&root).expect("registry opens");
        let lib_manifest = manifest("[package]\nname = \"lib\"\nversion = \"1.0.0\"\n");
        let mut lib_files = PackageFiles::new();
        lib_files.insert(
            Utf8PathBuf::from("src/world.brix"),
            b"package lib @ 1.0.0\n".to_vec(),
        );
        registry.publish(&lib_manifest, &lib_files, None).unwrap();
        let app = manifest(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\nlib = \"^1.0.0\"\n",
        );
        let lockfile = resolve(&app, &registry, &BTreeMap::new()).unwrap();

        // Overwrite the store blob with a *different* (still valid) packed tree
        // while leaving the lockfile's digest pointing at the original — the
        // fetch now succeeds but returns content whose digest no longer
        // matches, exactly a mutated-store attack.
        let lib_name = PackageName::parse("lib").unwrap();
        let hex = lockfile.entries[&lib_name].content_digest.to_hex();
        let store_blob = root.join("store").join(&hex);
        let mut other = PackageFiles::new();
        other.insert(
            Utf8PathBuf::from("src/world.brix"),
            b"package lib @ 1.0.0\nrel Mutated { x: I64 } key(x)\n".to_vec(),
        );
        std::fs::write(&store_blob, crate::digest::pack(&other)).unwrap();

        let err = hydrate(&lockfile, &registry, &root).unwrap_err();
        assert!(
            matches!(err, HydrateError::Tampered { .. }),
            "expected Tampered, got {err:?}"
        );
    }

    #[test]
    fn version_is_reachable_for_helpers() {
        // Touch Version so the import is always exercised in this module's
        // test build regardless of feature flags.
        let _ = Version::new(1, 0, 0);
    }
}
