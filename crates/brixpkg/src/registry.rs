//! The local content-addressed registry: a directory + index file (Ring0_Build_Plan
//! §1.9). Signatures and OCI distribution are post-G4 (Part XIII §4); this is the
//! v0 substrate they will sit on top of.
//!
//! On-disk layout under `root`:
//!
//! ```text
//! <root>/
//!   store/<content-digest-hex>          content-addressed package blob (crate::digest::pack)
//!   index/<package-name>.toml           per-package version index
//! ```
//!
//! A blob's name *is* its digest — [`Registry::publish`] never has to be told
//! where to put a package, and [`Registry::fetch`] never has to trust a path a
//! caller handed it.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;

use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};

use crate::digest::{self, ContentDigest};
use crate::manifest::{DependencySpec, Manifest};
use crate::version::{parse_version, PackageName, Version, VersionReq};

/// Compiler/toolchain compatibility metadata recorded at publish time, so a
/// lockfile-pinned consumer can tell which toolchain produced a package
/// (issue #44). Mirrors `brixc::cache::ToolchainId` but is defined here to keep
/// brixpkg independent of the compiler crates. Canon compatibility is folded
/// into `brixc_version` (there is no separate canon version).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Compat {
    pub brixc_version: String,
    pub rustc_version: String,
    pub target: String,
}

/// One published version of one package, as recorded in that package's index
/// file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexEntry {
    pub version: Version,
    pub content_digest: ContentDigest,
    /// The manifest's registry dependencies at publish time (name -> version
    /// requirement) — enough for a resolver to compute candidate ranges without
    /// fetching every candidate package's blob.
    pub dependencies: BTreeMap<PackageName, VersionReq>,
    pub yanked: bool,
    /// Toolchain that produced this version, if recorded (issue #44). `None`
    /// for entries published before compat metadata existed.
    pub compat: Option<Compat>,
}

/// A local content-addressed package registry.
pub struct Registry {
    root: Utf8PathBuf,
}

/// Errors from registry operations.
#[derive(Debug)]
pub enum RegistryError {
    Io(io::Error),
    Toml(toml::de::Error),
    TomlWrite(toml::ser::Error),
    Name(crate::version::PackageNameError),
    Version(crate::version::VersionError),
    Unpack(digest::UnpackError),
    /// A manifest with a `path` dependency was submitted for publishing — path
    /// dependencies only make sense inside a local workspace, never inside a
    /// content-addressed, immutable registry entry.
    PathDependencyNotPublishable {
        dependency: PackageName,
    },
    /// The version already exists in the index with a *different* content
    /// digest — the registry's immutability guarantee (Part XIII §4: packages
    /// are content-addressed).
    ImmutableVersionConflict {
        name: PackageName,
        version: Version,
    },
    /// Attempted to republish a version that was yanked.
    VersionYanked {
        name: PackageName,
        version: Version,
    },
    UnknownPackage(PackageName),
    UnknownVersion {
        name: PackageName,
        version: Version,
    },
    UnknownBlob(ContentDigest),
    BadDigest {
        name: PackageName,
        version: Version,
        text: String,
    },
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegistryError::Io(e) => write!(f, "registry I/O error: {e}"),
            RegistryError::Toml(e) => write!(f, "malformed registry index: {e}"),
            RegistryError::TomlWrite(e) => write!(f, "could not serialize registry index: {e}"),
            RegistryError::Name(e) => write!(f, "{e}"),
            RegistryError::Version(e) => write!(f, "{e}"),
            RegistryError::Unpack(e) => write!(f, "{e}"),
            RegistryError::PathDependencyNotPublishable { dependency } => write!(
                f,
                "cannot publish: dependency {dependency:?} is a path dependency, which is not \
                 publishable to a content-addressed registry"
            ),
            RegistryError::ImmutableVersionConflict { name, version } => write!(
                f,
                "{name} @ {version} is already published with a different content digest \
                 (registry entries are immutable — bump the version instead)"
            ),
            RegistryError::VersionYanked { name, version } => {
                write!(f, "{name} @ {version} was yanked and cannot be republished")
            }
            RegistryError::UnknownPackage(name) => write!(f, "no such package {name}"),
            RegistryError::UnknownVersion { name, version } => {
                write!(f, "{name} @ {version} is not published")
            }
            RegistryError::UnknownBlob(d) => write!(f, "no blob for digest {d}"),
            RegistryError::BadDigest {
                name,
                version,
                text,
            } => write!(
                f,
                "malformed content digest {text:?} for {name} @ {version} in registry index"
            ),
        }
    }
}

impl std::error::Error for RegistryError {}

impl From<io::Error> for RegistryError {
    fn from(e: io::Error) -> Self {
        RegistryError::Io(e)
    }
}

#[derive(Serialize, Deserialize, Default)]
struct RawIndex {
    #[serde(default, rename = "version")]
    versions: Vec<RawIndexEntry>,
}

#[derive(Serialize, Deserialize)]
struct RawIndexEntry {
    version: String,
    content_digest: String,
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
    #[serde(default)]
    yanked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    compat: Option<Compat>,
}

