//! The **literal-equality** realization regime — the simplest possible
//! `ρ_w` (ADR-0002 §7; `Build_Plan_v3_SOC.md` Step 5(a)).
//!
//! `ρ_w` here is the **diagonal**: `x ⊨_w y` iff `x` and `y` are canonically
//! equal — under interning, that is exactly `x == y`, the same
//! [`soc_core::Handle`]. Concretely: for every configuration this regime has
//! been told is "known" in the world, it proposes exactly one candidate — the
//! **reflexive** witness `x → x` — under [`RegimeId::named("literal-equality@1")`].
//!
//! **Why registration, not universal enumeration.** A genuinely naive
//! `Regime::candidates` would have to enumerate *every* configuration in the
//! world and propose `x → x` for each; this crate has no notion of "every
//! configuration" (that lives in whatever client owns the world), so instead
//! the regime is told, once, which configurations to reflexively relate
//! ([`LiteralEqualityRegime::register`]) — exactly the "config(s) it knows
//! about in the world" the Lane 1 brief calls for. This keeps the regime
//! honest (real reflexive-equality semantics, checked against the interner)
//! without inventing a walk over an unbounded world this crate doesn't own.
//!
//! **Why registration needs `&mut Interner`.** [`soc_core::regime::Candidate`]
//! carries only interned [`soc_core::Handle`]s; [`soc_core::commit::commit_tick`]
//! resolves a committed candidate's `witness` handle straight to a
//! [`brix_semantic::WitnessId`] via `interner.resolve(candidate.witness)` —
//! there is no re-hashing at the commit boundary (see that function's doc
//! comment). So for an independent rebuild of the committed `Judgement` to
//! match (the property `soc-core`'s own `commit` tests call
//! "observation_judgement_digest_matches_an_independently_rebuilt_judgement"),
//! the handle this regime emits as a candidate's `witness` MUST already
//! resolve to exactly `Witness::new(x, x, regime_id).id()`'s digest — not a
//! placeholder, not a fresh unrelated intern. [`register`] is where that
//! canonical witness digest is computed and interned, once per configuration,
//! at construction time (never inside the hot `candidates` path, per
//! ADR-0002 §9.2 "digests computed at boundaries, never per candidate in the
//! inner loop").

use std::collections::BTreeMap;

use brix_semantic::{ConfigId, Decomposition, GeneratorId, RegimeId, Witness};

use soc_core::audit::GeneratorSemantics;
use soc_core::commit::SettlementRegime;
use soc_core::exec::ExecConfig;
use soc_core::intern::{Handle, Interner};
use soc_core::regime::{Candidate, Regime};

/// A configuration this regime has been [`register`](LiteralEqualityRegime::register)ed
/// for: its reflexive witness handle (already interned to
/// `Witness::new(config, config, regime_id).id()`'s digest) plus the
/// configuration's own canonical [`ConfigId`] (cached so [`decompose`]
/// doesn't need interner access — see that method's doc).
#[derive(Clone, Copy, Debug)]
struct Known {
    witness_handle: Handle,
    config: ConfigId,
}

/// The literal-equality realization regime (ADR-0002 §7; module docs). Its
/// `ρ_w` is the diagonal relation over whichever configurations it has been
/// [`register`](LiteralEqualityRegime::register)ed for.
#[derive(Clone, Debug)]
pub struct LiteralEqualityRegime {
    regime_handle: Handle,
    regime_id: RegimeId,
    known: BTreeMap<Handle, Known>,
}

impl LiteralEqualityRegime {
    /// This regime's canonical name, versioned per ADR-0002 §6
    /// (`RegimeId::named`'s `name@version` convention).
    pub const NAME: &'static str = "literal-equality@1";

    /// The single generator this regime's tight decomposition ever cites: the
    /// reflexive-equality primitive `ρ_{literal.refl}`, the diagonal relation
    /// `{(x, x)}`.
    pub const GENERATOR_NAME: &'static str = "literal-equality.refl@1";

