//! `brix.toml` package manifest.
//!
//! The package's *identity* (name, version) is normative source-level state —
//! `PackageDecl := "package" QualIdent "@" SemVer` (Appendix D), asserted at the
//! top of the package's `.brix` file. The manifest is toolchain metadata *about*
//! that package: its dependency table, primarily. `brixpkg` cross-checks the two
//! (`Manifest::check_matches_source_decl`) rather than trusting either alone.
//!
//! TOML (de)serialization is confined to this module and [`crate::lock`] — see
//! `DEPS.md`'s `toml` entry. Nothing here is a semantic value; digests that must
//! be stable go through `brix-canon`, never through `toml`/`serde` byte output.

use std::collections::BTreeMap;
use std::fmt;

use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};

use crate::version::{
    parse_version, PackageName, PackageNameError, Version, VersionError, VersionReq,
};

/// A resolved, validated package manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub name: PackageName,
    pub version: Version,
    pub authors: Vec<String>,
    /// Dependency table, sorted by name (`BTreeMap` — Ring 0 determinism
    /// discipline: no `HashMap` in a path whose order can end up in a lockfile).
    pub dependencies: BTreeMap<PackageName, DependencySpec>,
}

/// Where a dependency's package content comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DependencySpec {
    /// Resolved from a registry (the local content-addressed registry for v0)
    /// against a version requirement.
    Registry(VersionReq),
    /// A path to a sibling package on disk, resolved directly — no registry
    /// lookup, no version negotiation (v0 scope; see `crate::resolve` docs).
    Path(Utf8PathBuf),
}

/// Errors constructing or parsing a [`Manifest`].
#[derive(Debug)]
pub enum ManifestError {
    Toml(toml::de::Error),
    TomlWrite(toml::ser::Error),
    Name(PackageNameError),
    Version(VersionError),
    DependencyVersion {
        dep: String,
        source: VersionError,
    },
    /// The in-source `package NAME @ VERSION` declaration and the manifest's
    /// `[package]` table disagree.
    SourceMismatch {
        manifest: String,
        source: String,
    },
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManifestError::Toml(e) => write!(f, "malformed brix.toml: {e}"),
            ManifestError::TomlWrite(e) => write!(f, "could not serialize manifest: {e}"),
            ManifestError::Name(e) => write!(f, "{e}"),
            ManifestError::Version(e) => write!(f, "{e}"),
            ManifestError::DependencyVersion { dep, source } => {
                write!(f, "dependency {dep:?}: {source}")
            }
            ManifestError::SourceMismatch { manifest, source } => write!(
                f,
                "brix.toml declares {manifest:?} but the source file declares {source:?}"
            ),
        }
    }
}

impl std::error::Error for ManifestError {}

/// The literal on-disk TOML shape. Kept separate from [`Manifest`] so parsing
/// (permissive, string-shaped) and validation (typed, fails closed) are two
/// distinct steps — a malformed dependency version never silently becomes a
/// wildcard.
#[derive(Serialize, Deserialize)]
struct RawManifest {
    package: RawPackage,
    #[serde(default)]
    dependencies: BTreeMap<String, RawDependency>,
}

#[derive(Serialize, Deserialize)]
struct RawPackage {
    name: String,
    version: String,
    #[serde(default)]
    authors: Vec<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum RawDependency {
    Version(String),
    Detailed {
        #[serde(default)]
        version: Option<String>,
        #[serde(default)]
        path: Option<Utf8PathBuf>,
    },
}

impl Manifest {
    /// Parse a `brix.toml` document.
    pub fn parse(toml_text: &str) -> Result<Self, ManifestError> {
        let raw: RawManifest = toml::from_str(toml_text).map_err(ManifestError::Toml)?;
        let name = PackageName::parse(&raw.package.name).map_err(ManifestError::Name)?;
        let version = parse_version(&raw.package.version).map_err(ManifestError::Version)?;
        let mut dependencies = BTreeMap::new();
        for (dep_name, raw_dep) in raw.dependencies {
            let dep_name = PackageName::parse(&dep_name).map_err(ManifestError::Name)?;
            let spec = match raw_dep {
                RawDependency::Version(v) => {
                    let req = VersionReq::parse(&v).map_err(|source| {
                        ManifestError::DependencyVersion {
                            dep: dep_name.to_string(),
                            source,
                        }
                    })?;
                    DependencySpec::Registry(req)
                }
                RawDependency::Detailed {
                    version: Some(v), ..
                } => {
                    let req = VersionReq::parse(&v).map_err(|source| {
                        ManifestError::DependencyVersion {
                            dep: dep_name.to_string(),
                            source,
                        }
                    })?;
                    DependencySpec::Registry(req)
                }
                RawDependency::Detailed {
                    version: None,
                    path: Some(p),
                } => DependencySpec::Path(p),
                RawDependency::Detailed {
                    version: None,
                    path: None,
                } => {
                    return Err(ManifestError::DependencyVersion {
                        dep: dep_name.to_string(),
                        source: VersionError::Empty,
                    })
                }
            };
            dependencies.insert(dep_name, spec);
        }
        Ok(Manifest {
            name,
            version,
            authors: raw.package.authors,
            dependencies,
        })
    }

