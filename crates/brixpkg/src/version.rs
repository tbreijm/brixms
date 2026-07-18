//! Package versions and version requirements.
//!
//! `brixpkg` reuses [`pubgrub::version::SemanticVersion`] as its `Version` type
//! rather than hand-rolling a second one: it is `major.minor.patch` — exactly the
//! `SemVer` production in Appendix D (`PackageDecl := "package" QualIdent "@"
//! SemVer`), it already implements pubgrub's `Version` trait, and reusing it means
//! one fewer parser to keep in sync with the resolver. Pre-release/build-metadata
//! suffixes are out of scope for v0 (no spec example uses them); if App. D grows
//! them, that is a resolver-affecting grammar change and gets an erratum, not a
//! silent guess here.

use std::fmt;
use std::str::FromStr;

pub use pubgrub::Ranges as VersionRange;
pub use pubgrub::SemanticVersion as Version;

/// A package version requirement as written in a manifest's `[dependencies]`
/// table, e.g. `"^1.2.3"`, `"~1.2.3"`, `"=1.2.3"`, `">=1.2.0, <2.0.0"`, `"*"`.
///
/// This is intentionally a small, hand-rolled subset (no new dependency: `semver`
/// is not on the Ring 0 whitelist and pubgrub's own `Range` is the thing we
/// ultimately need). It compiles straight down to a [`VersionRange`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionReq {
    range: RangeExpr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RangeExpr {
    Any,
    Exact(Version),
    /// `^major.minor.patch` — compatible-with: the leftmost nonzero component may
    /// not change (standard "caret" semantics).
    Caret(Version),
    /// `~major.minor.patch` — the patch component may increase, minor/major may
    /// not (standard "tilde" semantics).
    Tilde(Version),
    /// A conjunction of `>=`, `>`, `<=`, `<` comparator bounds, comma-separated.
    Bounds(Vec<Comparator>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Comparator {
    op: Op,
    version: Version,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Ge,
    Gt,
    Le,
    Lt,
}

/// Error parsing a [`VersionReq`] or a bare [`Version`] from manifest text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VersionError {
    Empty,
    BadVersion { text: String },
    BadComparator { text: String },
    BadRequirement { text: String },
}

impl fmt::Display for VersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VersionError::Empty => write!(f, "empty version requirement"),
            VersionError::BadVersion { text } => write!(f, "invalid version {text:?}"),
            VersionError::BadComparator { text } => {
                write!(f, "invalid version comparator {text:?}")
            }
            VersionError::BadRequirement { text } => {
                write!(f, "invalid version requirement {text:?}")
            }
        }
    }
}

impl std::error::Error for VersionError {}

/// Parse an exact [`Version`] (App. D `SemVer`), mapping pubgrub's parse error
/// into brixpkg's own [`VersionError`] so callers never handle two error types.
pub fn parse_version(text: &str) -> Result<Version, VersionError> {
    Version::from_str(text.trim()).map_err(|_| VersionError::BadVersion {
        text: text.to_string(),
    })
}

impl FromStr for VersionReq {
    type Err = VersionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.is_empty() {
            return Err(VersionError::Empty);
        }
        if s == "*" {
            return Ok(VersionReq {
                range: RangeExpr::Any,
            });
        }
        if let Some(rest) = s.strip_prefix('^') {
            return Ok(VersionReq {
                range: RangeExpr::Caret(parse_version(rest)?),
            });
        }
        if let Some(rest) = s.strip_prefix('~') {
            return Ok(VersionReq {
                range: RangeExpr::Tilde(parse_version(rest)?),
            });
        }
        if let Some(rest) = s.strip_prefix('=') {
            return Ok(VersionReq {
                range: RangeExpr::Exact(parse_version(rest)?),
            });
        }
        // A bare version, e.g. "1.2.3", means exact (same convention as Cargo's
        // default before ^ became implicit — brixpkg keeps it explicit-only at
        // the call site by preferring `=`/`^` in generated manifests).
        if s.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return Ok(VersionReq {
                range: RangeExpr::Exact(parse_version(s)?),
            });
        }
        // Comparator conjunction: ">=1.2.0, <2.0.0"
        let mut comparators = Vec::new();
        for part in s.split(',') {
            let part = part.trim();
            let (op, rest) = if let Some(r) = part.strip_prefix(">=") {
                (Op::Ge, r)
            } else if let Some(r) = part.strip_prefix("<=") {
                (Op::Le, r)
            } else if let Some(r) = part.strip_prefix('>') {
                (Op::Gt, r)
            } else if let Some(r) = part.strip_prefix('<') {
                (Op::Lt, r)
            } else {
                return Err(VersionError::BadComparator {
                    text: part.to_string(),
                });
            };
            comparators.push(Comparator {
                op,
                version: parse_version(rest)?,
            });
        }
        if comparators.is_empty() {
            return Err(VersionError::BadRequirement {
                text: s.to_string(),
            });
        }
        Ok(VersionReq {
            range: RangeExpr::Bounds(comparators),
        })
    }
}

impl fmt::Display for VersionReq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.range {
            RangeExpr::Any => write!(f, "*"),
            RangeExpr::Exact(v) => write!(f, "={v}"),
            RangeExpr::Caret(v) => write!(f, "^{v}"),
            RangeExpr::Tilde(v) => write!(f, "~{v}"),
            RangeExpr::Bounds(cs) => {
                for (i, c) in cs.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    let op = match c.op {
                        Op::Ge => ">=",
                        Op::Gt => ">",
                        Op::Le => "<=",
                        Op::Lt => "<",
                    };
                    write!(f, "{op}{}", c.version)?;
                }
                Ok(())
            }
        }
    }
}

