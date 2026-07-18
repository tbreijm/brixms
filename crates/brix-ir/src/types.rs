//! The type system representation (Part V §2-3, Appendix E `Key` judgment,
//! Appendix G canonical encoding rules).
//!
//! `Ty` is a plain data representation, not an inference engine: unification,
//! generalization, and row-variable solving are out of scope for this bounded
//! deliverable (OWNER.md's "minimal-coherent trait solving design... even if
//! inference is stubbed" applies equally here — [`TyVar`]/[`RowTail::Open`]
//! exist so the shape is right, [`crate::traits`] documents the solver
//! contract, and a real unifier is later work against this same `Ty`).
//!
//! No `f64`/`f32` *values* ever live in this module (or anywhere in brix-ir):
//! `F32`/`F64` are type tags only, never carried values, so `Ty` can derive
//! `Eq`/`Ord` honestly and stay out of the float-determinism minefield
//! (CONTRIBUTING.md's "no floats in a semantic path" — [`crate::pattern::Lit`]
//! is where a *value* would appear, and it stores IEEE bit patterns, not `f64`).

use crate::ident::{Ident, QualIdent};
use brix_canon::{CanonWriter, Canonical};
use core::fmt;

/// One ground dimension exponent. Dimension names include a currency where
/// needed (`money:EUR`), making currency mixing a dimension mismatch.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Dimension {
    pub name: Ident,
    pub exponent: i32,
}

/// Canonical byte encoding for one dimension exponent (#15 PR3 fact export:
/// [`Fact::DimTerm`](crate::reflect::Fact::DimTerm) hashes these). `Dimensions`
/// vectors are already maintained in sorted, zero-pruned canonical order by
/// [`dimensions_combine`], so encoding as a plain sequence (via the blanket
/// `Vec<T: Canonical>` impl) is order-faithful without a second sort here.
impl Canonical for Dimension {
    fn canon_write(&self, w: &mut CanonWriter) {
        self.name.canon_write(w);
        w.write_int(self.exponent as i64);
    }
}

/// Canonical, sorted exponent vector for a ground dimension.
pub type Dimensions = Vec<Dimension>;

pub fn dimensions_mul(a: &Dimensions, b: &Dimensions) -> Dimensions {
    dimensions_combine(a, b, 1)
}
pub fn dimensions_div(a: &Dimensions, b: &Dimensions) -> Dimensions {
    dimensions_combine(a, b, -1)
}
fn dimensions_combine(a: &Dimensions, b: &Dimensions, sign: i32) -> Dimensions {
    let mut out = a.clone();
    for rhs in b {
        match out.binary_search_by(|x| x.name.cmp(&rhs.name)) {
            Ok(pos) => out[pos].exponent += sign * rhs.exponent,
            Err(pos) => out.insert(
                pos,
                Dimension {
                    name: rhs.name.clone(),
                    exponent: sign * rhs.exponent,
                },
            ),
        }
    }
    out.retain(|d| d.exponent != 0);
    out
}
pub fn quantity_dimensions(measure: &Ident) -> Dimensions {
    vec![Dimension {
        name: measure.clone(),
        exponent: 1,
    }]
}
pub fn money_dimensions(currency: &Ident) -> Dimensions {
    vec![Dimension {
        name: Ident::new(format!("money:{currency}")),
        exponent: 1,
    }]
}

/// A type inference variable. Stubbed: brix-ir assigns these but does not yet
/// solve them (no unifier). `u32` is enough for a single compilation unit.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct TyVar(pub u32);

impl fmt::Display for TyVar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "?t{}", self.0)
    }
}

impl Canonical for TyVar {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_uint(self.0 as u64);
    }
}

/// Fixed-width and arbitrary-precision integer families (Part V §2).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[allow(non_camel_case_types)]
pub enum IntWidth {
    I8,
    I16,
    I32,
    I64,
    I128,
    U8,
    U16,
    U32,
    U64,
    U128,
    /// Arbitrary-precision signed `Int`.
    Int,
    /// Arbitrary-precision unsigned `Nat`.
    Nat,
}

