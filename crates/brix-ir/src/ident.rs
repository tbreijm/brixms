//! Identifiers. `brix-ast` will eventually hand these to us (interned, spanned);
//! until that lane lands, `Ident`/`QualIdent` are plain owned strings so every
//! other module can be written and tested against a stable name type today.
//! See [`crate::frontend`] for the documented seam.

use brix_canon::{CanonWriter, Canonical};
use core::fmt;

/// A single unqualified name (`order`, `ComputedPrice`, `x`).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Ident(String);

impl Ident {
    pub fn new(s: impl Into<String>) -> Self {
        Ident(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Ident {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for Ident {
    fn from(s: &str) -> Self {
        Ident::new(s)
    }
}

impl From<String> for Ident {
    fn from(s: String) -> Self {
        Ident(s)
    }
}

/// Canonical encoding of an identifier goes through `write_ident` (App. G:
/// identifiers are NFC-normalized; see the `APP-G:` TODO in brix-canon —
/// brix-ir does not re-implement that, it only ever calls the one writer).
impl Canonical for Ident {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_ident(&self.0);
    }
}

/// A dotted qualified name (`AssignVehicle.Decision`, `sim.Now`). Segment order
/// is semantic (unlike relation role order); two `QualIdent`s are equal iff
/// their segment sequences are equal.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct QualIdent(Vec<Ident>);

impl QualIdent {
    /// A single-segment qualified name, e.g. `Order`.
    pub fn simple(s: impl Into<String>) -> Self {
        QualIdent(vec![Ident::new(s)])
    }

    /// Build from an explicit segment list (`AssignVehicle`, `Decision`).
    pub fn from_segments(segments: impl IntoIterator<Item = Ident>) -> Self {
        let v: Vec<Ident> = segments.into_iter().collect();
        debug_assert!(!v.is_empty(), "QualIdent must have at least one segment");
        QualIdent(v)
    }

    pub fn segments(&self) -> &[Ident] {
        &self.0
    }
}

impl fmt::Display for QualIdent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, seg) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(".")?;
            }
            write!(f, "{seg}")?;
        }
        Ok(())
    }
}

impl From<&str> for QualIdent {
    fn from(s: &str) -> Self {
        QualIdent::from_segments(s.split('.').map(Ident::new))
    }
}

impl Canonical for QualIdent {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_uint(self.0.len() as u64);
        for seg in &self.0 {
            seg.canon_write(w);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qual_ident_display_joins_segments() {
        let q = QualIdent::from("AssignVehicle.Decision");
        assert_eq!(q.to_string(), "AssignVehicle.Decision");
        assert_eq!(q.segments().len(), 2);
    }

    #[test]
    fn qual_ident_canon_bytes_are_length_prefixed_per_segment() {
        let a = QualIdent::from("A.B");
        let b = QualIdent::from("A.B");
        assert_eq!(a.canon_bytes(), b.canon_bytes());
        let c = QualIdent::from("AB");
        assert_ne!(
            a.canon_bytes(),
            c.canon_bytes(),
            "segment split must be observable in the encoding, not just concatenation"
        );
    }

    #[test]
    fn qual_ident_round_trips_through_display_and_from_str() {
        // #15 native rule-side-conditions: typefacts.rs's OrdinaryFnOnDerivedRel
        // bridge relies on `QualIdent::from(qi.to_string())` canon-writing
        // identically to the original `qi` (verbatim `Value::Str` round-trip,
        // no dedicated `TokenValue::QualIdent`). Verify for both single- and
        // multi-segment names.
        for original in [
            QualIdent::simple("Order"),
            QualIdent::from("AssignVehicle.Decision"),
            QualIdent::from_segments([Ident::new("a"), Ident::new("b"), Ident::new("c")]),
        ] {
            let round_tripped = QualIdent::from(original.to_string().as_str());
            assert_eq!(
                original.canon_bytes(),
                round_tripped.canon_bytes(),
                "QualIdent::from(qi.to_string()) must canon-write identically to qi"
            );
            assert_eq!(original, round_tripped);
        }
    }
}
