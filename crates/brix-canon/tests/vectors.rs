//! Golden canon vectors — the G0 freeze artifact.
//!
//! This test defines a declarative catalog of canon values (one per Appendix G
//! type, plus edge cases), encodes each through `brix-canon`, and emits the
//! frozen manifest `vectors/canon_vectors.json`. The manifest records, for every
//! case, a *declarative* input spec and the expected canonical bytes (hex). Two
//! independent consumers verify it:
//!
//! 1. this test itself — its encoder is `brix-canon`, and it re-reads the
//!    committed manifest to guard against silent drift (a changed encoding fails
//!    the test unless `BLESS_VECTORS=1` regenerates it);
//! 2. `scripts/canon_crosscheck.py` — an independent from-spec implementation
//!    that replays the same specs and must reproduce the same bytes.
//!
//! After G0 this manifest is append-only; any change to an existing vector is a
//! spec erratum plus a `CANON_VERSION` bump (see `crates/brix-canon/OWNER.md`).

use brix_canon::{
    total_order_key_f64, CanonWriter, Canonical, Decimal, Digest, Domain, Money, Quantity,
    CANON_VERSION,
};

/// Declarative input spec, mirrored one-for-one in the Python cross-check.
#[derive(Clone)]
enum Spec {
    Uint(u64),
    Uint128(u128),
    Int(i64),
    Int128(i128),
    Bytes(Vec<u8>),
    Str(String),
    Ident(String),
    Bool(bool),
    Unit,
    Char(char),
    Decimal {
        unscaled: i128,
        scale: u8,
    },
    Quantity {
        measure: String,
        unscaled: i128,
        scale: u8,
    },
    Money {
        currency: String,
        minor: i128,
    },
    List(Vec<Spec>),
    Set(Vec<Spec>),
    Bag(Vec<Spec>),
    Map(Vec<(Spec, Spec)>),
    Record(Vec<(String, Spec)>),
    Enum {
        ordinal: u64,
        payload: Option<Box<Spec>>,
    },
    TotalOrderF64(f64),
}

impl Spec {
    fn encode(&self) -> Vec<u8> {
        // Total-order keys are deliberately not `canon/1` values: Appendix G
        // admits them only as the final aggregation-order tiebreak. Keep this
        // test vector out of `CanonWriter`, whose public surface is restricted
        // to encodings that may participate in canonical value/key bytes.
        if let Self::TotalOrderF64(x) = self {
            return total_order_key_f64(*x).to_vec();
        }
        let mut w = CanonWriter::new();
        self.write(&mut w);
        w.finish()
    }

    fn write(&self, w: &mut CanonWriter) {
        match self {
            Spec::Uint(v) => w.write_uint(*v),
            Spec::Uint128(v) => w.write_uint128(*v),
            Spec::Int(v) => w.write_int(*v),
            Spec::Int128(v) => w.write_int128(*v),
            Spec::Bytes(b) => w.write_bytes(b),
            Spec::Str(s) => w.write_str(s),
            Spec::Ident(s) => w.write_ident(s),
            Spec::Bool(b) => w.write_bool(*b),
            Spec::Unit => w.write_unit(),
            Spec::Char(c) => w.write_char(*c),
            Spec::Decimal { unscaled, scale } => Decimal::new(*unscaled, *scale).canon_write(w),
            Spec::Quantity {
                measure,
                unscaled,
                scale,
            } => Quantity::new(measure.clone(), Decimal::new(*unscaled, *scale)).canon_write(w),
            Spec::Money { currency, minor } => Money::new(currency.clone(), *minor).canon_write(w),
            Spec::List(items) => w.write_list(items.iter().map(|s| s.encode())),
            Spec::Set(items) => w.write_set(items.iter().map(|s| s.encode())),
            Spec::Bag(items) => w.write_bag(items.iter().map(|s| s.encode())),
            Spec::Map(entries) => {
                w.write_map(entries.iter().map(|(k, v)| (k.encode(), v.encode())))
            }
            Spec::Record(fields) => {
                w.write_record(fields.iter().map(|(n, v)| (n.clone(), v.encode())))
            }
            Spec::Enum { ordinal, payload } => {
                w.write_enum(*ordinal, |cw| {
                    if let Some(p) = payload {
                        p.write(cw);
                    }
                });
            }
            Spec::TotalOrderF64(_) => unreachable!("handled directly by Spec::encode"),
        }
    }

