//! Certificate identity integration tests (ADR-0013), driven through the real
//! `acceptance` entry point rather than the `certificate` module's encoder
//! directly. See `crates/brix-kernel/src/certificate.rs` for the pinned v1
//! preimage this identity is built from.

use brix_canon::{CanonWriter, Canonical};
use brix_kernel::{
    acceptance, certificate_id_v1, decode_material_v1, encode_material_v1, native_verifier,
    validate_material_v1, Budget, CertificateMaterialV1, ExplicitTerm, Prop, TermKind, Var,
    Verdict, CERTIFICATE_FORMAT_V1, CERTIFICATE_MARKER, KERNEL_PROFILE_V1,
};
use brix_semantic::{ContextId, Outcome, PropositionId, VerifierId};

fn sample_context_a() -> ContextId {
    ContextId::from_canon(b"context_a")
}

fn sample_context_b() -> ContextId {
    ContextId::from_canon(b"context_b")
}

/// `P -> P`, for the given atom name.
fn identity_goal(atom: &[u8]) -> Prop {
    let p = Prop::Atom(PropositionId::from_canon(atom));
    Prop::Impl(Box::new(p.clone()), Box::new(p))
}

/// `\x. x`, proving `identity_goal(_)` under `ctx`.
fn identity_term(ctx: ContextId) -> ExplicitTerm {
    ExplicitTerm::new(
        ctx,
        TermKind::Lam {
            var_name: Some("x".into()),
            body: Box::new(TermKind::Hyp(Var::Index(0))),
        },
    )
}

/// `P -> (P -> P)`.
fn nested_goal() -> Prop {
    let p = Prop::Atom(PropositionId::from_canon(b"P"));
    Prop::Impl(
        Box::new(p.clone()),
        Box::new(Prop::Impl(Box::new(p.clone()), Box::new(p))),
    )
}

/// `\x. \y. y` — proves [`nested_goal`] by returning the innermost hypothesis.
fn nested_term_returns_inner(ctx: ContextId) -> ExplicitTerm {
    ExplicitTerm::new(
        ctx,
        TermKind::Lam {
            var_name: Some("x".into()),
            body: Box::new(TermKind::Lam {
                var_name: Some("y".into()),
                body: Box::new(TermKind::Hyp(Var::Index(0))),
            }),
        },
    )
}

/// `\x. \y. x` — proves [`nested_goal`] by returning the outer hypothesis: a
/// different (but equally valid) proof term for the same proposition.
fn nested_term_returns_outer(ctx: ContextId) -> ExplicitTerm {
    ExplicitTerm::new(
        ctx,
        TermKind::Lam {
            var_name: Some("x".into()),
            body: Box::new(TermKind::Lam {
                var_name: Some("y".into()),
                body: Box::new(TermKind::Hyp(Var::Index(1))),
            }),
        },
    )
}

fn accepted_certificate(verdict: &Verdict) -> &brix_kernel::Certificate {
    match verdict {
        Verdict::Accepted(cert) => cert,
        other => panic!("expected Accepted, got {other:?}"),
    }
}

#[test]
fn accepted_certificate_id_equals_public_v1_encoder() {
    let ctx = sample_context_a();
    let goal = identity_goal(b"P");
    let term = identity_term(ctx);
    let budget = Budget::new(100, 100);

    let verdict = acceptance(&ctx, &goal, &term, budget);
    let cert = accepted_certificate(&verdict);

    let expected = certificate_id_v1(&CertificateMaterialV1::new(&ctx, &goal, &term));
    assert_eq!(cert.certificate_id, expected);
}

#[test]
fn accepted_certificate_names_the_native_verifier() {
    let ctx = sample_context_a();
    let goal = identity_goal(b"P");
    let term = identity_term(ctx);
    let budget = Budget::new(100, 100);

    let verdict = acceptance(&ctx, &goal, &term, budget);
    let cert = accepted_certificate(&verdict);

    assert_eq!(cert.verifier, native_verifier());
}

#[test]
fn certificate_id_is_independent_of_budget() {
    let ctx = sample_context_a();
    let goal = identity_goal(b"P");
    let term = identity_term(ctx);

    let small = Budget::new(50, 50);
    let large = Budget::new(100_000, 100_000);

    let verdict_small = acceptance(&ctx, &goal, &term, small);
    let verdict_large = acceptance(&ctx, &goal, &term, large);

    let cert_small = accepted_certificate(&verdict_small);
    let cert_large = accepted_certificate(&verdict_large);

    assert_eq!(cert_small.certificate_id, cert_large.certificate_id);
}