impl fmt::Display for IntWidth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use IntWidth::*;
        f.write_str(match self {
            I8 => "I8",
            I16 => "I16",
            I32 => "I32",
            I64 => "I64",
            I128 => "I128",
            U8 => "U8",
            U16 => "U16",
            U32 => "U32",
            U64 => "U64",
            U128 => "U128",
            Int => "Int",
            Nat => "Nat",
        })
    }
}

impl Canonical for IntWidth {
    /// Ordinal is declaration order (App. G enum rule); this is an internal
    /// digest-identity encoding (#15 PR3), not a spec-frozen wire ABI, so it
    /// only has to be stable *within* one `canon/1` build, same as
    /// [`Ty::canon_write`]'s ordinals below.
    fn canon_write(&self, w: &mut CanonWriter) {
        use IntWidth::*;
        let ordinal: u8 = match self {
            I8 => 0,
            I16 => 1,
            I32 => 2,
            I64 => 3,
            I128 => 4,
            U8 => 5,
            U16 => 6,
            U32 => 7,
            U64 => 8,
            U128 => 9,
            Int => 10,
            Nat => 11,
        };
        w.write_uint(ordinal as u64);
    }
}

/// The core type language (Part V §2). A small, closed set — new scalar
/// families are a spec change, not a local extension.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Ty {
    Unit,
    Bool,
    Char,
    Str,
    Bytes,
    Int(IntWidth),
    /// `Decimal<P, S>`: precision and scale are part of the type, per App. G's
    /// "scale byte + unscaled integer encoding; normalized."
    Decimal {
        precision: u32,
        scale: u32,
    },
    F32,
    F64,
    Instant,
    Duration,
    Date,
    TimeOfDay,
    TimeZone,
    Option(Box<Ty>),
    Result(Box<Ty>, Box<Ty>),
    List(Box<Ty>),
    Vector(Box<Ty>),
    Set(Box<Ty>),
    Map(Box<Ty>, Box<Ty>),
    Bag(Box<Ty>),
    /// `Rel<S>`, a first-class finite relation value over schema row `S`
    /// (Part IV §5).
    Rel(Box<Row>),
    NodeRef(Ident),
    EdgeRef(Ident),
    ClaimRef(Ident),
    /// A nominal, closed-set enum type declared by `enum E { V1; V2; ... }`
    /// (Part V §2 addendum; Appendix G "enums encode by declaration-order
    /// ordinal"). The `QualIdent` names the declared enum; the *value*
    /// domain for this type is [`crate::pattern::Lit::Enum`], which carries
    /// the variant's declaration-order ordinal (never its name) as the
    /// canonical encoding. `Ty::Enum` itself needs no `Canonical` impl (it
    /// names a type, not a value) but is unconditionally admissible in key
    /// position: it falls through `walk_key`'s scalar wildcard below, same
    /// as `NodeRef`/`Quantity`/`Money`.
    Enum(QualIdent),
    Quantity(Ident),
    Money(Ident),
    /// A ground compound physical dimension, e.g. `Money<EUR> / Kilometre`.
    Dimensioned(Dimensions),
    Probability,
    EventId,
    /// `Estimate<T> = { value, error, confidence, method }` (Part V §7).
    Estimate(Box<Ty>),
    /// A structural, row-typed anonymous record (Part V §3).
    Record(Box<Row>),
    Fn {
        params: Vec<Ty>,
        ret: Box<Ty>,
        effects: crate::effects::EffectRow,
    },
    /// An unsolved inference variable.
    Var(TyVar),
    /// An error-recovery marker: some check already failed at this
    /// expression (unknown field, non-`Result` `?`, arity mismatch, ground
    /// dimension conflict, unresolved call, ...) and there is no real type
    /// to report. Distinct from [`Ty::Var`] on purpose: a `Var` is a real
    /// substitution variable a unifier may legitimately bind, while `Error`
    /// must never enter the substitution — every error-recovery site used
    /// to return the same sentinel `Ty::Var(TyVar(u32::MAX))`, which *is*
    /// bindable, so binding it at one failure site silently leaked that
    /// binding into every other, unrelated failure site (#15 PR2). `Error`
    /// unifies with nothing but itself, so each failure stays isolated.
    Error,
}

