//! Chained history digest (ADR-0002 §9.2 "State"):
//!
//! > The history digest is a chain: `h' = H(h_digest, step)`, O(1) per step.
//!
//! [`History`] holds only the *current* running digest and a step count — it
//! does not retain the sequence of past steps. `append` is therefore O(1) by
//! construction: there is nothing to rescan, because there is nothing kept
//! that a rescan could touch. This is the SOC `e = ⟨x, p, h⟩` history
//! component `h`.

use brix_canon::{Canonical, CanonWriter, Digest, Domain};

/// The running history digest chain: `h' = H(h_digest, step)`.
///
/// Deliberately minimal — it stores the current [`Digest`] and a step count,
/// nothing more. Appending never touches anything but those two fields, so
/// it costs the same whether this is step 1 or step one million.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct History {
    digest: Digest,
    len: u64,
}

impl History {
    /// The empty history: the canonical anchor every chain starts from,
    /// `Digest::of(Domain::Value, &[])` — derivable from `brix-canon` alone,
    /// no external seed needed.
    pub fn empty() -> Self {
        History {
            digest: Digest::of(Domain::Value, &[]),
            len: 0,
        }
    }

    /// Reconstruct a `History` value from a previously-recorded digest and
    /// step count (e.g. read back from a stored [`crate::intern::Handle`]-keyed
    /// snapshot, or from a replay boundary). `append`ing onto the result
    /// behaves identically to appending onto the original chain that
    /// produced `digest` — by construction, since `append` never looks past
    /// `self.digest` — which is exactly the "O(1), no rescan" property this
    /// module's tests demonstrate.
    pub fn from_digest(digest: Digest, len: u64) -> Self {
        History { digest, len }
    }

    /// The current running digest — this chain's `h`.
    pub fn digest(&self) -> Digest {
        self.digest
    }

    /// Number of steps folded into this chain so far.
    pub fn len(&self) -> u64 {
        self.len
    }

    /// Whether no step has been appended yet.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Fold `step`'s canonical bytes into the chain: `h' = H(h_digest,
    /// step)`. Reads only `self.digest` (never the steps before it), so this
    /// is O(1) regardless of how long the chain already is.
    pub fn append<S: Canonical>(&self, step: &S) -> Self {
        let mut w = CanonWriter::new();
        w.write_bytes(self.digest.as_bytes());
        w.write_bytes(&step.canon_bytes());
        History {
            digest: w.digest(Domain::Value),
            len: self.len + 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_history_has_zero_length() {
        let h = History::empty();
        assert!(h.is_empty());
        assert_eq!(h.len(), 0);
    }

    #[test]
    fn same_steps_reproduce_the_same_final_digest() {
        let a = History::empty().append(&1u64).append(&2u64).append(&3u64);
        let b = History::empty().append(&1u64).append(&2u64).append(&3u64);
        assert_eq!(a.digest(), b.digest());
        assert_eq!(a.len(), 3);
    }

    #[test]
    fn a_different_step_gives_a_different_digest() {
        let base = History::empty().append(&1u64);
        let a = base.append(&2u64);
        let b = base.append(&99u64);
        assert_ne!(a.digest(), b.digest());
    }

    #[test]
    fn a_different_history_prefix_gives_a_different_digest_even_for_the_same_step() {
        let a = History::empty().append(&1u64).append(&2u64);
        let b = History::empty().append(&7u64).append(&2u64);
        assert_ne!(
            a.digest(),
            b.digest(),
            "the same step over a different prefix must not collide"
        );
    }

    #[test]
    fn append_depends_only_on_the_current_digest_not_on_retained_history() {
        // Build a long chain, then reconstruct a `History` that starts *fresh*
        // from just its resulting digest (no memory of the 1000 steps behind
        // it) and check that appending the same next step from either one
        // produces the identical result. Since `from_digest` cannot possibly
        // retain the discarded prefix, this is only true because `append`
        // never looks past `self.digest` — i.e. it is O(1)/no-rescan by
        // construction, not merely by coincidence of this test's inputs.
        let long_chain = (0..1000u64).fold(History::empty(), |h, i| h.append(&i));
        let reconstructed = History::from_digest(long_chain.digest(), long_chain.len());

        let step = 424242u64;
        assert_eq!(
            long_chain.append(&step).digest(),
            reconstructed.append(&step).digest()
        );
    }

    #[test]
    fn append_increments_len() {
        let h = History::empty().append(&1u64).append(&2u64);
        assert_eq!(h.len(), 2);
    }
}
