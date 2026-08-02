//! Frozen native proof-certificate envelope vectors (ADR-0013).
//!
//! Three accepted proofs — a simple implication, a realization composition, and
//! a finite-sum/coverage-shaped case — are encoded through the production v1
//! envelope and frozen in `vectors/kernel_certificate_v1.json` together with
//! their material bytes and certificate ids.
//!
//! Two consumers guard the manifest:
//!
//! 1. `kernel_certificate_vectors_are_frozen` — the production encoder must keep
//!    reproducing the committed bytes (regenerate with `BLESS_VECTORS=1`);
//! 2. `kernel_certificate_vectors_reproduced_by_primitive_canon_writes` — a
//!    second construction path that spells out every pinned envelope field with
//!    primitive `CanonWriter` operations and never calls the production
//!    encoder, so a vector cannot be vacuously satisfied by the code it guards.
//!
//! After the freeze this manifest is append-only: an existing case may never
//! change without a new envelope format version (ADR-0013 §7).

use std::path::{Path, PathBuf};

use brix_canon::{CanonWriter, Canonical, Digest, Domain};
use brix_kernel::{
    acceptance, certificate_id_v1, encode_material_v1, native_verifier, Budget,
    CertificateMaterialV1, ExplicitTerm, ObjectTerm, Prop, TermKind, Var, Verdict,
    CERTIFICATE_FORMAT_V1, KERNEL_PROFILE_V1, NATIVE_VERIFIER_NAME,
};
use brix_semantic::{ContextId, PropositionId};

/// Budget large enough that every fixture is accepted on its merits.
const VECTOR_BUDGET: Budget = Budget {
    max_steps: 1000,
    max_depth: 1000,
};

/// One frozen certificate case: the artifacts, plus the human-readable shapes
/// recorded in the manifest so the vector can be replayed from JSON alone.
struct Fixture {
    name: &'static str,
    description: &'static str,
    context_seed: &'static str,
    context: ContextId,
    proposition: Prop,
    proposition_shape: &'static str,
    term: ExplicitTerm,
    term_shape: &'static str,
}

fn fixtures() -> Vec<Fixture> {
    vec![
        identity_implication(),
        realizes_composition(),
        finite_sum_case(),
    ]
}

/// `P -> P` proved by the identity term. The smallest accepted proof there is.
fn identity_implication() -> Fixture {
    let context = ContextId::from_canon(b"context_a");
    let p = Prop::Atom(PropositionId::from_canon(b"P"));
    let proposition = Prop::Impl(Box::new(p.clone()), Box::new(p));
    let term = ExplicitTerm::new(
        context,
        TermKind::Lam {
            var_name: Some("x".into()),
            body: Box::new(TermKind::Hyp(Var::Index(0))),
        },
    );

    Fixture {
        name: "identity_implication",
        description: "P -> P proved by the identity term \\x. x",
        context_seed: "context_a",
        context,
        proposition,
        proposition_shape: "Impl(Atom(P), Atom(P))",
        term,
        term_shape: "ExplicitTerm(context_a, Lam(Some('x'), Hyp(Index(0))))",
    }
}

/// Profile 1.1 realization composition (ADR-0004): from `g1` realizing `x -> y`
/// and `g2` realizing `y -> z`, conclude `compose(g2, g1)` realizes `x -> z`.
fn realizes_composition() -> Fixture {
    let context = ContextId::from_canon(b"context_a");
    let g1 = ObjectTerm::Const(PropositionId::from_canon(b"g1"));
    let g2 = ObjectTerm::Const(PropositionId::from_canon(b"g2"));
    let x = ObjectTerm::Const(PropositionId::from_canon(b"obj_a"));
    let y = ObjectTerm::Const(PropositionId::from_canon(b"obj_b"));
    let z = ObjectTerm::Const(PropositionId::from_canon(b"obj_c"));

    let premise_one = Prop::Realizes(g1.clone(), x.clone(), y.clone());
    let premise_two = Prop::Realizes(g2.clone(), y, z.clone());
    let goal = Prop::Realizes(ObjectTerm::Compose(Box::new(g2), Box::new(g1)), x, z);

    let proposition = Prop::Impl(
        Box::new(premise_one),
        Box::new(Prop::Impl(Box::new(premise_two), Box::new(goal))),
    );

    let term = ExplicitTerm::new(
        context,
        TermKind::Lam {
            var_name: Some("h_p1".into()),
            body: Box::new(TermKind::Lam {
                var_name: Some("h_p2".into()),
                body: Box::new(TermKind::RealizesComp {
                    left: Box::new(TermKind::Hyp(Var::Named("h_p1".into()))),
                    right: Box::new(TermKind::Hyp(Var::Named("h_p2".into()))),
                }),
            }),
        },
    );

    Fixture {
        name: "realizes_composition",
        description: "Realizes(g1,x,y) -> Realizes(g2,y,z) -> Realizes(compose(g2,g1),x,z)",
        context_seed: "context_a",
        context,
        proposition,
        proposition_shape: concat!(
            "Impl(Realizes(Const(g1), Const(obj_a), Const(obj_b)), ",
            "Impl(Realizes(Const(g2), Const(obj_b), Const(obj_c)), ",
            "Realizes(Compose(Const(g2), Const(g1)), Const(obj_a), Const(obj_c))))"
        ),
        term,
        term_shape: concat!(
            "ExplicitTerm(context_a, Lam(Some('h_p1'), Lam(Some('h_p2'), ",
            "RealizesComp(Hyp(Named('h_p1')), Hyp(Named('h_p2'))))))"
        ),
    }
}

