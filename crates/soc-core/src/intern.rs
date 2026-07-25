//! Canonical-digest → dense `u32` handle interner (ADR-0002 §9.2 "Interning").
//!
//! > Canonical digests are interned to dense `u32` handles; the hot loop uses
//! > handles only. Digests are computed at boundaries (commit, evidence,
//! > replay), never per candidate in the inner loop.
//!
//! `intern` is idempotent: interning the same [`Digest`] twice returns the
//! same [`Handle`] without growing the table. `resolve` is its inverse.
//! Backed by a `BTreeMap` (digest byte order is already canonical, Ring0
//! §0/App. G) plus a `Vec` for O(1) reverse lookup — deterministic, no
//! hash-iteration-order dependence, no `HashMap` (Ring0 §0, `clippy.toml`
//! `disallowed-types`).

use brix_canon::Digest;
use std::collections::BTreeMap;

/// A dense handle into an [`Interner`]. A newtype over `u32` so it cannot be
/// confused with an arbitrary integer, an index into a different interner, or
/// another crate's handle type.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Handle(u32);

impl Handle {
    /// The raw dense index, for callers that use it as a `Vec` index.
    pub fn index(self) -> usize {
        self.0 as usize
    }

    /// The raw `u32` value, for callers that need to canon-encode a handle
    /// (e.g. folding it into the history digest chain, `crate::history`).
    /// Handles are only ever meaningful relative to the [`Interner`] that
    /// minted them — encoding the raw index is correct within one run of a
    /// fixed interner, which is the only context this crate uses it in.
    pub fn raw(self) -> u32 {
        self.0
    }
}

/// A canonical-digest → dense-`u32`-handle interner (ADR-0002 §9.2).
///
/// The hot loop (candidate enumeration, the history chain, the future
/// calendar/commit step) operates on [`Handle`]s only; a [`Digest`] is
/// resolved only at a boundary — never recomputed or rehashed per candidate.
#[derive(Clone, Debug, Default)]
pub struct Interner {
    by_digest: BTreeMap<Digest, Handle>,
    by_handle: Vec<Digest>,
}

impl Interner {
    /// A fresh, empty interner.
    pub fn new() -> Self {
        Self::default()
    }

    /// Intern `digest`, returning its handle. Idempotent: interning the same
    /// digest again returns the same handle and does not grow the table.
    pub fn intern(&mut self, digest: Digest) -> Handle {
        if let Some(&h) = self.by_digest.get(&digest) {
            return h;
        }
        let h = Handle(self.by_handle.len() as u32);
        self.by_handle.push(digest);
        self.by_digest.insert(digest, h);
        h
    }

    /// Resolve a handle back to the digest it was interned from.
    ///
    /// # Panics
    /// Panics if `handle` was not produced by this interner — an
    /// internal-consistency bug (handles never escape the interner that
    /// minted them in this crate's usage), not a recoverable user error.
    pub fn resolve(&self, handle: Handle) -> Digest {
        self.by_handle[handle.index()]
    }

    /// Number of digests interned so far.
    pub fn len(&self) -> usize {
        self.by_handle.len()
    }

    /// Whether nothing has been interned yet.
    pub fn is_empty(&self) -> bool {
        self.by_handle.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brix_canon::Domain;
    use proptest::prelude::*;

    fn digest_of(tag: &str) -> Digest {
        Digest::of(Domain::Value, tag.as_bytes())
    }

    #[test]
    fn intern_is_idempotent() {
        let mut i = Interner::new();
        let d = digest_of("a");
        let h1 = i.intern(d);
        let h2 = i.intern(d);
        assert_eq!(h1, h2);
        assert_eq!(i.len(), 1, "re-interning the same digest must not grow the table");
    }

    #[test]
    fn distinct_digests_get_distinct_handles() {
        let mut i = Interner::new();
        let ha = i.intern(digest_of("a"));
        let hb = i.intern(digest_of("b"));
        assert_ne!(ha, hb);
        assert_eq!(i.len(), 2);
    }

    #[test]
    fn handle_digest_handle_roundtrips() {
        let mut i = Interner::new();
        let d = digest_of("roundtrip");
        let h = i.intern(d);
        assert_eq!(i.resolve(h), d, "resolve must invert intern");
        let h2 = i.intern(i.resolve(h));
        assert_eq!(h, h2, "interning a resolved digest must return the same handle");
    }

    #[test]
    fn empty_interner_reports_empty() {
        let i = Interner::new();
        assert!(i.is_empty());
        assert_eq!(i.len(), 0);
    }

    proptest! {
        /// Interning round-trips (handle→digest→handle) under proptest, per
        /// the Step 2 (E1) gate in Build_Plan_v3_SOC.md.
        #[test]
        fn roundtrip_property(tags in proptest::collection::vec("[a-z]{1,8}", 1..30)) {
            let mut i = Interner::new();
            let mut handles = Vec::new();
            for t in &tags {
                handles.push(i.intern(digest_of(t)));
            }
            for (t, h) in tags.iter().zip(handles.iter()) {
                let d = digest_of(t);
                prop_assert_eq!(i.resolve(*h), d);
                prop_assert_eq!(i.intern(d), *h);
            }
        }
    }
}
