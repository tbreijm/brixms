//! Oracle-internal value representation.
//!
//! This is **not** the full BrixMS value language (that is `brix-ir`'s type
//! tower: Decimal, Quantity, Money, generic enums, records, ...). It is the
//! narrow, closed set of value shapes needed to prove Part III kernel
//! semantics against hand-built programs: naturals, integers (money is
//! represented in minor units as `Int`, deliberately avoiding a Decimal
//! rabbit hole that belongs to brix-ast/brix-ir), strings, booleans, the
//! three surface reference types (Part III §3), and closed enums.
//!
//! Floats are absent by construction — there is no variant for them. This
//! keeps every semantic path float-free without needing a lint to enforce it
//! (Ring0 §0: "no floats in a semantic path except behind the strict-IEEE ops
//! module"; the oracle's kernel proof does not need that module at all).
//!
//! Every `Value` implements [`Canonical`] so rows built from `Value`s hash
//! and order exactly the way Appendix G requires downstream.

use std::sync::Arc;

use brix_canon::{CanonWriter, Canonical, ClaimId, Digest, EdgeId, NodeId};

/// A value bound to a role or a rule variable.
///
/// Variant order below is this crate's own canonical tag ABI (an
/// oracle-internal encoding, not Appendix G's user-facing enum encoding).
/// Reordering variants changes the tag bytes and is a canon-relevant change.
///
/// `PartialEq`/`Eq`/`PartialOrd`/`Ord`/`Hash` are hand-implemented, not
/// derived (see [`Value::key`]): `Enum`'s `name` field must **not**
/// participate in identity. Appendix G is explicit that "enums encode by
/// declaration-order ordinal, never the variant's name" (quoted verbatim
/// in `brix_ir::pattern::Lit::Enum`'s own doc) — two `Value::Enum`s with
/// the same `(ty, ordinal)` are the same value regardless of what display
/// name each was constructed with. A derived comparison would make
/// otherwise-identical enum values silently fail to unify whenever their
/// `name` strings happened to differ — exactly what broke `OrderStatus
/// (order: o, value: Open)`-style literal matches the first time a real,
/// mechanically-adapted program (issue #24) exercised this path (a
/// hand-built `dsl.rs` program's literals and row data always used the
/// same hand-picked name string, so the bug had no way to surface before).
#[derive(Clone, Debug)]
pub enum Value {
    /// Unsigned natural, e.g. counts, ordinals, epoch instants.
    Nat(u64),
    /// Signed integer. Money is represented as minor units here (e.g. cents)
    /// — see module docs; a real `Decimal`/`Money` tower is brix-ir's job.
    Int(i64),
    Bool(bool),
    Str(String),
    /// `NodeRef<E>` (Part III §3): a reference to an entity.
    Node(NodeId),
    /// `EdgeRef<R>` (Part III §3): a reference to a relation tuple.
    Edge(EdgeId),
    /// `ClaimRef<R>` (Part III §3): opaque, retry-stable ground-assertion id.
    Claim(ClaimId),
    /// A closed enum value: type name and declaration-order ordinal (the
    /// ABI, Appendix G) are the value's identity; `name` is carried
    /// *purely* for readable dumps/why-output and never affects equality,
    /// ordering, hashing, or canonical bytes (see the type's own doc).
    /// Owned (`Arc<str>`, cheap to clone) rather than `&'static str`: a
    /// program built mechanically from parsed source (issue #24) recovers
    /// these strings from the source text at adapt time, not from string
    /// literals baked into the binary.
    Enum {
        ty: Arc<str>,
        ordinal: u32,
        name: Arc<str>,
    },
    /// The unit value, used by nullary outcomes and marker roles.
    Unit,
}

/// The identity-relevant projection of a [`Value`] — everything
/// `PartialEq`/`Ord`/`Hash`/`Canonical` actually key on. Exists solely to
/// give `Enum` an identity of `(ty, ordinal)`, excluding `name`, without
/// hand-writing five structurally-identical trait impls (`derive` handles
/// this one).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum ValueKey<'a> {
    Nat(u64),
    Int(i64),
    Bool(bool),
    Str(&'a str),
    Node(NodeId),
    Edge(EdgeId),
    Claim(ClaimId),
    Enum { ty: &'a str, ordinal: u32 },
    Unit,
}

impl Value {
    fn key(&self) -> ValueKey<'_> {
        match self {
            Value::Nat(n) => ValueKey::Nat(*n),
            Value::Int(n) => ValueKey::Int(*n),
            Value::Bool(b) => ValueKey::Bool(*b),
            Value::Str(s) => ValueKey::Str(s.as_str()),
            Value::Node(id) => ValueKey::Node(*id),
            Value::Edge(id) => ValueKey::Edge(*id),
            Value::Claim(id) => ValueKey::Claim(*id),
            Value::Enum { ty, ordinal, .. } => ValueKey::Enum {
                ty,
                ordinal: *ordinal,
            },
            Value::Unit => ValueKey::Unit,
        }
    }

    /// Best-effort ordering helper for numeric comparisons in guards
    /// (`when risk > 0.8`-style expressions use `Value::Int`/`Value::Nat`
    /// in the oracle's expression language — see `program::Expr`).
    pub fn as_i128(&self) -> Option<i128> {
        match self {
            Value::Nat(n) => Some(*n as i128),
            Value::Int(n) => Some(*n as i128),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}
impl Eq for Value {}

impl PartialOrd for Value {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Value {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.key().cmp(&other.key())
    }
}

impl std::hash::Hash for Value {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.key().hash(state)
    }
}

impl Canonical for Value {
    fn canon_write(&self, w: &mut CanonWriter) {
        match self {
            Value::Nat(n) => {
                w.write_uint(0);
                w.write_uint(*n);
            }
            Value::Int(n) => {
                w.write_uint(1);
                w.write_int(*n);
            }
            Value::Bool(b) => {
                w.write_uint(2);
                w.write_uint(*b as u64);
            }
            Value::Str(s) => {
                w.write_uint(3);
                w.write_str(s);
            }
            Value::Node(id) => {
                w.write_uint(4);
                w.write_bytes(id.digest().as_bytes());
            }
            Value::Edge(id) => {
                w.write_uint(5);
                w.write_bytes(id.digest().as_bytes());
            }
            Value::Claim(id) => {
                w.write_uint(6);
                w.write_bytes(id.digest().as_bytes());
            }
            // `name` is deliberately not written: Appendix G's canonical
            // enum encoding is `(ty, ordinal)` only (see the type's doc).
            Value::Enum { ty, ordinal, .. } => {
                w.write_uint(7);
                w.write_tag(ty);
                w.write_uint(*ordinal as u64);
            }
            Value::Unit => {
                w.write_uint(8);
            }
        }
    }
}

/// Digest a value on its own — used for match digests (Part III §9's
/// `MatchDigest`) and for hashing sub-environments.
pub fn digest_value(v: &Value, domain: brix_canon::Domain) -> Digest {
    Digest::of(domain, &v.canon_bytes())
}