#[test]
fn certificate_id_separates_contexts() {
    let ctx_a = sample_context_a();
    let ctx_b = sample_context_b();
    let goal = identity_goal(b"P");
    let budget = Budget::new(100, 100);

    let verdict_a = acceptance(&ctx_a, &goal, &identity_term(ctx_a), budget);
    let verdict_b = acceptance(&ctx_b, &goal, &identity_term(ctx_b), budget);

    let cert_a = accepted_certificate(&verdict_a);
    let cert_b = accepted_certificate(&verdict_b);

    assert_ne!(cert_a.certificate_id, cert_b.certificate_id);
}

#[test]
fn certificate_id_separates_propositions() {
    let ctx = sample_context_a();
    let goal_p = identity_goal(b"P");
    let goal_q = identity_goal(b"Q");
    let budget = Budget::new(100, 100);

    // `\x. x` proves `P -> P` and `Q -> Q` alike — the term is structurally
    // identical, only the goal differs.
    let term = identity_term(ctx);

    let verdict_p = acceptance(&ctx, &goal_p, &term, budget);
    let verdict_q = acceptance(&ctx, &goal_q, &term, budget);

    let cert_p = accepted_certificate(&verdict_p);
    let cert_q = accepted_certificate(&verdict_q);

    assert_ne!(cert_p.certificate_id, cert_q.certificate_id);
}

#[test]
fn certificate_id_separates_terms() {
    let ctx = sample_context_a();
    let goal = nested_goal();
    let budget = Budget::new(100, 100);

    let term_inner = nested_term_returns_inner(ctx);
    let term_outer = nested_term_returns_outer(ctx);

    let verdict_inner = acceptance(&ctx, &goal, &term_inner, budget);
    let verdict_outer = acceptance(&ctx, &goal, &term_outer, budget);

    let cert_inner = accepted_certificate(&verdict_inner);
    let cert_outer = accepted_certificate(&verdict_outer);

    assert_ne!(cert_inner.certificate_id, cert_outer.certificate_id);
}

#[test]
fn accepted_certificate_material_validates_against_its_own_artifacts() {
    let ctx = sample_context_a();
    let goal = identity_goal(b"P");
    let term = identity_term(ctx);
    let budget = Budget::new(100, 100);

    let verdict = acceptance(&ctx, &goal, &term, budget);
    let cert = accepted_certificate(&verdict);

    let material = CertificateMaterialV1::new(&ctx, &goal, &term);
    let bytes = encode_material_v1(&material);
    let id = validate_material_v1(&bytes, &material).expect("matching material must validate");

    assert_eq!(id, cert.certificate_id);
}

/// Hand-assemble a v1 envelope field-by-field, independent of
/// `encode_material_v1`.
#[allow(clippy::too_many_arguments)]
fn build_envelope(
    marker: &[u8],
    version: u64,
    profile: &[u8],
    verifier_digest: [u8; 32],
    context_digest: [u8; 32],
    prop_bytes: &[u8],
    term_bytes: &[u8],
) -> Vec<u8> {
    let mut w = CanonWriter::new();
    w.write_bytes(marker);
    w.write_uint(version);
    w.write_bytes(profile);
    w.write_bytes(&verifier_digest);
    w.write_bytes(&context_digest);
    w.write_bytes(prop_bytes);
    w.write_bytes(term_bytes);
    w.finish()
}

