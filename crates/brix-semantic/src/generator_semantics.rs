//! Declared settlement semantics — the relation `ρ_g` as **canonical data**
//! rather than caller-supplied code (ADR-0020 §3 D2/D3).
//!
//! **Why this replaced a trait.** ADR-0019 made the `ReplayVerified` tag
//! earnable only by executing `realizes` over every link. That proves the
//! predicate *ran*; it does not identify *which* predicate ran. An
//! implementation returning `true` for everything passed any chain, and the
//! verified artifact's id named neither the semantics nor the registry it was
//! checked against (ADR-0019 §6 residuals 1–2).
//!
//! Removing the open trait is only possible because both production oracles
//! were already data-shaped:
//!
//! ```text
//! L3:       generator ↦ one exact (source, destination) row
//! Literal:  generator ↦ the diagonal relation {(x, x)}
//! ```
//!
//! So there is no production need for an executable predicate that can do
//! anything at all. [`GeneratorSemanticsV1`] is that vocabulary, and
//! [`GeneratorSemanticsIdV1`] gives it a content identity, which is what a
//! settlement audit receipt binds (ADR-0020 D5).
//!
//! **What this does and does not buy** (ADR-0020 §5 residuals 1–2, stated here
//! because the limit belongs next to the mechanism). A content-addressed
//! declaration makes a substituted oracle **detectable**: two audits over the
//! same chain under different declarations produce different ids. It does not
//! make the declared rows **correct**. A caller can still declare a fabricated
//! exact row — it simply earns a different id, which a consumer holding the
//! independently expected id can reject. A consumer that adopts the id shipped
//! alongside a receipt has authenticated nothing.

use std::collections::{BTreeMap, BTreeSet};

use brix_canon::{CanonWriter, Canonical};

use crate::id::digest_id;
use crate::{ConfigId, GeneratorId, GeneratorRegistry};

/// The fixed marker opening a [`GeneratorSemanticsIdV1`] preimage
/// (ADR-0020 D3 field 1). Frozen v1 ABI.
pub const GENERATOR_SEMANTICS_MARKER_V1: &[u8] = b"brix.semantic.generator-semantics";

/// The format version written into every manifest preimage (ADR-0020 D3
/// field 2). A new relation form requires **v2**; it is never appended
/// opportunistically to v1.
pub const GENERATOR_SEMANTICS_VERSION_V1: u64 = 1;

/// The declared relation `ρ_g` for one generator.
///
/// Canonical ABI ordinals — append-only, never reordered: like
/// [`crate::DecompVerification`], the ordinal here is not incidental, it
/// selects which relation was executed and therefore contributes to the
/// manifest's identity.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SettlementRelationV1 {
    /// `ρ_g` is exactly the declared finite set of `(src, dst)` rows. Nothing
    /// outside the set is realized. This is the L3 shape: one row per
    /// generator, re-derived from the plan's transition table.
    ExactRows(BTreeSet<(ConfigId, ConfigId)>),
    /// `ρ_g = {(x, x)}` — the diagonal. A closed form rather than an
    /// enumeration, because the reflexive relation over every configuration is
    /// not finitely presentable and writing it as `ExactRows` would be
    /// dishonest about what was declared. This is the literal-equality shape.
    Diagonal,
}

impl SettlementRelationV1 {
    /// Canonical ABI ordinal. Append-only; never reorder.
    const fn ordinal(&self) -> u64 {
        match self {
            SettlementRelationV1::ExactRows(_) => 0,
            SettlementRelationV1::Diagonal => 1,
        }
    }

    /// Whether this relation realizes `src → dst`.
    fn realizes(&self, src: &ConfigId, dst: &ConfigId) -> bool {
        match self {
            SettlementRelationV1::ExactRows(rows) => rows.contains(&(*src, *dst)),
            SettlementRelationV1::Diagonal => src == dst,
        }
    }
}

