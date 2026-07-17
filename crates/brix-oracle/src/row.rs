//! Rows and extents.
//!
//! `Row` is a role-name-sorted record — the oracle's tuple representation for
//! both entity rows and n-ary relation rows (Part III §1). `Extent` is the
//! contract's centerpiece: **"Extents as `BTreeMap<CanonBytes, Row>`"**. The
//! map key is a row's own canonical byte string (Appendix G record/relation
//! encoding), *not* a hash — so iterating an extent visits rows in canonical
//! order, which is exactly the order Appendix G requires for aggregation
//! (`Ord` = Appendix G byte order) and for reproducible dumps. Identity
//! hashes (`NodeId`/`EdgeId`) are computed on demand from a `Row` plus its
//! `RelationDef`, never stored as the map key.

use std::collections::{BTreeMap, BTreeSet};

use brix_canon::{CanonWriter, Canonical, ClaimId};

use crate::value::Value;

/// Canonical byte string of a row — the `Extent` map key.
pub type CanonBytes = Vec<u8>;

/// A role name. Ordinary `String` `Ord` (byte-lexicographic over the UTF-8
/// encoding for the ASCII identifiers used throughout this crate) stands in
/// for Appendix G's NFC-normalized field-name byte order; full NFC folding is
/// brix-canon's `write_ident` TODO (see brix-canon/src/lib.rs), not
/// reproduced here.
pub type RoleName = String;

/// A role-sorted record. `BTreeMap` gives the "fields sorted by canonical
/// field-name bytes" rule of Appendix G for free.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Row(pub BTreeMap<RoleName, Value>);

impl Row {
    pub fn new() -> Self {
        Row(BTreeMap::new())
    }

    pub fn insert(&mut self, role: impl Into<RoleName>, value: Value) -> &mut Self {
        self.0.insert(role.into(), value);
        self
    }

    pub fn get(&self, role: &str) -> Option<&Value> {
        self.0.get(role)
    }

    pub fn of<I: IntoIterator<Item = (RoleName, Value)>>(iter: I) -> Self {
        Row(iter.into_iter().collect())
    }
}

impl Canonical for Row {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_uint(self.0.len() as u64);
        for (name, value) in &self.0 {
            w.write_tag(name);
            value.canon_write(w);
        }
    }
}

/// Provenance attached to one live row.
///
/// Ground-kind relations (`rel`, `state rel`, `event rel`) carry `claims`:
/// the set of source assertions keeping the row alive (Part III §3,
/// `ClaimRef`). Derived relations carry `supports`: the set of rule matches
/// keeping the row alive (Part III §2 — "an edge is derived-live while at
/// least one rule match supports it"). A row can in principle carry both
/// (nothing in the kernel forbids a ground relation also being a rule head,
/// though the flagship never does this); the oracle tracks both sets
/// unconditionally so support-counting (Appendix I.3) is uniform.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct EdgeRecord {
    pub row: Row,
    pub claims: BTreeSet<ClaimId>,
    pub supports: BTreeSet<crate::provenance::SupportRef>,
}

impl EdgeRecord {
    pub fn is_live(&self) -> bool {
        !self.claims.is_empty() || !self.supports.is_empty()
    }
}

/// `BTreeMap<CanonBytes, Row>` widened with per-row provenance — the literal
/// data structure named in the lane contract. Keyed by the row's own
/// canonical bytes, so extent iteration order **is** canonical row order.
pub type Extent = BTreeMap<CanonBytes, EdgeRecord>;

/// Compute the map key for `row` — the row's Appendix-G canonical bytes.
pub fn row_key(row: &Row) -> CanonBytes {
    row.canon_bytes()
}