impl Registry {
    /// Open (creating if necessary) a registry rooted at `root`.
    pub fn open(root: impl Into<Utf8PathBuf>) -> Result<Self, RegistryError> {
        let root = root.into();
        fs::create_dir_all(root.join("store"))?;
        fs::create_dir_all(root.join("index"))?;
        Ok(Registry { root })
    }

    fn index_path(&self, name: &PackageName) -> Utf8PathBuf {
        self.root.join("index").join(format!("{name}.toml"))
    }

    fn blob_path(&self, d: &ContentDigest) -> Utf8PathBuf {
        self.root.join("store").join(d.to_hex())
    }

    fn read_index(&self, name: &PackageName) -> Result<RawIndex, RegistryError> {
        let path = self.index_path(name);
        if !path.exists() {
            return Ok(RawIndex::default());
        }
        let text = fs::read_to_string(&path)?;
        toml::from_str(&text).map_err(RegistryError::Toml)
    }

    fn write_index(&self, name: &PackageName, index: &RawIndex) -> Result<(), RegistryError> {
        let text = toml::to_string_pretty(index).map_err(RegistryError::TomlWrite)?;
        fs::write(self.index_path(name), text)?;
        Ok(())
    }

    /// Publish `manifest` with its file tree. Idempotent if the exact same
    /// `(version, content digest)` is republished; an error if the version
    /// already exists with different content (immutability) or was yanked.
    pub fn publish(
        &self,
        manifest: &Manifest,
        files: &BTreeMap<Utf8PathBuf, Vec<u8>>,
        compat: Option<Compat>,
    ) -> Result<ContentDigest, RegistryError> {
        let mut dependencies = BTreeMap::new();
        for (dep_name, spec) in &manifest.dependencies {
            match spec {
                DependencySpec::Registry(req) => {
                    dependencies.insert(dep_name.clone(), req.clone());
                }
                DependencySpec::Path(_) => {
                    return Err(RegistryError::PathDependencyNotPublishable {
                        dependency: dep_name.clone(),
                    })
                }
            }
        }

        let blob = digest::pack(files);
        let content_digest = digest::tree_digest(files);

        let mut index = self.read_index(&manifest.name)?;
        if let Some(existing) = index
            .versions
            .iter()
            .find(|v| v.version == manifest.version.to_string())
        {
            if existing.yanked {
                return Err(RegistryError::VersionYanked {
                    name: manifest.name.clone(),
                    version: manifest.version,
                });
            }
            if existing.content_digest != content_digest.to_hex() {
                return Err(RegistryError::ImmutableVersionConflict {
                    name: manifest.name.clone(),
                    version: manifest.version,
                });
            }
            // Idempotent republish of identical content: nothing to do but make
            // sure the blob is actually on disk (it should be already).
            let blob_path = self.blob_path(&content_digest);
            if !blob_path.exists() {
                fs::write(&blob_path, &blob)?;
            }
            return Ok(content_digest);
        }

        fs::write(self.blob_path(&content_digest), &blob)?;
        index.versions.push(RawIndexEntry {
            version: manifest.version.to_string(),
            content_digest: content_digest.to_hex(),
            dependencies: dependencies
                .iter()
                .map(|(n, r)| (n.to_string(), r.to_string()))
                .collect(),
            yanked: false,
            compat,
        });
        // Keep the index sorted by version for reproducible diffs on disk.
        index.versions.sort_by(|a, b| a.version.cmp(&b.version));
        self.write_index(&manifest.name, &index)?;
        Ok(content_digest)
    }

    /// Mark `name @ version` as yanked: it stays fetchable (existing lockfiles
    /// keep resolving) but is excluded from fresh resolution
    /// ([`Registry::versions`] reports it; callers filter).
    pub fn yank(&self, name: &PackageName, version: &Version) -> Result<(), RegistryError> {
        let mut index = self.read_index(name)?;
        let entry = index
            .versions
            .iter_mut()
            .find(|v| v.version == version.to_string())
            .ok_or_else(|| RegistryError::UnknownVersion {
                name: name.clone(),
                version: *version,
            })?;
        entry.yanked = true;
        self.write_index(name, &index)
    }

    /// All published versions of `name`, most recent last by the index's sort
    /// order, including yanked ones (callers filter for resolution).
    pub fn versions(&self, name: &PackageName) -> Result<Vec<IndexEntry>, RegistryError> {
        let index = self.read_index(name)?;
        index
            .versions
            .into_iter()
            .map(|v| {
                let version: Version = parse_version(&v.version).map_err(RegistryError::Version)?;
                let content_digest =
                    ContentDigest::from_hex(&v.content_digest).ok_or_else(|| {
                        RegistryError::BadDigest {
                            name: name.clone(),
                            version,
                            text: v.content_digest.clone(),
                        }
                    })?;
                let mut dependencies = BTreeMap::new();
                for (dep_name, req) in v.dependencies {
                    let dep_name = PackageName::parse(&dep_name).map_err(RegistryError::Name)?;
                    let req = VersionReq::parse(&req).map_err(RegistryError::Version)?;
                    dependencies.insert(dep_name, req);
                }
                Ok(IndexEntry {
                    version,
                    content_digest,
                    dependencies,
                    yanked: v.yanked,
                    compat: v.compat,
                })
            })
            .collect()
    }