impl Canonical for SettlementRelationV1 {
    fn canon_write(&self, w: &mut CanonWriter) {
        match self {
            SettlementRelationV1::ExactRows(rows) => w.write_enum(self.ordinal(), |w| {
                // Row field order is ABI: src then dst. A set, so the encoding
                // is order-independent — two manifests declaring the same rows
                // in different insertion orders are the same manifest.
                w.write_set(rows.iter().map(|(src, dst)| {
                    let mut row = CanonWriter::new();
                    row.write_bytes(src.digest().as_bytes());
                    row.write_bytes(dst.digest().as_bytes());
                    row.finish()
                }));
            }),
            SettlementRelationV1::Diagonal => w.write_enum(self.ordinal(), |_| {}),
        }
    }
}

/// Why a manifest could not be used for a settlement audit (ADR-0020 D2).
///
/// Rust-side validation only — never canonically encoded, so it carries no ABI
/// ordinal. Every variant means the audit **fails closed**: no chain is
/// verified and no artifact is produced.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SemanticsError {
    /// The manifest declares no relation for a generator the chain cites.
    /// A missing generator is a refusal, never a silent `false` that could be
    /// mistaken for a checked negative.
    UndeclaredGenerator(GeneratorId),
    /// The manifest's generator-key set is not **exactly** the supplied
    /// registry's member set.
    ///
    /// Exact equality, not containment, is deliberate (ADR-0020 D2): a
    /// manifest that merely covers the links one candidate chain happens to
    /// exercise would let the receipt identify a *subset* of the audit
    /// environment while claiming to identify the environment. The receipt
    /// names the whole thing or it names nothing.
    RegistryMismatch {
        /// Declared in the manifest but absent from the registry.
        undeclared_in_registry: Vec<GeneratorId>,
        /// Present in the registry but absent from the manifest.
        missing_from_manifest: Vec<GeneratorId>,
    },
}

/// A complete declaration of the settlement relations for one audit
/// environment: one [`SettlementRelationV1`] per generator (ADR-0020 D2).
///
/// Fields are private for the same reason [`crate::Decomposition`]'s are
/// (ADR-0019 D1): the declaration *is* the authenticated object, and its id is
/// a function of exactly these rows.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct GeneratorSemanticsV1 {
    relations: BTreeMap<GeneratorId, SettlementRelationV1>,
}

impl GeneratorSemanticsV1 {
    /// An empty declaration — realizes nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Declare `relation` for `generator`, replacing any previous declaration.
    /// One relation per generator: the map shape enforces it, so a manifest
    /// cannot declare a generator twice and leave which one applies ambiguous.
    pub fn declare(&mut self, generator: GeneratorId, relation: SettlementRelationV1) {
        self.relations.insert(generator, relation);
    }

    /// Declare `generator ↦ ExactRows(rows)`.
    pub fn declare_rows(
        &mut self,
        generator: GeneratorId,
        rows: impl IntoIterator<Item = (ConfigId, ConfigId)>,
    ) {
        self.declare(
            generator,
            SettlementRelationV1::ExactRows(rows.into_iter().collect()),
        );
    }

    /// Declare `generator ↦ Diagonal`.
    pub fn declare_diagonal(&mut self, generator: GeneratorId) {
        self.declare(generator, SettlementRelationV1::Diagonal);
    }

    /// The relation declared for `generator`, if any.
    pub fn relation(&self, generator: &GeneratorId) -> Option<&SettlementRelationV1> {
        self.relations.get(generator)
    }

    /// The declared generators, in canonical order.
    pub fn generators(&self) -> impl Iterator<Item = &GeneratorId> {
        self.relations.keys()
    }

    /// How many generators this manifest declares.
    pub fn len(&self) -> usize {
        self.relations.len()
    }

    /// Whether this manifest declares nothing.
    pub fn is_empty(&self) -> bool {
        self.relations.is_empty()
    }