    /// JSON object body (fields between the braces), ASCII-only and deterministic.
    fn to_json(&self) -> String {
        match self {
            Spec::Uint(v) => format!(r#""kind":"uint","v":{v}"#),
            Spec::Uint128(v) => format!(r#""kind":"uint128","v":"{v}""#),
            Spec::Int(v) => format!(r#""kind":"int","v":{v}"#),
            Spec::Int128(v) => format!(r#""kind":"int128","v":"{v}""#),
            Spec::Bytes(b) => format!(r#""kind":"bytes","v":"{}""#, hex(b)),
            Spec::Str(s) => format!(r#""kind":"str","v":{}"#, json_str(s)),
            Spec::Ident(s) => format!(r#""kind":"ident","v":{}"#, json_str(s)),
            Spec::Bool(b) => format!(r#""kind":"bool","v":{b}"#),
            Spec::Unit => r#""kind":"unit""#.to_string(),
            Spec::Char(c) => format!(r#""kind":"char","cp":{}"#, *c as u32),
            Spec::Decimal { unscaled, scale } => {
                format!(r#""kind":"decimal","unscaled":"{unscaled}","scale":{scale}"#)
            }
            Spec::Quantity {
                measure,
                unscaled,
                scale,
            } => format!(
                r#""kind":"quantity","measure":{},"unscaled":"{unscaled}","scale":{scale}"#,
                json_str(measure)
            ),
            Spec::Money { currency, minor } => {
                format!(
                    r#""kind":"money","currency":{},"minor":"{minor}""#,
                    json_str(currency)
                )
            }
            Spec::List(items) => format!(r#""kind":"list","elems":{}"#, json_spec_array(items)),
            Spec::Set(items) => format!(r#""kind":"set","elems":{}"#, json_spec_array(items)),
            Spec::Bag(items) => format!(r#""kind":"bag","elems":{}"#, json_spec_array(items)),
            Spec::Map(entries) => {
                let inner: Vec<String> = entries
                    .iter()
                    .map(|(k, v)| format!("[{{{}}},{{{}}}]", k.to_json(), v.to_json()))
                    .collect();
                format!(r#""kind":"map","entries":[{}]"#, inner.join(","))
            }
            Spec::Record(fields) => {
                let inner: Vec<String> = fields
                    .iter()
                    .map(|(n, v)| format!("[{},{{{}}}]", json_str(n), v.to_json()))
                    .collect();
                format!(r#""kind":"record","fields":[{}]"#, inner.join(","))
            }
            Spec::Enum { ordinal, payload } => match payload {
                Some(p) => {
                    format!(
                        r#""kind":"enum","ordinal":{ordinal},"payload":{{{}}}"#,
                        p.to_json()
                    )
                }
                None => format!(r#""kind":"enum","ordinal":{ordinal},"payload":null"#),
            },
            Spec::TotalOrderF64(x) => {
                format!(r#""kind":"totalorder_f64","bits":"{:016x}""#, x.to_bits())
            }
        }
    }
}

fn json_spec_array(items: &[Spec]) -> String {
    let inner: Vec<String> = items
        .iter()
        .map(|s| format!("{{{}}}", s.to_json()))
        .collect();
    format!("[{}]", inner.join(","))
}

fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for byte in b {
        s.push(char::from_digit((byte >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((byte & 0xf) as u32, 16).unwrap());
    }
    s
}

/// Minimal ASCII-safe JSON string literal (non-ASCII escaped as \uXXXX).
fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{:04x}", unit));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

struct DigestCase {
    name: &'static str,
    domain: Domain,
    spec: Spec,
}

fn cases() -> Vec<(&'static str, Spec)> {
    use Spec::*;
    vec![
        // --- integers (order-preserving, errata 0001) ---
        ("uint_zero", Uint(0)),
        ("uint_1", Uint(1)),
        ("uint_127", Uint(127)),
        ("uint_128", Uint(128)),
        ("uint_255", Uint(255)),
        ("uint_256", Uint(256)),
        ("uint_65535", Uint(65535)),
        ("uint_16777216", Uint(16_777_216)),
        ("uint_u64max", Uint(u64::MAX)),
        ("uint128_big", Uint128(1u128 << 100)),
        ("uint128_max", Uint128(u128::MAX)),
        ("int_zero", Int(0)),
        ("int_1", Int(1)),
        ("int_neg1", Int(-1)),
        ("int_neg2", Int(-2)),
        ("int_255", Int(255)),
        ("int_neg255", Int(-255)),
        ("int_256", Int(256)),
        ("int_neg256", Int(-256)),
        ("int_i64max", Int(i64::MAX)),
        ("int_i64min", Int(i64::MIN)),
        ("int128_min", Int128(i128::MIN)),
        ("int128_max", Int128(i128::MAX)),
        // --- bytes / strings / identifiers ---
        ("bytes_empty", Bytes(vec![])),
        ("bytes_deadbeef", Bytes(vec![0xde, 0xad, 0xbe, 0xef])),
        ("str_empty", Str(String::new())),
        ("str_ascii", Str("hello".into())),
        ("str_unicode", Str("héllo \u{4e16}\u{754c}".into())),
        // café: precomposed é (U+00E9) vs decomposed e + U+0301.
        ("str_value_decomposed", Str("cafe\u{0301}".into())),
        ("ident_ascii", Ident("orderTotal".into())),
        ("ident_precomposed", Ident("caf\u{00e9}".into())),
        ("ident_decomposed", Ident("cafe\u{0301}".into())),
        // --- bool / unit / char ---
        ("bool_false", Bool(false)),
        ("bool_true", Bool(true)),
        ("unit", Unit),
        ("char_A", Char('A')),
        ("char_world", Char('\u{4e16}')),
        ("char_max", Char('\u{10ffff}')),
        // --- decimals (normalized, order-preserving, errata 0002) ---
        (
            "dec_zero",
            Decimal {
                unscaled: 0,
                scale: 0,
            },
        ),
        (
            "dec_zero_scaled",
            Decimal {
                unscaled: 0,
                scale: 5,
            },
        ),
        (
            "dec_15",
            Decimal {
                unscaled: 15,
                scale: 0,
            },
        ),
        (
            "dec_1_5",
            Decimal {
                unscaled: 15,
                scale: 1,
            },
        ),
        (
            "dec_1_50_normalizes",
            Decimal {
                unscaled: 150,
                scale: 2,
            },
        ),
        (
            "dec_0_05",
            Decimal {
                unscaled: 5,
                scale: 2,
            },
        ),
        (
            "dec_neg_1_5",
            Decimal {
                unscaled: -15,
                scale: 1,
            },
        ),
        (
            "dec_neg_2",
            Decimal {
                unscaled: -2,
                scale: 0,
            },
        ),
        (
            "dec_big",
            Decimal {
                unscaled: 123_456_789_012_345,
                scale: 6,
            },
        ),
        // --- quantity / money ---
        (
            "qty_length_1500",
            Quantity {
                measure: "Length".into(),
                unscaled: 1500,
                scale: 0,
            },
        ),
        (
            "qty_mass_2_5",
            Quantity {
                measure: "Mass".into(),
                unscaled: 25,
                scale: 1,
            },
        ),
        (
            "money_eur_100",
            Money {
                currency: "EUR".into(),
                minor: 100,
            },
        ),
        (
            "money_usd_neg_50",
            Money {
                currency: "USD".into(),
                minor: -50,
            },
        ),
        // --- collections ---
        ("list_ints", List(vec![Uint(3), Uint(1), Uint(2)])),
        ("list_empty", List(vec![])),
        (
            "set_ints_unsorted_dups",
            Set(vec![Uint(3), Uint(1), Uint(2), Uint(1)]),
        ),
        (
            "set_strings",
            Set(vec![Str("b".into()), Str("a".into()), Str("c".into())]),
        ),
        (
            "bag_ints",
            Bag(vec![Uint(1), Uint(1), Uint(2), Uint(3), Uint(3), Uint(3)]),
        ),
        (
            "map_str_int",
            Map(vec![
                (Str("two".into()), Uint(2)),
                (Str("one".into()), Uint(1)),
                (Str("three".into()), Uint(3)),
            ]),
        ),
        // --- records (field-name sorted) ---
        (
            "record_unsorted",
            Record(vec![
                ("zeta".into(), Uint(26)),
                ("alpha".into(), Uint(1)),
                ("mu".into(), Str("m".into())),
            ]),
        ),
        (
            "record_nested",
            Record(vec![
                ("id".into(), Uint(42)),
                (
                    "price".into(),
                    Money {
                        currency: "EUR".into(),
                        minor: 1999,
                    },
                ),
                ("tags".into(), Set(vec![Str("b".into()), Str("a".into())])),
            ]),
        ),
        // --- enums ---
        (
            "enum_unit_variant",
            Enum {
                ordinal: 0,
                payload: None,
            },
        ),
        (
            "enum_payload",
            Enum {
                ordinal: 2,
                payload: Some(Box::new(Str("data".into()))),
            },
        ),
        (
            "option_some",
            Enum {
                ordinal: 1,
                payload: Some(Box::new(Uint(7))),
            },
        ),
        // --- float totalOrder tiebreak (aggregation order only) ---
        ("f64_neg_inf", TotalOrderF64(f64::NEG_INFINITY)),
        ("f64_neg_1_5", TotalOrderF64(-1.5)),
        ("f64_neg_zero", TotalOrderF64(-0.0)),
        ("f64_pos_zero", TotalOrderF64(0.0)),
        ("f64_pos_1_5", TotalOrderF64(1.5)),
        ("f64_pos_inf", TotalOrderF64(f64::INFINITY)),
        ("f64_nan", TotalOrderF64(f64::NAN)),
    ]
}

fn digest_cases() -> Vec<DigestCase> {
    use Spec::*;
    vec![
        DigestCase {
            name: "node_from_record",
            domain: Domain::Node,
            spec: Record(vec![
                ("customer".into(), Str("acme".into())),
                ("seq".into(), Uint(7)),
            ]),
        },
        DigestCase {
            name: "edge_from_list",
            domain: Domain::Edge,
            spec: List(vec![Str("owns".into()), Uint(1), Uint(2)]),
        },
        DigestCase {
            name: "value_decimal",
            domain: Domain::Value,
            spec: Decimal {
                unscaled: -1234,
                scale: 2,
            },
        },
    ]
}

fn build_manifest() -> String {
    let mut out = String::new();
    out.push_str("{\n");
    out.push_str(&format!(
        "  \"canon_version\": {},\n",
        json_str(CANON_VERSION)
    ));
    out.push_str("  \"note\": \"Frozen G0 canon golden vectors. Append-only after freeze; changes require a spec erratum + CANON_VERSION bump. Regenerate with BLESS_VECTORS=1.\",\n");

    out.push_str("  \"cases\": [\n");
    let all = cases();
    for (i, (name, spec)) in all.iter().enumerate() {
        let bytes = spec.encode();
        let comma = if i + 1 < all.len() { "," } else { "" };
        out.push_str(&format!(
            "    {{\"name\": {}, {}, \"hex\": \"{}\"}}{}\n",
            json_str(name),
            spec.to_json(),
            hex(&bytes),
            comma
        ));
    }
    out.push_str("  ],\n");

    out.push_str("  \"digests\": [\n");
    let dcs = digest_cases();
    for (i, dc) in dcs.iter().enumerate() {
        let payload = dc.spec.encode();
        let digest = Digest::of(dc.domain, &payload);
        let comma = if i + 1 < dcs.len() { "," } else { "" };
        out.push_str(&format!(
            "    {{\"name\": {}, \"domain\": {}, \"spec\": {{{}}}, \"payload_hex\": \"{}\", \"digest\": \"{}\"}}{}\n",
            json_str(dc.name),
            json_str(dc.domain.tag()),
            dc.spec.to_json(),
            hex(&payload),
            digest.to_hex(),
            comma
        ));
    }
    out.push_str("  ]\n");
    out.push_str("}\n");
    out
}

fn manifest_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../vectors/canon_vectors.json")
        .canonicalize()
        .unwrap_or_else(|_| {
            // File may not exist yet on first bless.
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../vectors/canon_vectors.json")
        })
}

#[test]
fn golden_vectors_are_frozen() {
    let generated = build_manifest();

    // insta guards the human-reviewable golden under vectors/.
    insta::with_settings!({ snapshot_path => "../../../vectors", prepend_module_to_snapshot => false }, {
        insta::assert_snapshot!("canon_vectors", generated.clone());
    });

    let path = manifest_path();
    let bless = std::env::var("BLESS_VECTORS").is_ok();
    match std::fs::read_to_string(&path) {
        Ok(existing) if existing == generated => { /* frozen, matches */ }
        Ok(_existing) if bless => {
            std::fs::write(&path, &generated).unwrap();
        }
        Ok(_existing) => {
            std::fs::write(&path, &generated).unwrap();
            panic!(
                "vectors/canon_vectors.json changed. If this is an intentional pre-G0 change, \
                 the write has been applied — re-run tests. After G0 this requires an erratum + \
                 CANON_VERSION bump."
            );
        }
        Err(_) => {
            // First generation.
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            std::fs::write(&path, &generated).unwrap();
        }
    }
}

/// Sanity: every declarative case name is unique (so the manifest is a map).
#[test]
fn case_names_unique() {
    let mut names: Vec<&str> = cases().iter().map(|(n, _)| *n).collect();
    names.sort_unstable();
    let before = names.len();
    names.dedup();
    assert_eq!(before, names.len(), "duplicate case name in vectors");
}
