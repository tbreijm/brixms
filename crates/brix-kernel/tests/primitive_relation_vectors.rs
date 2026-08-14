//! Frozen `TypingArithV1` primitive-relation vectors (ADR-0015 §5 Stage B,
//! ADR-0013 §7).
//!
//! **Why this artifact is frozen.** From Stage B onward the registry decides
//! `g_arith`'s realization by exact membership over these rows. Three things are
//! therefore load-bearing and were previously unguarded: the row *set* (which
//! `(operator, operands, promotion paths) → result` triples the kernel
//! authorizes), the endpoint *encodings* (`ArithTypingInputV1` and
//! `NumericResultTypeV1`, whose ordinals are documented "append-only, never
//! reordered"), and the derived `PrimitiveRelationId` that a certificate's
//! `PrimRealizes` term names. A silent change to any of them changes what a
//! kernel `Accepted` verdict means, which is exactly what ADR-0015 §7 forbids:
//! "otherwise identical certificate bytes would mean different things under
//! different kernel releases."
//!
//! **Three consumers guard it**, one more than the usual two, because a
//! relation has a set of rows rather than a handful of cases:
//!
//! 1. `typing_arith_vectors_are_frozen` — the production registry must keep
//!    reproducing the committed manifest (regenerate with `BLESS_VECTORS=1`);
//! 2. `typing_arith_vectors_reproduced_by_primitive_canon_writes` — every row's
//!    `src`/`dst` digest and the relation id itself are rebuilt from raw
//!    ordinals and literal markers with primitive `CanonWriter` operations,
//!    **never calling the schemas' own `canon_write`** and never naming the
//!    kernel's enums;
//! 3. `the_declared_matrix_is_exactly_the_registrys_rows` — the 30 joinable
//!    operand pairs are spelled out here as a literal table rather than computed
//!    by a second copy of the join, so the kernel's `join`/`field_of` cannot
//!    drift without this file disagreeing.
//!
//! The manifest keeps each row in **readable form** (operator, operand types,
//! promotion paths, result) beside its digests. 120 opaque hex pairs would be a
//! fence nobody can actually read; the point of reviewing a vector diff by hand
//! is defeated if the diff is unreadable.

use std::path::{Path, PathBuf};

use brix_canon::{CanonWriter, Digest, Domain};
use brix_kernel::{resolve_primitive_relation, typing_arith_v1, Row};
use brix_semantic::{GeneratorId, PropositionId};

/// The four operators and their frozen ordinals.
const OPERATORS: &[(&str, u64)] = &[("Add", 0), ("Sub", 1), ("Mul", 2), ("Div", 3)];

/// The numeric tower nodes and their frozen ordinals.
const TYPES: &[(&str, u64)] = &[
    ("Nat", 0),
    ("Int", 1),
    ("Rat", 2),
    ("Real", 3),
    ("Complex", 4),
    ("Float", 5),
];

/// Every operand pair that has a join, with the type the operation is performed
/// at — **written out literally**, not computed.
///
/// This is consumer 3's independence: if the kernel's `join` gained or lost a
/// pair, or moved one, no shared code would carry the change into this table.
/// The absent pairs are the point as much as the present ones — `Float` mixed
/// with `Rat`/`Real`/`Complex` has no join, which is why
/// `arithmetic_rule_has_no_unchecked_join` holds by construction.
const JOINABLE: &[(&str, &str, &str)] = &[
    // (lhs, rhs, base)  — the exact branch ℕ ⊂ ℤ ⊂ ℚ ⊂ ℝ ⊂ ℂ, all 25 pairs.
    ("Nat", "Nat", "Nat"),
    ("Nat", "Int", "Int"),
    ("Nat", "Rat", "Rat"),
    ("Nat", "Real", "Real"),
    ("Nat", "Complex", "Complex"),
    ("Int", "Nat", "Int"),
    ("Int", "Int", "Int"),
    ("Int", "Rat", "Rat"),
    ("Int", "Real", "Real"),
    ("Int", "Complex", "Complex"),
    ("Rat", "Nat", "Rat"),
    ("Rat", "Int", "Rat"),
    ("Rat", "Rat", "Rat"),
    ("Rat", "Real", "Real"),
    ("Rat", "Complex", "Complex"),
    ("Real", "Nat", "Real"),
    ("Real", "Int", "Real"),
    ("Real", "Rat", "Real"),
    ("Real", "Real", "Real"),
    ("Real", "Complex", "Complex"),
    ("Complex", "Nat", "Complex"),
    ("Complex", "Int", "Complex"),
    ("Complex", "Rat", "Complex"),
    ("Complex", "Real", "Complex"),
    ("Complex", "Complex", "Complex"),
    // The lossy `Float` branch: only `Nat`/`Int`/`Float` reach it.
    ("Nat", "Float", "Float"),
    ("Int", "Float", "Float"),
    ("Float", "Nat", "Float"),
    ("Float", "Int", "Float"),
    ("Float", "Float", "Float"),
];

