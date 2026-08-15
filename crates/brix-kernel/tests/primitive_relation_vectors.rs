//! Frozen arithmetic primitive-relation vectors (ADR-0015 §5 Stage B,
//! ADR-0013 §7).
//!
//! One relation is frozen here: `TypingArithV2`. The superseded `TypingArithV1`
//! was retired (ADR-0024 §3) and its vector deleted with it; the legacy row set
//! survives only as this file's `Naming::LegacyAllPromote` baseline, which
//! bounds what Stage E changed without any of it being resolvable.
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
use brix_kernel::{resolve_primitive_relation, typing_arith_v2, Row};
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

/// Which generator family names a coercion edge, per relation version.
///
/// Declared here as literal prefix strings rather than imported, so consumer 2
/// stays independent of the kernel's own naming function.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Naming {
    /// `TypingArithV1` — every edge under the promotion family, including the
    /// lossy one. **Retired** from the kernel (ADR-0024 §3); kept here as the
    /// historical baseline that bounds what Stage E changed. Nothing in the
    /// registry resolves to it, and this file no longer freezes a vector for it.
    LegacyAllPromote,
    /// `TypingArithV2` — ADR-0015 Stage E <D-PROMOTE>: exact edges keep the
    /// promotion family, the lossy edge moves to an explicitly-labelled
    /// conversion family.
    FamilyByExactness,
}

