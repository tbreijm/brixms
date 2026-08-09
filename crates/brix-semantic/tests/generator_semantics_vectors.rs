//! Frozen `GeneratorSemanticsV1` manifest vectors (ADR-0020 D3, ADR-0013 §7).
//!
//! Five cases — an empty manifest, a lone diagonal, a single exact row, a
//! multi-row relation, and a mixed manifest declaring both forms — are encoded
//! through the production `Canonical` impl and frozen in
//! `vectors/generator_semantics_v1.json` together with their canonical bytes
//! and `GeneratorSemanticsIdV1`s.
//!
//! Two consumers guard the manifest, mirroring `tree_derivation_v1`:
//!
//! 1. `generator_semantics_vectors_are_frozen` — the production encoder must
//!    keep reproducing the committed bytes (regenerate with `BLESS_VECTORS=1`);
//! 2. `generator_semantics_vectors_reproduced_by_primitive_canon_writes` — a
//!    second construction path spelling out the marker, version, map framing,
//!    relation ordinals, and row field order with primitive `CanonWriter`
//!    operations, never calling the artifact's own `canon_write`, so a vector
//!    cannot be vacuously satisfied by the code it guards.
//!
//! **Why this artifact is frozen at all.** The id is what a settlement audit
//! receipt binds to say *which oracle ran* (ADR-0020 D5). If the encoding
//! drifted, two different declarations could collide, and the receipt's whole
//! claim — that a substituted oracle is detectable — would silently weaken.
//! After the freeze this manifest is append-only; a new relation form requires
//! v2 (ADR-0020 D3).

use std::path::{Path, PathBuf};

use brix_canon::{CanonWriter, Canonical};
use brix_semantic::{
    ConfigId, GeneratorId, GeneratorSemanticsV1, SettlementRelationV1,
    GENERATOR_SEMANTICS_MARKER_V1, GENERATOR_SEMANTICS_VERSION_V1,
};