impl Ty {
    pub fn option(t: Ty) -> Ty {
        Ty::Option(Box::new(t))
    }
    pub fn list(t: Ty) -> Ty {
        Ty::List(Box::new(t))
    }
    pub fn rel(row: Row) -> Ty {
        Ty::Rel(Box::new(row))
    }
    pub fn record(row: Row) -> Ty {
        Ty::Record(Box::new(row))
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::Unit => write!(f, "Unit"),
            Ty::Bool => write!(f, "Bool"),
            Ty::Char => write!(f, "Char"),
            Ty::Str => write!(f, "String"),
            Ty::Bytes => write!(f, "Bytes"),
            Ty::Int(w) => write!(f, "{w}"),
            Ty::Decimal { precision, scale } => write!(f, "Decimal<{precision},{scale}>"),
            Ty::F32 => write!(f, "F32"),
            Ty::F64 => write!(f, "F64"),
            Ty::Instant => write!(f, "Instant"),
            Ty::Duration => write!(f, "Duration"),
            Ty::Date => write!(f, "Date"),
            Ty::TimeOfDay => write!(f, "TimeOfDay"),
            Ty::TimeZone => write!(f, "TimeZone"),
            Ty::Option(t) => write!(f, "Option<{t}>"),
            Ty::Result(t, e) => write!(f, "Result<{t},{e}>"),
            Ty::List(t) => write!(f, "List<{t}>"),
            Ty::Vector(t) => write!(f, "Vector<{t}>"),
            Ty::Set(t) => write!(f, "Set<{t}>"),
            Ty::Map(k, v) => write!(f, "Map<{k},{v}>"),
            Ty::Bag(t) => write!(f, "Bag<{t}>"),
            Ty::Rel(row) => write!(f, "Rel<{row}>"),
            Ty::NodeRef(e) => write!(f, "NodeRef<{e}>"),
            Ty::EdgeRef(e) => write!(f, "EdgeRef<{e}>"),
            Ty::ClaimRef(e) => write!(f, "ClaimRef<{e}>"),
            Ty::Enum(q) => write!(f, "Enum<{q}>"),
            Ty::Quantity(m) => write!(f, "Quantity<{m}>"),
            Ty::Money(c) => write!(f, "Money<{c}>"),
            Ty::Dimensioned(ds) => {
                if ds.is_empty() {
                    return write!(f, "Number");
                }
                for (i, d) in ds.iter().enumerate() {
                    if i > 0 {
                        write!(f, " * ")?;
                    }
                    if d.exponent == 1 {
                        write!(f, "{}", d.name)?;
                    } else {
                        write!(f, "{}^{}", d.name, d.exponent)?;
                    }
                }
                Ok(())
            }
            Ty::Probability => write!(f, "Probability"),
            Ty::EventId => write!(f, "EventId"),
            Ty::Estimate(t) => write!(f, "Estimate<{t}>"),
            Ty::Record(row) => write!(f, "{row}"),
            Ty::Fn {
                params,
                ret,
                effects,
            } => {
                write!(f, "(")?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{p}")?;
                }
                write!(f, ") -> {ret} {effects}")
            }
            Ty::Var(v) => write!(f, "{v}"),
            Ty::Error => write!(f, "<error>"),
        }
    }
}