impl VersionReq {
    /// Parse from manifest text.
    pub fn parse(text: &str) -> Result<Self, VersionError> {
        text.parse()
    }

    /// Lower to the [`VersionRange`] pubgrub resolves against.
    pub fn to_range(&self) -> VersionRange<Version> {
        match &self.range {
            RangeExpr::Any => VersionRange::full(),
            RangeExpr::Exact(v) => VersionRange::singleton(*v),
            RangeExpr::Caret(v) => caret_range(*v),
            RangeExpr::Tilde(v) => tilde_range(*v),
            RangeExpr::Bounds(cs) => cs.iter().fold(VersionRange::full(), |acc, c| {
                let bound = match c.op {
                    Op::Ge => VersionRange::higher_than(c.version),
                    Op::Gt => VersionRange::higher_than(c.version.bump_patch()),
                    Op::Le => VersionRange::strictly_lower_than(c.version.bump_patch()),
                    Op::Lt => VersionRange::strictly_lower_than(c.version),
                };
                acc.intersection(&bound)
            }),
        }
    }

    /// Whether `version` satisfies this requirement.
    pub fn matches(&self, version: &Version) -> bool {
        self.to_range().contains(version)
    }
}

fn caret_range(v: Version) -> VersionRange<Version> {
    let (major, minor, patch): (u32, u32, u32) = v.into();
    let upper = if major > 0 {
        Version::new(major + 1, 0, 0)
    } else if minor > 0 {
        Version::new(0, minor + 1, 0)
    } else {
        Version::new(0, 0, patch + 1)
    };
    VersionRange::between(v, upper)
}

fn tilde_range(v: Version) -> VersionRange<Version> {
    let (major, minor, _patch): (u32, u32, u32) = v.into();
    let upper = Version::new(major, minor + 1, 0);
    VersionRange::between(v, upper)
}

/// A validated package name — a dotted qualified identifier, per Appendix D's
/// `QualIdent` production used in `PackageDecl`. `brix-canon` NFC-normalization
/// (App. G) is deferred workspace-wide (see `DEPS.md` pending justifications);
/// package names here are restricted to ASCII identifiers until that lands, so
/// there is nothing yet to normalize.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageName(String);

impl PackageName {
    pub fn parse(s: &str) -> Result<Self, PackageNameError> {
        if s.is_empty() {
            return Err(PackageNameError::Empty);
        }
        for segment in s.split('.') {
            let mut chars = segment.chars();
            let first = chars.next().ok_or_else(|| PackageNameError::Invalid {
                text: s.to_string(),
            })?;
            if !(first.is_ascii_alphabetic() || first == '_') {
                return Err(PackageNameError::Invalid {
                    text: s.to_string(),
                });
            }
            if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return Err(PackageNameError::Invalid {
                    text: s.to_string(),
                });
            }
        }
        Ok(PackageName(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PackageName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for PackageName {
    type Err = PackageNameError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        PackageName::parse(s)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PackageNameError {
    Empty,
    Invalid { text: String },
}

impl fmt::Display for PackageNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PackageNameError::Empty => write!(f, "package name must not be empty"),
            PackageNameError::Invalid { text } => {
                write!(
                    f,
                    "invalid package name {text:?} (expected a dotted identifier)"
                )
            }
        }
    }
}

impl std::error::Error for PackageNameError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_and_prefixed_versions() {
        assert_eq!(
            VersionReq::parse("1.2.3").unwrap().to_range(),
            VersionRange::singleton(Version::new(1, 2, 3))
        );
        assert_eq!(
            VersionReq::parse("=1.2.3").unwrap().to_range(),
            VersionRange::singleton(Version::new(1, 2, 3))
        );
    }

    #[test]
    fn caret_excludes_next_major() {
        let req = VersionReq::parse("^1.2.3").unwrap();
        assert!(req.matches(&Version::new(1, 2, 3)));
        assert!(req.matches(&Version::new(1, 9, 0)));
        assert!(!req.matches(&Version::new(2, 0, 0)));
        assert!(!req.matches(&Version::new(1, 2, 2)));
    }

    #[test]
    fn caret_zero_major_is_minor_locked() {
        let req = VersionReq::parse("^0.2.3").unwrap();
        assert!(req.matches(&Version::new(0, 2, 9)));
        assert!(!req.matches(&Version::new(0, 3, 0)));
    }

    #[test]
    fn tilde_locks_minor() {
        let req = VersionReq::parse("~1.2.3").unwrap();
        assert!(req.matches(&Version::new(1, 2, 9)));
        assert!(!req.matches(&Version::new(1, 3, 0)));
    }

    #[test]
    fn bounds_conjunction() {
        let req = VersionReq::parse(">=1.2.0, <2.0.0").unwrap();
        assert!(req.matches(&Version::new(1, 2, 0)));
        assert!(req.matches(&Version::new(1, 9, 9)));
        assert!(!req.matches(&Version::new(2, 0, 0)));
        assert!(!req.matches(&Version::new(1, 1, 9)));
    }

    #[test]
    fn wildcard_matches_everything() {
        let req = VersionReq::parse("*").unwrap();
        assert!(req.matches(&Version::new(0, 0, 0)));
        assert!(req.matches(&Version::new(99, 0, 0)));
    }

    #[test]
    fn rejects_garbage() {
        assert!(VersionReq::parse("").is_err());
        assert!(VersionReq::parse("banana").is_err());
        assert!(VersionReq::parse("^1.x").is_err());
    }

    #[test]
    fn package_name_rejects_leading_digit_and_empty_segment() {
        assert!(PackageName::parse("demo.logistics").is_ok());
        assert!(PackageName::parse("1demo").is_err());
        assert!(PackageName::parse("demo..logistics").is_err());
        assert!(PackageName::parse("").is_err());
    }
}
