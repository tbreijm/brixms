//! `brix.lock` — the resolved dependency closure with exact digests.
//!
//! Every identity-bearing byte in a lock entry is canon-encoded and hashed
//! through `brix-canon` ([`crate::digest`]); the TOML on-disk shape only ever
//! carries the resulting hex digests, never re-derives them from the TOML text.
//! `brixc`'s `brix run` cache key (Ring0_Build_Plan §1.9) is `canonical source ++
//! Lockfile::digest() ++ toolchain`, which is why lockfile digest *stability* —
//! same resolved set in, same digest out, regardless of resolution order — is
//! load-bearing and property-tested below.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use brix_canon::{CanonWriter, Canonical, Digest, Domain};
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

use crate::digest::ContentDigest;
use crate::version::{parse_version, PackageName, Version};

/// The current lockfile format. Bumped on any incompatible change to entry
/// shape; old lockfiles fail to parse rather than being silently reinterpreted.
pub const LOCK_FORMAT_VERSION: u32 = 1;

/// A fully resolved dependency closure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lockfile {
    pub format_version: u32,
    pub root: PackageName,
    /// Sorted by name: `BTreeMap`, per the workspace's no-`HashMap`-in paths-
    /// whose-order-is-observed discipline. Lockfile entry order *is* observed —
    /// it's part of what gets hashed for [`Lockfile::digest`].
    pub entries: BTreeMap<PackageName, LockEntry>,
}

/// One resolved package in the closure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockEntry {
    pub version: Version,
    pub source: LockSource,
    /// Content digest of the package's file tree ([`crate::digest::tree_digest`]).
    pub content_digest: ContentDigest,
    /// This entry's direct dependencies, by name (sorted — `BTreeSet`).
    pub dependencies: BTreeSet<PackageName>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LockSource {
    /// Resolved from the local content-addressed registry.
    Registry,
    /// A path dependency, resolved directly from disk.
    Path(Utf8PathBuf),
}

impl Canonical for LockSource {
    fn canon_write(&self, w: &mut CanonWriter) {
        match self {
            LockSource::Registry => {
                w.write_uint(0);
            }
            LockSource::Path(p) => {
                w.write_uint(1);
                w.write_str(p.as_str());
            }
        }
    }
}

impl Canonical for LockEntry {
    fn canon_write(&self, w: &mut CanonWriter) {
        let (major, minor, patch): (u32, u32, u32) = self.version.into();
        w.write_uint(major as u64);
        w.write_uint(minor as u64);
        w.write_uint(patch as u64);
        self.source.canon_write(w);
        w.write_bytes(self.content_digest.as_bytes());
        w.write_uint(self.dependencies.len() as u64);
        for dep in &self.dependencies {
            w.write_ident(dep.as_str());
        }
    }
}

impl LockEntry {
    /// This entry's own identity digest — what a `brix why`-style tool would
    /// print as "this exact locked package".
    pub fn digest(&self) -> Digest {
        self.canon_digest(Domain::Value)
    }
}

impl Canonical for Lockfile {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_uint(self.format_version as u64);
        w.write_ident(self.root.as_str());
        w.write_uint(self.entries.len() as u64);
        // `entries` is a BTreeMap, so this iterates in canon byte order already —
        // no separate sort needed and none possible to forget.
        for (name, entry) in &self.entries {
            w.write_ident(name.as_str());
            entry.canon_write(w);
        }
    }
}

impl Lockfile {
    /// The whole-lockfile digest fed into `brix run`'s cache key.
    pub fn digest(&self) -> Digest {
        self.canon_digest(Domain::Value)
    }

    pub fn to_toml_string(&self) -> Result<String, LockError> {
        let raw = RawLockfile {
            format_version: self.format_version,
            root: self.root.to_string(),
            digest: self.digest().to_hex(),
            package: self
                .entries
                .iter()
                .map(|(name, e)| RawLockEntry {
                    name: name.to_string(),
                    version: e.version.to_string(),
                    source: match &e.source {
                        LockSource::Registry => "registry".to_string(),
                        LockSource::Path(p) => format!("path+{p}"),
                    },
                    content_digest: e.content_digest.to_hex(),
                    dependencies: e.dependencies.iter().map(|d| d.to_string()).collect(),
                })
                .collect(),
        };
        toml::to_string_pretty(&raw).map_err(LockError::TomlWrite)
    }

