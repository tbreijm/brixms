//! The `brix run` content-hash cache key.
//!
//! `brix run` must feel like a REPL — a warm rebuild is a cache *hit* in under
//! 100 ms (Ring0_Build_Plan §1.9, OWNER.md conformance). The cache is keyed so
//! that the key changes **exactly** when the generated binary would differ and
//! never otherwise. Per spec Part XXVIII §28.1 a `ProgramRevision` digest "covers
//! canonical source + lockfile, never binaries", and §26.8 "Quality determinism"
//! (conformance §I "given identical source, lockfile, profile, compiler and
//! target …") enumerates the full determinism basis. This module makes that basis
//! the literal cache key:
//!
//! ```text
//! CacheKey = Digest_value(
//!     canon( canonical_source_digest    // brix fmt output, hashed by brixc's ast lane
//!            ++ lockfile_digest          // brixpkg::Lockfile::digest()
//!            ++ toolchain_id             // brixc + rustc + target + profile
//!            ++ profile ))
//! ```
//!
//! Every component is itself a `brix-canon` digest or a canon-encoded string, so
//! there is exactly one hashing surface. Nothing about wall-clock time, absolute
//! paths, or environment leaks in — two machines with the same four inputs
//! compute the same key (the G3 "bit-identical on two machines" gate).

use brix_canon::{CanonWriter, Canonical, Digest, Domain};

/// Identifies the compilation toolchain precisely enough that a toolchain
/// upgrade invalidates the cache (Part XXVIII §28.2: "moving toolchains is
/// deliberate, never ambient"). Ring 1 upgrades the pinned toolchain explicitly;
/// this is what makes a silent `rustc` bump a cache miss rather than a
/// reproducibility bug.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolchainId {
    /// brixc's own version (pass-1 semantics live here — Part XXVIII §28.1).
    pub brixc_version: String,
    /// The pinned `rustc` version driving pass 2.
    pub rustc_version: String,
    /// Target triple, e.g. `aarch64-apple-darwin`.
    pub target: String,
}

impl Canonical for ToolchainId {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_str(&self.brixc_version);
        w.write_str(&self.rustc_version);
        w.write_str(&self.target);
    }
}

/// The build profile. `brix run` uses [`Profile::Run`] (opt-0, fast rebuild);
/// `brix serve --release` uses [`Profile::Serve`] (LLVM + LTO). They must never
/// share a cache slot — same source, different binary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    Run,
    Serve,
}

impl Profile {
    fn tag(self) -> &'static str {
        match self {
            Profile::Run => "run",
            Profile::Serve => "serve",
        }
    }
}

impl Canonical for Profile {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_tag(self.tag());
    }
}

/// The hermetic build-input contract (issue #41, spec §26.8 / conformance §I):
/// these four fields are the **complete** identity of a build. Two builds that
/// agree on all of them must produce byte-identical generated artifacts and
/// identical canonical results; two that differ in any one must not share a
/// cache slot. The G3 five-tuple *{canonical source, lockfile, compiler/runtime
/// toolchain version, profile, target}* maps here exactly — `target` lives
/// inside [`ToolchainId`] alongside the brixc/rustc versions. Nothing else may
/// enter the key: no wall-clock, no absolute paths, no ambient environment (see
/// this module's header). Adding a genuinely new determinism input means adding
/// a field here *and* to [`CacheKey::compute`]'s hashed sequence — never
/// consulting state outside this struct.
#[derive(Clone, Debug)]
pub struct CacheInputs {
    /// Digest of the canonical (`brix fmt`) source of the whole program. Supplied
    /// by brixc's ast lane; taking a digest here (not raw source) keeps this
    /// module independent of brix-ast's not-yet-merged types.
    pub canonical_source: Digest,
    /// Digest of the resolved lockfile — `brixpkg::Lockfile::digest()`.
    pub lockfile: Digest,
    pub toolchain: ToolchainId,
    pub profile: Profile,
}

/// A cache key: the digest under which a built artifact is stored and looked up.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct CacheKey(Digest);

impl CacheKey {
    /// Compute the key from its inputs. Deterministic and total.
    pub fn compute(inputs: &CacheInputs) -> Self {
        let mut w = CanonWriter::new();
        // Order is part of the ABI: never reorder these without treating it as a
        // cache-format change (all existing entries become misses, which is safe
        // but wasteful, so it wants an explicit version bump if it ever happens).
        w.write_bytes(inputs.canonical_source.as_bytes());
        w.write_bytes(inputs.lockfile.as_bytes());
        inputs.toolchain.canon_write(&mut w);
        inputs.profile.canon_write(&mut w);
        CacheKey(w.digest(Domain::Value))
    }

    /// The underlying digest.
    pub fn digest(&self) -> Digest {
        self.0
    }

    /// Lowercase hex — the on-disk cache directory / entry name.
    pub fn to_hex(&self) -> String {
        self.0.to_hex()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs() -> CacheInputs {
        CacheInputs {
            canonical_source: Digest::of(Domain::Value, b"source-a"),
            lockfile: Digest::of(Domain::Value, b"lock-a"),
            toolchain: ToolchainId {
                brixc_version: "0.0.0".into(),
                rustc_version: "1.96.1".into(),
                target: "aarch64-apple-darwin".into(),
            },
            profile: Profile::Run,
        }
    }

    #[test]
    fn key_is_deterministic() {
        assert_eq!(CacheKey::compute(&inputs()), CacheKey::compute(&inputs()));
    }

    #[test]
    fn source_change_changes_key() {
        let a = CacheKey::compute(&inputs());
        let mut i = inputs();
        i.canonical_source = Digest::of(Domain::Value, b"source-b");
        assert_ne!(a, CacheKey::compute(&i));
    }

    #[test]
    fn lock_change_changes_key() {
        let a = CacheKey::compute(&inputs());
        let mut i = inputs();
        i.lockfile = Digest::of(Domain::Value, b"lock-b");
        assert_ne!(a, CacheKey::compute(&i));
    }

    #[test]
    fn toolchain_change_changes_key() {
        let a = CacheKey::compute(&inputs());
        let mut i = inputs();
        i.toolchain.rustc_version = "1.97.0".into();
        assert_ne!(a, CacheKey::compute(&i));
    }

    #[test]
    fn profile_change_changes_key() {
        let a = CacheKey::compute(&inputs());
        let mut i = inputs();
        i.profile = Profile::Serve;
        assert_ne!(a, CacheKey::compute(&i));
    }

    #[test]
    fn field_swap_is_not_a_collision() {
        // Swapping which digest is "source" vs "lock" must change the key —
        // guards against a length-unprefixed concatenation bug.
        let mut i = inputs();
        let s = i.canonical_source;
        let l = i.lockfile;
        i.canonical_source = l;
        i.lockfile = s;
        assert_ne!(CacheKey::compute(&inputs()), CacheKey::compute(&i));
    }
}