/// The unique upward path between two tower nodes, as ordered
/// `(from, to, exactness ordinal)` edges — **written out literally**.
///
/// `Int → Float` is tagged `1` (lossy) and every other edge `0` (exact), per
/// ADR-0015 ⟨D-PROMOTE⟩: a lossy map is not injective and does not preserve
/// numeric identity, so it must never be recorded under a name asserting
/// exactness.
fn declared_path(from: &str, to: &str) -> Vec<(&'static str, &'static str, u64)> {
    const NAT_INT: (&str, &str, u64) = ("Nat", "Int", 0);
    const INT_RAT: (&str, &str, u64) = ("Int", "Rat", 0);
    const RAT_REAL: (&str, &str, u64) = ("Rat", "Real", 0);
    const REAL_COMPLEX: (&str, &str, u64) = ("Real", "Complex", 0);
    const INT_FLOAT: (&str, &str, u64) = ("Int", "Float", 1);

    match (from, to) {
        (a, b) if a == b => vec![],
        ("Nat", "Int") => vec![NAT_INT],
        ("Nat", "Rat") => vec![NAT_INT, INT_RAT],
        ("Nat", "Real") => vec![NAT_INT, INT_RAT, RAT_REAL],
        ("Nat", "Complex") => vec![NAT_INT, INT_RAT, RAT_REAL, REAL_COMPLEX],
        ("Nat", "Float") => vec![NAT_INT, INT_FLOAT],
        ("Int", "Rat") => vec![INT_RAT],
        ("Int", "Real") => vec![INT_RAT, RAT_REAL],
        ("Int", "Complex") => vec![INT_RAT, RAT_REAL, REAL_COMPLEX],
        ("Int", "Float") => vec![INT_FLOAT],
        ("Rat", "Real") => vec![RAT_REAL],
        ("Rat", "Complex") => vec![RAT_REAL, REAL_COMPLEX],
        ("Real", "Complex") => vec![REAL_COMPLEX],
        other => panic!("no declared upward path for {other:?}"),
    }
}

/// The language's declared division result rule: `Nat`/`Int` divide into
/// `Float`, everything else stays put. This is the field that makes `Div`
/// differ from the other three operators.
fn declared_field_of(base: &str) -> &str {
    match base {
        "Nat" | "Int" => "Float",
        other => other,
    }
}

/// One row in readable form, independent of the kernel's types.
struct DeclaredRow {
    operator: &'static str,
    lhs: &'static str,
    rhs: &'static str,
    lhs_path: Vec<(&'static str, &'static str, u64)>,
    rhs_path: Vec<(&'static str, &'static str, u64)>,
    result: &'static str,
}

fn ordinal(table: &[(&str, u64)], name: &str) -> u64 {
    table
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, o)| *o)
        .unwrap_or_else(|| panic!("{name} is not in the frozen ordinal table"))
}

