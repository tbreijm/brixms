//! Version resolution over the local registry, via pubgrub.
//!
//! `brixpkg` feeds the registry's per-package version indices into pubgrub's
//! [`OfflineDependencyProvider`] and lets pubgrub pick a satisfying set, then
//! projects the result into a [`Lockfile`] whose entries carry the exact content
//! digest of every chosen version. Path dependencies are resolved directly (no
//! version negotiation) and folded into the same lockfile.
//!
//! Determinism note: pubgrub's own internal maps are `rustc_hash` maps, not
//! `std::collections::HashMap`, so they don't trip the Ring 0 `disallowed_types`
//! lint; and everything *observable* from brixpkg — the lockfile's `entries` —
//! is a `BTreeMap` in canon (package-name) order, so resolution order never
//! leaks into an artifact.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use camino::Utf8PathBuf;
use pubgrub::{
    resolve as pubgrub_resolve, DefaultStringReporter, OfflineDependencyProvider, PubGrubError,
    Reporter,
};

use crate::digest::ContentDigest;
use crate::lock::{LockEntry, LockSource, Lockfile, LOCK_FORMAT_VERSION};
use crate::manifest::{DependencySpec, Manifest};
use crate::registry::Registry;
use crate::version::{PackageName, Version};

/// A path dependency the caller has already located on disk, with its parsed
/// manifest and content digest. brixpkg does not itself walk the filesystem to
/// find these (that is `brix-cli`'s job); it takes them pre-loaded so the
/// resolver stays a pure function of its inputs and is trivially testable.
#[derive(Clone, Debug)]
pub struct PathPackage {
    pub manifest: Manifest,
    pub path: Utf8PathBuf,
    pub content_digest: ContentDigest,
}

/// Errors from resolution.
#[derive(Debug)]
pub enum ResolveError {
    Registry(crate::registry::RegistryError),
    /// pubgrub could not find a satisfying set. The `String` is pubgrub's
    /// human-readable derivation tree — later routed into a `brix-diag`
    /// structured conflict once that crate's API lands.
    NoSolution(String),
    /// A path dependency named a package the resolver was not given a
    /// `PathPackage` for.
    MissingPathPackage(PackageName),
    Solver(String),
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolveError::Registry(e) => write!(f, "{e}"),
            ResolveError::NoSolution(s) => write!(f, "no version solution:\n{s}"),
            ResolveError::MissingPathPackage(n) => {
                write!(f, "path dependency {n} was not provided to the resolver")
            }
            ResolveError::Solver(s) => write!(f, "resolver error: {s}"),
        }
    }
}

impl std::error::Error for ResolveError {}

impl From<crate::registry::RegistryError> for ResolveError {
    fn from(e: crate::registry::RegistryError) -> Self {
        ResolveError::Registry(e)
    }
}

