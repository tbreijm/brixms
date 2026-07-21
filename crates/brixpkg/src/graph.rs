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

/// A package-dependency cycle found in a locked graph (issue #42 Slice 2):
/// `path[0] -> path[1] -> ... -> path[n]` where `path[n] == path[0]` (or, for
/// a back-edge to the root, `path[n] == Lockfile::root`). Produced by
/// [`check_acyclic`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cycle {
    /// The cycle, in DFS discovery order, ending with the repeated name.
    pub path: Vec<PackageName>,
}

impl fmt::Display for Cycle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "package dependency cycle: ")?;
        for (i, name) in self.path.iter().enumerate() {
            if i > 0 {
                write!(f, " -> ")?;
            }
            write!(f, "{name}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Cycle {}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    InProgress,
    Done,
}

/// Verify `lockfile`'s dependency graph is acyclic (issue #42 Slice 2).
///
/// The root package is **not** itself a lockfile entry (only its
/// dependencies are — see [`Lockfile::root`] docs), so there is no single
/// literal "root node" to start a textbook DFS from. Every entry present in
/// `lockfile.entries` is, by construction of [`crate::resolve::resolve`],
/// already known to be reachable from the root's own declared dependencies
/// (that is precisely why it got selected and locked) — so this DFS visits
/// every entry as a start point (skipping ones already settled by an earlier
/// start), which finds any cycle among entries. A dependency edge that names
/// the root itself (a dependency depending back on the package that pulled
/// it in) closes a cycle back to the root and is reported the same way.
///
/// Deterministic: entries are visited in `BTreeMap` (canon package-name)
/// order and each entry's `dependencies` in `BTreeSet` (canon) order, so the
/// reported cycle path is identical regardless of resolution or publish
/// order.
pub fn check_acyclic(lockfile: &Lockfile) -> Result<(), Cycle> {
    let mut marks: BTreeMap<PackageName, Mark> = BTreeMap::new();
    let mut stack: Vec<PackageName> = Vec::new();

    for start in lockfile.entries.keys() {
        if marks.contains_key(start) {
            continue;
        }
        visit(start, lockfile, &mut marks, &mut stack)?;
    }
    Ok(())
}

fn visit(
    name: &PackageName,
    lockfile: &Lockfile,
    marks: &mut BTreeMap<PackageName, Mark>,
    stack: &mut Vec<PackageName>,
) -> Result<(), Cycle> {
    marks.insert(name.clone(), Mark::InProgress);
    stack.push(name.clone());

    if let Some(entry) = lockfile.entries.get(name) {
        for dep in &entry.dependencies {
            if *dep == lockfile.root {
                // Every entry on `stack` is reachable from the root by
                // construction, so a dependency edge back to the root name
                // closes a cycle through the whole current stack.
                let mut path = stack.clone();
                path.push(dep.clone());
                return Err(Cycle { path });
            }
            match marks.get(dep) {
                Some(Mark::InProgress) => {
                    let start_idx = stack.iter().position(|n| n == dep).unwrap_or(0);
                    let mut path = stack[start_idx..].to_vec();
                    path.push(dep.clone());
                    return Err(Cycle { path });
                }
                Some(Mark::Done) => {}
                None => visit(dep, lockfile, marks, stack)?,
            }
        }
    }
    // A dependency name with no matching entry at all is a dangling
    // reference, not a cycle — Slice 1's missing-dependency detection (via
    // `resolve`) is what rejects that; not this function's job.

    stack.pop();
    marks.insert(name.clone(), Mark::Done);
    Ok(())
}

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
    /// The locked graph contains a package-dependency cycle (issue #42
    /// Slice 2). Checked and rejected before any fetch, so a cyclic graph
    /// fails closed rather than hydrating a nonsensical partial tree.
    Cycle(Cycle),
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
            HydrateError::Cycle(cycle) => write!(f, "{cycle}"),
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
    // Fail closed on a cyclic locked graph before touching the registry or
    // filesystem at all (issue #42 Slice 2).
    check_acyclic(lockfile).map_err(HydrateError::Cycle)?;

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
    use crate::lock::{LockEntry, LOCK_FORMAT_VERSION};
    use crate::manifest::Manifest;
    use crate::resolve::resolve;
    use crate::version::Version;

    fn dummy_digest() -> ContentDigest {
        tree_digest(&PackageFiles::new())
    }

    fn deps(names: &[&str]) -> std::collections::BTreeSet<PackageName> {
        names
            .iter()
            .map(|n| PackageName::parse(n).unwrap())
            .collect()
    }

    fn locked(deps_of: &[(&str, &[&str])]) -> Lockfile {
        let mut entries = BTreeMap::new();
        for (name, ds) in deps_of {
            entries.insert(
                PackageName::parse(name).unwrap(),
                LockEntry {
                    version: Version::new(1, 0, 0),
                    source: LockSource::Registry,
                    content_digest: dummy_digest(),
                    dependencies: deps(ds),
                },
            );
        }
        Lockfile {
            format_version: LOCK_FORMAT_VERSION,
            root: PackageName::parse("app").unwrap(),
            entries,
        }
    }

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

    /// `check_acyclic` on a hand-built acyclic lockfile (a diamond: `app ->
    /// a, b`; `a -> c`; `b -> c`) is `Ok`.
    #[test]
    fn check_acyclic_accepts_a_dag() {
        let lock = locked(&[("a", &["c"]), ("b", &["c"]), ("c", &[])]);
        assert!(check_acyclic(&lock).is_ok());
    }

    /// `check_acyclic` on a hand-built mutual cycle (`a -> b -> a`) reports a
    /// `Cycle` naming the exact path, deterministically.
    #[test]
    fn check_acyclic_rejects_a_hand_built_cycle() {
        let lock = locked(&[("a", &["b"]), ("b", &["a"])]);
        let err = check_acyclic(&lock).unwrap_err();
        assert_eq!(err.path, vec![pkg("a"), pkg("b"), pkg("a")]);
        assert_eq!(err.to_string(), "package dependency cycle: a -> b -> a");
    }

    /// A dependency edge that names the root itself (a pulled-in package
    /// depending back on the package that pulled it in) is also a cycle,
    /// even though the root is never a lockfile entry.
    #[test]
    fn check_acyclic_rejects_a_back_edge_to_the_root() {
        let lock = locked(&[("a", &["app"])]);
        let err = check_acyclic(&lock).unwrap_err();
        assert_eq!(err.path, vec![pkg("a"), pkg("app")]);
    }

    /// Determinism: the reported cycle path is identical no matter what
    /// order the entries happen to be constructed/iterated in — `BTreeMap`/
    /// `BTreeSet` order is canon, not insertion order.
    #[test]
    fn cycle_path_is_stable_regardless_of_entry_order() {
        let lock_ab = locked(&[("a", &["b"]), ("b", &["a"])]);
        let lock_ba = locked(&[("b", &["a"]), ("a", &["b"])]);
        assert_eq!(
            check_acyclic(&lock_ab).unwrap_err(),
            check_acyclic(&lock_ba).unwrap_err()
        );
    }

    fn pkg(name: &str) -> PackageName {
        PackageName::parse(name).unwrap()
    }

    /// A genuinely cyclic *registry* graph (`a` depends on `b`, `b` depends
    /// on `a`) does not hang `resolve` (the worklist's `loaded` guard
    /// already prevents an infinite requeue — see `resolve::resolve`) and
    /// pubgrub is happy to pick versions for it (a version cycle is not a
    /// version *conflict*); `hydrate` is what rejects the resulting locked
    /// graph, deterministically, before touching the registry store.
    #[test]
    fn resolve_terminates_and_hydrate_rejects_a_cyclic_dependency_graph() {
        let root = tmp_dir("resolve-cycle");
        let registry = Registry::open(&root).expect("registry opens");

        let a_manifest = manifest(
            "[package]\nname = \"a\"\nversion = \"1.0.0\"\n[dependencies]\nb = \"^1.0.0\"\n",
        );
        let mut a_files = PackageFiles::new();
        a_files.insert(
            Utf8PathBuf::from("src/world.brix"),
            b"package a @ 1.0.0\n".to_vec(),
        );
        registry.publish(&a_manifest, &a_files, None).expect("publish a");

        let b_manifest = manifest(
            "[package]\nname = \"b\"\nversion = \"1.0.0\"\n[dependencies]\na = \"^1.0.0\"\n",
        );
        let mut b_files = PackageFiles::new();
        b_files.insert(
            Utf8PathBuf::from("src/world.brix"),
            b"package b @ 1.0.0\n".to_vec(),
        );
        registry.publish(&b_manifest, &b_files, None).expect("publish b");

        let app = manifest(
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n[dependencies]\na = \"^1.0.0\"\n",
        );
        let lockfile = resolve(&app, &registry, &BTreeMap::new())
            .expect("resolve terminates and succeeds despite the mutual a<->b edge");

        let err = hydrate(&lockfile, &registry, &root).unwrap_err();
        assert!(
            matches!(err, HydrateError::Cycle(_)),
            "expected Cycle, got {err:?}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn version_is_reachable_for_helpers() {
        // Touch Version so the import is always exercised in this module's
        // test build regardless of feature flags.
        let _ = Version::new(1, 0, 0);
    }
}