/// The full declared matrix: every operator against every joinable pair.
fn declared_rows() -> Vec<DeclaredRow> {
    let mut out = Vec::new();
    for (operator, _) in OPERATORS {
        for (lhs, rhs, base) in JOINABLE {
            let result = if *operator == "Div" {
                declared_field_of(base)
            } else {
                base
            };
            out.push(DeclaredRow {
                operator,
                lhs,
                rhs,
                lhs_path: declared_path(lhs, result),
                rhs_path: declared_path(rhs, result),
                result,
            });
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Consumer 2: rebuild every digest with primitive writes and literal constants.
// Nothing below names `ArithTypingInputV1`, `NumericResultTypeV1`,
// `ArithOperatorV1`, `NumericTypeNameV1`, `CoercionKind`, `SchemaId`, or
// `PrimitiveRelation`, nor calls any of their `canon_write` impls.
// ---------------------------------------------------------------------------

fn independent_src(row: &DeclaredRow) -> Digest {
    let path_bytes = |steps: &[(&'static str, &'static str, u64)]| -> Vec<Vec<u8>> {
        steps
            .iter()
            .map(|(from, to, kind)| {
                let mut e = CanonWriter::new();
                e.write_bytes(
                    GeneratorId::named(&format!("type.rule.num.promote.{from}_{to}@1"))
                        .digest()
                        .as_bytes(),
                );
                e.write_enum(*kind, |_| {});
                e.finish()
            })
            .collect()
    };

    let mut w = CanonWriter::new();
    w.write_bytes(b"brix.kernel.arith-typing-input");
    w.write_uint(1);
    w.write_enum(ordinal(OPERATORS, row.operator), |_| {});
    w.write_enum(ordinal(TYPES, row.lhs), |_| {});
    w.write_enum(ordinal(TYPES, row.rhs), |_| {});
    w.write_list(path_bytes(&row.lhs_path));
    w.write_list(path_bytes(&row.rhs_path));
    Digest::of(Domain::Value, &w.finish())
}

fn independent_dst(row: &DeclaredRow) -> Digest {
    let mut w = CanonWriter::new();
    w.write_bytes(b"brix.kernel.numeric-result-type");
    w.write_uint(1);
    w.write_enum(ordinal(TYPES, row.result), |_| {});
    Digest::of(Domain::Value, &w.finish())
}

fn independent_schema_id(marker: &[u8], version: u64) -> Digest {
    let mut w = CanonWriter::new();
    w.write_bytes(marker);
    w.write_uint(version);
    Digest::of(Domain::Value, &w.finish())
}

fn independent_relation_id(rows: &[DeclaredRow]) -> Digest {
    let mut w = CanonWriter::new();
    w.write_bytes(b"brix.kernel.primitive-relation");
    w.write_uint(1);
    w.write_enum(0, |_| {}); // JudgmentKind::Typing
    w.write_bytes(GeneratorId::named("type.rule.arith@1").digest().as_bytes());
    w.write_bytes(independent_schema_id(b"brix.kernel.arith-typing-input", 1).as_bytes());
    w.write_bytes(independent_schema_id(b"brix.kernel.numeric-result-type", 1).as_bytes());
    w.write_set(rows.iter().map(|row| {
        let mut e = CanonWriter::new();
        e.write_bytes(independent_src(row).as_bytes());
        e.write_bytes(independent_dst(row).as_bytes());
        e.finish()
    }));
    Digest::of(Domain::Value, &w.finish())
}

// ---------------------------------------------------------------------------
// Manifest
// ---------------------------------------------------------------------------

fn manifest_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("vectors")
        .join("primitive_relation_typing_arith_v1.json")
}

fn render_path(steps: &[(&'static str, &'static str, u64)]) -> String {
    let rendered: Vec<String> = steps
        .iter()
        .map(|(from, to, kind)| {
            let exactness = if *kind == 0 { "exact" } else { "lossy" };
            format!("\"{from}->{to}:{exactness}\"")
        })
        .collect();
    format!("[{}]", rendered.join(", "))
}

fn build_manifest() -> String {
    let rows = declared_rows();
    let relation = resolve_primitive_relation(&typing_arith_v1()).expect("TypingArithV1 resolves");

    let mut out = String::new();
    out.push_str("{\n");
    out.push_str("  \"format\": \"brix.kernel.PrimitiveRelation\",\n");
    out.push_str("  \"version\": 1,\n");
    out.push_str("  \"adr\": \"ADR-0015\",\n");
    out.push_str("  \"relation\": \"TypingArithV1\",\n");
    out.push_str("  \"judgment_kind\": \"Typing\",\n");
    out.push_str("  \"generator\": \"type.rule.arith@1\",\n");
    out.push_str(&format!(
        "  \"source_schema\": \"{}\",\n",
        brix_kernel::arith_typing_input_schema_id().to_hex()
    ));
    out.push_str(&format!(
        "  \"destination_schema\": \"{}\",\n",
        brix_kernel::numeric_result_type_schema_id().to_hex()
    ));
    out.push_str(&format!(
        "  \"relation_id\": \"{}\",\n",
        typing_arith_v1().to_hex()
    ));
    out.push_str(&format!("  \"row_count\": {},\n", relation.rows.len()));
    out.push_str("  \"rows\": [\n");

    let total = rows.len();
    for (i, row) in rows.iter().enumerate() {
        out.push_str("    {\n");
        out.push_str(&format!("      \"operator\": \"{}\",\n", row.operator));
        out.push_str(&format!("      \"lhs\": \"{}\",\n", row.lhs));
        out.push_str(&format!("      \"rhs\": \"{}\",\n", row.rhs));
        out.push_str(&format!(
            "      \"lhs_path\": {},\n",
            render_path(&row.lhs_path)
        ));
        out.push_str(&format!(
            "      \"rhs_path\": {},\n",
            render_path(&row.rhs_path)
        ));
        out.push_str(&format!("      \"result\": \"{}\",\n", row.result));
        out.push_str(&format!(
            "      \"src\": \"{}\",\n",
            independent_src(row).to_hex()
        ));
        out.push_str(&format!(
            "      \"dst\": \"{}\"\n",
            independent_dst(row).to_hex()
        ));
        out.push_str("    }");
        out.push_str(if i + 1 == total { "\n" } else { ",\n" });
    }

    out.push_str("  ]\n");
    out.push_str("}\n");
    out
}

#[test]
fn typing_arith_vectors_are_frozen() {
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
        "TypingArithV1 vectors drifted from {}.\n\
         This relation is what a kernel `Accepted` verdict means for `g_arith`. \
         Adding, removing, or changing a row does not update `TypingArithV1` — \
         it allocates `TypingArithV2` (ADR-0015 §7), because otherwise \
         identical certificate bytes would mean different things under \
         different kernel releases. Regenerate with BLESS_VECTORS=1 only if \
         you intend exactly that.",
        path.display()
    );
}

/// Consumer 2. Every endpoint digest and the relation id itself, rebuilt from
/// raw ordinals and literal markers.
#[test]
fn typing_arith_vectors_reproduced_by_primitive_canon_writes() {
    let relation = resolve_primitive_relation(&typing_arith_v1()).expect("TypingArithV1 resolves");
    let rows = declared_rows();

    for row in &rows {
        let src = PropositionId(independent_src(row));
        let dst = PropositionId(independent_dst(row));
        assert!(
            relation.rows.contains(&(src, dst)),
            "row {} {} {} -> {} must be reproducible without the schemas' own canon_write",
            row.operator,
            row.lhs,
            row.rhs,
            row.result
        );
    }

    assert_eq!(
        typing_arith_v1().digest(),
        independent_relation_id(&rows),
        "the relation id must be reproducible from literals alone"
    );
}

/// Consumer 3. The literal matrix declared in this file is *exactly* the
/// registry's row set — no extra rows the table forgot, no missing rows the
/// kernel dropped.
#[test]
fn the_declared_matrix_is_exactly_the_registrys_rows() {
    let relation = resolve_primitive_relation(&typing_arith_v1()).expect("TypingArithV1 resolves");

    let declared: std::collections::BTreeSet<Row> = declared_rows()
        .iter()
        .map(|row| {
            (
                PropositionId(independent_src(row)),
                PropositionId(independent_dst(row)),
            )
        })
        .collect();

    assert_eq!(
        declared.len(),
        JOINABLE.len() * OPERATORS.len(),
        "the declared table must not contain duplicates"
    );
    assert_eq!(declared, relation.rows);
    assert_eq!(relation.rows.len(), 120);
}

/// The pairs deliberately absent from `JOINABLE` stay absent. Stated as its own
/// assertion because an omission is invisible in a table of what *is* there,
/// and this particular omission is gate 3's whole content.
#[test]
fn float_mixed_with_the_exact_branch_is_absent_from_the_matrix() {
    for other in ["Rat", "Real", "Complex"] {
        assert!(
            !JOINABLE
                .iter()
                .any(|(l, r, _)| (*l == "Float" && *r == other) || (*l == other && *r == "Float")),
            "Float mixed with {other} must have no join and therefore no row"
        );
    }
}