    /// Whether `g` realizes `src → dst` under this declaration.
    ///
    /// Fails closed on an undeclared generator: [`SemanticsError`] rather than
    /// `false`, so "I have no declaration for this" can never be mistaken for
    /// "I checked and it does not hold".
    pub fn realizes(
        &self,
        g: &GeneratorId,
        src: &ConfigId,
        dst: &ConfigId,
    ) -> Result<bool, SemanticsError> {
        match self.relations.get(g) {
            Some(relation) => Ok(relation.realizes(src, dst)),
            None => Err(SemanticsError::UndeclaredGenerator(*g)),
        }
    }

    /// Require this manifest's generator set to be **exactly** `registry`'s
    /// (ADR-0020 D2).
    ///
    /// This is what makes a receipt identify the complete audit environment
    /// rather than the subset one chain exercised. See
    /// [`SemanticsError::RegistryMismatch`].
    pub fn require_matches_registry(
        &self,
        registry: &GeneratorRegistry,
    ) -> Result<(), SemanticsError> {
        let undeclared_in_registry: Vec<GeneratorId> = self
            .relations
            .keys()
            .filter(|g| !registry.contains(g))
            .copied()
            .collect();
        let missing_from_manifest: Vec<GeneratorId> = registry
            .iter()
            .filter(|g| !self.relations.contains_key(g))
            .copied()
            .collect();

        if undeclared_in_registry.is_empty() && missing_from_manifest.is_empty() {
            Ok(())
        } else {
            Err(SemanticsError::RegistryMismatch {
                undeclared_in_registry,
                missing_from_manifest,
            })
        }
    }

    /// The content-addressed identity of this declaration (ADR-0020 D3).
    pub fn id(&self) -> GeneratorSemanticsIdV1 {
        GeneratorSemanticsIdV1::of(self)
    }
}

impl Canonical for GeneratorSemanticsV1 {
    fn canon_write(&self, w: &mut CanonWriter) {
        // Frozen v1 preimage (ADR-0020 D3): marker, version, relations map.
        // Field order and framing are ABI.
        w.write_bytes(GENERATOR_SEMANTICS_MARKER_V1);
        w.write_uint(GENERATOR_SEMANTICS_VERSION_V1);
        w.write_map(self.relations.iter().map(|(g, relation)| {
            let mut key = CanonWriter::new();
            key.write_bytes(g.digest().as_bytes());
            let mut value = CanonWriter::new();
            relation.canon_write(&mut value);
            (key.finish(), value.finish())
        }));
    }
}

digest_id!(
    /// Content-addressed identity of a whole [`GeneratorSemanticsV1`] — *which*
    /// relation declaration a settlement audit executed (ADR-0020 D3).
    ///
    /// This is the field a [`crate::Decomposition`]'s id deliberately does not
    /// carry (ADR-0019 §6 residual 2): identical chains verified under
    /// different declarations share a `DecompositionId` but not this id.
    GeneratorSemanticsIdV1
);

#[cfg(test)]
mod tests {
    use super::*;

    fn g(name: &str) -> GeneratorId {
        GeneratorId::named(name)
    }

    fn cfg(tag: &str) -> ConfigId {
        ConfigId::from_canon(tag.as_bytes())
    }

    fn registry_of(names: &[&str]) -> GeneratorRegistry {
        let mut r = GeneratorRegistry::new();
        for n in names {
            r.insert(g(n));
        }
        r
    }

    #[test]
    fn exact_rows_realize_only_declared_pairs() {
        let mut m = GeneratorSemanticsV1::new();
        m.declare_rows(g("step@1"), [(cfg("x0"), cfg("x1"))]);

        assert_eq!(m.realizes(&g("step@1"), &cfg("x0"), &cfg("x1")), Ok(true));
        assert_eq!(m.realizes(&g("step@1"), &cfg("x0"), &cfg("x9")), Ok(false));
        assert_eq!(m.realizes(&g("step@1"), &cfg("x1"), &cfg("x0")), Ok(false));
    }