    /// Construct a fresh regime, interning its own [`RegimeId`] digest.
    /// Starts with no known configurations — call [`register`](Self::register)
    /// for each configuration this regime should propose the reflexive
    /// witness for.
    pub fn new(interner: &mut Interner) -> Self {
        let regime_id = RegimeId::named(Self::NAME);
        let regime_handle = interner.intern(regime_id.digest());
        LiteralEqualityRegime {
            regime_handle,
            regime_id,
            known: BTreeMap::new(),
        }
    }

    /// This regime's canonical identity.
    pub fn regime_id(&self) -> RegimeId {
        self.regime_id
    }

    /// Register `config` (an interned configuration handle — typically an
    /// `ExecConfig::world`) as known to this regime: computes and interns its
    /// reflexive witness `Witness::new(x, x, regime_id)` once (idempotent —
    /// re-registering the same handle is a no-op), so
    /// [`Regime::candidates`]/[`SettlementRegime::decompose`] can look it up
    /// without ever hashing inside the hot enumeration path.
    ///
    /// # Panics
    /// Panics if `config` was not interned by `interner` — the same
    /// internal-consistency contract [`Interner::resolve`] documents.
    pub fn register(&mut self, interner: &mut Interner, config: Handle) {
        if self.known.contains_key(&config) {
            return;
        }
        let config_id = ConfigId(interner.resolve(config));
        let witness = Witness::new(config_id, config_id, self.regime_id);
        let witness_handle = interner.intern(witness.id().digest());
        self.known.insert(
            config,
            Known {
                witness_handle,
                config: config_id,
            },
        );
    }
}

impl Regime for LiteralEqualityRegime {
    /// Enumerate the reflexive candidate `x → x` for every configuration
    /// registered for `e.world` — naive by construction (no incremental
    /// state beyond the registration table), matching this crate's Step 5(a)
    /// scope. Unregistered worlds propose nothing: this regime never invents
    /// a witness for a configuration it was not told about (module docs).
    fn candidates(&self, e: &ExecConfig) -> Vec<Candidate> {
        match self.known.get(&e.world) {
            Some(known) => vec![Candidate {
                regime: self.regime_handle,
                witness: known.witness_handle,
                // Reflexive: the successor IS the source, x → x.
                successor: e.world,
            }],
            None => vec![],
        }
    }
}

impl SettlementRegime for LiteralEqualityRegime {
    /// The tight decomposition realizing a reflexive candidate is a single
    /// generator step over the diagonal: `x =[literal-equality.refl@1]=> x`
    /// — one generator, the two (identical) endpoints `[x, x]`.
    ///
    /// # Panics
    /// Panics if `c` was not produced by [`Self::candidates`] for `e.world`
    /// registered on this regime (an internal-consistency bug: `commit_tick`
    /// only calls `decompose` on a candidate this same regime just
    /// enumerated).
    fn decompose(&self, e: &ExecConfig, _c: &Candidate) -> Decomposition {
        let known = self
            .known
            .get(&e.world)
            .expect("decompose called on a candidate this regime did not enumerate for e.world");
        Decomposition::recorded(
            vec![GeneratorId::named(Self::GENERATOR_NAME)],
            vec![known.config, known.config],
        )
        .expect("a 1-generator, 2-identical-config reflexive chain is always a valid Decomposition")
    }
}

/// The generator semantics for [`LiteralEqualityRegime::GENERATOR_NAME`]: the
/// diagonal relation `ρ_{literal.refl} = {(x, x)}`. `realizes(g, src, dst)`
/// is true iff `g` names this regime's reflexive generator **and** `src ==
/// dst` — genuinely checking the equality semantics the regime's name
/// promises, not a hardcoded pass.
#[derive(Clone, Copy, Debug, Default)]
pub struct LiteralEqualitySemantics;

