//! Frozen `TreeDerivation` artifact vectors (ADR-0017 §5 D1, ADR-0013 §7).
//!
//! Four cases — a bare leaf, a `Seq`, a `Tensor`, and a mixed
//! `Seq(a, Tensor(b, c))` over a `Prod` middle — each in both verification
//! forms, are encoded through the production `Canonical` impl and frozen in
//! `vectors/tree_derivation_v1.json` together with their canonical bytes and
//! `TreeDerivationId`s.
//!
//! Two consumers guard the manifest, mirroring `kernel_certificate_v1`:
//!
//! 1. `tree_derivation_vectors_are_frozen` — the production encoder must keep
//!    reproducing the committed bytes (regenerate with `BLESS_VECTORS=1`);
//! 2. `tree_derivation_vectors_reproduced_by_primitive_canon_writes` — a second
//!    construction path spelling out every enum ordinal and field with
//!    primitive `CanonWriter` operations, never calling `TreeDerivation`'s own
//!    `canon_write`, so a vector cannot be vacuously satisfied by the code it
//!    guards.
//!
//! After the freeze this manifest is append-only: an existing case may never
//! change without a new artifact version (ADR-0013 §7). The pair of forms per
//! case is deliberate — it is the vector-level statement of the property the
//! artifact exists for: a built and a checked derivation over an identical tree
//! must not share an id.

use std::path::{Path, PathBuf};

use brix_canon::{CanonWriter, Canonical, Digest, Domain};
use brix_semantic::{
    ConfigId, GeneratorId, GeneratorRegistry, RealizesTree, TreeDerivation, TreeObj,
    TreeVerification,
};

struct Fixture {
    name: &'static str,
    description: &'static str,
    tree: RealizesTree,
}

fn cfg(tag: &str) -> ConfigId {
    ConfigId::from_canon(tag.as_bytes())
}

fn leaf(name: &str, src: TreeObj, dst: TreeObj) -> RealizesTree {
    RealizesTree::Leaf {
        generator: GeneratorId::named(name),
        src,
        dst,
    }
}

fn atom(tag: &str) -> TreeObj {
    TreeObj::Atom(cfg(tag))
}

fn fixtures() -> Vec<Fixture> {
    let a = leaf("g_a@1", atom("x0"), atom("x1"));
    let b = leaf("g_b@1", atom("x1"), atom("x2"));
    let c = leaf("g_c@1", atom("x2"), atom("x3"));

    // `a2: x0 → x1⊗x2`, so the Seq middle matches the Tensor's Prod source.
    let a2 = leaf(
        "g_a@1",
        atom("x0"),
        TreeObj::Prod(Box::new(atom("x1")), Box::new(atom("x2"))),
    );
    let b2 = leaf("g_b@1", atom("x1"), atom("x3"));
    let c2 = leaf("g_c@1", atom("x2"), atom("x4"));

    vec![
        Fixture {
            name: "leaf",
            description: "a single generator step x0 -> x1",
            tree: a.clone(),
        },
        Fixture {
            name: "seq",
            description: "Seq(a: x0->x1, b: x1->x2), matching middle",
            tree: RealizesTree::Seq {
                left: Box::new(a),
                right: Box::new(b.clone()),
            },
        },
        Fixture {
            name: "tensor",
            description: "Tensor(b: x1->x2, c: x2->x3), independent branches",
            tree: RealizesTree::Tensor {
                left: Box::new(b),
                right: Box::new(c),
            },
        },
        Fixture {
            name: "seq_over_tensor",
            description: "Seq(a: x0->x1(x)x2, Tensor(b: x1->x3, c: x2->x4)) over a Prod middle",
            tree: RealizesTree::Seq {
                left: Box::new(a2),
                right: Box::new(RealizesTree::Tensor {
                    left: Box::new(b2),
                    right: Box::new(c2),
                }),
            },
        },
    ]
}

/// Spell out the canonical bytes with primitive `CanonWriter` calls only —
/// never `TreeObj`/`RealizesTree`/`TreeDerivation::canon_write`. This is the
/// independent reproduction that keeps the vectors honest.
fn independent_tree_obj(obj: &TreeObj, w: &mut CanonWriter) {
    match obj {
        TreeObj::Atom(config) => w.write_enum(0, |w| w.write_bytes(config.digest().as_bytes())),
        TreeObj::Prod(left, right) => w.write_enum(1, |w| {
            independent_tree_obj(left, w);
            independent_tree_obj(right, w);
        }),
    }
}

fn independent_tree(tree: &RealizesTree, w: &mut CanonWriter) {
    match tree {
        RealizesTree::Leaf {
            generator,
            src,
            dst,
        } => w.write_enum(0, |w| {
            w.write_bytes(generator.digest().as_bytes());
            independent_tree_obj(src, w);
            independent_tree_obj(dst, w);
        }),
        RealizesTree::Seq { left, right } => w.write_enum(1, |w| {
            independent_tree(left, w);
            independent_tree(right, w);
        }),
        RealizesTree::Tensor { left, right } => w.write_enum(2, |w| {
            independent_tree(left, w);
            independent_tree(right, w);
        }),
    }
}

