//! `brix_ir::types::Ty` -> generated-Rust-type-string mapping for
//! `ColumnDesc::rust_type`. Every variant maps to a *plausible* Rust type
//! token — syntactically valid so `emit_relation_module`'s output parses,
//! even for named types (`NodeId`-family, `Decimal`, `Money`, ...) that
//! don't exist as real definitions yet. Same scaffold posture the rest of
//! `emit` already takes (module doc: "codegen shape now, real runtime
//! types later"). Types with no representation at all in a relation-column
//! position (`Var`, `Fn`, `Rel`, `Record`) degrade to a `compile_error!`
//! sentinel, mirroring `emit_relation_module`'s existing unparseable-type
//! fallback — a legible generated error, never a panic or a silently wrong
//! type.

use brix_ir::types::{IntWidth, Ty};

pub fn rust_type_of(ty: &Ty) -> String {
    match ty {
        Ty::Unit => "()".to_string(),
        Ty::Bool => "bool".to_string(),
        Ty::Char => "char".to_string(),
        Ty::Str => "String".to_string(),
        Ty::Bytes => "Vec<u8>".to_string(),
        Ty::Int(w) => int_width_rust_type(*w),
        Ty::Decimal { .. } => "Decimal".to_string(),
        Ty::F32 => "f32".to_string(),
        Ty::F64 => "f64".to_string(),
        Ty::Instant => "Instant".to_string(),
        Ty::Duration => "Duration".to_string(),
        Ty::Date => "Date".to_string(),
        Ty::TimeOfDay => "TimeOfDay".to_string(),
        Ty::TimeZone => "TimeZone".to_string(),
        Ty::Option(t) => format!("Option<{}>", rust_type_of(t)),
        Ty::Result(t, e) => format!("Result<{}, {}>", rust_type_of(t), rust_type_of(e)),
        Ty::List(t) => format!("Vec<{}>", rust_type_of(t)),
        Ty::Vector(t) => format!("Vec<{}>", rust_type_of(t)),
        Ty::Set(t) => format!("std::collections::BTreeSet<{}>", rust_type_of(t)),
        Ty::Map(k, v) => format!(
            "std::collections::BTreeMap<{}, {}>",
            rust_type_of(k),
            rust_type_of(v)
        ),
        Ty::Bag(t) => format!("Vec<{}>", rust_type_of(t)),
        Ty::Rel(_) => unresolved("a Rel<_> value cannot be a relation column"),
        Ty::NodeRef(e) => {
            let s = e.segments().last().map(|s| s.as_str()).unwrap_or("");
            format!("{s}Id")
        }
        Ty::EdgeRef(e) => {
            let s = e.segments().last().map(|s| s.as_str()).unwrap_or("");
            format!("{s}EdgeId")
        }
        Ty::ClaimRef(e) => {
            let s = e.segments().last().map(|s| s.as_str()).unwrap_or("");
            format!("{s}ClaimId")
        }
        Ty::Enum(q) => q
            .segments()
            .last()
            .map(|s| s.as_str().to_string())
            .unwrap_or_else(|| unresolved("enum type with no segments")),
        Ty::Quantity(_) => "Quantity".to_string(),
        Ty::Money(_) => "Money".to_string(),
        // A ground compound physical dimension is a decimal-scaled number
        // with a dimension tag carried separately (Part V §5) — same
        // underlying representation as `Decimal`.
        Ty::Dimensioned(_) => "Decimal".to_string(),
        Ty::Probability => "Probability".to_string(),
        Ty::EventId => "EventId".to_string(),
        Ty::Estimate(t) => format!("Estimate<{}>", rust_type_of(t)),
        Ty::Record(_) => unresolved("a Record<_> value cannot be a relation column"),
        Ty::Fn { .. } => unresolved("a function type cannot be a relation column"),
        Ty::Var(_) => unresolved("unresolved type variable reached codegen"),
        Ty::Error => unresolved("a type-error-recovery marker reached codegen"),
        Ty::Missing(t) => format!("Missing<{}>", rust_type_of(t)),
    }
}

fn int_width_rust_type(w: IntWidth) -> String {
    match w {
        IntWidth::I8 => "i8",
        IntWidth::I16 => "i16",
        IntWidth::I32 => "i32",
        IntWidth::I64 => "i64",
        IntWidth::I128 => "i128",
        IntWidth::U8 => "u8",
        IntWidth::U16 => "u16",
        IntWidth::U32 => "u32",
        IntWidth::U64 => "u64",
        IntWidth::U128 => "u128",
        // Arbitrary-precision Int/Nat have no fixed-width Rust primitive;
        // scaffold as a placeholder bignum type name, same posture as
        // `Decimal`/`NodeId` elsewhere in this module.
        IntWidth::Int => "BigInt",
        IntWidth::Nat => "BigNat",
    }
    .to_string()
}

fn unresolved(reason: &str) -> String {
    format!("compile_error!({reason:?})")
}

#[cfg(test)]
mod tests {
    use super::*;
    use brix_ir::ident::QualIdent;
    use brix_ir::types::{Row, RowTail};

    #[test]
    fn enum_maps_to_its_last_segment_name() {
        assert_eq!(
            rust_type_of(&Ty::Enum(QualIdent::from("VehicleClass"))),
            "VehicleClass"
        );
    }

    #[test]
    fn node_ref_maps_to_a_per_entity_id_type() {
        assert_eq!(
            rust_type_of(&Ty::NodeRef(QualIdent::simple("Order"))),
            "OrderId"
        );
    }

    #[test]
    fn fixed_width_int_maps_to_the_matching_rust_primitive() {
        assert_eq!(rust_type_of(&Ty::Int(IntWidth::I64)), "i64");
    }

    #[test]
    fn decimal_maps_to_a_named_scaffold_type() {
        assert_eq!(
            rust_type_of(&Ty::Decimal {
                precision: 10,
                scale: 2
            }),
            "Decimal"
        );
    }

    #[test]
    fn bool_and_str_map_to_rust_primitives() {
        assert_eq!(rust_type_of(&Ty::Bool), "bool");
        assert_eq!(rust_type_of(&Ty::Str), "String");
    }

    #[test]
    fn option_wraps_the_inner_mapped_type() {
        assert_eq!(
            rust_type_of(&Ty::option(Ty::Int(IntWidth::I64))),
            "Option<i64>"
        );
    }

    #[test]
    fn unresolvable_types_fall_back_to_a_legible_compile_error() {
        let rel = Ty::Rel(Box::new(Row {
            fields: vec![],
            tail: RowTail::Closed,
        }));
        assert!(rust_type_of(&rel).starts_with("compile_error!("));
    }
}