    pub fn parse(text: &str) -> Result<Self, LockError> {
        let raw: RawLockfile = toml::from_str(text).map_err(LockError::Toml)?;
        if raw.format_version != LOCK_FORMAT_VERSION {
            return Err(LockError::UnsupportedFormat {
                found: raw.format_version,
                supported: LOCK_FORMAT_VERSION,
            });
        }
        let root = PackageName::parse(&raw.root).map_err(LockError::Name)?;
        let mut entries = BTreeMap::new();
        for pkg in raw.package {
            let name = PackageName::parse(&pkg.name).map_err(LockError::Name)?;
            let version = parse_version(&pkg.version).map_err(LockError::Version)?;
            let source = if let Some(path) = pkg.source.strip_prefix("path+") {
                LockSource::Path(Utf8PathBuf::from(path))
            } else if pkg.source == "registry" {
                LockSource::Registry
            } else {
                return Err(LockError::UnknownSource { text: pkg.source });
            };
            let content_digest = ContentDigest::from_hex(&pkg.content_digest).ok_or_else(|| {
                LockError::BadDigest {
                    text: pkg.content_digest.clone(),
                }
            })?;
            let mut dependencies = BTreeSet::new();
            for dep in pkg.dependencies {
                dependencies.insert(PackageName::parse(&dep).map_err(LockError::Name)?);
            }
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
        let lock = Lockfile {
            format_version: raw.format_version,
            root,
            entries,
        };
        // Self-check: a hand-edited lockfile (entry tampered with, digest left
        // stale) is rejected rather than silently trusted. Plain hex-string
        // comparison — no need to reconstruct a `Digest` from the stored text,
        // since `lock.digest()` below is freshly computed by brix-canon.
        if raw.digest.len() != 64 || !raw.digest.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(LockError::BadDigest { text: raw.digest });
        }
        let recomputed = lock.digest().to_hex();
        if raw.digest != recomputed {
            return Err(LockError::DigestMismatch {
                stored: raw.digest,
                recomputed,
            });
        }
        Ok(lock)
    }
}

#[derive(Serialize, Deserialize)]
struct RawLockfile {
    format_version: u32,
    root: String,
    /// Self-check digest over the entries below; [`Lockfile::parse`] recomputes
    /// and rejects a lockfile that was hand-edited into an inconsistent state.
    digest: String,
    #[serde(default, rename = "package")]
    package: Vec<RawLockEntry>,
}

#[derive(Serialize, Deserialize)]
struct RawLockEntry {
    name: String,
    version: String,
    source: String,
    content_digest: String,
    #[serde(default)]
    dependencies: Vec<String>,
}

#[derive(Debug)]
pub enum LockError {
    Toml(toml::de::Error),
    TomlWrite(toml::ser::Error),
    Name(crate::version::PackageNameError),
    Version(crate::version::VersionError),
    UnsupportedFormat { found: u32, supported: u32 },
    UnknownSource { text: String },
    BadDigest { text: String },
    DigestMismatch { stored: String, recomputed: String },
}

impl fmt::Display for LockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LockError::Toml(e) => write!(f, "malformed brix.lock: {e}"),
            LockError::TomlWrite(e) => write!(f, "could not serialize lockfile: {e}"),
            LockError::Name(e) => write!(f, "{e}"),
            LockError::Version(e) => write!(f, "{e}"),
            LockError::UnsupportedFormat { found, supported } => write!(
                f,
                "lockfile format {found} is not supported (this brixpkg supports {supported})"
            ),
            LockError::UnknownSource { text } => write!(f, "unknown lock source {text:?}"),
            LockError::BadDigest { text } => write!(f, "malformed digest {text:?}"),
            LockError::DigestMismatch { stored, recomputed } => write!(
                f,
                "brix.lock has been hand-edited: stored digest {stored} does not match \
                 recomputed digest {recomputed}"
            ),
        }
    }
}

impl std::error::Error for LockError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::tree_digest;
    use std::collections::BTreeMap as StdBTreeMap;

    fn sample_lockfile() -> Lockfile {
        let mut files = StdBTreeMap::new();
        files.insert(
            Utf8PathBuf::from("world.brix"),
            b"package a @ 1.0.0".to_vec(),
        );
        let mut entries = BTreeMap::new();
        entries.insert(
            PackageName::parse("brix.stdlib").unwrap(),
            LockEntry {
                version: Version::new(1, 0, 0),
                source: LockSource::Registry,
                content_digest: tree_digest(&files),
                dependencies: BTreeSet::new(),
            },
        );
        entries.insert(
            PackageName::parse("sibling").unwrap(),
            LockEntry {
                version: Version::new(0, 1, 0),
                source: LockSource::Path(Utf8PathBuf::from("../sibling")),
                content_digest: tree_digest(&files),
                dependencies: [PackageName::parse("brix.stdlib").unwrap()]
                    .into_iter()
                    .collect(),
            },
        );
        Lockfile {
            format_version: LOCK_FORMAT_VERSION,
            root: PackageName::parse("demo.logistics").unwrap(),
            entries,
        }
    }

    #[test]
    fn roundtrips_through_toml() {
        let lock = sample_lockfile();
        let text = lock.to_toml_string().unwrap();
        let parsed = Lockfile::parse(&text).unwrap();
        assert_eq!(lock, parsed);
    }

    #[test]
    fn digest_is_stable_across_rebuilds() {
        let a = sample_lockfile();
        let b = sample_lockfile();
        assert_eq!(a.digest(), b.digest());
    }

    #[test]
    fn digest_changes_if_an_entry_changes() {
        let a = sample_lockfile();
        let mut b = sample_lockfile();
        let key = PackageName::parse("sibling").unwrap();
        b.entries.get_mut(&key).unwrap().version = Version::new(0, 2, 0);
        assert_ne!(a.digest(), b.digest());
    }

    #[test]
    fn hand_edited_digest_is_rejected() {
        let lock = sample_lockfile();
        let mut text = lock.to_toml_string().unwrap();
        // Flip the stored digest string so it no longer matches the entries.
        text = text.replacen(&lock.digest().to_hex(), &"0".repeat(64), 1);
        assert!(matches!(
            Lockfile::parse(&text),
            Err(LockError::DigestMismatch { .. })
        ));
    }
}