/// Digest-identity encoding for `Ty` (#15 PR3: `reflect::Fact` payloads embed
/// `Ty` directly, and `FactId::derive` needs a canonical byte string for
/// "same fact ⇒ same digest"). Every variant is enum-tagged by declaration
/// order (App. G's ordinal rule for enums), so this is a straightforward
/// structural walk — no key-position float ban applies here (App. G's "no
/// floats in key positions" governs *values*; `F32`/`F64` here are type
/// *tags*, and this module's own doc already establishes that `Ty` never
/// carries a float value).
impl Canonical for Ty {
    fn canon_write(&self, w: &mut CanonWriter) {
        match self {
            Ty::Unit => w.write_enum(0, |_| {}),
            Ty::Bool => w.write_enum(1, |_| {}),
            Ty::Char => w.write_enum(2, |_| {}),
            Ty::Str => w.write_enum(3, |_| {}),
            Ty::Bytes => w.write_enum(4, |_| {}),
            Ty::Int(width) => w.write_enum(5, |w| width.canon_write(w)),
            Ty::Decimal { precision, scale } => w.write_enum(6, |w| {
                w.write_uint(*precision as u64);
                w.write_uint(*scale as u64);
            }),
            Ty::F32 => w.write_enum(7, |_| {}),
            Ty::F64 => w.write_enum(8, |_| {}),
            Ty::Instant => w.write_enum(9, |_| {}),
            Ty::Duration => w.write_enum(10, |_| {}),
            Ty::Date => w.write_enum(11, |_| {}),
            Ty::TimeOfDay => w.write_enum(12, |_| {}),
            Ty::TimeZone => w.write_enum(13, |_| {}),
            Ty::Option(t) => w.write_enum(14, |w| t.canon_write(w)),
            Ty::Result(ok, err) => w.write_enum(15, |w| {
                ok.canon_write(w);
                err.canon_write(w);
            }),
            Ty::List(t) => w.write_enum(16, |w| t.canon_write(w)),
            Ty::Vector(t) => w.write_enum(17, |w| t.canon_write(w)),
            Ty::Set(t) => w.write_enum(18, |w| t.canon_write(w)),
            Ty::Map(k, v) => w.write_enum(19, |w| {
                k.canon_write(w);
                v.canon_write(w);
            }),
            Ty::Bag(t) => w.write_enum(20, |w| t.canon_write(w)),
            Ty::Rel(row) => w.write_enum(21, |w| row.canon_write(w)),
            Ty::NodeRef(id) => w.write_enum(22, |w| id.canon_write(w)),
            Ty::EdgeRef(id) => w.write_enum(23, |w| id.canon_write(w)),
            Ty::ClaimRef(id) => w.write_enum(24, |w| id.canon_write(w)),
            Ty::Enum(q) => w.write_enum(25, |w| q.canon_write(w)),
            Ty::Quantity(id) => w.write_enum(26, |w| id.canon_write(w)),
            Ty::Money(id) => w.write_enum(27, |w| id.canon_write(w)),
            Ty::Dimensioned(dims) => w.write_enum(28, |w| dims.canon_write(w)),
            Ty::Probability => w.write_enum(29, |_| {}),
            Ty::EventId => w.write_enum(30, |_| {}),
            Ty::Estimate(t) => w.write_enum(31, |w| t.canon_write(w)),
            Ty::Record(row) => w.write_enum(32, |w| row.canon_write(w)),
            Ty::Fn {
                params,
                ret,
                effects,
            } => w.write_enum(33, |w| {
                params.canon_write(w);
                ret.canon_write(w);
                effects.canon_write(w);
            }),
            Ty::Var(v) => w.write_enum(34, |w| v.canon_write(w)),
            Ty::Error => w.write_enum(35, |_| {}),
        }
    }
}

/// One field of a [`Row`].
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct RowField {
    pub name: Ident,
    pub ty: Ty,
}

