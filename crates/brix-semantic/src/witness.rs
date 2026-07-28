//! [`Witness`] — the identity of a witness `w: A → B` (ADR-0002 §6): a typed
//! correspondence between two configurations under a [`crate::RegimeId`].
//! Configurations reuse the canonical value-digest identity
//! ([`crate::ConfigId`]) verbatim; a witness is the *arrow*, not another
//! object — it does not introduce a new configuration-like value kind.

use brix_canon::{CanonWriter, Canonical};

use crate::id::digest_id;
use crate::{ConfigId, GeneratorId, RegimeId};

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

impl WitnessId {
    /// The canonical identity of the composite witness `outer ∘ inner`.
    pub fn compose(outer: WitnessId, inner: WitnessId) -> WitnessId {
        compose(outer, inner)
    }

    /// The committed-witness identity of a generator decomposition `[g_1, g_2, ..., g_n]`.
    pub fn compose_chain(generators: &[GeneratorId]) -> Option<WitnessId> {
        compose_chain(generators)
    }
}

/// Dedicated composition tag for canonical witness identity hashing.
///
/// **Frozen ABI tag.** Canonical witness composition hashes under [`brix_canon::Domain::Value`]
/// using `write_tag(WITNESS_COMPOSE_TAG)` followed by `outer`'s digest bytes and `inner`'s
/// digest bytes.
pub const WITNESS_COMPOSE_TAG: &str = "brix.semantic.WitnessId.compose";

/// The canonical identity of the composite witness `outer ∘ inner` (apply `inner` first, then `outer`).
///
/// **Encoding (Frozen ABI):**
/// - `write_tag("brix.semantic.WitnessId.compose")`
/// - `outer` digest bytes (`write_bytes`)
/// - `inner` digest bytes (`write_bytes`)
/// - Digest payload under [`brix_canon::Domain::Value`].
///
/// Composition is non-commutative (`outer ∘ inner != inner ∘ outer`) and
/// non-associative at the identity digest level.
pub fn compose(outer: WitnessId, inner: WitnessId) -> WitnessId {
    let mut w = CanonWriter::new();
    w.write_tag(WITNESS_COMPOSE_TAG);
    w.write_bytes(outer.digest().as_bytes());
    w.write_bytes(inner.digest().as_bytes());
    WitnessId::from_canon(&w.finish())
}

/// The committed-witness identity of a generator decomposition `[g_1, g_2, ..., g_n]`.
///
/// Converts each [`GeneratorId`] to its [`WitnessId`] identity (a generator is a primitive
/// witness whose witness identity is its own digest) and folds left-nested:
/// `compose(g_n, compose(g_{n-1}, ... compose(g_2, g_1)...))`.
///
/// - For `n == 0`: returns `None`.
/// - For `n == 1`: returns `Some(g_1.witness_id())` (no composition wrapper).
/// - For `n >= 2`: returns `Some(...)` folded left-nested as specified.
pub fn compose_chain(generators: &[GeneratorId]) -> Option<WitnessId> {
    let first = generators.first()?;
    let mut acc = WitnessId::from(*first);
    for g in &generators[1..] {
        acc = compose(WitnessId::from(*g), acc);
    }
    Some(acc)
}

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

    #[test]
    fn generator_id_to_witness_id_conversion() {
        let g = GeneratorId::named("gen-step");
        let w_id_method = g.witness_id();
        let w_id_from: WitnessId = g.into();
        assert_eq!(w_id_method, w_id_from);
        assert_eq!(w_id_method.digest(), g.digest());
    }

    #[test]
    fn compose_is_non_commutative() {
        let a = WitnessId::from_canon(b"witness-A");
        let b = WitnessId::from_canon(b"witness-B");
        assert_ne!(compose(a, b), compose(b, a));
    }

    #[test]
    fn compose_chain_empty_returns_none() {
        assert_eq!(compose_chain(&[]), None);
    }

    #[test]
    fn compose_chain_single_generator_has_no_wrapper() {
        let g1 = GeneratorId::named("gen-1");
        let res = compose_chain(&[g1]);
        assert_eq!(res, Some(g1.witness_id()));
    }

    #[test]
    fn compose_chain_two_generators_equals_compose() {
        let g1 = GeneratorId::named("gen-1");
        let g2 = GeneratorId::named("gen-2");
        let res = compose_chain(&[g1, g2]);
        let expected = compose(g2.witness_id(), g1.witness_id());
        assert_eq!(res, Some(expected));
    }

    #[test]
    fn compose_canon_encoding_matches_independent_reproduction() {
        let a = WitnessId::from_canon(b"witness-A");
        let b = WitnessId::from_canon(b"witness-B");
        let got = compose(a, b);

        let mut expected = CanonWriter::new();
        expected.write_tag(WITNESS_COMPOSE_TAG);
        expected.write_bytes(a.digest().as_bytes());
        expected.write_bytes(b.digest().as_bytes());
        let expected_id = WitnessId::from_canon(&expected.finish());

        assert_eq!(got, expected_id);
    }

    /// Frozen golden vector for `compose(a, b)` over fixed inputs
    /// `a = WitnessId::from_canon(b"witness-A")` and `b = WitnessId::from_canon(b"witness-B")`.
    #[test]
    fn golden_vector_witness_compose() {
        const GOLDEN_WITNESS_COMPOSE_HEX: &str =
            "d16fc95948ae9138a3749a6da8327b7c6e279480da220b4fb48e67a7c8b067fc";
        let a = WitnessId::from_canon(b"witness-A");
        let b = WitnessId::from_canon(b"witness-B");
        let comp = compose(a, b);
        assert_eq!(comp.to_hex(), GOLDEN_WITNESS_COMPOSE_HEX);
    }

    /// Frozen golden vector for `compose_chain` over fixed 3-generator list
    /// `[GeneratorId::named("gen-1"), GeneratorId::named("gen-2"), GeneratorId::named("gen-3")]`.
    #[test]
    fn golden_vector_witness_compose_chain_3() {
        const GOLDEN_WITNESS_COMPOSE_CHAIN_HEX: &str =
            "ce73829558bda6a6013993d85c7b4cc3c3a3cf49995b842bf7dd56a17993557d";
        let g1 = GeneratorId::named("gen-1");
        let g2 = GeneratorId::named("gen-2");
        let g3 = GeneratorId::named("gen-3");
        let chain_id = compose_chain(&[g1, g2, g3]).unwrap();
        // Verify explicit fold equality: compose(g3, compose(g2, g1))
        let expected = compose(g3.witness_id(), compose(g2.witness_id(), g1.witness_id()));
        assert_eq!(chain_id, expected);
        assert_eq!(chain_id.to_hex(), GOLDEN_WITNESS_COMPOSE_CHAIN_HEX);
    }
}
