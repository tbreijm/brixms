//! Frozen `ArithTypingInputV1` source-object vectors (ADR-0015 §5 Stage B0,
//! ADR-0013 §7).
//!
//! **Why this artifact is frozen.** `ArithTypingInputV1::config_id()` mints a
//! content-addressed `ConfigId`, and that id *is* the `src` endpoint of every
//! `g_arith` leaf. Three ordinal spaces feed it — `ArithOperatorV1`,
//! `NumericTypeNameV1`, and `CoercionKind` — each documented "append-only,
//! never reordered". Without a fence that claim is unenforced: reordering
//! `NumericTypeNameV1` would silently change every arithmetic leaf's identity
//! and no gate would fire. Once Stage B's registry rows are keyed on these
//! bytes, the same reorder would silently change which rows match.
//!
//! Two consumers guard it, mirroring `generator_semantics_v1` and
//! `tree_derivation_v1`:
//!
//! 1. `arith_typing_input_vectors_are_frozen` — the production `Canonical`
//!    impl must keep reproducing the committed bytes (regenerate with
//!    `BLESS_VECTORS=1`);
//! 2. `arith_typing_input_vectors_reproduced_by_primitive_canon_writes` — a
//!    second construction path spelling out the marker, version, every
//!    ordinal, and the path list framing with primitive `CanonWriter`
//!    operations, **never calling the artifact's own `canon_write`** and never
//!    naming its enums, so a vector cannot be vacuously satisfied by the code
//!    it guards.
//!
//! The six cases below cover every ordinal in all three enum spaces: all four
//! operators, all six numeric type names, and both coercion kinds — plus an
//! empty path, a one-edge path, and a four-edge path, since path order is
//! semantic and `write_list` framing is part of the ABI.
//!
//! After the freeze this schema is append-only; a new field, or a new meaning
//! for an existing one, requires v2 (ADR-0015 §7 — a source schema is half of
//! a primitive relation's identity, and relation identities are immutable).

use std::path::{Path, PathBuf};

use brix_canon::{CanonWriter, Canonical};
use brix_kernel::{
    ArithOperatorV1, ArithTypingInputV1, CoercionEdgeV1, CoercionKind, NumericTypeNameV1,
    ARITH_TYPING_INPUT_MARKER_V1, ARITH_TYPING_INPUT_VERSION_V1,
};
use brix_semantic::GeneratorId;