    /// Serialize back to `brix.toml` text. Round-trips with [`Manifest::parse`]
    /// (tested below) — this is what `brix package publish` and `brix new` write.
    pub fn to_toml_string(&self) -> Result<String, ManifestError> {
        let raw = RawManifest {
            package: RawPackage {
                name: self.name.to_string(),
                version: self.version.to_string(),
                authors: self.authors.clone(),
            },
            dependencies: self
                .dependencies
                .iter()
                .map(|(name, spec)| {
                    let raw_dep = match spec {
                        DependencySpec::Registry(req) => RawDependency::Version(req.to_string()),
                        DependencySpec::Path(p) => RawDependency::Detailed {
                            version: None,
                            path: Some(p.clone()),
                        },
                    };
                    (name.to_string(), raw_dep)
                })
                .collect(),
        };
        toml::to_string_pretty(&raw).map_err(ManifestError::TomlWrite)
    }

    /// Cross-check against the package's own `package NAME @ VERSION` source
    /// declaration (Appendix D `PackageDecl`). `brixc`'s ast stage owns actually
    /// parsing that line; this takes the already-extracted `(name, version)` pair
    /// so brixpkg has no parser dependency on brix-ast.
    pub fn check_matches_source_decl(
        &self,
        source_name: &str,
        source_version: &str,
    ) -> Result<(), ManifestError> {
        let manifest_id = format!("{} @ {}", self.name, self.version);
        let source_id = format!("{source_name} @ {source_version}");
        if manifest_id == source_id {
            Ok(())
        } else {
            Err(ManifestError::SourceMismatch {
                manifest: manifest_id,
                source: source_id,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> &'static str {
        r#"
[package]
name = "demo.logistics"
version = "1.1.0"
authors = ["Ada"]

[dependencies]
"brix.stdlib" = "^1.0.0"
sibling = { path = "../sibling" }
"#
    }

    #[test]
    fn parses_manifest() {
        let m = Manifest::parse(sample()).unwrap();
        assert_eq!(m.name.as_str(), "demo.logistics");
        assert_eq!(m.version.to_string(), "1.1.0");
        assert_eq!(m.authors, vec!["Ada".to_string()]);
        assert_eq!(m.dependencies.len(), 2);
        assert!(matches!(
            m.dependencies[&PackageName::parse("brix.stdlib").unwrap()],
            DependencySpec::Registry(_)
        ));
        assert!(matches!(
            m.dependencies[&PackageName::parse("sibling").unwrap()],
            DependencySpec::Path(_)
        ));
    }

    #[test]
    fn roundtrips_through_toml() {
        let m = Manifest::parse(sample()).unwrap();
        let text = m.to_toml_string().unwrap();
        let m2 = Manifest::parse(&text).unwrap();
        assert_eq!(m, m2);
    }

    #[test]
    fn rejects_malformed_toml() {
        assert!(Manifest::parse("not valid toml {{{").is_err());
    }

    #[test]
    fn rejects_bad_dependency_version() {
        let bad = r#"
[package]
name = "x"
version = "1.0.0"
[dependencies]
y = "not-a-version"
"#;
        assert!(Manifest::parse(bad).is_err());
    }

    #[test]
    fn source_decl_cross_check() {
        let m = Manifest::parse(sample()).unwrap();
        assert!(m
            .check_matches_source_decl("demo.logistics", "1.1.0")
            .is_ok());
        assert!(m
            .check_matches_source_decl("demo.logistics", "1.1.1")
            .is_err());
    }
}
