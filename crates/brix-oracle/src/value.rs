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

use brix_canon::{CanonWriter, Canonical, ClaimId, Digest, EdgeId, NodeId};

/// A value bound to a role or a rule variable.
///
/// Variant order below is this crate's own canonical tag ABI (an
/// oracle-internal encoding, not Appendix G's user-facing enum encoding).
/// Reordering variants changes the tag bytes and is a canon-relevant change.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
    /// A closed enum value: type name, declaration-order ordinal (the ABI,
    /// Appendix G), and variant name (carried for readable dumps/why-output).
    Enum {
        ty: &'static str,
        ordinal: u32,
        name: &'static str,
    },
    /// The unit value, used by nullary outcomes and marker roles.
    Unit,
}

impl Value {
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
            Value::Enum { ty, ordinal, name } => {
                w.write_uint(7);
                w.write_tag(ty);
                w.write_uint(*ordinal as u64);
                w.write_tag(name);
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