struct Fixture {
    name: &'static str,
    description: &'static str,
    manifest: GeneratorSemanticsV1,
    /// The declarations, spelled out again for the independent path. Kept
    /// separate on purpose: the second consumer must not read them back off
    /// `manifest`, or it would be reproducing the artifact from itself.
    declared: Vec<(&'static str, Decl)>,
}

/// The independent path's own description of a relation — deliberately not
/// `SettlementRelationV1`, so the reproduction cannot borrow the production
/// type's encoding decisions.
enum Decl {
    Diagonal,
    Rows(Vec<(&'static str, &'static str)>),
}

fn g(name: &str) -> GeneratorId {
    GeneratorId::named(name)
}

fn cfg(tag: &str) -> ConfigId {
    ConfigId::from_canon(tag.as_bytes())
}

fn fixtures() -> Vec<Fixture> {
    let mut out = Vec::new();

    out.push(Fixture {
        name: "empty",
        description: "a manifest declaring nothing realizes nothing",
        manifest: GeneratorSemanticsV1::new(),
        declared: vec![],
    });

    let mut diagonal = GeneratorSemanticsV1::new();
    diagonal.declare_diagonal(g("literal-equality.refl@1"));
    out.push(Fixture {
        name: "diagonal",
        description: "the literal-equality lane: one generator, the diagonal",
        manifest: diagonal,
        declared: vec![("literal-equality.refl@1", Decl::Diagonal)],
    });

    let mut single = GeneratorSemanticsV1::new();
    single.declare_rows(g("g_a@1"), [(cfg("x0"), cfg("x1"))]);
    out.push(Fixture {
        name: "single_row",
        description: "the L3 shape: one generator, one exact (src, dst) row",
        manifest: single,
        declared: vec![("g_a@1", Decl::Rows(vec![("x0", "x1")]))],
    });

    let mut multi = GeneratorSemanticsV1::new();
    multi.declare_rows(
        g("g_b@1"),
        [
            (cfg("x0"), cfg("x1")),
            (cfg("x1"), cfg("x2")),
            (cfg("x2"), cfg("x3")),
        ],
    );
    out.push(Fixture {
        name: "multi_row",
        description: "one generator relating three ordered pairs",
        manifest: multi,
        declared: vec![(
            "g_b@1",
            Decl::Rows(vec![("x0", "x1"), ("x1", "x2"), ("x2", "x3")]),
        )],
    });

    let mut mixed = GeneratorSemanticsV1::new();
    mixed.declare_diagonal(g("g_refl@1"));
    mixed.declare_rows(g("g_step@1"), [(cfg("x0"), cfg("x1"))]);
    out.push(Fixture {
        name: "mixed_forms",
        description: "both relation forms in one manifest, exercising ordinals 0 and 1",
        manifest: mixed,
        declared: vec![
            ("g_refl@1", Decl::Diagonal),
            ("g_step@1", Decl::Rows(vec![("x0", "x1")])),
        ],
    });

    out
}

/// Spell out the canonical bytes with primitive `CanonWriter` calls only —
/// never `GeneratorSemanticsV1`/`SettlementRelationV1::canon_write`. This is
/// the independent reproduction that keeps the vectors honest.
fn independent_manifest(declared: &[(&'static str, Decl)]) -> Vec<u8> {
    let mut w = CanonWriter::new();
    // Field 1: marker. Field 2: format version. Frozen ADR-0020 D3.
    w.write_bytes(b"brix.semantic.generator-semantics");
    w.write_uint(1);
    // Field 3: the relations map, keyed by canonical GeneratorId bytes.
    w.write_map(declared.iter().map(|(name, decl)| {
        let mut key = CanonWriter::new();
        key.write_bytes(GeneratorId::named(name).digest().as_bytes());

        let mut value = CanonWriter::new();
        match decl {
            // Ordinal 1, empty payload.
            Decl::Diagonal => value.write_enum(1, |_| {}),
            // Ordinal 0, a set of rows; each row is src bytes then dst bytes.
            Decl::Rows(rows) => value.write_enum(0, |v| {
                v.write_set(rows.iter().map(|(src, dst)| {
                    let mut row = CanonWriter::new();
                    row.write_bytes(cfg(src).digest().as_bytes());
                    row.write_bytes(cfg(dst).digest().as_bytes());
                    row.finish()
                }));
            }),
        }
        (key.finish(), value.finish())
    }));
    w.finish()
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

fn build_manifest() -> String {
    let cases = fixtures();
    let mut out = String::new();
    out.push_str("{\n");
    out.push_str("  \"format\": \"brix.semantic.GeneratorSemanticsV1\",\n");
    out.push_str("  \"version\": 1,\n");
    out.push_str("  \"adr\": \"ADR-0020\",\n");
    out.push_str("  \"cases\": [\n");

    let total = cases.len();
    for (i, fixture) in cases.iter().enumerate() {
        let bytes = fixture.manifest.canon_bytes();
        out.push_str("    {\n");
        out.push_str(&format!("      \"name\": {},\n", json_str(fixture.name)));
        out.push_str(&format!(
            "      \"description\": {},\n",
            json_str(fixture.description)
        ));
        out.push_str(&format!("      \"canon_hex\": \"{}\",\n", to_hex(&bytes)));
        out.push_str(&format!(
            "      \"semantics_id\": \"{}\"\n",
            fixture.manifest.id().to_hex()
        ));
        out.push_str("    }");
        out.push_str(if i + 1 == total { "\n" } else { ",\n" });
    }

    out.push_str("  ]\n");
    out.push_str("}\n");
    out
}

fn manifest_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("vectors")
        .join("generator_semantics_v1.json")
}

#[test]
fn generator_semantics_vectors_are_frozen() {
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
        "generator semantics vectors drifted from {}.\n\
         The v1 manifest encoding is frozen ABI — a drift changes which oracle \
         a settlement audit receipt says was executed (ADR-0020 D3/D5). \
         Regenerate deliberately with BLESS_VECTORS=1 only if you intend a \
         compatibility break.",
        path.display()
    );
}

#[test]
fn generator_semantics_vectors_reproduced_by_primitive_canon_writes() {
    for fixture in fixtures() {
        let produced = fixture.manifest.canon_bytes();
        let independent = independent_manifest(&fixture.declared);
        assert_eq!(
            to_hex(&produced),
            to_hex(&independent),
            "case {} must be reproducible without the artifact's own canon_write",
            fixture.name
        );
    }
}

#[test]
fn case_names_unique() {
    let cases = fixtures();
    let mut names: Vec<&str> = cases.iter().map(|c| c.name).collect();
    names.sort_unstable();
    let before = names.len();
    names.dedup();
    assert_eq!(before, names.len(), "vector case names must be unique");
}

/// The property the whole artifact exists for: distinct declarations get
/// distinct ids, so a substituted oracle is detectable (ADR-0020 §4).
#[test]
fn every_vector_case_has_a_distinct_identity() {
    let cases = fixtures();
    let mut ids: Vec<String> = cases.iter().map(|c| c.manifest.id().to_hex()).collect();
    ids.sort();
    let before = ids.len();
    ids.dedup();
    assert_eq!(
        before,
        ids.len(),
        "two distinct declarations must never share a GeneratorSemanticsIdV1"
    );
}

/// The frozen constants, pinned independently of the encoder that uses them.
#[test]
fn marker_and_version_are_frozen() {
    assert_eq!(
        GENERATOR_SEMANTICS_MARKER_V1,
        b"brix.semantic.generator-semantics"
    );
    assert_eq!(GENERATOR_SEMANTICS_VERSION_V1, 1);
    // And the relation ordinals, reachable from outside the crate.
    let mut w = CanonWriter::new();
    SettlementRelationV1::Diagonal.canon_write(&mut w);
    let mut expected = CanonWriter::new();
    expected.write_enum(1, |_| {});
    assert_eq!(w.finish(), expected.finish());
}
