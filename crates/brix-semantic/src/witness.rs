//! [`Witness`] — the identity of a witness `w: A → B` (ADR-0002 §6): a typed
//! correspondence between two configurations under a [`crate::RegimeId`].
//! Configurations reuse the canonical value-digest identity
//! ([`crate::ConfigId`]) verbatim; a witness is the *arrow*, not another
//! object — it does not introduce a new configuration-like value kind.

use brix_canon::{CanonWriter, Canonical};

use crate::id::digest_id;
use crate::{ConfigId, RegimeId};

/// A witness `w: src → dst` under `regime` — *which* `ρ_w` interpretation
/// this correspondence carries (literal equality, structural, entailment,
/// …). Frozen field order (`src`, `dst`, `regime`) is ABI.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Witness {
    pub src: ConfigId,
    pub dst: ConfigId,
    pub regime: RegimeId,
}

impl Witness {
    pub fn new(src: ConfigId, dst: ConfigId, regime: RegimeId) -> Self {
        Witness { src, dst, regime }
    }

    /// The content-addressed id of this witness.
    pub fn id(&self) -> WitnessId {
        WitnessId::of(self)
    }
}

impl Canonical for Witness {
    fn canon_write(&self, w: &mut CanonWriter) {
        // Field order is ABI: src, dst, regime.
        self.src.canon_write(w);
        self.dst.canon_write(w);
        self.regime.canon_write(w);
    }
}

digest_id!(
    /// Content-addressed identity of a [`Witness`].
    WitnessId
);

#[cfg(test)]
mod tests {
    use super::*;

    fn config(tag: &[u8]) -> ConfigId {
        ConfigId::from_canon(tag)
    }

    /// Golden vector, reproduced independently with a fresh `CanonWriter`
    /// (not via `Witness::canon_write`), so this cannot be vacuously
    /// satisfied by the code it guards.
    #[test]
    fn witness_canon_encoding_matches_independent_reproduction() {
        let src = config(b"config-A");
        let dst = config(b"config-B");
        let regime = RegimeId::named("has-type@0.1");
        let w = Witness::new(src, dst, regime);

        let mut got = CanonWriter::new();
        w.canon_write(&mut got);

        let mut expected = CanonWriter::new();
        expected.write_bytes(src.digest().as_bytes());
        expected.write_bytes(dst.digest().as_bytes());
        expected.write_bytes(regime.digest().as_bytes());

        assert_eq!(got.finish(), expected.finish());
    }

    #[test]
    fn distinct_src_dst_regime_give_distinct_witness_ids() {
        let a = Witness::new(config(b"A"), config(b"B"), RegimeId::named("r1"));
        let b = Witness::new(config(b"X"), config(b"B"), RegimeId::named("r1")); // src differs
        let c = Witness::new(config(b"A"), config(b"Y"), RegimeId::named("r1")); // dst differs
        let d = Witness::new(config(b"A"), config(b"B"), RegimeId::named("r2")); // regime differs
        let ids = [a.id(), b.id(), c.id(), d.id()];
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                assert_ne!(ids[i], ids[j], "witness {i} and {j} collided");
            }
        }
    }

    #[test]
    fn witness_id_is_deterministic() {
        let w = Witness::new(config(b"A"), config(b"B"), RegimeId::named("r"));
        assert_eq!(w.id(), w.id());
    }
}