/// Row-polymorphism tail (Part V §3: "structural row-typed anonymous
/// records... row-polymorphic, which is what lets a pattern bind a subset of
/// roles"). `Closed` rows are exactly their listed fields; `Open` rows admit
/// more, bound to a row variable a real unifier would solve.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum RowTail {
    Closed,
    Open(TyVar),
}

impl Canonical for RowTail {
    fn canon_write(&self, w: &mut CanonWriter) {
        match self {
            RowTail::Closed => w.write_enum(0, |_| {}),
            RowTail::Open(v) => w.write_enum(1, |w| v.canon_write(w)),
        }
    }
}

/// A record / relation-pattern row: the schema `S` in `Rel<S>`, an entity's
/// attribute set, or a rule's pattern-bound role subset.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Row {
    pub fields: Vec<RowField>,
    pub tail: RowTail,
}

impl Canonical for RowField {
    fn canon_write(&self, w: &mut CanonWriter) {
        self.name.canon_write(w);
        self.ty.canon_write(w);
    }
}

/// App. G "records/rows: fields sorted by canonical field-name bytes" —
/// delegates to [`CanonWriter::write_record`] (which does the sort) rather
/// than [`Row::canonical_field_order`], so there is exactly one place that
/// implements the sort rule.
impl Canonical for Row {
    fn canon_write(&self, w: &mut CanonWriter) {
        w.write_record(
            self.fields
                .iter()
                .map(|field| (field.name.as_str().to_owned(), field.ty.canon_bytes())),
        );
        self.tail.canon_write(w);
    }
}

impl Row {
    pub fn closed(fields: Vec<RowField>) -> Self {
        Row {
            fields,
            tail: RowTail::Closed,
        }
    }

    pub fn open(fields: Vec<RowField>, tail: TyVar) -> Self {
        Row {
            fields,
            tail: RowTail::Open(tail),
        }
    }

    /// Appendix G: "records/rows: fields sorted by canonical field-name
    /// bytes, each name-prefixed." This is the canonical field order; `fields`
    /// itself keeps declaration order for `Display`/diagnostics.
    pub fn canonical_field_order(&self) -> Vec<&RowField> {
        let mut v: Vec<&RowField> = self.fields.iter().collect();
        v.sort_by(|a, b| a.name.as_str().as_bytes().cmp(b.name.as_str().as_bytes()));
        v
    }
}

impl fmt::Display for Row {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{{ ")?;
        for (i, field) in self.fields.iter().enumerate() {
            if i > 0 {
                write!(f, "; ")?;
            }
            write!(f, "{}: {}", field.name, field.ty)?;
        }
        match &self.tail {
            RowTail::Closed => {}
            RowTail::Open(v) => write!(f, " | {v}")?,
        }
        write!(f, " }}")
    }
}

/// Appendix E `Key` judgment plus Appendix G "floats: NOT encodable in
/// `canon/1` key positions": every component type reachable from a key
/// position must be `Canonical`, and floats are unconditionally excluded from
/// key positions (not merely from *some* encodings of them).
///
/// See `spec/errata/0001-estimate-canonical-in-value-domain.md` for why this
/// is stricter than [`is_value_canonical`]: `Estimate<F64>` is fine as an
/// ordinary value (Part XII §2's suggestion row) but never as key material.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum KeyCanonicalError {
    FloatInKey {
        path: String,
    },
    FnTypeInKey {
        path: String,
    },
    UnresolvedTypeVar {
        path: String,
    },
    /// `Rel<S>` is a first-class value, not scalar key material — Part III §3
    /// admits only `Canonical` *scalar/record* encodings into a key position.
    RelInKey {
        path: String,
    },
}

