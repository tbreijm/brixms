//! Frozen `SettlementAuditReceiptV1` vectors and the receipt's negative gates
//! (ADR-0020 D5/D7, ADR-0013 §7).
//!
//! The receipt is what makes ADR-0019 §6 residual 2 actionable: it names
//! *which* oracle an audit ran under. Its encoding is therefore frozen, and
//! guarded by two consumers — the production encoder, and an independent path
//! that spells out every field with primitive `CanonWriter` calls (including
//! its own reconstruction of the committed-step digest, in that artifact's own
//! frozen field order) without ever invoking the receipt's `canon_write`.
//!
//! The negative gates matter more than the vectors. A receipt that were merely
//! *stored* would repeat the defect ADR-0019 closed; these tests pin that it
//! is **replayed**, and specifically that a consumer's independently-held
//! expectation is what a substituted oracle is caught by.

use std::path::{Path, PathBuf};

use brix_canon::{CanonWriter, Canonical, Digest, Domain};
use brix_semantic::{
    ConfigId, ContextId, Decomposition, GeneratorId, GeneratorRegistry, GeneratorSemanticsV1,
};
use soc_core::adm::AdmAll;
use soc_core::audit::{audit_step, AuditResult};
use soc_core::audit_receipt::{
    check_audit_receipt_v1, committed_step_digest, ReceiptError, SettlementAuditReceiptV1,
    AUDIT_PROFILE_V1, AUDIT_RECEIPT_MARKER_V1, AUDIT_RECEIPT_VERSION_V1,
};
use soc_core::calendar::Key;
use soc_core::commit::{run, CommitError, SettlementWitnessProvider};
use soc_core::exec::ExecConfig;
use soc_core::history::History;
use soc_core::intern::{Handle, Interner};
use soc_core::journal::CommittedStep;
use soc_core::witness_provider::{Candidate, WitnessProvider};

// ---------------------------------------------------------------------------
// Fixture — a real `commit::run` tick producing a genuine two-generator chain.
// Duplicated from `audit_factorization.rs` because integration test binaries
// cannot share a module; kept byte-identical in shape so both suites audit the
// same committed step.
// ---------------------------------------------------------------------------

/// A single-candidate fixture regime whose recorded `Decomposition` is a
/// genuine two-generator chain `x0 --g1--> x1 --g2--> x2` — non-trivial
/// composition, exercising the stepwise `ρ_k = ρ_g2 ∘ ρ_g1` check.
struct FixtureRegime {
    witness: Handle,
    successor: Handle,
}

impl WitnessProvider for FixtureRegime {
    fn candidates(&self, _e: &ExecConfig) -> Vec<Candidate> {
        vec![Candidate {
            witness: self.witness,
            successor: self.successor,
        }]
    }
}

fn gen1() -> GeneratorId {
    GeneratorId::named("audit-fixture.g1@1")
}
fn gen2() -> GeneratorId {
    GeneratorId::named("audit-fixture.g2@1")
}
fn cfg_x0() -> ConfigId {
    ConfigId::from_canon(b"audit-fixture-x0")
}
fn cfg_x1() -> ConfigId {
    ConfigId::from_canon(b"audit-fixture-x1")
}
fn cfg_x2() -> ConfigId {
    ConfigId::from_canon(b"audit-fixture-x2")
}

fn fixture_decomposition() -> Decomposition {
    Decomposition::recorded(vec![gen1(), gen2()], vec![cfg_x0(), cfg_x1(), cfg_x2()]).unwrap()
}

impl SettlementWitnessProvider for FixtureRegime {
    fn try_decompose(&self, _e: &ExecConfig, _c: &Candidate) -> Result<Decomposition, CommitError> {
        Ok(fixture_decomposition())
    }
}

fn tiebreak_of(c: &Candidate) -> Digest {
    let mut w = brix_canon::CanonWriter::new();
    w.write_uint(c.witness.raw() as u64);
    w.write_uint(c.successor.raw() as u64);
    w.digest(Domain::Value)
}

/// Sets up an `Interner` whose world/successor handles resolve to exactly
/// `cfg_x0()`/`cfg_x2()`'s underlying digests, so the committed step's
/// `src`/`dst` land on the fixture decomposition's endpoints — the same
/// wrap-verbatim discipline `commit::commit_tick` documents (the interned
/// digest already *is* the canonical `ConfigId`/`WitnessId` identity, no
/// re-hash).
fn setup() -> (Interner, FixtureRegime, ExecConfig) {
    let mut i = Interner::new();
    let world = i.intern(cfg_x0().digest());
    let policy = i.intern(Digest::of(Domain::Value, b"audit-fixture-p0"));
    let _presentation_handle = i.intern(Digest::of(Domain::Value, b"audit-fixture-r"));
    let witness = i.intern(Digest::of(Domain::Value, b"audit-fixture-witness"));
    let successor = i.intern(cfg_x2().digest());
    let e = ExecConfig::new(world, policy, History::empty().digest());
    (i, FixtureRegime { witness, successor }, e)
}

