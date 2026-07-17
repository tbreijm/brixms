//! Minimal-coherent trait solving *design* (Part V §3: "Traits provide
//! constrained polymorphism with associated types; coherence per package
//! graph; no inheritance").
//!
//! This module represents the trait system; it does not yet *solve*. The
//! design is deliberately the weakest coherent one — the OWNER contract's
//! "minimal-coherent trait solving":
//!
//! - **No specialization, no overlapping impls.** Coherence is enforced by an
//!   orphan-free, non-overlap rule: at most one `impl Trait for Head` may exist
//!   across the package graph. [`TraitEnv::insert_impl`] rejects a second impl
//!   for the same `(trait, head)` key rather than picking a "more specific"
//!   one.
//! - **Plain associated types.** An impl provides exactly one type per
//!   associated-type name; no defaults, no where-clause-conditioned outputs.
//! - **No inheritance / supertraits.** A bound is a flat set of `Trait`
//!   requirements.
//!
//! Selection (given `Ty: Trait`, find the impl) is stubbed: the shape is here,
//! [`TraitEnv::select`] does the exact-head lookup a real solver would start
//! from, but unification of generic impl heads is future work.

use crate::ident::Ident;
use crate::types::Ty;
use core::fmt;

/// A trait name plus its associated-type names (order preserved for `Display`,
/// but identity is by name set).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct TraitDef {
    pub name: Ident,
    pub assoc_types: Vec<Ident>,
}

impl fmt::Display for TraitDef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "trait {}", self.name)?;
        if !self.assoc_types.is_empty() {
            write!(f, " {{ ")?;
            for (i, a) in self.assoc_types.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "type {a}")?;
            }
            write!(f, " }}")?;
        }
        Ok(())
    }
}

/// The head a trait is implemented for. Minimal: a named type constructor
/// (`Order`, `List`, `Money`). Generic-parameterized heads reduce to the head
/// constructor name for the coherence key — that is what makes "no overlapping
/// impls" a simple exact-match rule.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ImplHead(pub Ident);

impl fmt::Display for ImplHead {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One resolved associated-type binding an impl provides.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct AssocBinding {
    pub name: Ident,
    pub ty: Ty,
}

/// An `impl Trait for Head { type A = ...; ... }`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ImplDef {
    pub trait_name: Ident,
    pub head: ImplHead,
    pub assoc: Vec<AssocBinding>,
}

impl fmt::Display for ImplDef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "impl {} for {}", self.trait_name, self.head)?;
        if !self.assoc.is_empty() {
            write!(f, " {{ ")?;
            for (i, b) in self.assoc.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "type {} = {}", b.name, b.ty)?;
            }
            write!(f, " }}")?;
        }
        Ok(())
    }
}

/// A coherence violation: a second impl for a `(trait, head)` already covered.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CoherenceError {
    pub trait_name: Ident,
    pub head: ImplHead,
}

impl fmt::Display for CoherenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "overlapping impl: {} for {} already exists (no specialization, no overlap)",
            self.trait_name, self.head
        )
    }
}

/// The trait environment Γ (trait part). Impls are keyed by `(trait, head)`;
/// the key set *is* the coherence invariant. Stored in sorted `Vec`s (no
/// `HashMap` — semantic path) so iteration order is canonical.
#[derive(Default, Debug)]
pub struct TraitEnv {
    traits: Vec<TraitDef>,
    impls: Vec<ImplDef>,
}

impl TraitEnv {
    pub fn new() -> Self {
        TraitEnv::default()
    }

    pub fn insert_trait(&mut self, def: TraitDef) {
        match self.traits.binary_search_by(|t| t.name.cmp(&def.name)) {
            Ok(pos) => self.traits[pos] = def,
            Err(pos) => self.traits.insert(pos, def),
        }
    }