    #[test]
    fn diagonal_realizes_exactly_reflexive_pairs() {
        let mut m = GeneratorSemanticsV1::new();
        m.declare_diagonal(g("refl@1"));

        assert_eq!(m.realizes(&g("refl@1"), &cfg("x"), &cfg("x")), Ok(true));
        assert_eq!(m.realizes(&g("refl@1"), &cfg("x"), &cfg("y")), Ok(false));
    }

    #[test]
    fn an_undeclared_generator_fails_closed_rather_than_returning_false() {
        // The distinction that matters: "no declaration" must not be
        // reportable as "checked, does not hold".
        let m = GeneratorSemanticsV1::new();
        assert_eq!(
            m.realizes(&g("nope@1"), &cfg("x0"), &cfg("x1")),
            Err(SemanticsError::UndeclaredGenerator(g("nope@1")))
        );
    }

    #[test]
    fn registry_agreement_requires_exact_equality_not_containment() {
        let mut m = GeneratorSemanticsV1::new();
        m.declare_diagonal(g("a@1"));

        // Superset registry: the manifest would cover a chain over `a@1` only.
        match m.require_matches_registry(&registry_of(&["a@1", "b@1"])) {
            Err(SemanticsError::RegistryMismatch {
                undeclared_in_registry,
                missing_from_manifest,
            }) => {
                assert!(undeclared_in_registry.is_empty());
                assert_eq!(missing_from_manifest, vec![g("b@1")]);
            }
            other => panic!("a covering-but-unequal manifest must be refused, got {other:?}"),
        }

        // Subset registry: the manifest declares something 𝒢 does not contain.
        match m.require_matches_registry(&registry_of(&[])) {
            Err(SemanticsError::RegistryMismatch {
                undeclared_in_registry,
                ..
            }) => assert_eq!(undeclared_in_registry, vec![g("a@1")]),
            other => panic!("an over-declaring manifest must be refused, got {other:?}"),
        }

        assert_eq!(m.require_matches_registry(&registry_of(&["a@1"])), Ok(()));
    }

    #[test]
    fn distinct_declarations_have_distinct_ids() {
        // The whole point of ADR-0020: a substituted oracle is *detectable*.
        let mut honest = GeneratorSemanticsV1::new();
        honest.declare_rows(g("step@1"), [(cfg("x0"), cfg("x1"))]);

        let mut fabricated = GeneratorSemanticsV1::new();
        fabricated.declare_rows(g("step@1"), [(cfg("x0"), cfg("forged"))]);

        assert_ne!(honest.id(), fabricated.id());

        // And a different *relation form* over the same generator differs too.
        let mut diagonal = GeneratorSemanticsV1::new();
        diagonal.declare_diagonal(g("step@1"));
        assert_ne!(honest.id(), diagonal.id());
    }

    #[test]
    fn manifest_id_is_declaration_order_independent() {
        let mut a = GeneratorSemanticsV1::new();
        a.declare_diagonal(g("a@1"));
        a.declare_rows(g("b@1"), [(cfg("x0"), cfg("x1")), (cfg("x1"), cfg("x2"))]);

        let mut b = GeneratorSemanticsV1::new();
        b.declare_rows(g("b@1"), [(cfg("x1"), cfg("x2")), (cfg("x0"), cfg("x1"))]);
        b.declare_diagonal(g("a@1"));

        assert_eq!(a.id(), b.id());
    }

    #[test]
    fn relation_ordinals_are_frozen() {
        // A reorder would silently reinterpret every manifest that declares
        // the affected form, changing what a receipt says was executed.
        assert_eq!(
            SettlementRelationV1::ExactRows(BTreeSet::new()).ordinal(),
            0
        );
        assert_eq!(SettlementRelationV1::Diagonal.ordinal(), 1);
    }
}