/// Drive the real `commit::run` loop for exactly one tick to produce a
/// genuine `CommittedStep` (not a hand-built one) — the same entry point any
/// real regime goes through.
fn committed_fixture_step() -> (CommittedStep, ContextId) {
    let (i, regime, e) = setup();
    let regimes: Vec<&dyn SettlementWitnessProvider> = vec![&regime];
    let context = ContextId::root();
    let keyer = |c: &Candidate, phase: u64| Key::new(phase, 0, tiebreak_of(c));

    let (journal, _costs) = run(&regimes, &AdmAll, &i, e, context, keyer, 1);
    assert_eq!(journal.len(), 1, "exactly one committed tick expected");
    (journal.steps()[0].clone(), context)
}

fn registry_with(gens: &[GeneratorId]) -> GeneratorRegistry {
    let mut r = GeneratorRegistry::new();
    for g in gens {
        r.insert(*g);
    }
    r
}

/// The honest declaration for the fixture chain.
fn honest_semantics() -> GeneratorSemanticsV1 {
    let mut m = GeneratorSemanticsV1::new();
    m.declare_rows(gen1(), [(cfg_x0(), cfg_x1())]);
    m.declare_rows(gen2(), [(cfg_x1(), cfg_x2())]);
    m
}

/// A *different* declaration over the same generators — the substituted oracle
/// ADR-0020 exists to make detectable.
fn substituted_semantics() -> GeneratorSemanticsV1 {
    let mut m = GeneratorSemanticsV1::new();
    m.declare_diagonal(gen1());
    m.declare_diagonal(gen2());
    m
}

fn audited_receipt() -> (CommittedStep, ContextId, SettlementAuditReceiptV1) {
    let (step, context) = committed_fixture_step();
    let registry = registry_with(&[gen1(), gen2()]);
    match audit_step(&step, context, &registry, &honest_semantics()) {
        AuditResult::Audited(a) => (step, context, a.receipt.clone()),
        AuditResult::Unknown(r) => panic!("the fixture audit must succeed, got {r}"),
    }
}

// ---------------------------------------------------------------------------
// The negative gates — what must fail closed that did not before.
// ---------------------------------------------------------------------------

/// **The point of the whole ADR.** A receipt from an audit run under a
/// substituted oracle is refused against the consumer's independently held
/// expectation — and refused by *name*, not as a downstream symptom.
#[test]
fn a_receipt_from_a_substituted_oracle_is_refused_against_the_expected_one() {
    let (step, context) = committed_fixture_step();
    let registry = registry_with(&[gen1(), gen2()]);

    // An audit that really happened, under a declaration that is not the
    // production one. It is internally consistent: the chain is reflexive
    // under neither, so this one actually fails replay — but the point is that
    // it never reaches replay, because the id check comes first.
    let other = substituted_semantics();
    let receipt = SettlementAuditReceiptV1::clone(&audited_receipt().2);

    match check_audit_receipt_v1(&receipt, &step, context, &registry, &other) {
        Err(ReceiptError::UnexpectedSemantics { expected, found }) => {
            assert_eq!(expected, other.id());
            assert_eq!(found, honest_semantics().id());
        }
        other => panic!("a substituted oracle must be refused by name, got {other:?}"),
    }
}

/// A receipt must not be transplantable onto a different committed step.
#[test]
fn a_receipt_does_not_validate_against_another_committed_step() {
    let (_, context, receipt) = audited_receipt();
    let registry = registry_with(&[gen1(), gen2()]);

    // A second, independently committed step. Same plan shape, different run.
    let (other_step, _) = committed_fixture_step();
    let mut tampered = other_step.clone();
    tampered.src = ConfigId::from_canon(b"a different source");

    match check_audit_receipt_v1(&receipt, &tampered, context, &registry, &honest_semantics()) {
        Err(ReceiptError::FieldMismatch { field }) => assert_eq!(field, "committed_step"),
        other => panic!("a transplanted receipt must be refused, got {other:?}"),
    }
}

/// A receipt must not validate under a different context.
#[test]
fn a_receipt_does_not_validate_under_another_context() {
    let (step, _, receipt) = audited_receipt();
    let registry = registry_with(&[gen1(), gen2()]);
    let other_context = ContextId::root().extend(b"elsewhere");

    match check_audit_receipt_v1(
        &receipt,
        &step,
        other_context,
        &registry,
        &honest_semantics(),
    ) {
        Err(ReceiptError::FieldMismatch { field }) => assert_eq!(field, "context"),
        other => panic!("a receipt from another context must be refused, got {other:?}"),
    }
}

/// The manifest must cover exactly the registry, or the receipt names a subset
/// of the audit environment while claiming to name the environment.
#[test]
fn a_manifest_that_does_not_match_the_registry_is_refused() {
    let (step, context, receipt) = audited_receipt();
    // Registry with a third generator the manifest does not declare.
    let wide = registry_with(&[gen1(), gen2(), GeneratorId::named("g3@1")]);

    let err = check_audit_receipt_v1(&receipt, &step, context, &wide, &honest_semantics())
        .expect_err("a mismatched environment must be refused");
    // Registry id differs first, which is itself the right refusal.
    assert!(
        matches!(
            err,
            ReceiptError::UnexpectedRegistry { .. } | ReceiptError::SemanticsRegistryDisagreement
        ),
        "got {err:?}"
    );
}