/// Finite-sum coverage shape: `P + Q -> Q + P` proved by a total two-arm case
/// split — the certified-exhaustiveness shape ADR-0011 builds on.
fn finite_sum_case() -> Fixture {
    let context = ContextId::from_canon(b"context_a");
    let p = Prop::Atom(PropositionId::from_canon(b"P"));
    let q = Prop::Atom(PropositionId::from_canon(b"Q"));
    let proposition = Prop::Impl(
        Box::new(Prop::Sum(Box::new(p.clone()), Box::new(q.clone()))),
        Box::new(Prop::Sum(Box::new(q), Box::new(p))),
    );

    let term = ExplicitTerm::new(
        context,
        TermKind::Lam {
            var_name: Some("s".into()),
            body: Box::new(TermKind::Case {
                discriminant: Box::new(TermKind::Hyp(Var::Named("s".into()))),
                left_var: Some("x".into()),
                left_body: Box::new(TermKind::Inr(Box::new(TermKind::Hyp(Var::Named(
                    "x".into(),
                ))))),
                right_var: Some("y".into()),
                right_body: Box::new(TermKind::Inl(Box::new(TermKind::Hyp(Var::Named(
                    "y".into(),
                ))))),
            }),
        },
    );

    Fixture {
        name: "finite_sum_case",
        description: "P + Q -> Q + P proved by a total two-arm case split",
        context_seed: "context_a",
        context,
        proposition,
        proposition_shape: "Impl(Sum(Atom(P), Atom(Q)), Sum(Atom(Q), Atom(P)))",
        term,
        term_shape: concat!(
            "ExplicitTerm(context_a, Lam(Some('s'), Case(Hyp(Named('s')), ",
            "Some('x'), Inr(Hyp(Named('x'))), Some('y'), Inl(Hyp(Named('y'))))))"
        ),
    }
}

// ---------------------------------------------------------------------------
// The independent construction path.
//
// This deliberately spells out the marker, version, profile, verifier, context,
// and payload framing with primitive `CanonWriter` calls and repeats the frozen
// string literals rather than importing the constants, so a typo'd constant in
// `certificate.rs` cannot silently agree with itself. It MUST NOT call
// `encode_material_v1` or `certificate_id_v1`.
//
// The proposition and term payloads reuse their `Canonical` impls: those enum
// ordinals are a separately frozen ABI (`term.rs`, append-only), and
// `CanonWriter::write_raw` is crate-private, so a proof tree cannot be
// hand-assembled from outside `brix-canon` anyway.
// ---------------------------------------------------------------------------

/// Rebuild `VerifierId::named("brix.kernel@0.1")`'s digest from first
/// principles: a canon string preimage hashed in the value domain.
fn independent_verifier_digest() -> Digest {
    let mut preimage = CanonWriter::new();
    preimage.write_bytes(b"brix.kernel@0.1");
    Digest::of(Domain::Value, &preimage.finish())
}

/// Rebuild the pinned v1 preimage field by field.
fn independent_material(context: &ContextId, proposition: &Prop, term: &ExplicitTerm) -> Vec<u8> {
    let mut w = CanonWriter::new();
    w.write_bytes(b"brix.kernel.certificate");
    w.write_uint(1);
    w.write_str("brix.kernel.profile@1.2");
    w.write_bytes(independent_verifier_digest().as_bytes());
    w.write_bytes(context.digest().as_bytes());
    w.write_bytes(&proposition.canon_bytes());
    w.write_bytes(&term.canon_bytes());
    w.finish()
}

