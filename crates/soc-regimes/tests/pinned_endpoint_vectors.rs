//! The readable frozen manifest for ADR-0025 Stage A's pinned identities —
//! requirement (1) of ⟨D-REDERIVE⟩.
//!
//! Requirement (2), the re-derivation itself, is
//! `tests/pinned_endpoint_rederivation.rs`. Both are mandatory: the manifest
//! says *what value* a digest denotes in a form a reviewer can read, and the
//! re-derivation says the digest is *actually* that value's. Either alone is
//! insufficient — a manifest without re-derivation is an assertion, and a
//! re-derivation without a manifest is thirty-two opaque bytes.
//!
//! The manifest lives in `soc-regimes` rather than `brix-kernel` for the same
//! reason the re-derivation does: this crate owns the encoder, so a change here
//! is what would invalidate the file, and the failure should surface next to
//! its cause.
//!
//! Regenerate deliberately with `BLESS_VECTORS=1`, never as a reflex — a diff
//! in this file means a pinned identity changed meaning, which is a kernel-ABI
//! event.

use std::path::PathBuf;

use brix_kernel::{
    PinnedArithOp, PinnedNumericTy, PINNED_ENDPOINT_MANIFEST_V1, PINNED_ENDPOINT_VERSION_V1,
};
use soc_regimes::type_realization::{ArithOp, Ty};

fn manifest_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("vectors")
        .join("pinned_endpoint_identities_v1.json")
}

/// Built from **this crate's** values and encoder, not from the kernel's
/// constants: the manifest must record what the regime actually produces, so
/// that a drifted kernel constant shows up as a mismatch rather than being
/// copied into the file.
fn build_manifest() -> String {
    let mut out = String::new();
    out.push_str("{\n");
    out.push_str(&format!(
        "  \"format\": \"{}\",\n",
        String::from_utf8_lossy(PINNED_ENDPOINT_MANIFEST_V1)
    ));
    out.push_str(&format!("  \"version\": {PINNED_ENDPOINT_VERSION_V1},\n"));
    out.push_str("  \"adr\": \"ADR-0025\",\n");
    out.push_str("  \"decision\": \"D-PINNED / D-REDERIVE\",\n");
    out.push_str("  \"encoder\": \"soc-regimes::type_realization\",\n");
    out.push_str(
        "  \"note\": \"Identities the kernel pins as literal constants. The kernel \
         never decodes these and never reproduces this encoder; it compares digests. \
         A change here breaks brix-kernel's rows and is a kernel-ABI event.\",\n",
    );

    out.push_str("  \"numeric_types\": [\n");
    let types = PinnedNumericTy::ALL;
    for (i, t) in types.iter().enumerate() {
        let ty = Ty::Con(t.lattice_name());
        out.push_str("    {\n");
        out.push_str(&format!(
            "      \"value\": \"Ty::Con(\\\"{}\\\")\",\n",
            t.lattice_name()
        ));
        out.push_str(&format!(
            "      \"config_id\": \"{}\"\n",
            ty.config_id().digest().to_hex()
        ));
        out.push_str(if i + 1 == types.len() {
            "    }\n"
        } else {
            "    },\n"
        });
    }
    out.push_str("  ],\n");

    out.push_str("  \"operators\": [\n");
    let ops = [
        (PinnedArithOp::Add, ArithOp::Add),
        (PinnedArithOp::Sub, ArithOp::Sub),
        (PinnedArithOp::Mul, ArithOp::Mul),
        (PinnedArithOp::Div, ArithOp::Div),
    ];
    for (i, (pinned, op)) in ops.iter().enumerate() {
        out.push_str("    {\n");
        out.push_str(&format!(
            "      \"value\": \"ArithOp::{}\",\n",
            pinned.op_name()
        ));
        out.push_str(&format!(
            "      \"config_id\": \"{}\"\n",
            op.config_id().digest().to_hex()
        ));
        out.push_str(if i + 1 == ops.len() {
            "    }\n"
        } else {
            "    },\n"
        });
    }
    out.push_str("  ]\n");
    out.push_str("}\n");
    out
}

#[test]
fn pinned_endpoint_vectors_are_frozen() {
    let path = manifest_path();
    let generated = build_manifest();
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
        "pinned endpoint identities drifted from {}.\n\
         These digests are compiled into brix-kernel as literal constants \
         (ADR-0025 D-PINNED). A change here means the kernel's pinned table now \
         names different values — its rows stop matching, leaves go unclosed, \
         and grades silently stop moving.\n\
         If the encoding change is deliberate, re-pin brix-kernel's constants \
         in the same change and regenerate with BLESS_VECTORS=1.\n\n\
         generated:\n{generated}",
        path.display()
    );
}

/// The manifest's readable value strings are not decoration — each one is
/// parsed back and re-digested, so a mislabelled entry fails.
///
/// Without this, the manifest could name `Ty::Con("Int")` beside `Nat`'s digest
/// and every other test would still pass: the kernel's constant would be right,
/// the re-derivation would be right, and only the human-readable half — the
/// half ⟨D-REDERIVE⟩ requires precisely so a reviewer can check by eye — would
/// be lying.
#[test]
fn every_manifest_entry_names_the_value_it_digests() {
    let committed = std::fs::read_to_string(manifest_path()).expect("manifest is committed");

    for t in PinnedNumericTy::ALL {
        let label = format!("Ty::Con(\\\"{}\\\")", t.lattice_name());
        let digest = Ty::Con(t.lattice_name()).config_id().digest().to_hex();
        let idx = committed
            .find(&label)
            .unwrap_or_else(|| panic!("manifest is missing an entry for {label}"));
        let tail = &committed[idx..];
        let end = tail.find('}').expect("entry is closed");
        assert!(
            tail[..end].contains(&digest),
            "manifest entry for {label} does not carry that value's digest"
        );
    }

    for (pinned, op) in [
        (PinnedArithOp::Add, ArithOp::Add),
        (PinnedArithOp::Sub, ArithOp::Sub),
        (PinnedArithOp::Mul, ArithOp::Mul),
        (PinnedArithOp::Div, ArithOp::Div),
    ] {
        let label = format!("ArithOp::{}", pinned.op_name());
        let digest = op.config_id().digest().to_hex();
        let idx = committed
            .find(&label)
            .unwrap_or_else(|| panic!("manifest is missing an entry for {label}"));
        let tail = &committed[idx..];
        let end = tail.find('}').expect("entry is closed");
        assert!(
            tail[..end].contains(&digest),
            "manifest entry for {label} does not carry that value's digest"
        );
    }
}