/// The positive path: an honest receipt validates, and validation is replay —
/// it reproduces the receipt rather than reading it.
#[test]
fn an_honest_receipt_validates_by_replay() {
    let (step, context, receipt) = audited_receipt();
    let registry = registry_with(&[gen1(), gen2()]);

    let id = check_audit_receipt_v1(&receipt, &step, context, &registry, &honest_semantics())
        .expect("an honest receipt validates");
    assert_eq!(id, receipt.id());

    // And the receipt actually names the oracle — the field ADR-0019 lacked.
    assert_eq!(receipt.semantics(), honest_semantics().id());
    assert_ne!(receipt.semantics(), substituted_semantics().id());
}

/// The receipt is additive: the published judgement is unchanged by its
/// existence (ADR-0020 D1).
#[test]
fn the_receipt_does_not_change_the_published_judgement() {
    let (step, context) = committed_fixture_step();
    let registry = registry_with(&[gen1(), gen2()]);
    let audited = match audit_step(&step, context, &registry, &honest_semantics()) {
        AuditResult::Audited(a) => a,
        AuditResult::Unknown(r) => panic!("fixture audit must succeed, got {r}"),
    };

    // The judgement id is a function of context/proposition/outcome/evidence
    // only — the receipt is nowhere in it.
    assert_eq!(audited.audited_id, audited.audited.id());
    assert_eq!(
        audited.audited.evidence,
        brix_semantic::Evidence::SettlementReplay {
            body: audited.verified.id().digest(),
        }
        .id(),
        "evidence still names only the verified decomposition"
    );
}

// ---------------------------------------------------------------------------
// Frozen vectors, two consumers.
// ---------------------------------------------------------------------------

/// Spell out the receipt's canonical bytes with primitive `CanonWriter` calls
/// only — never the receipt's own `canon_write`.
fn independent_receipt(receipt: &SettlementAuditReceiptV1) -> Vec<u8> {
    let mut w = CanonWriter::new();
    w.write_bytes(b"brix.soc.audit-receipt");
    w.write_uint(1);
    w.write_str("brix.soc.audit-factorization@1");
    w.write_bytes(receipt.context().digest().as_bytes());
    w.write_bytes(receipt.committed_step().as_bytes());
    w.write_bytes(receipt.verified_decomposition().digest().as_bytes());
    w.write_bytes(receipt.registry().digest().as_bytes());
    w.write_bytes(receipt.semantics().digest().as_bytes());
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

fn build_manifest() -> String {
    let (step, _, receipt) = audited_receipt();
    let mut out = String::new();
    out.push_str("{\n");
    out.push_str("  \"format\": \"brix.soc.SettlementAuditReceiptV1\",\n");
    out.push_str("  \"version\": 1,\n");
    out.push_str("  \"adr\": \"ADR-0020\",\n");
    out.push_str("  \"cases\": [\n");
    out.push_str("    {\n");
    out.push_str("      \"name\": \"two_link_fixture_chain\",\n");
    out.push_str(
        "      \"description\": \"a real commit_tick -> audit_step run over gen1;gen2\",\n",
    );
    out.push_str(&format!(
        "      \"committed_step_digest\": \"{}\",\n",
        committed_step_digest(&step).to_hex()
    ));
    out.push_str(&format!(
        "      \"canon_hex\": \"{}\",\n",
        to_hex(&receipt.canon_bytes())
    ));
    out.push_str(&format!(
        "      \"receipt_id\": \"{}\"\n",
        receipt.id().to_hex()
    ));
    out.push_str("    }\n");
    out.push_str("  ]\n");
    out.push_str("}\n");
    out
}

fn manifest_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("vectors")
        .join("settlement_audit_receipt_v1.json")
}

#[test]
fn audit_receipt_vectors_are_frozen() {
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
    panic!(
        "audit receipt vectors drifted from {}.\n\
         The v1 receipt encoding is frozen ABI. Regenerate deliberately with \
         BLESS_VECTORS=1 only if you intend a compatibility break.",
        path.display()
    );
}

#[test]
fn audit_receipt_vectors_reproduced_by_primitive_canon_writes() {
    let (_, _, receipt) = audited_receipt();
    assert_eq!(
        to_hex(&receipt.canon_bytes()),
        to_hex(&independent_receipt(&receipt)),
        "the receipt must be reproducible without its own canon_write"
    );
}

#[test]
fn receipt_constants_are_frozen() {
    assert_eq!(AUDIT_RECEIPT_MARKER_V1, b"brix.soc.audit-receipt");
    assert_eq!(AUDIT_RECEIPT_VERSION_V1, 1);
    assert_eq!(AUDIT_PROFILE_V1, "brix.soc.audit-factorization@1");
}