/// The independent path's own description of one input — deliberately raw
/// ordinals and generator names rather than the kernel's enums, so the
/// reproduction cannot borrow the production types' encoding decisions. If
/// someone reorders `NumericTypeNameV1`, these literals do not move with it,
/// and consumer 2 fails.
struct Decl {
    operator: u64,
    lhs_type: u64,
    rhs_type: u64,
    /// `(generator name, kind ordinal)`, in path order.
    lhs_path: Vec<(&'static str, u64)>,
    rhs_path: Vec<(&'static str, u64)>,
}

struct Fixture {
    name: &'static str,
    description: &'static str,
    input: ArithTypingInputV1,
    declared: Decl,
}

fn promote(from: &str, to: &str) -> GeneratorId {
    GeneratorId::named(&format!("type.rule.num.promote.{from}_{to}@1"))
}

fn edge(from: &str, to: &str, kind: CoercionKind) -> CoercionEdgeV1 {
    CoercionEdgeV1 {
        generator: promote(from, to),
        kind,
    }
}

fn fixtures() -> Vec<Fixture> {
    vec![
        Fixture {
            name: "add_int_int",
            description: "`1 + 2` — the identity case: no promotion on either side",
            input: ArithTypingInputV1 {
                operator: ArithOperatorV1::Add,
                lhs_type: NumericTypeNameV1::Int,
                rhs_type: NumericTypeNameV1::Int,
                lhs_promotion_path: vec![],
                rhs_promotion_path: vec![],
            },
            declared: Decl {
                operator: 0,
                lhs_type: 1,
                rhs_type: 1,
                lhs_path: vec![],
                rhs_path: vec![],
            },
        },
        Fixture {
            name: "sub_nat_int",
            description: "Sub with a one-edge exact promotion on the left only",
            input: ArithTypingInputV1 {
                operator: ArithOperatorV1::Sub,
                lhs_type: NumericTypeNameV1::Nat,
                rhs_type: NumericTypeNameV1::Int,
                lhs_promotion_path: vec![edge("Nat", "Int", CoercionKind::Exact)],
                rhs_promotion_path: vec![],
            },
            declared: Decl {
                operator: 1,
                lhs_type: 0,
                rhs_type: 1,
                lhs_path: vec![("type.rule.num.promote.Nat_Int@1", 0)],
                rhs_path: vec![],
            },
        },
        Fixture {
            name: "mul_rat_real",
            description: "Mul with the exact Rat↪Real edge on the left",
            input: ArithTypingInputV1 {
                operator: ArithOperatorV1::Mul,
                lhs_type: NumericTypeNameV1::Rat,
                rhs_type: NumericTypeNameV1::Real,
                lhs_promotion_path: vec![edge("Rat", "Real", CoercionKind::Exact)],
                rhs_promotion_path: vec![],
            },
            declared: Decl {
                operator: 2,
                lhs_type: 2,
                rhs_type: 3,
                lhs_path: vec![("type.rule.num.promote.Rat_Real@1", 0)],
                rhs_path: vec![],
            },
        },
        Fixture {
            name: "div_int_int_lossy",
            description: "`7 / 2` — Div through the LOSSY Int↪Float edge on both operands \
                          (ADR-0015 ⟨D-PROMOTE⟩: never an embedding)",
            input: ArithTypingInputV1 {
                operator: ArithOperatorV1::Div,
                lhs_type: NumericTypeNameV1::Int,
                rhs_type: NumericTypeNameV1::Int,
                lhs_promotion_path: vec![edge("Int", "Float", CoercionKind::Lossy)],
                rhs_promotion_path: vec![edge("Int", "Float", CoercionKind::Lossy)],
            },
            declared: Decl {
                operator: 3,
                lhs_type: 1,
                rhs_type: 1,
                lhs_path: vec![("type.rule.num.promote.Int_Float@1", 1)],
                rhs_path: vec![("type.rule.num.promote.Int_Float@1", 1)],
            },
        },
        Fixture {
            name: "add_float_float",
            description: "`1.0 + 2.0` — results in Float like div_int_int_lossy, and must \
                          NOT share its bytes (the collision Stage B0 removed)",
            input: ArithTypingInputV1 {
                operator: ArithOperatorV1::Add,
                lhs_type: NumericTypeNameV1::Float,
                rhs_type: NumericTypeNameV1::Float,
                lhs_promotion_path: vec![],
                rhs_promotion_path: vec![],
            },
            declared: Decl {
                operator: 0,
                lhs_type: 5,
                rhs_type: 5,
                lhs_path: vec![],
                rhs_path: vec![],
            },
        },
        Fixture {
            name: "add_nat_complex_four_edges",
            description: "the full exact tower as one ordered four-edge path — pins list \
                          framing and path order",
            input: ArithTypingInputV1 {
                operator: ArithOperatorV1::Add,
                lhs_type: NumericTypeNameV1::Nat,
                rhs_type: NumericTypeNameV1::Complex,
                lhs_promotion_path: vec![
                    edge("Nat", "Int", CoercionKind::Exact),
                    edge("Int", "Rat", CoercionKind::Exact),
                    edge("Rat", "Real", CoercionKind::Exact),
                    edge("Real", "Complex", CoercionKind::Exact),
                ],
                rhs_promotion_path: vec![],
            },
            declared: Decl {
                operator: 0,
                lhs_type: 0,
                rhs_type: 4,
                lhs_path: vec![
                    ("type.rule.num.promote.Nat_Int@1", 0),
                    ("type.rule.num.promote.Int_Rat@1", 0),
                    ("type.rule.num.promote.Rat_Real@1", 0),
                    ("type.rule.num.promote.Real_Complex@1", 0),
                ],
                rhs_path: vec![],
            },
        },
    ]
}

/// Rebuild the canonical preimage from `decl` alone, using only primitive
/// `CanonWriter` operations and literal constants. This never touches
/// `ArithTypingInputV1`, `ArithOperatorV1`, `NumericTypeNameV1`, or
/// `CoercionKind` — that independence is the whole point of consumer 2.
fn independent_input(decl: &Decl) -> Vec<u8> {
    let path_bytes = |steps: &[(&'static str, u64)]| -> Vec<Vec<u8>> {
        steps
            .iter()
            .map(|(generator, kind)| {
                let mut e = CanonWriter::new();
                e.write_bytes(GeneratorId::named(generator).digest().as_bytes());
                e.write_enum(*kind, |_| {});
                e.finish()
            })
            .collect()
    };

    let mut w = CanonWriter::new();
    w.write_bytes(b"brix.kernel.arith-typing-input");
    w.write_uint(1);
    w.write_enum(decl.operator, |_| {});
    w.write_enum(decl.lhs_type, |_| {});
    w.write_enum(decl.rhs_type, |_| {});
    w.write_list(path_bytes(&decl.lhs_path));
    w.write_list(path_bytes(&decl.rhs_path));
    w.finish()
}

fn manifest_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("vectors")
        .join("arith_typing_input_v1.json")
}

fn build_manifest() -> String {
    let cases = fixtures();
    let mut out = String::new();
    out.push_str("{\n");
    out.push_str("  \"format\": \"brix.kernel.ArithTypingInputV1\",\n");
    out.push_str("  \"version\": 1,\n");
    out.push_str("  \"adr\": \"ADR-0015\",\n");
    out.push_str("  \"cases\": [\n");

    let total = cases.len();
    for (i, fixture) in cases.iter().enumerate() {
        let bytes = fixture.input.canon_bytes();
        out.push_str("    {\n");
        out.push_str(&format!("      \"name\": {},\n", json_str(fixture.name)));
        out.push_str(&format!(
            "      \"description\": {},\n",
            json_str(fixture.description)
        ));
        out.push_str(&format!("      \"canon_hex\": \"{}\",\n", to_hex(&bytes)));
        out.push_str(&format!(
            "      \"config_id\": \"{}\"\n",
            fixture.input.config_id().to_hex()
        ));
        out.push_str("    }");
        out.push_str(if i + 1 == total { "\n" } else { ",\n" });
    }

    out.push_str("  ]\n");
    out.push_str("}\n");
    out
}

#[test]
fn arith_typing_input_vectors_are_frozen() {
    let generated = build_manifest();
    let path = manifest_path();
    let committed = std::fs::read_to_string(&path).unwrap_or_default();

    if generated == committed {
        return;
    }

    if std::env::var_os("BLESS_VECTORS").is_some() {
        std::fs::write(&path, &generated).expect("vector manifest is writable");
        return;
    }

    // Deliberately do NOT write on the failing path: the CI determinism job
    // re-runs the suite and requires a clean working tree.
    panic!(
        "arith typing input vectors drifted from {}.\n\
         The v1 source-object encoding is frozen ABI — a drift changes the \
         `ConfigId` at the `src` endpoint of every `g_arith` leaf, and (from \
         Stage B) which primitive-relation rows match it (ADR-0015 §5/§7). \
         Regenerate deliberately with BLESS_VECTORS=1 only if you intend a \
         compatibility break.",
        path.display()
    );
}

#[test]
fn arith_typing_input_vectors_reproduced_by_primitive_canon_writes() {
    for fixture in fixtures() {
        let produced = fixture.input.canon_bytes();
        let independent = independent_input(&fixture.declared);
        assert_eq!(
            to_hex(&produced),
            to_hex(&independent),
            "case {} must be reproducible without the artifact's own canon_write",
            fixture.name
        );
    }
}

/// The marker and version the frozen preimage opens with are the ones the
/// crate exports — checked against literals here so a rename of either
/// constant cannot quietly move the whole encoding while both consumers above
/// keep agreeing with each other.
#[test]
fn the_frozen_preimage_header_is_the_exported_marker_and_version() {
    assert_eq!(
        ARITH_TYPING_INPUT_MARKER_V1,
        b"brix.kernel.arith-typing-input"
    );
    assert_eq!(ARITH_TYPING_INPUT_VERSION_V1, 1);
}

/// Every ordinal in all three enum spaces appears in at least one frozen case,
/// so a reorder cannot slip through by touching only an uncovered variant.
///
/// This is the specific hole the vectors exist to close: `config_id()` makes
/// these ordinals load-bearing for an artifact identity, and before this file
/// nothing guarded them.
#[test]
fn the_frozen_cases_cover_every_ordinal() {
    let cases = fixtures();
    let seen = |f: &dyn Fn(&Decl) -> Vec<u64>| -> Vec<u64> {
        let mut v: Vec<u64> = cases.iter().flat_map(|c| f(&c.declared)).collect();
        v.sort_unstable();
        v.dedup();
        v
    };

    assert_eq!(
        seen(&|d| vec![d.operator]),
        vec![0, 1, 2, 3],
        "every ArithOperatorV1 ordinal must be frozen by some case"
    );
    assert_eq!(
        seen(&|d| vec![d.lhs_type, d.rhs_type]),
        vec![0, 1, 2, 3, 4, 5],
        "every NumericTypeNameV1 ordinal must be frozen by some case"
    );
    assert_eq!(
        seen(&|d| d
            .lhs_path
            .iter()
            .chain(d.rhs_path.iter())
            .map(|(_, k)| *k)
            .collect()),
        vec![0, 1],
        "both CoercionKind ordinals must be frozen by some case"
    );

    // Path lengths 0, 1 and 4 are all represented, so the `write_list` count
    // prefix and element framing are exercised beyond the degenerate case.
    let mut lengths: Vec<usize> = cases
        .iter()
        .flat_map(|c| [c.declared.lhs_path.len(), c.declared.rhs_path.len()])
        .collect();
    lengths.sort_unstable();
    lengths.dedup();
    assert_eq!(lengths, vec![0, 1, 4]);
}

/// The two expressions ADR-0015 §5 Stage B0 names must not share a `ConfigId`.
/// Frozen here as well as in `soc-regimes` so the guarantee survives even if
/// the regime's own emission test is refactored.
#[test]
fn float_addition_and_integer_division_are_distinct_frozen_cases() {
    let cases = fixtures();
    let by_name = |want: &str| {
        cases
            .iter()
            .find(|c| c.name == want)
            .map(|c| c.input.config_id())
            .expect("named case exists")
    };
    assert_ne!(by_name("add_float_float"), by_name("div_int_int_lossy"));
}

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).expect("nibble is a hex digit"));
        s.push(char::from_digit((b & 0xf) as u32, 16).expect("nibble is a hex digit"));
    }
    s
}

fn json_str(value: &str) -> String {
    let mut s = String::with_capacity(value.len() + 2);
    s.push('"');
    for c in value.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            other => s.push(other),
        }
    }
    s.push('"');
    s
}