fn independent_derivation(tree: &RealizesTree, verification: TreeVerification) -> Vec<u8> {
    let mut w = CanonWriter::new();
    independent_tree(tree, &mut w);
    w.write_enum(
        match verification {
            TreeVerification::Recorded => 0,
            TreeVerification::StructureVerified => 1,
        },
        |_| {},
    );
    w.finish()
}

fn derivation(tree: &RealizesTree, verification: TreeVerification) -> TreeDerivation {
    let recorded = TreeDerivation::recorded(tree.clone());
    match verification {
        TreeVerification::Recorded => recorded,
        // ADR-0019 D5/D7: the verified form is earned here too — the frozen
        // bytes are the same, but they are now reached through the real
        // transition rather than a stamp.
        //
        // This fixture registry is built from the vector tree's own leaves.
        // That would be circular in a *checker* and is exactly what
        // `verify_structure`'s doc forbids there — but this is a vector
        // fixture whose job is to produce a known-good artifact and freeze
        // its encoding, not to test membership. The real membership negative
        // lives in `soc-regimes`' `tree_audit`, against the regime's declared
        // registry.
        TreeVerification::StructureVerified => {
            let mut registry = GeneratorRegistry::new();
            for leaf in tree.leaves() {
                if let RealizesTree::Leaf { generator, .. } = leaf {
                    registry.insert(*generator);
                }
            }
            recorded
                .verify_structure(&tree.src(), &tree.dst(), &registry)
                .expect("the frozen vector trees are well-formed by construction")
        }
    }
}

const FORMS: [(&str, TreeVerification); 2] = [
    ("recorded", TreeVerification::Recorded),
    ("structure_verified", TreeVerification::StructureVerified),
];

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
    out.push_str("  \"format\": \"brix.semantic.TreeDerivation\",\n");
    out.push_str("  \"version\": 1,\n");
    out.push_str("  \"adr\": \"ADR-0017\",\n");
    out.push_str("  \"cases\": [\n");

    let total = cases.len() * FORMS.len();
    let mut emitted = 0usize;
    for fixture in &cases {
        for (form_name, form) in FORMS {
            let d = derivation(&fixture.tree, form);
            let bytes = d.canon_bytes();
            out.push_str("    {\n");
            out.push_str(&format!("      \"name\": {},\n", json_str(fixture.name)));
            out.push_str(&format!("      \"form\": {},\n", json_str(form_name)));
            out.push_str(&format!(
                "      \"description\": {},\n",
                json_str(fixture.description)
            ));
            out.push_str(&format!("      \"canon_hex\": \"{}\",\n", to_hex(&bytes)));
            out.push_str(&format!(
                "      \"derivation_id\": \"{}\"\n",
                d.id().to_hex()
            ));
            out.push_str("    }");
            emitted += 1;
            out.push_str(if emitted == total { "\n" } else { ",\n" });
        }
    }

    out.push_str("  ]\n");
    out.push_str("}\n");
    out
}

fn manifest_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("vectors")
        .join("tree_derivation_v1.json")
}

#[test]
fn tree_derivation_vectors_are_frozen() {
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
        "tree derivation vectors drifted from {}.\n\
         The v1 artifact encoding is frozen ABI — this is a compatibility break, \
         not a refresh. If the change is intended and versioned, regenerate with \
         BLESS_VECTORS=1 and review the diff by hand.",
        path.display()
    );
}

#[test]
fn tree_derivation_vectors_reproduced_by_primitive_canon_writes() {
    for fixture in fixtures() {
        for (form_name, form) in FORMS {
            let produced = derivation(&fixture.tree, form).canon_bytes();
            let independent = independent_derivation(&fixture.tree, form);
            assert_eq!(
                to_hex(&independent),
                to_hex(&produced),
                "{}/{form_name}: independent canonical bytes differ from the production encoder",
                fixture.name
            );

            let independent_id = Digest::of(Domain::Value, &independent);
            assert_eq!(
                independent_id.to_hex(),
                derivation(&fixture.tree, form).id().digest().to_hex(),
                "{}/{form_name}: independent id differs from the production id",
                fixture.name
            );
        }
    }
}

#[test]
fn the_two_forms_never_share_an_id() {
    // The vector-level statement of what the artifact is for: a derivation the
    // inference pass built and one the checker verified are different evidence,
    // because they *are* different evidence (ADR-0017 §5 D1).
    for fixture in fixtures() {
        let recorded = derivation(&fixture.tree, TreeVerification::Recorded);
        let verified = derivation(&fixture.tree, TreeVerification::StructureVerified);
        assert_ne!(
            recorded.id(),
            verified.id(),
            "{}: recorded and structure-verified MUST NOT collide",
            fixture.name
        );
    }
}