#[test]
fn no_malformed_envelope_mints_theorem_evidence() {
    let ctx = sample_context_a();
    let goal = identity_goal(b"P");
    let term = identity_term(ctx);
    let budget = Budget::new(100, 100);

    // Confirm the fixture really is provable before using it as the basis for
    // every malformed variant below.
    let verdict = acceptance(&ctx, &goal, &term, budget);
    assert!(
        matches!(verdict, Verdict::Accepted(_)),
        "fixture must be provable, got {verdict:?}"
    );

    let material = CertificateMaterialV1::new(&ctx, &goal, &term);
    let valid_bytes = encode_material_v1(&material);

    let verifier_digest = *native_verifier().0.as_bytes();
    let context_digest = *ctx.0.as_bytes();
    let prop_bytes = goal.canon_bytes();
    let term_bytes = term.canon_bytes();
    let foreign_verifier = VerifierId::named("foreign.verifier@1");

    let malformed: Vec<(&str, Vec<u8>)> = vec![
        (
            "bad marker",
            build_envelope(
                b"not.the.marker",
                CERTIFICATE_FORMAT_V1,
                KERNEL_PROFILE_V1.as_bytes(),
                verifier_digest,
                context_digest,
                &prop_bytes,
                &term_bytes,
            ),
        ),
        (
            "wrong version",
            build_envelope(
                CERTIFICATE_MARKER,
                999,
                KERNEL_PROFILE_V1.as_bytes(),
                verifier_digest,
                context_digest,
                &prop_bytes,
                &term_bytes,
            ),
        ),
        (
            "wrong profile",
            build_envelope(
                CERTIFICATE_MARKER,
                CERTIFICATE_FORMAT_V1,
                b"not.the.profile",
                verifier_digest,
                context_digest,
                &prop_bytes,
                &term_bytes,
            ),
        ),
        (
            "foreign verifier",
            build_envelope(
                CERTIFICATE_MARKER,
                CERTIFICATE_FORMAT_V1,
                KERNEL_PROFILE_V1.as_bytes(),
                *foreign_verifier.0.as_bytes(),
                context_digest,
                &prop_bytes,
                &term_bytes,
            ),
        ),
        ("trailing byte", {
            let mut bytes = valid_bytes.clone();
            bytes.push(0);
            bytes
        }),
        ("truncated", valid_bytes[..valid_bytes.len() - 1].to_vec()),
    ];

    for (label, bytes) in &malformed {
        assert!(
            decode_material_v1(bytes).is_err(),
            "{label}: decode_material_v1 must reject malformed bytes"
        );
        assert!(
            validate_material_v1(bytes, &material).is_err(),
            "{label}: validate_material_v1 must reject malformed bytes"
        );
    }

    // These are bytes, not a `Verdict` — there is no `acceptance` call that
    // could ever hand a `Verdict::Accepted` back for any of them, because
    // `Verdict::Accepted` is only ever constructed from `certificate_id_v1`
    // over a term that has *actually* type-checked (see `check.rs`). The
    // property the malformed table stands in for is: every non-`Accepted`
    // verdict — the only kind reachable from a bytes-level format violation —
    // is fail-closed on `Outcome`. Demonstrate that on a real rejected
    // `acceptance` path over the very same context/term used above.
    let unprovable_goal = Prop::Impl(
        Box::new(Prop::Atom(PropositionId::from_canon(b"P"))),
        Box::new(Prop::Atom(PropositionId::from_canon(b"Q"))),
    );
    let rejected = acceptance(&ctx, &unprovable_goal, &term, budget);
    assert!(!matches!(rejected, Verdict::Accepted(_)));
    assert_ne!(rejected.outcome(), Some(Outcome::Proven));
}

fn kernel_src(name: &str) -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join(name),
    )
    .expect("kernel source is readable from the checkout")
}

#[test]
fn certificate_identity_does_not_use_debug_or_display() {
    let files = [
        "check.rs",
        "certificate.rs",
        "term.rs",
        "verdict.rs",
        "lib.rs",
    ];

    // (a) None of the five kernel source files formats `{context:?}` or
    // references `cert_payload` in *code*. `certificate.rs`'s own module doc
    // comment mentions the old `format!("{context:?}:…")` payload by name (to
    // explain what ADR-0013 replaced) — comment lines are skipped so that
    // historical documentation doesn't trip a check about live code.
    for name in files {
        let src = kernel_src(name);
        for line in src.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            assert!(
                !line.contains("{context:?}"),
                "{name} must not format {{context:?}} in code: {line:?}"
            );
            assert!(
                !line.contains("cert_payload"),
                "{name} must not reference cert_payload in code: {line:?}"
            );
        }
    }

    // (b) Certificate identity construction lives wholly in `certificate.rs`:
    // `check.rs` must never construct a `CertificateId` itself.
    let check_src = kernel_src("check.rs");
    assert!(
        !check_src.contains("CertificateId::"),
        "check.rs must not construct CertificateId directly"
    );

    // (c) The encoder/identity region of `certificate.rs` — from
    // `encode_material_v1` through the end of `certificate_id_v1` — is
    // Debug/format!-free.
    let cert_src = kernel_src("certificate.rs");
    let encode_start = cert_src
        .find("pub fn encode_material_v1")
        .expect("encode_material_v1 present in certificate.rs");
    let cert_id_start = cert_src
        .find("pub fn certificate_id_v1")
        .expect("certificate_id_v1 present in certificate.rs");
    assert!(cert_id_start > encode_start);
    let region_end = cert_src[cert_id_start..]
        .find("fn read_digest")
        .map(|offset| cert_id_start + offset)
        .expect("read_digest follows certificate_id_v1 in certificate.rs");
    let region = &cert_src[encode_start..region_end];

    for needle in ["format!", "{:?}", ":?}", ".to_string()"] {
        assert!(
            !region.contains(needle),
            "encode_material_v1..certificate_id_v1 region must not contain {needle:?}"
        );
    }

    // (d) The whole production region of `certificate.rs` — everything before
    // the first `#[cfg(test)]` — is Debug/format!-free, skipping comment and
    // derive-attribute lines (which may legitimately mention these tokens in
    // prose or in `#[derive(..., Debug)]`).
    let test_mod_start = cert_src.find("#[cfg(test)]").unwrap_or(cert_src.len());
    let production = &cert_src[..test_mod_start];
    for line in production.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") || trimmed.starts_with("#[derive") {
            continue;
        }
        for needle in ["format!", ":?}", ".to_string()"] {
            assert!(
                !line.contains(needle),
                "certificate.rs production region must not contain {needle:?}: {line:?}"
            );
        }
    }
}