/// Resolve `root`'s dependency closure against `registry`, producing a
/// [`Lockfile`]. `path_deps` supplies any path dependencies (transitively)
/// reachable from the root, keyed by package name.
///
/// Path dependencies are pinned directly and excluded from pubgrub's registry
/// search; registry dependencies of a path package are still resolved normally.
pub fn resolve(
    root: &Manifest,
    registry: &Registry,
    path_deps: &BTreeMap<PackageName, PathPackage>,
) -> Result<Lockfile, ResolveError> {
    let root_name = root.name.clone();

    // 1. Load the registry-visible package universe reachable from the root into
    //    an offline provider. We add path packages under their own name/version
    //    with their (registry) dependencies too, so pubgrub sees a complete
    //    graph; the *content* of a path package just isn't fetched from the
    //    registry.
    let mut provider: Provider = OfflineDependencyProvider::new();

    // The registry deps we still need to enumerate versions for.
    let mut to_load: Vec<PackageName> = Vec::new();
    let mut loaded: BTreeSet<PackageName> = BTreeSet::new();

    add_manifest_edges(&mut provider, root, path_deps, &mut to_load);

    while let Some(name) = to_load.pop() {
        if path_deps.contains_key(&name) || !loaded.insert(name.clone()) {
            continue;
        }
        for entry in registry.versions(&name)? {
            if entry.yanked {
                continue;
            }
            let deps: Vec<(PubName, crate::version::VersionRange<Version>)> = entry
                .dependencies
                .iter()
                .map(|(dep, req)| {
                    to_load.push(dep.clone());
                    (PubName(dep.clone()), req.to_range())
                })
                .collect();
            provider.add_dependencies(PubName(name.clone()), entry.version, deps);
        }
    }

    // Path packages: register their own registry-dependency edges.
    for pkg in path_deps.values() {
        let deps: Vec<(PubName, crate::version::VersionRange<Version>)> = pkg
            .manifest
            .dependencies
            .iter()
            .filter_map(|(dep, spec)| match spec {
                DependencySpec::Registry(req) => {
                    to_load.push(dep.clone());
                    Some((PubName(dep.clone()), req.to_range()))
                }
                DependencySpec::Path(_) => None,
            })
            .collect();
        provider.add_dependencies(
            PubName(pkg.manifest.name.clone()),
            pkg.manifest.version,
            deps,
        );
        // A path package may pull in further registry deps discovered above.
        while let Some(name) = to_load.pop() {
            if path_deps.contains_key(&name) || !loaded.insert(name.clone()) {
                continue;
            }
            for entry in registry.versions(&name)? {
                if entry.yanked {
                    continue;
                }
                let deps: Vec<(PubName, crate::version::VersionRange<Version>)> = entry
                    .dependencies
                    .iter()
                    .map(|(d, req)| {
                        to_load.push(d.clone());
                        (PubName(d.clone()), req.to_range())
                    })
                    .collect();
                provider.add_dependencies(PubName(name.clone()), entry.version, deps);
            }
        }
    }

    // 2. Run pubgrub from the root.
    let selection = pubgrub_resolve(&provider, PubName(root_name.clone()), root.version)
        .map_err(map_pubgrub_error)?;

    // 3. Project the selection into a lockfile with exact content digests.
    let mut entries: BTreeMap<PackageName, LockEntry> = BTreeMap::new();
    for (pub_name, version) in selection {
        let name = pub_name.0;
        if name == root_name {
            continue; // the root itself is not a lock entry
        }
        let (source, content_digest, dependencies) = if let Some(pkg) = path_deps.get(&name) {
            let deps = registry_dep_names(&pkg.manifest);
            (LockSource::Path(pkg.path.clone()), pkg.content_digest, deps)
        } else {
            let versions = registry.versions(&name)?;
            let entry = versions
                .iter()
                .find(|e| e.version == version)
                .ok_or_else(|| {
                    ResolveError::Registry(crate::registry::RegistryError::UnknownVersion {
                        name: name.clone(),
                        version,
                    })
                })?;
            let deps = entry.dependencies.keys().cloned().collect();
            (LockSource::Registry, entry.content_digest, deps)
        };
        entries.insert(
            name,
            LockEntry {
                version,
                source,
                content_digest,
                dependencies,
            },
        );
    }

    Ok(Lockfile {
        format_version: LOCK_FORMAT_VERSION,
        root: root_name,
        entries,
    })
}

fn registry_dep_names(manifest: &Manifest) -> BTreeSet<PackageName> {
    manifest.dependencies.keys().cloned().collect()
}

fn add_manifest_edges(
    provider: &mut Provider,
    manifest: &Manifest,
    path_deps: &BTreeMap<PackageName, PathPackage>,
    to_load: &mut Vec<PackageName>,
) {
    let deps: Vec<(PubName, crate::version::VersionRange<Version>)> = manifest
        .dependencies
        .iter()
        .map(|(dep, spec)| {
            let range = match spec {
                DependencySpec::Registry(req) => {
                    to_load.push(dep.clone());
                    req.to_range()
                }
                DependencySpec::Path(_) => {
                    // Pin the path package at its exact on-disk version.
                    let version = path_deps
                        .get(dep)
                        .map(|p| p.manifest.version)
                        .unwrap_or_else(Version::zero);
                    crate::version::VersionRange::singleton(version)
                }
            };
            (PubName(dep.clone()), range)
        })
        .collect();
    provider.add_dependencies(PubName(manifest.name.clone()), manifest.version, deps);
}

fn map_pubgrub_error(err: PubGrubError<Provider>) -> ResolveError {
    match err {
        PubGrubError::NoSolution(mut tree) => {
            tree.collapse_no_versions();
            ResolveError::NoSolution(DefaultStringReporter::report(&tree))
        }
        other => ResolveError::Solver(format!("{other:?}")),
    }
}