impl fmt::Display for KeyCanonicalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyCanonicalError::FloatInKey { path } => {
                write!(f, "{path}: float type in key position (App. G)")
            }
            KeyCanonicalError::FnTypeInKey { path } => {
                write!(f, "{path}: function type is not Canonical")
            }
            KeyCanonicalError::UnresolvedTypeVar { path } => {
                write!(f, "{path}: unresolved type variable in key position")
            }
            KeyCanonicalError::RelInKey { path } => {
                write!(f, "{path}: Rel<S> is not admissible key material")
            }
        }
    }
}

/// Checks `Γ ⊢ T : Canonical` for every type reachable from a key position
/// (Appendix E `Key` judgment), which is strictly the App. G "no floats in
/// keys" rule plus "no function types, no unresolved inference variables, no
/// bare relation values."
pub fn check_key_canonical(ty: &Ty) -> Result<(), Vec<KeyCanonicalError>> {
    let mut errs = Vec::new();
    walk_key(ty, "$", &mut errs);
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs)
    }
}

fn walk_key(ty: &Ty, path: &str, out: &mut Vec<KeyCanonicalError>) {
    match ty {
        Ty::F32 | Ty::F64 => out.push(KeyCanonicalError::FloatInKey { path: path.into() }),
        Ty::Fn { .. } => out.push(KeyCanonicalError::FnTypeInKey { path: path.into() }),
        Ty::Var(_) => out.push(KeyCanonicalError::UnresolvedTypeVar { path: path.into() }),
        Ty::Rel(_) => out.push(KeyCanonicalError::RelInKey { path: path.into() }),
        Ty::Estimate(t) => walk_key(t, &format!("{path}.value"), out),
        Ty::Option(t) | Ty::List(t) | Ty::Vector(t) | Ty::Set(t) | Ty::Bag(t) => {
            walk_key(t, &format!("{path}.0"), out)
        }
        Ty::Result(a, b) => {
            walk_key(a, &format!("{path}.ok"), out);
            walk_key(b, &format!("{path}.err"), out);
        }
        Ty::Map(k, v) => {
            walk_key(k, &format!("{path}.key"), out);
            walk_key(v, &format!("{path}.value"), out);
        }
        Ty::Record(row) => {
            for field in &row.fields {
                walk_key(&field.ty, &format!("{path}.{}", field.name), out);
            }
        }
        // Scalars (Unit, Bool, Char, Str, Bytes, Int, Decimal, time types,
        // NodeRef/EdgeRef/ClaimRef, Enum, Quantity, Money, Probability,
        // EventId) are Canonical and admissible in key positions. `Enum`
        // encodes by declaration-order ordinal (App. G), never by name, so
        // it carries no float/var/fn/rel hazard.
        _ => {}
    }
}

/// The general `S: Canonical` bound used *outside* key positions (Part IV §5
/// `Rel<S>` hashing; Part XII §2 candidate/suggestion rows for
/// `candidatesDigest`). Per the errata ruling this admits floats (via App.
/// G's totalOrder tiebreak encoding for the *value* domain) but still
/// excludes function types and unresolved inference variables.
pub fn check_value_canonical(ty: &Ty) -> Result<(), Vec<KeyCanonicalError>> {
    let mut errs = Vec::new();
    walk_value(ty, "$", &mut errs);
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs)
    }
}