    /// Insert an impl, enforcing non-overlap coherence: a second impl for the
    /// same `(trait, head)` is a [`CoherenceError`], never a specialization.
    pub fn insert_impl(&mut self, def: ImplDef) -> Result<(), CoherenceError> {
        let key = (def.trait_name.clone(), def.head.clone());
        let pos = self
            .impls
            .binary_search_by(|i| (i.trait_name.clone(), i.head.clone()).cmp(&key));
        match pos {
            Ok(_) => Err(CoherenceError {
                trait_name: def.trait_name,
                head: def.head,
            }),
            Err(insert_at) => {
                self.impls.insert(insert_at, def);
                Ok(())
            }
        }
    }

    /// Select the impl for `trait_name` applied to `ty`. STUB: only exact
    /// head-constructor matching (the base case a real solver bottoms out on);
    /// generic-head unification is future work. Returns the single coherent
    /// impl or `None`.
    pub fn select(&self, trait_name: &Ident, ty: &Ty) -> Option<&ImplDef> {
        let head = head_of(ty)?;
        self.impls
            .iter()
            .find(|i| &i.trait_name == trait_name && i.head == head)
    }

    /// Resolve `<ty as Trait>::AssocName` via the selected impl. Stubbed on top
    /// of [`Self::select`].
    pub fn project_assoc(&self, trait_name: &Ident, ty: &Ty, assoc: &Ident) -> Option<&Ty> {
        let imp = self.select(trait_name, ty)?;
        imp.assoc.iter().find(|b| &b.name == assoc).map(|b| &b.ty)
    }

    pub fn traits(&self) -> &[TraitDef] {
        &self.traits
    }

    pub fn impls(&self) -> &[ImplDef] {
        &self.impls
    }
}

/// The head constructor name of a type, for exact-match impl selection.
fn head_of(ty: &Ty) -> Option<ImplHead> {
    let name = match ty {
        Ty::NodeRef(e) | Ty::EdgeRef(e) | Ty::ClaimRef(e) => e.as_str(),
        Ty::Quantity(m) => m.as_str(),
        Ty::Money(c) => c.as_str(),
        Ty::List(_) => "List",
        Ty::Vector(_) => "Vector",
        Ty::Set(_) => "Set",
        Ty::Map(_, _) => "Map",
        Ty::Bag(_) => "Bag",
        Ty::Option(_) => "Option",
        Ty::Bool => "Bool",
        Ty::Str => "String",
        _ => return None,
    };
    Some(ImplHead(Ident::new(name)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ordish() -> TraitDef {
        TraitDef {
            name: Ident::new("Ord"),
            assoc_types: vec![],
        }
    }

    #[test]
    fn second_impl_for_same_head_is_a_coherence_error() {
        let mut env = TraitEnv::new();
        env.insert_trait(ordish());
        let imp = ImplDef {
            trait_name: Ident::new("Ord"),
            head: ImplHead(Ident::new("Money")),
            assoc: vec![],
        };
        assert!(env.insert_impl(imp.clone()).is_ok());
        let err = env.insert_impl(imp).unwrap_err();
        assert_eq!(err.head.0.as_str(), "Money");
    }

    #[test]
    fn select_finds_the_unique_impl_by_head() {
        let mut env = TraitEnv::new();
        env.insert_impl(ImplDef {
            trait_name: Ident::new("Canonical"),
            head: ImplHead(Ident::new("List")),
            assoc: vec![],
        })
        .unwrap();
        let ty = Ty::list(Ty::Bool);
        assert!(env.select(&Ident::new("Canonical"), &ty).is_some());
        assert!(env.select(&Ident::new("Canonical"), &Ty::Unit).is_none());
    }

    #[test]
    fn associated_type_projection_reads_the_impl_binding() {
        let mut env = TraitEnv::new();
        env.insert_impl(ImplDef {
            trait_name: Ident::new("Collection"),
            head: ImplHead(Ident::new("List")),
            assoc: vec![AssocBinding {
                name: Ident::new("Item"),
                ty: Ty::Bool,
            }],
        })
        .unwrap();
        let got = env.project_assoc(
            &Ident::new("Collection"),
            &Ty::list(Ty::Bool),
            &Ident::new("Item"),
        );
        assert_eq!(got, Some(&Ty::Bool));
    }
}