impl Naming {
    fn prefix(self, kind: u64) -> &'static str {
        match (self, kind) {
            (Naming::FamilyByExactness, 1) => "type.rule.num.convert.lossy",
            _ => "type.rule.num.promote",
        }
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

fn independent_src(row: &DeclaredRow, naming: Naming) -> Digest {
    let path_bytes = |steps: &[(&'static str, &'static str, u64)]| -> Vec<Vec<u8>> {
        steps
            .iter()
            .map(|(from, to, kind)| {
                let prefix = naming.prefix(*kind);
                let mut e = CanonWriter::new();
                e.write_bytes(
                    GeneratorId::named(&format!("{prefix}.{from}_{to}@1"))
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

fn independent_relation_id(rows: &[DeclaredRow], naming: Naming) -> Digest {
    let mut w = CanonWriter::new();
    w.write_bytes(b"brix.kernel.primitive-relation");
    w.write_uint(1);
    w.write_enum(0, |_| {}); // JudgmentKind::Typing
    w.write_bytes(GeneratorId::named("type.rule.arith@1").digest().as_bytes());
    w.write_bytes(independent_schema_id(b"brix.kernel.arith-typing-input", 1).as_bytes());
    w.write_bytes(independent_schema_id(b"brix.kernel.numeric-result-type", 1).as_bytes());
    w.write_set(rows.iter().map(|row| {
        let mut e = CanonWriter::new();
        e.write_bytes(independent_src(row, naming).as_bytes());
        e.write_bytes(independent_dst(row).as_bytes());
        e.finish()
    }));
    Digest::of(Domain::Value, &w.finish())
}

// ---------------------------------------------------------------------------
// Manifest
// ---------------------------------------------------------------------------

fn manifest_path(version: u32) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("vectors")
        .join(format!("primitive_relation_typing_arith_v{version}.json"))
}

/// Every arithmetic relation this kernel compiles in — one, after `TypingArithV1`
/// was retired (ADR-0024 §3). The shape stays a list because the discipline is
/// per-relation and the next relation added should inherit all three consumers
/// without restructuring this file.
fn versions() -> Vec<(u32, Naming, brix_kernel::PrimitiveRelationId)> {
    vec![(2, Naming::FamilyByExactness, typing_arith_v2())]
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

fn build_manifest(version: u32, naming: Naming, id: brix_kernel::PrimitiveRelationId) -> String {
    let rows = declared_rows();
    let relation = resolve_primitive_relation(&id).expect("the relation resolves");

    let mut out = String::new();
    out.push_str("{\n");
    out.push_str("  \"format\": \"brix.kernel.PrimitiveRelation\",\n");
    out.push_str("  \"version\": 1,\n");
    out.push_str("  \"adr\": \"ADR-0015\",\n");
    out.push_str(&format!("  \"relation\": \"TypingArithV{version}\",\n"));
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
    out.push_str(&format!("  \"relation_id\": \"{}\",\n", id.to_hex()));
    // The one thing that distinguished V2 from the retired V1, said in the
    // manifest rather than left to be inferred from 20 differing digests. The
    // version gate is kept rather than inlined: this file freezes whatever
    // relations exist, and a future relation predating this field must still
    // reproduce its own frozen bytes (ADR-0013 §7).
    if version >= 2 {
        out.push_str(
            "  \"edge_families\": { \"exact\": \"type.rule.num.promote\", \
             \"lossy\": \"type.rule.num.convert.lossy\" },\n",
        );
    }

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
            independent_src(row, naming).to_hex()
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
    for (version, naming, id) in versions() {
        let generated = build_manifest(version, naming, id);
        let path = manifest_path(version);
        let committed = std::fs::read_to_string(&path).unwrap_or_default();

        if generated == committed {
            continue;
        }

        if std::env::var_os("BLESS_VECTORS").is_some() {
            std::fs::write(&path, &generated).expect("vector manifest is writable");
            continue;
        }

        // Deliberately do NOT write on the failing path: the CI determinism job
        // re-runs the suite and requires a clean working tree.
        panic!(
            "TypingArithV{version} vectors drifted from {}.\n\
             This relation is what a kernel `Accepted` verdict means for \
             `g_arith`. Adding, removing, or changing a row does not update an \
             existing relation — it allocates the next one (ADR-0015 §7), \
             because otherwise identical certificate bytes would mean different \
             things under different kernel releases. Regenerate with \
             BLESS_VECTORS=1 only if you intend exactly that.",
            path.display()
        );
    }
}

/// Consumer 2. Every endpoint digest and the relation id itself, rebuilt from
/// raw ordinals and literal markers, for both relations.
#[test]
fn typing_arith_vectors_reproduced_by_primitive_canon_writes() {
    let rows = declared_rows();

    for (version, naming, id) in versions() {
        let relation = resolve_primitive_relation(&id).expect("the relation resolves");

        for row in &rows {
            let src = PropositionId(independent_src(row, naming));
            let dst = PropositionId(independent_dst(row));
            assert!(
                relation.rows.contains(&(src, dst)),
                "V{version} row {} {} {} -> {} must be reproducible without the \
                 schemas' own canon_write",
                row.operator,
                row.lhs,
                row.rhs,
                row.result
            );
        }

        assert_eq!(
            id.digest(),
            independent_relation_id(&rows, naming),
            "the V{version} relation id must be reproducible from literals alone"
        );
    }
}

/// Consumer 3. The literal matrix declared in this file is *exactly* each
/// registry relation's row set — no extra rows the table forgot, no missing
/// rows the kernel dropped.
#[test]
fn the_declared_matrix_is_exactly_the_registrys_rows() {
    let rows = declared_rows();

    for (version, naming, id) in versions() {
        let relation = resolve_primitive_relation(&id).expect("the relation resolves");

        let declared: std::collections::BTreeSet<Row> = rows
            .iter()
            .map(|row| {
                (
                    PropositionId(independent_src(row, naming)),
                    PropositionId(independent_dst(row)),
                )
            })
            .collect();

        assert_eq!(
            declared.len(),
            JOINABLE.len() * OPERATORS.len(),
            "the declared V{version} table must not contain duplicates"
        );
        assert_eq!(declared, relation.rows);
        assert_eq!(relation.rows.len(), 120);
    }
}

/// ADR-0015 Stage E <D-PROMOTE>, stated as a property of the two frozen row
/// sets: the relocation touches exactly the rows whose promotion path crosses
/// the lossy `Int -> Float` edge, and nothing else. 100 rows are byte-identical
/// across V1 and V2; 20 move.
///
/// This is what bounds the change. A relocation that altered an exact path, a
/// result type, or the operand matrix would show up here as a different count,
/// not as a subtle behavioural difference discovered later.
#[test]
fn stage_e_relocates_exactly_the_lossy_paths() {
    let rows = declared_rows();

    let v1: std::collections::BTreeSet<Row> = rows
        .iter()
        .map(|r| {
            (
                PropositionId(independent_src(r, Naming::LegacyAllPromote)),
                PropositionId(independent_dst(r)),
            )
        })
        .collect();
    let v2: std::collections::BTreeSet<Row> = rows
        .iter()
        .map(|r| {
            (
                PropositionId(independent_src(r, Naming::FamilyByExactness)),
                PropositionId(independent_dst(r)),
            )
        })
        .collect();

    let crosses_lossy = |r: &DeclaredRow| {
        r.lhs_path
            .iter()
            .chain(r.rhs_path.iter())
            .any(|(_, _, kind)| *kind == 1)
    };
    let expected_moved = rows.iter().filter(|r| crosses_lossy(r)).count();

    assert_eq!(expected_moved, 20);
    assert_eq!(v1.intersection(&v2).count(), 120 - expected_moved);

    // The relocation produced a genuinely different identity. Both sides are
    // reconstructed here rather than asked of the kernel, which no longer knows
    // the legacy id at all -- that is what retiring it means.
    assert_ne!(
        independent_relation_id(&rows, Naming::LegacyAllPromote),
        independent_relation_id(&rows, Naming::FamilyByExactness)
    );
    assert_eq!(
        independent_relation_id(&rows, Naming::FamilyByExactness),
        typing_arith_v2().digest(),
        "the surviving relation is the one the kernel resolves"
    );
}

/// No id in the current relation asserts a promotion for the lossy edge -- the
/// literal point of <D-PROMOTE>. Checked on the *generator ids themselves*,
/// not on the `CoercionKind` tag, because the tag was already right before
/// Stage E and the id was the thing that lied.
#[test]
fn no_current_row_names_the_lossy_edge_as_a_promotion() {
    let promoted = GeneratorId::named("type.rule.num.promote.Int_Float@1");
    let relocated = GeneratorId::named("type.rule.num.convert.lossy.Int_Float@1");
    assert_ne!(promoted, relocated);

    for row in declared_rows().iter().filter(|r| {
        r.lhs_path
            .iter()
            .chain(r.rhs_path.iter())
            .any(|(_, _, kind)| *kind == 1)
    }) {
        // Rebuilding this row under the *legacy* naming must not reproduce its
        // V2 digest: the lossy edge genuinely carries a different id now.
        assert_ne!(
            independent_src(row, Naming::LegacyAllPromote),
            independent_src(row, Naming::FamilyByExactness),
            "{} {} {} still names the lossy edge as a promotion",
            row.operator,
            row.lhs,
            row.rhs
        );
    }
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