/// The concrete `OfflineDependencyProvider` this crate resolves against —
/// named so `PubGrubError<Provider>` and friends don't need to spell out the
/// full generic shape at every call site (pubgrub 0.4 parameterizes errors
/// and results over the whole provider type, not `(Package, Version)`
/// separately).
type Provider = OfflineDependencyProvider<PubName, crate::version::VersionRange<Version>>;

/// Newtype wrapping [`PackageName`] to satisfy pubgrub's `Package` bound
/// (`Clone + Eq + Hash + Debug + Display`). Kept private: the `Hash` impl this
/// gets from `PackageName` is only ever used inside pubgrub's own maps, never in
/// a brixpkg-observable path.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
struct PubName(PackageName);

impl fmt::Display for PubName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> Utf8PathBuf {
        let mut p = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("system temp dir must be UTF-8");
        p.push(format!(
            "brixpkg-resolve-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        p
    }

    fn publish(reg: &Registry, name: &str, version: &str, deps: &[(&str, &str)]) {
        let mut text = format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\n");
        if !deps.is_empty() {
            text.push_str("[dependencies]\n");
            for (d, r) in deps {
                text.push_str(&format!("\"{d}\" = \"{r}\"\n"));
            }
        }
        let m = Manifest::parse(&text).unwrap();
        let mut files = BTreeMap::new();
        files.insert(
            Utf8PathBuf::from("world.brix"),
            format!("package {name} @ {version}").into_bytes(),
        );
        reg.publish(&m, &files).unwrap();
    }

    #[test]
    fn resolves_a_simple_diamond() {
        let root = tmp_dir("diamond");
        let reg = Registry::open(&root).unwrap();
        // root -> a, b ; a -> c ^1 ; b -> c ^1 ; c 1.0.0, 1.1.0
        publish(&reg, "c", "1.0.0", &[]);
        publish(&reg, "c", "1.1.0", &[]);
        publish(&reg, "a", "1.0.0", &[("c", "^1.0.0")]);
        publish(&reg, "b", "1.0.0", &[("c", "^1.0.0")]);

        let root_manifest = Manifest::parse(
            "[package]\nname = \"app\"\nversion = \"1.0.0\"\n[dependencies]\na = \"^1.0.0\"\nb = \"^1.0.0\"\n",
        )
        .unwrap();
        let lock = resolve(&root_manifest, &reg, &BTreeMap::new()).unwrap();
        assert_eq!(lock.root.as_str(), "app");
        // a, b, c all resolved; c is the single shared 1.1.0.
        let c = PackageName::parse("c").unwrap();
        assert_eq!(lock.entries[&c].version, Version::new(1, 1, 0));
        assert!(lock.entries.contains_key(&PackageName::parse("a").unwrap()));
        assert!(lock.entries.contains_key(&PackageName::parse("b").unwrap()));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn conflicting_requirements_have_no_solution() {
        let root = tmp_dir("conflict");
        let reg = Registry::open(&root).unwrap();
        publish(&reg, "c", "1.0.0", &[]);
        publish(&reg, "c", "2.0.0", &[]);
        publish(&reg, "a", "1.0.0", &[("c", "^1.0.0")]);
        publish(&reg, "b", "1.0.0", &[("c", "^2.0.0")]);
        let root_manifest = Manifest::parse(
            "[package]\nname = \"app\"\nversion = \"1.0.0\"\n[dependencies]\na = \"^1.0.0\"\nb = \"^1.0.0\"\n",
        )
        .unwrap();
        assert!(matches!(
            resolve(&root_manifest, &reg, &BTreeMap::new()),
            Err(ResolveError::NoSolution(_))
        ));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn lockfile_digest_is_stable_across_repeated_resolution() {
        let root = tmp_dir("stable");
        let reg = Registry::open(&root).unwrap();
        publish(&reg, "c", "1.0.0", &[]);
        publish(&reg, "a", "1.0.0", &[("c", "^1.0.0")]);
        let root_manifest = Manifest::parse(
            "[package]\nname = \"app\"\nversion = \"1.0.0\"\n[dependencies]\na = \"^1.0.0\"\n",
        )
        .unwrap();
        let l1 = resolve(&root_manifest, &reg, &BTreeMap::new()).unwrap();
        let l2 = resolve(&root_manifest, &reg, &BTreeMap::new()).unwrap();
        assert_eq!(l1.digest(), l2.digest());
        std::fs::remove_dir_all(&root).ok();
    }
}
