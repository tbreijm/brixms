//! brixpkg — Manifest, lockfile, pubgrub resolve, content-addressed local registry.
//! Ring 0 lane owner: see OWNER.md. Spec: ../../spec/BrixMS_v9_0.md
//!
//! # Shape
//!
//! - [`manifest`] — `brix.toml` parse/validate/serialize. Package *identity*
//!   (`name @ version`) is normative source state (Appendix D `PackageDecl`); the
//!   manifest is toolchain metadata about it, cross-checked
//!   ([`manifest::Manifest::check_matches_source_decl`]).
//! - [`version`] — [`Version`] (reusing pubgrub's `SemanticVersion`, = App. D
//!   `SemVer`), a hand-rolled [`VersionReq`] (`^ ~ = >=,<` — no `semver`
//!   dependency), and [`PackageName`].
//! - [`digest`] — content digests computed *only* through `brix-canon`
//!   ([`digest::tree_digest`]), plus the [`digest::pack`]/[`digest::unpack`]
//!   blob encoding the registry stores. [`digest::ContentDigest`] is the on-disk
//!   hex form; it has no raw-bytes-to-`brix_canon::Digest` back door, so there is
//!   never a second hasher "reconstructing" a digest from text.
//! - [`lock`] — [`Lockfile`] with exact per-entry content digests and a
//!   whole-file self-check digest; [`lock::Lockfile::digest`] is one of the three
//!   inputs to `brix run`'s content-hash cache key (see `brixc`'s cache module).
//! - [`registry`] — the content-addressed local [`Registry`]: a `store/` of
//!   digest-named blobs + per-package `index/*.toml`, with `publish`/`yank`.
//! - [`resolve`] — pubgrub [`resolve`] over the registry into a [`Lockfile`].
//!
//! # Determinism
//!
//! Every observable order is canon byte order: manifests and lockfiles hold
//! `BTreeMap`/`BTreeSet`, and the whole `resolve` result is projected into a
//! `BTreeMap` so pubgrub's internal (non-`std`) hash-map order never leaks into
//! an artifact. Every stable digest goes through `brix-canon`, never `toml`.

pub mod digest;
pub mod graph;
pub mod lock;
pub mod manifest;
pub mod registry;
pub mod resolve;
pub mod version;

pub use digest::{tree_digest, ContentDigest};
pub use graph::{hydrate, HydrateError, PackageFiles, PackageGraph, LOCKFILE_NAME};
pub use lock::{LockEntry, LockSource, Lockfile, LOCK_FORMAT_VERSION};
pub use manifest::{DependencySpec, Manifest, ManifestError};
pub use registry::{IndexEntry, Registry, RegistryError};
pub use resolve::{resolve, PathPackage, ResolveError};
pub use version::{PackageName, Version, VersionReq};