    /// Fetch and unpack a published package's file tree by content digest.
    pub fn fetch(
        &self,
        digest: &ContentDigest,
    ) -> Result<BTreeMap<Utf8PathBuf, Vec<u8>>, RegistryError> {
        let path = self.blob_path(digest);
        if !path.exists() {
            return Err(RegistryError::UnknownBlob(*digest));
        }
        let bytes = fs::read(&path)?;
        crate::digest::unpack(&bytes).map_err(RegistryError::Unpack)
    }

    /// All package names with at least one published version.
    pub fn packages(&self) -> Result<BTreeSet<PackageName>, RegistryError> {
        let mut names = BTreeSet::new();
        let dir = self.root.join("index");
        if !dir.exists() {
            return Ok(names);
        }
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = Utf8Path::from_path(&entry.path())
                .expect("registry paths are UTF-8 by construction")
                .to_path_buf();
            if let Some(stem) = path.file_stem() {
                names.insert(PackageName::parse(stem).map_err(RegistryError::Name)?);
            }
        }
        Ok(names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Manifest;

    fn tmp_dir(tag: &str) -> Utf8PathBuf {
        let mut p = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("system temp dir must be UTF-8");
        p.push(format!(
            "brixpkg-test-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        p
    }

    fn manifest(name: &str, version: &str) -> Manifest {
        Manifest::parse(&format!(
            "[package]\nname = \"{name}\"\nversion = \"{version}\"\n"
        ))
        .unwrap()
    }

    fn files() -> BTreeMap<Utf8PathBuf, Vec<u8>> {
        let mut f = BTreeMap::new();
        f.insert(
            Utf8PathBuf::from("world.brix"),
            b"package a @ 1.0.0\nmodule World\n".to_vec(),
        );
        f
    }

    #[test]
    fn publish_then_fetch_roundtrips() {
        let root = tmp_dir("publish-fetch");
        let reg = Registry::open(&root).unwrap();
        let m = manifest("a", "1.0.0");
        let digest = reg.publish(&m, &files(), None).unwrap();
        let fetched = reg.fetch(&digest).unwrap();
        assert_eq!(fetched, files());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn republishing_same_content_is_idempotent() {
        let root = tmp_dir("idempotent");
        let reg = Registry::open(&root).unwrap();
        let m = manifest("a", "1.0.0");
        let d1 = reg.publish(&m, &files(), None).unwrap();
        let d2 = reg.publish(&m, &files(), None).unwrap();
        assert_eq!(d1, d2);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn republishing_different_content_is_rejected() {
        let root = tmp_dir("immutable");
        let reg = Registry::open(&root).unwrap();
        let m = manifest("a", "1.0.0");
        reg.publish(&m, &files(), None).unwrap();
        let mut other = files();
        other.insert(Utf8PathBuf::from("extra.brix"), b"stuff".to_vec());
        assert!(matches!(
            reg.publish(&m, &other, None),
            Err(RegistryError::ImmutableVersionConflict { .. })
        ));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn yank_marks_version_and_blocks_republish() {
        let root = tmp_dir("yank");
        let reg = Registry::open(&root).unwrap();
        let m = manifest("a", "1.0.0");
        reg.publish(&m, &files(), None).unwrap();
        reg.yank(&m.name, &m.version).unwrap();
        let versions = reg.versions(&m.name).unwrap();
        assert!(versions[0].yanked);
        assert!(matches!(
            reg.publish(&m, &files(), None),
            Err(RegistryError::VersionYanked { .. })
        ));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn path_dependency_is_not_publishable() {
        let root = tmp_dir("path-dep");
        let reg = Registry::open(&root).unwrap();
        let m = Manifest::parse(
            "[package]\nname = \"a\"\nversion = \"1.0.0\"\n[dependencies]\nsibling = { path = \"../sibling\" }\n",
        )
        .unwrap();
        assert!(matches!(
            reg.publish(&m, &files(), None),
            Err(RegistryError::PathDependencyNotPublishable { .. })
        ));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn versions_lists_dependencies_for_resolution() {
        let root = tmp_dir("versions");
        let reg = Registry::open(&root).unwrap();
        let m = Manifest::parse(
            "[package]\nname = \"a\"\nversion = \"1.0.0\"\n[dependencies]\nb = \"^1.0.0\"\n",
        )
        .unwrap();
        reg.publish(&m, &files(), None).unwrap();
        let versions = reg.versions(&m.name).unwrap();
        assert_eq!(versions.len(), 1);
        assert!(versions[0]
            .dependencies
            .contains_key(&PackageName::parse("b").unwrap()));
        fs::remove_dir_all(&root).ok();
    }
}