fn walk_value(ty: &Ty, path: &str, out: &mut Vec<KeyCanonicalError>) {
    match ty {
        Ty::Fn { .. } => out.push(KeyCanonicalError::FnTypeInKey { path: path.into() }),
        Ty::Var(_) => out.push(KeyCanonicalError::UnresolvedTypeVar { path: path.into() }),
        Ty::Estimate(t) | Ty::Option(t) | Ty::List(t) | Ty::Vector(t) | Ty::Set(t) | Ty::Bag(t) => {
            walk_value(t, &format!("{path}.0"), out)
        }
        Ty::Result(a, b) => {
            walk_value(a, &format!("{path}.ok"), out);
            walk_value(b, &format!("{path}.err"), out);
        }
        Ty::Map(k, v) => {
            walk_value(k, &format!("{path}.key"), out);
            walk_value(v, &format!("{path}.value"), out);
        }
        Ty::Record(row) => {
            for field in &row.fields {
                walk_value(&field.ty, &format!("{path}.{}", field.name), out);
            }
        }
        Ty::Rel(row) => {
            for field in &row.fields {
                walk_value(&field.ty, &format!("{path}.{}", field.name), out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::EffectRow;

    #[test]
    fn scalar_types_are_key_canonical() {
        assert!(check_key_canonical(&Ty::Int(IntWidth::I64)).is_ok());
        assert!(check_key_canonical(&Ty::Str).is_ok());
        assert!(check_key_canonical(&Ty::EventId).is_ok());
    }

    #[test]
    fn float_is_never_key_canonical() {
        let err = check_key_canonical(&Ty::F64).unwrap_err();
        assert_eq!(err.len(), 1);
        assert!(matches!(err[0], KeyCanonicalError::FloatInKey { .. }));
    }

    #[test]
    fn float_nested_in_a_record_key_is_rejected_with_a_path() {
        let row = Row::closed(vec![
            RowField {
                name: Ident::new("amount"),
                ty: Ty::F64,
            },
            RowField {
                name: Ident::new("order"),
                ty: Ty::NodeRef(Ident::new("Order")),
            },
        ]);
        let err = check_key_canonical(&Ty::record(row)).unwrap_err();
        assert_eq!(err.len(), 1);
        assert_eq!(
            err[0],
            KeyCanonicalError::FloatInKey {
                path: "$.amount".into()
            }
        );
    }

    #[test]
    fn fn_type_is_never_key_canonical() {
        let fn_ty = Ty::Fn {
            params: vec![],
            ret: Box::new(Ty::Unit),
            effects: EffectRow::empty(),
        };
        assert!(check_key_canonical(&fn_ty).is_err());
    }

    #[test]
    fn estimate_f64_is_value_canonical_but_not_key_canonical() {
        let estimate = Ty::Estimate(Box::new(Ty::F64));
        assert!(
            check_value_canonical(&estimate).is_ok(),
            "Part XII §2 suggestion rows carry Estimate<F64> and require S: Canonical"
        );
        assert!(
            check_key_canonical(&estimate).is_err(),
            "floats stay inadmissible in keys regardless of the Estimate wrapper"
        );
    }

    #[test]
    fn enum_type_is_key_canonical() {
        // Mismatch (A): a role typed `Tier`/`VehicleClass`/... must be
        // admissible as a key role, not fall back to `Ty::Var` (which would
        // trip `UnresolvedTypeVar` — a false positive on the flagship).
        assert!(check_key_canonical(&Ty::Enum(crate::ident::QualIdent::simple("Tier"))).is_ok());
    }

    #[test]
    fn enum_type_display_shows_the_qualified_name() {
        let ty = Ty::Enum(crate::ident::QualIdent::simple("VehicleClass"));
        assert_eq!(ty.to_string(), "Enum<VehicleClass>");
    }

    #[test]
    fn enum_key_role_nested_in_a_record_is_still_canonical() {
        let row = Row::closed(vec![RowField {
            name: Ident::new("class"),
            ty: Ty::Enum(crate::ident::QualIdent::simple("VehicleClass")),
        }]);
        assert!(check_key_canonical(&Ty::record(row)).is_ok());
    }

    #[test]
    fn row_display_matches_declaration_order_not_canonical_order() {
        let row = Row::closed(vec![
            RowField {
                name: Ident::new("b"),
                ty: Ty::Bool,
            },
            RowField {
                name: Ident::new("a"),
                ty: Ty::Bool,
            },
        ]);
        assert_eq!(row.to_string(), "{ b: Bool; a: Bool }");
        let ordered = row.canonical_field_order();
        assert_eq!(ordered[0].name.as_str(), "a");
    }
}