// ---------------------------------------------------------------------------
// Manifest rendering. Hand-built ASCII JSON, like `brix-canon`'s vector test —
// `brix-kernel` carries no dependencies beyond `brix-canon`/`brix-semantic` and
// gains none for its tests.
// ---------------------------------------------------------------------------

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
    let mut out = String::new();
    out.push_str("{\n");
    out.push_str("  \"format\": \"brix.kernel.certificate\",\n");
    out.push_str(&format!("  \"version\": {CERTIFICATE_FORMAT_V1},\n"));
    out.push_str(&format!(
        "  \"profile\": {},\n",
        json_str(KERNEL_PROFILE_V1)
    ));
    out.push_str(&format!(
        "  \"verifier_name\": {},\n",
        json_str(NATIVE_VERIFIER_NAME)
    ));
    out.push_str(&format!(
        "  \"verifier_id\": \"{}\",\n",
        native_verifier().to_hex()
    ));
    out.push_str(
        "  \"note\": \"Frozen native proof-certificate envelope vectors (ADR-0013). \
Append-only: an existing case may never change without a new envelope format version. \
Regenerate with BLESS_VECTORS=1.\",\n",
    );
    out.push_str("  \"cases\": [\n");

    let cases = fixtures();
    for (index, fixture) in cases.iter().enumerate() {
        let material = encode_material_v1(&CertificateMaterialV1::new(
            &fixture.context,
            &fixture.proposition,
            &fixture.term,
        ));
        let certificate_id = certificate_id_v1(&CertificateMaterialV1::new(
            &fixture.context,
            &fixture.proposition,
            &fixture.term,
        ));

        out.push_str("    {\n");
        out.push_str(&format!("      \"name\": {},\n", json_str(fixture.name)));
        out.push_str(&format!(
            "      \"description\": {},\n",
            json_str(fixture.description)
        ));
        out.push_str(&format!(
            "      \"context_seed\": {},\n",
            json_str(fixture.context_seed)
        ));
        out.push_str(&format!(
            "      \"context_id\": \"{}\",\n",
            fixture.context.to_hex()
        ));
        out.push_str(&format!(
            "      \"proposition\": {},\n",
            json_str(fixture.proposition_shape)
        ));
        out.push_str(&format!(
            "      \"proposition_hex\": \"{}\",\n",
            to_hex(&fixture.proposition.canon_bytes())
        ));
        out.push_str(&format!(
            "      \"term\": {},\n",
            json_str(fixture.term_shape)
        ));
        out.push_str(&format!(
            "      \"term_hex\": \"{}\",\n",
            to_hex(&fixture.term.canon_bytes())
        ));
        out.push_str(&format!(
            "      \"material_hex\": \"{}\",\n",
            to_hex(&material)
        ));
        out.push_str(&format!(
            "      \"certificate_id\": \"{}\"\n",
            certificate_id.to_hex()
        ));
        out.push_str("    }");
        out.push_str(if index + 1 == cases.len() {
            "\n"
        } else {
            ",\n"
        });
    }

    out.push_str("  ]\n");
    out.push_str("}\n");
    out
}

fn manifest_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("vectors")
        .join("kernel_certificate_v1.json")
}

#[test]
fn kernel_certificate_vectors_are_frozen() {
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
        "kernel certificate vectors drifted from {}.\n\
         The v1 envelope is frozen ABI — this is a compatibility break, not a \
         refresh. If the change is intended and versioned, regenerate with \
         BLESS_VECTORS=1 and review the diff by hand.",
        path.display()
    );
}

#[test]
fn kernel_certificate_vectors_reproduced_by_primitive_canon_writes() {
    for fixture in fixtures() {
        let material = encode_material_v1(&CertificateMaterialV1::new(
            &fixture.context,
            &fixture.proposition,
            &fixture.term,
        ));
        let independent =
            independent_material(&fixture.context, &fixture.proposition, &fixture.term);

        assert_eq!(
            to_hex(&independent),
            to_hex(&material),
            "{}: independent envelope bytes differ from the production encoder",
            fixture.name
        );

        let independent_id = Digest::of(Domain::Value, &independent);
        let produced = certificate_id_v1(&CertificateMaterialV1::new(
            &fixture.context,
            &fixture.proposition,
            &fixture.term,
        ));
        assert_eq!(
            independent_id.to_hex(),
            produced.to_hex(),
            "{}: independent certificate id differs from the production id",
            fixture.name
        );
    }
}

#[test]
fn every_vector_case_is_accepted_by_the_kernel() {
    for fixture in fixtures() {
        let verdict = acceptance(
            &fixture.context,
            &fixture.proposition,
            &fixture.term,
            VECTOR_BUDGET,
        );

        match verdict {
            Verdict::Accepted(certificate) => {
                assert_eq!(
                    certificate.verifier,
                    native_verifier(),
                    "{}: accepted under a foreign verifier",
                    fixture.name
                );
                assert_eq!(
                    certificate.certificate_id,
                    certificate_id_v1(&CertificateMaterialV1::new(
                        &fixture.context,
                        &fixture.proposition,
                        &fixture.term,
                    )),
                    "{}: accepted id differs from the frozen envelope id",
                    fixture.name
                );
            }
            other => panic!("{}: expected Accepted, got {other:?}", fixture.name),
        }
    }
}

#[test]
fn vector_case_names_are_unique() {
    let mut names: Vec<&'static str> = fixtures().iter().map(|f| f.name).collect();
    let total = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), total, "vector case names must be unique");
}