impl GeneratorSemantics for LiteralEqualitySemantics {
    fn realizes(&self, g: &GeneratorId, src: &ConfigId, dst: &ConfigId) -> bool {
        *g == GeneratorId::named(LiteralEqualityRegime::GENERATOR_NAME) && src == dst
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brix_canon::{Digest, Domain};

    fn setup() -> (Interner, Handle, LiteralEqualityRegime) {
        let mut interner = Interner::new();
        let world = interner.intern(Digest::of(Domain::Value, b"world"));
        let mut regime = LiteralEqualityRegime::new(&mut interner);
        regime.register(&mut interner, world);
        (interner, world, regime)
    }

    #[test]
    fn regime_id_is_the_named_literal_equality_id() {
        let mut i = Interner::new();
        let r = LiteralEqualityRegime::new(&mut i);
        assert_eq!(r.regime_id(), RegimeId::named(LiteralEqualityRegime::NAME));
    }

    #[test]
    fn an_unregistered_world_proposes_nothing() {
        let mut i = Interner::new();
        let regime = LiteralEqualityRegime::new(&mut i);
        let other_world = i.intern(Digest::of(Domain::Value, b"unregistered"));
        let policy = i.intern(Digest::of(Domain::Value, b"policy"));
        let e = ExecConfig::new(other_world, policy, Digest::of(Domain::Value, b"history"));
        assert!(regime.candidates(&e).is_empty());
    }

    #[test]
    fn a_registered_world_proposes_exactly_one_reflexive_candidate() {
        let (mut i, world, regime) = setup();
        let policy = i.intern(Digest::of(Domain::Value, b"policy"));
        let e = ExecConfig::new(world, policy, Digest::of(Domain::Value, b"history"));

        let cs = regime.candidates(&e);
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].regime, regime.regime_handle);
        assert_eq!(cs[0].successor, world, "reflexive: successor is the source");
    }

    #[test]
    fn the_witness_handle_resolves_to_the_canonical_reflexive_witness_digest() {
        // Independently rebuild Witness::new(x, x, regime_id).id() and check
        // the candidate's witness handle resolves to exactly that digest —
        // the property commit_tick's commit boundary depends on (module
        // docs).
        let (mut i, world, regime) = setup();
        let policy = i.intern(Digest::of(Domain::Value, b"policy"));
        let e = ExecConfig::new(world, policy, Digest::of(Domain::Value, b"history"));
        let cs = regime.candidates(&e);

        let config_id = ConfigId(i.resolve(world));
        let expected_witness = Witness::new(config_id, config_id, regime.regime_id()).id();

        assert_eq!(i.resolve(cs[0].witness), expected_witness.digest());
    }

    #[test]
    fn registering_the_same_handle_twice_is_idempotent() {
        let mut i = Interner::new();
        let world = i.intern(Digest::of(Domain::Value, b"world"));
        let mut regime = LiteralEqualityRegime::new(&mut i);
        regime.register(&mut i, world);
        let before = regime.known.get(&world).copied().unwrap();
        regime.register(&mut i, world);
        let after = regime.known.get(&world).copied().unwrap();
        assert_eq!(before.witness_handle, after.witness_handle);
        assert_eq!(before.config, after.config);
    }

    #[test]
    fn decompose_returns_a_single_reflexive_generator_step() {
        let (mut i, world, regime) = setup();
        let policy = i.intern(Digest::of(Domain::Value, b"policy"));
        let e = ExecConfig::new(world, policy, Digest::of(Domain::Value, b"history"));
        let cs = regime.candidates(&e);

        let decomposition = regime.decompose(&e, &cs[0]);
        assert_eq!(decomposition.generators.len(), 1);
        assert_eq!(
            decomposition.generators[0],
            GeneratorId::named(LiteralEqualityRegime::GENERATOR_NAME)
        );
        assert_eq!(decomposition.configs.len(), 2);
        assert_eq!(decomposition.configs[0], decomposition.configs[1]);
    }

    #[test]
    fn semantics_realizes_the_diagonal_only() {
        let sem = LiteralEqualitySemantics;
        let g = GeneratorId::named(LiteralEqualityRegime::GENERATOR_NAME);
        let x = ConfigId::from_canon(b"x");
        let y = ConfigId::from_canon(b"y");

        assert!(sem.realizes(&g, &x, &x), "x realizes x under the diagonal");
        assert!(
            !sem.realizes(&g, &x, &y),
            "x must not realize a distinct y under the diagonal"
        );
        let other_g = GeneratorId::named("some-other-generator@1");
        assert!(
            !sem.realizes(&other_g, &x, &x),
            "a different generator name must not be realized by this semantics"
        );
    }
}
