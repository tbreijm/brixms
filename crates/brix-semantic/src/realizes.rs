//! [`Realizes`] — the `Realizes(w, x, y)` proposition kind (ADR-0002 §6): a
//! canonical statement asserting `x ⟨realizes via w⟩ y`. Makes a realization
//! judgment a first-class [`crate::PropositionId`]-identified statement that
//! the kernel and realization regimes reason about, rather than a
//! distinguished ontological relation (ADR-0002 §1: "has-type is one
//! realization judgment under one regime among many").

use brix_canon::{CanonWriter, Canonical};

use crate::{ConfigId, PropositionId, WitnessId};

/// The statement `src ⟨realizes via witness⟩ dst` — `src` realizes `dst`
/// under the witness `w: src → dst`. `Canonical`-encoded so its
/// [`PropositionId`] is produced by the same [`PropositionId::of`] every
/// other canonical domain statement uses — no separate statement vocabulary
/// for realization. Frozen field order (`witness`, `src`, `dst`) is ABI.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Realizes {
    pub witness: WitnessId,
    pub src: ConfigId,
    pub dst: ConfigId,
}

impl Realizes {
    pub fn new(witness: WitnessId, src: ConfigId, dst: ConfigId) -> Self {
        Realizes { witness, src, dst }
    }

    /// The `PropositionId` of this statement — reuses [`PropositionId::of`]
    /// verbatim, the same constructor any other canonical domain statement
    /// uses (ADR-0001 §5.2).
    pub fn proposition_id(&self) -> PropositionId {
        PropositionId::of(self)
    }
}

impl Canonical for Realizes {
    fn canon_write(&self, w: &mut CanonWriter) {
        // Field order is ABI: witness, src, dst.
        self.witness.canon_write(w);
        self.src.canon_write(w);
        self.dst.canon_write(w);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RegimeId, Witness};

    fn witness_id(tag: &str) -> WitnessId {
        Witness::new(
            ConfigId::from_canon(b"a"),
            ConfigId::from_canon(b"b"),
            RegimeId::named(tag),
        )
        .id()
    }

    /// Golden vector, reproduced independently with a fresh `CanonWriter`
    /// (not via `Realizes::canon_write`), so it cannot be vacuously
    /// satisfied by the code it guards.
    #[test]
    fn realizes_proposition_id_matches_independent_reproduction() {
        let witness = witness_id("has-type@0.1");
        let src = ConfigId::from_canon(b"x");
        let dst = ConfigId::from_canon(b"y");
        let r = Realizes::new(witness, src, dst);

        let got = r.proposition_id();

        let mut w = CanonWriter::new();
        w.write_bytes(witness.digest().as_bytes());
        w.write_bytes(src.digest().as_bytes());
        w.write_bytes(dst.digest().as_bytes());
        let expected = PropositionId::from_canon(&w.finish());

        assert_eq!(got, expected);
    }

    #[test]
    fn distinct_witness_or_endpoints_give_distinct_propositions() {
        let base = Realizes::new(
            witness_id("r1"),
            ConfigId::from_canon(b"x"),
            ConfigId::from_canon(b"y"),
        );
        let other_witness = Realizes::new(
            witness_id("r2"),
            ConfigId::from_canon(b"x"),
            ConfigId::from_canon(b"y"),
        );
        let other_src = Realizes::new(
            witness_id("r1"),
            ConfigId::from_canon(b"z"),
            ConfigId::from_canon(b"y"),
        );
        let other_dst = Realizes::new(
            witness_id("r1"),
            ConfigId::from_canon(b"x"),
            ConfigId::from_canon(b"z"),
        );
        let ids = [
            base.proposition_id(),
            other_witness.proposition_id(),
            other_src.proposition_id(),
            other_dst.proposition_id(),
        ];
        for i in 0..ids.len() {
            for j in (i + 1)..ids.len() {
                assert_ne!(ids[i], ids[j], "realizes {i} and {j} collided");
            }
        }
    }

    #[test]
    fn proposition_id_is_deterministic() {
        let r = Realizes::new(
            witness_id("r"),
            ConfigId::from_canon(b"x"),
            ConfigId::from_canon(b"y"),
        );
        assert_eq!(r.proposition_id(), r.proposition_id());
    }
}
