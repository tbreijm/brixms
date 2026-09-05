//! Integration tests and frozen vectors for ADR-0026 settlement audit-input transport bundles.
//!
//! Tests:
//! - Frozen vector manifest `vectors/settlement_audit_input_bundle_v1.json`
//! - Independent primitive `CanonWriter` reconstruction (two-consumer discipline)
//! - Frozen constants (marker, version, profile)
//! - Honest produce, encode, decode, snapshot validation, and receipt checking
//! - Deterministic bytes and bundle identity
//! - Marker domain noncollision (against all ADR-0026/ADR-0023 domains)
//! - Tamper tests for every single field:
//!   - context, ordinal, prefix_digest, key fields (phase, priority, tiebreak),
//!   - observation outcome class and judgement digest, generators, configs,
//!   - src, dst, witness, receipt_bytes, final_chain_digest, marker, version, profile
//! - Hostile decode inputs:
//!   - truncations, nonminimal lengths, huge counts, sparse/decreasing/duplicate ordinals,
//!   - config-generator mismatch, nested trailing bytes, outer trailing bytes
//! - Exact boundary and one-over for ALL EIGHT limits in [`AuditDecodeLimits`]:
//!   1. max_total_bundle_bytes
//!   2. max_steps
//!   3. max_entry_bytes
//!   4. max_receipt_bytes
//!   5. max_generators_per_step
//!   6. max_configs_per_step
//!   7. max_cumulative_links
//!   8. max_cumulative_framed_bytes
//! - Typed construction cannot bypass validation in public checker
//! - Limit-aware producer and validated encoder

use std::path::{Path, PathBuf};

use brix_canon::{CanonWriter, Canonical, Digest, Domain};
use brix_semantic::{
    ConfigId, ContextId, Decomposition, GeneratorId, GeneratorRegistry, GeneratorSemanticsV1,
    Outcome, WitnessId,
};
use soc_core::adm::AdmAll;
use soc_core::audit_bundle::{
    check_audit_input_bundle_v1, decode_audit_input_bundle_v1, encode_audit_input_bundle_v1,
    produce_audit_input_bundle_v1, produce_audit_input_bundle_with_limits_v1, AuditDecodeLimits,
    BundleCheckError, BundleDecodeError, BundleProducerError, SettlementAuditInputBundleIdV1,
    SettlementAuditInputBundleV1, SnapshotValidationError, BUNDLE_MARKER_V1, BUNDLE_PROFILE_V1,
    BUNDLE_VERSION_V1,
};
use soc_core::audit_receipt::SettlementAuditReceiptIdV1;
use soc_core::calendar::Key;
use soc_core::commit::{run, CommitError, SettlementWitnessProvider};
use soc_core::exec::ExecConfig;
use soc_core::history::History;
use soc_core::intern::{Handle, Interner};
use soc_core::journal::Journal;
use soc_core::witness_provider::{Candidate, WitnessProvider};

// ---------------------------------------------------------------------------
// Fixture setup
// ---------------------------------------------------------------------------

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
    let mut w = CanonWriter::new();
    w.write_uint(c.witness.raw() as u64);
    w.write_uint(c.successor.raw() as u64);
    w.digest(Domain::Value)
}

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

fn committed_fixture_journal() -> (Journal, ContextId) {
    let (i, regime, e) = setup();
    let regimes: Vec<&dyn SettlementWitnessProvider> = vec![&regime];
    let context = ContextId::root();
    let keyer = |c: &Candidate, phase: u64| Key::new(phase, 0, tiebreak_of(c));

    let (journal, _costs) = run(&regimes, &AdmAll, &i, e, context, keyer, 1);
    assert_eq!(journal.len(), 1, "exactly one committed tick expected");
    (journal, context)
}

fn registry_with(gens: &[GeneratorId]) -> GeneratorRegistry {
    let mut r = GeneratorRegistry::new();
    for g in gens {
        r.insert(*g);
    }
    r
}

fn honest_semantics() -> GeneratorSemanticsV1 {
    let mut m = GeneratorSemanticsV1::new();
    m.declare_rows(gen1(), [(cfg_x0(), cfg_x1())]);
    m.declare_rows(gen2(), [(cfg_x1(), cfg_x2())]);
    m
}

fn substituted_semantics() -> GeneratorSemanticsV1 {
    let mut m = GeneratorSemanticsV1::new();
    m.declare_diagonal(gen1());
    m.declare_diagonal(gen2());
    m
}

fn honest_bundle() -> (
    SettlementAuditInputBundleV1,
    GeneratorRegistry,
    GeneratorSemanticsV1,
    Journal,
) {
    let (journal, context) = committed_fixture_journal();
    let registry = registry_with(&[gen1(), gen2()]);
    let semantics = honest_semantics();
    let bundle = produce_audit_input_bundle_v1(&journal, context, &registry, &semantics)
        .expect("honest bundle production succeeds");
    (bundle, registry, semantics, journal)
}

// ---------------------------------------------------------------------------
// Independent primitive CanonWriter reconstruction (ADR-0013 §8, ADR-0026 §11)
// ---------------------------------------------------------------------------

fn independent_bundle(bundle: &SettlementAuditInputBundleV1) -> Vec<u8> {
    let mut w = CanonWriter::new();
    w.write_bytes(b"brix.soc.audit-input-bundle");
    w.write_uint(1);
    w.write_str("brix.soc.audit-input-bundle@1");
    w.write_bytes(bundle.context.digest().as_bytes());

    w.write_uint(bundle.entries.len() as u64);
    for entry in &bundle.entries {
        let mut ew = CanonWriter::new();
        ew.write_uint(entry.ordinal);
        ew.write_bytes(entry.prefix_digest.as_bytes());

        // Step material
        ew.write_uint(entry.step_material.key.phase);
        ew.write_uint(entry.step_material.key.priority);
        ew.write_bytes(entry.step_material.key.tiebreak.as_bytes());

        // Observation: Derived (2)
        ew.write_uint(2);
        ew.write_bytes(entry.step_material.observation.judgement_digest.as_bytes());

        // Generators
        ew.write_uint(entry.step_material.generators.len() as u64);
        for g in &entry.step_material.generators {
            let mut gw = CanonWriter::new();
            gw.write_bytes(g.0.as_bytes());
            ew.write_bytes(&gw.finish());
        }

        // Configs
        ew.write_uint(entry.step_material.configs.len() as u64);
        for c in &entry.step_material.configs {
            let mut cw = CanonWriter::new();
            cw.write_bytes(c.0.as_bytes());
            ew.write_bytes(&cw.finish());
        }

        ew.write_bytes(entry.step_material.src.0.as_bytes());
        ew.write_bytes(entry.step_material.dst.0.as_bytes());
        ew.write_bytes(entry.step_material.witness.0.as_bytes());

        // Receipt bytes
        ew.write_bytes(&entry.receipt_bytes);

        w.write_bytes(&ew.finish());
    }

    w.write_bytes(bundle.final_chain_digest.as_bytes());
    w.finish()
}

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).expect("hex nibble"));
        s.push(char::from_digit((b & 0xf) as u32, 16).expect("hex nibble"));
    }
    s
}

fn manifest_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("vectors")
        .join("settlement_audit_input_bundle_v1.json")
}

fn build_manifest() -> String {
    let (bundle, _, _, _) = honest_bundle();
    let mut out = String::new();
    out.push_str("{\n");
    out.push_str("  \"format\": \"brix.soc.SettlementAuditInputBundleV1\",\n");
    out.push_str("  \"version\": 1,\n");
    out.push_str("  \"adr\": \"ADR-0026\",\n");
    out.push_str("  \"cases\": [\n");
    out.push_str("    {\n");
    out.push_str("      \"name\": \"two_link_fixture_chain_bundle\",\n");
    out.push_str(
        "      \"description\": \"a real commit_tick -> audit_step run over gen1;gen2 produced into SettlementAuditInputBundleV1\",\n",
    );
    out.push_str(&format!(
        "      \"context_id\": \"{}\",\n",
        bundle.context.digest().to_hex()
    ));
    out.push_str(&format!(
        "      \"final_chain_digest\": \"{}\",\n",
        bundle.final_chain_digest.to_hex()
    ));
    out.push_str(&format!(
        "      \"steps_count\": {},\n",
        bundle.entries.len()
    ));
    out.push_str(&format!(
        "      \"bundle_id\": \"{}\",\n",
        bundle.id().to_hex()
    ));
    out.push_str(&format!(
        "      \"canon_hex\": \"{}\"\n",
        to_hex(&bundle.canon_bytes())
    ));
    out.push_str("    }\n");
    out.push_str("  ]\n");
    out.push_str("}\n");
    out
}

// ---------------------------------------------------------------------------
// 1. Frozen vector manifest & constants
// ---------------------------------------------------------------------------

#[test]
fn audit_bundle_vectors_are_frozen() {
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
        "audit bundle vectors drifted from {}.\n\
         The v1 bundle encoding is frozen ABI. Regenerate deliberately with \
         BLESS_VECTORS=1 only if you intend a compatibility break.",
        path.display()
    );
}

#[test]
fn audit_bundle_vectors_reproduced_by_primitive_canon_writes() {
    let (bundle, _, _, _) = honest_bundle();
    assert_eq!(
        to_hex(&bundle.canon_bytes()),
        to_hex(&independent_bundle(&bundle)),
        "the bundle must be reproducible without its own canon_write"
    );
}

#[test]
fn bundle_constants_are_frozen() {
    assert_eq!(BUNDLE_MARKER_V1, b"brix.soc.audit-input-bundle");
    assert_eq!(BUNDLE_VERSION_V1, 1);
    assert_eq!(BUNDLE_PROFILE_V1, "brix.soc.audit-input-bundle@1");
}

// ---------------------------------------------------------------------------
// 2. Honest produce, encode, decode, snapshot and receipt checking
// ---------------------------------------------------------------------------

#[test]
fn honest_bundle_produce_encode_decode_roundtrip() {
    let (bundle, registry, semantics, journal) = honest_bundle();
    let limits = AuditDecodeLimits::strict();

    let bytes = encode_audit_input_bundle_v1(&bundle, &limits)
        .expect("honest bundle encoding must succeed");
    assert_eq!(bytes, bundle.canon_bytes());
    assert_eq!(bundle.encode(&limits).unwrap(), bytes);

    let decoded =
        decode_audit_input_bundle_v1(&bytes, &limits).expect("honest bundle decoding must succeed");
    assert_eq!(decoded, bundle);

    let validated_history = decoded
        .validate_snapshot()
        .expect("snapshot validation must succeed");
    assert_eq!(validated_history.digest(), journal.chain_digest());

    let receipt_ids = check_audit_input_bundle_v1(&decoded, &registry, &semantics, &limits)
        .expect("receipt checking must succeed");
    assert_eq!(receipt_ids.len(), 1);
    assert_eq!(
        receipt_ids[0],
        SettlementAuditReceiptIdV1(Digest::of(Domain::Value, &bundle.entries[0].receipt_bytes))
    );
}

#[test]
fn substituted_oracle_refused() {
    let (bundle, registry, _, _) = honest_bundle();
    let other = substituted_semantics();
    let limits = AuditDecodeLimits::strict();

    let err = check_audit_input_bundle_v1(&bundle, &registry, &other, &limits)
        .expect_err("substituted oracle must fail");
    assert!(matches!(err, BundleCheckError::Receipt { .. }));
}

// ---------------------------------------------------------------------------
// 3. Deterministic bytes and identity
// ---------------------------------------------------------------------------

#[test]
fn bundle_deterministic_bytes_and_identity() {
    let (b1, _, _, _) = honest_bundle();
    let (b2, _, _, _) = honest_bundle();

    assert_eq!(b1.canon_bytes(), b2.canon_bytes());
    assert_eq!(b1.id(), b2.id());
    assert_eq!(b1.id(), SettlementAuditInputBundleIdV1::of(&b1));

    let expected_digest = Digest::of(Domain::Value, &b1.canon_bytes());
    assert_eq!(b1.id().digest(), expected_digest);
}

// ---------------------------------------------------------------------------
// 4. Marker domain noncollision (ADR-0026 §5, ADR-0023)
// ---------------------------------------------------------------------------

#[test]
fn bundle_marker_domain_noncollision() {
    let domains: &[&[u8]] = &[
        b"brix.kernel.primitive-relation",
        b"brix.semantic.generator-semantics",
        b"brix.soc.audit-receipt",
        b"brix.kernel.certificate",
        b"brix.soc.quiescence",
        b"brix.soc.divergence",
        b"brix.l3.plan",
        b"brix.l3.run",
    ];

    for d in domains {
        assert_ne!(
            BUNDLE_MARKER_V1,
            *d,
            "BUNDLE_MARKER_V1 must not collide with domain {:?}",
            std::str::from_utf8(d)
        );
    }

    let payload: &[u8] = b"common-test-payload-for-noncollision";
    let bundle_bytes = [BUNDLE_MARKER_V1, payload].concat();
    let bundle_digest = Digest::of(Domain::Value, &bundle_bytes);
    for d in domains {
        let other_bytes = [*d, payload].concat();
        let other_digest = Digest::of(Domain::Value, &other_bytes);
        assert_ne!(
            bundle_digest,
            other_digest,
            "Digest over domain {:?} collided with bundle marker digest",
            std::str::from_utf8(d)
        );
    }
}

// ---------------------------------------------------------------------------
// 5. Tamper every field
// ---------------------------------------------------------------------------

#[test]
fn tamper_context_refused() {
    let (mut bundle, registry, semantics, _) = honest_bundle();
    let limits = AuditDecodeLimits::strict();

    bundle.context = ContextId::root().extend(b"tampered-context");
    let err = check_audit_input_bundle_v1(&bundle, &registry, &semantics, &limits)
        .expect_err("tampered context must fail receipt check");
    assert!(matches!(err, BundleCheckError::Receipt { .. }));
}

#[test]
fn tamper_ordinal_refused() {
    let (mut bundle, registry, semantics, _) = honest_bundle();
    let limits = AuditDecodeLimits::strict();

    bundle.entries[0].ordinal = 1; // expected 0
    let snap_err = bundle
        .validate_snapshot()
        .expect_err("tampered ordinal must fail snapshot");
    assert_eq!(
        snap_err,
        SnapshotValidationError::OrdinalMismatch {
            expected: 0,
            found: 1,
        }
    );

    let check_err = check_audit_input_bundle_v1(&bundle, &registry, &semantics, &limits)
        .expect_err("tampered ordinal must fail bundle check");
    assert!(matches!(
        check_err,
        BundleCheckError::SnapshotValidation(SnapshotValidationError::OrdinalMismatch { .. })
    ));

    let bytes = bundle.canon_bytes();
    let dec_err = decode_audit_input_bundle_v1(&bytes, &limits)
        .expect_err("tampered ordinal must fail decode");
    assert_eq!(
        dec_err,
        BundleDecodeError::OrdinalMismatch {
            expected: 0,
            found: 1,
        }
    );
}

#[test]
fn tamper_prefix_digest_refused() {
    let (mut bundle, _, _, _) = honest_bundle();
    bundle.entries[0].prefix_digest = Digest::of(Domain::Value, b"wrong-prefix");

    let err = bundle
        .validate_snapshot()
        .expect_err("tampered prefix digest must fail snapshot");
    assert!(matches!(
        err,
        SnapshotValidationError::PrefixDigestMismatch { .. }
    ));
}

#[test]
fn tamper_key_fields_refused() {
    let (bundle, registry, semantics, _) = honest_bundle();
    let limits = AuditDecodeLimits::strict();

    // 1. phase
    let mut b1 = bundle.clone();
    b1.entries[0].step_material.key = Key::new(
        b1.entries[0].step_material.key.phase + 1,
        b1.entries[0].step_material.key.priority,
        b1.entries[0].step_material.key.tiebreak,
    );
    assert!(check_audit_input_bundle_v1(&b1, &registry, &semantics, &limits).is_err());

    // 2. priority
    let mut b2 = bundle.clone();
    b2.entries[0].step_material.key = Key::new(
        b2.entries[0].step_material.key.phase,
        b2.entries[0].step_material.key.priority + 1,
        b2.entries[0].step_material.key.tiebreak,
    );
    assert!(check_audit_input_bundle_v1(&b2, &registry, &semantics, &limits).is_err());

    // 3. tiebreak
    let mut b3 = bundle.clone();
    b3.entries[0].step_material.key = Key::new(
        b3.entries[0].step_material.key.phase,
        b3.entries[0].step_material.key.priority,
        Digest::of(Domain::Value, b"tampered-tiebreak"),
    );
    assert!(check_audit_input_bundle_v1(&b3, &registry, &semantics, &limits).is_err());
}

#[test]
fn tamper_observation_outcome_and_digest_refused() {
    let (bundle, registry, semantics, _) = honest_bundle();
    let limits = AuditDecodeLimits::strict();

    // 1. observation outcome
    let mut b1 = bundle.clone();
    b1.entries[0].step_material.observation.outcome_class = Outcome::Audited;
    let err = check_audit_input_bundle_v1(&b1, &registry, &semantics, &limits)
        .expect_err("non-Derived outcome must be refused");
    assert_eq!(
        err,
        BundleCheckError::LimitExceeded(BundleDecodeError::ObservationNotDerived {
            found: Outcome::Audited as u64,
        })
    );

    // 2. judgement digest
    let mut b2 = bundle.clone();
    b2.entries[0].step_material.observation.judgement_digest =
        Digest::of(Domain::Value, b"tampered-judgement");
    assert!(check_audit_input_bundle_v1(&b2, &registry, &semantics, &limits).is_err());

    // 3. wire decode with non-Derived outcome ordinal (e.g. 0)
    let mut ew = CanonWriter::new();
    ew.write_uint(0); // ordinal 0
    ew.write_bytes(bundle.entries[0].prefix_digest.as_bytes());
    // key
    ew.write_uint(0);
    ew.write_uint(0);
    ew.write_bytes(bundle.entries[0].step_material.key.tiebreak.as_bytes());
    // observation: outcome 0 (Admitted)
    ew.write_uint(0);
    ew.write_bytes(
        bundle.entries[0]
            .step_material
            .observation
            .judgement_digest
            .as_bytes(),
    );
    // generators
    ew.write_uint(2);
    for g in &[gen1(), gen2()] {
        let mut gw = CanonWriter::new();
        gw.write_bytes(g.0.as_bytes());
        ew.write_bytes(&gw.finish());
    }
    // configs
    ew.write_uint(3);
    for c in &[cfg_x0(), cfg_x1(), cfg_x2()] {
        let mut cw = CanonWriter::new();
        cw.write_bytes(c.0.as_bytes());
        ew.write_bytes(&cw.finish());
    }
    ew.write_bytes(cfg_x0().0.as_bytes());
    ew.write_bytes(cfg_x2().0.as_bytes());
    ew.write_bytes(bundle.entries[0].step_material.witness.0.as_bytes());
    ew.write_bytes(&bundle.entries[0].receipt_bytes);

    let mut bw = CanonWriter::new();
    bw.write_bytes(BUNDLE_MARKER_V1);
    bw.write_uint(BUNDLE_VERSION_V1);
    bw.write_str(BUNDLE_PROFILE_V1);
    bw.write_bytes(bundle.context.digest().as_bytes());
    bw.write_uint(1);
    bw.write_bytes(&ew.finish());
    bw.write_bytes(bundle.final_chain_digest.as_bytes());

    let dec_err = decode_audit_input_bundle_v1(&bw.finish(), &limits)
        .expect_err("non-Derived outcome ordinal must be refused by decoder");
    assert_eq!(
        dec_err,
        BundleDecodeError::ObservationNotDerived { found: 0 }
    );
}

#[test]
fn tamper_generators_and_configs_refused() {
    let (bundle, registry, semantics, _) = honest_bundle();
    let limits = AuditDecodeLimits::strict();

    // 1. generator altered
    let mut b1 = bundle.clone();
    b1.entries[0].step_material.generators[0] = GeneratorId::named("other.g1@1");
    assert!(check_audit_input_bundle_v1(&b1, &registry, &semantics, &limits).is_err());

    // 2. intermediate config altered
    let mut b2 = bundle.clone();
    b2.entries[0].step_material.configs[1] = ConfigId::from_canon(b"tampered-intermediate");
    assert!(check_audit_input_bundle_v1(&b2, &registry, &semantics, &limits).is_err());
}

#[test]
fn tamper_src_dst_witness_refused() {
    let (bundle, registry, semantics, _) = honest_bundle();
    let limits = AuditDecodeLimits::strict();

    // src
    let mut b1 = bundle.clone();
    b1.entries[0].step_material.src = ConfigId::from_canon(b"different-src");
    assert!(check_audit_input_bundle_v1(&b1, &registry, &semantics, &limits).is_err());

    // dst
    let mut b2 = bundle.clone();
    b2.entries[0].step_material.dst = ConfigId::from_canon(b"different-dst");
    assert!(check_audit_input_bundle_v1(&b2, &registry, &semantics, &limits).is_err());

    // witness
    let mut b3 = bundle.clone();
    b3.entries[0].step_material.witness =
        WitnessId(Digest::of(Domain::Value, b"different-witness"));
    assert!(check_audit_input_bundle_v1(&b3, &registry, &semantics, &limits).is_err());
}

#[test]
fn tamper_receipt_bytes_refused() {
    let (mut bundle, registry, semantics, _) = honest_bundle();
    let limits = AuditDecodeLimits::strict();

    let last = bundle.entries[0].receipt_bytes.len() - 1;
    bundle.entries[0].receipt_bytes[last] ^= 0xff;

    assert!(check_audit_input_bundle_v1(&bundle, &registry, &semantics, &limits).is_err());
}

#[test]
fn tamper_final_chain_digest_refused() {
    let (mut bundle, _, _, _) = honest_bundle();
    bundle.final_chain_digest = Digest::of(Domain::Value, b"tampered-final-digest");

    let err = bundle
        .validate_snapshot()
        .expect_err("tampered final digest must fail snapshot");
    assert!(matches!(
        err,
        SnapshotValidationError::FinalChainDigestMismatch { .. }
    ));
}

#[test]
fn tamper_marker_version_profile_refused() {
    let (bundle, _, _, _) = honest_bundle();
    let limits = AuditDecodeLimits::strict();

    // 1. Bad marker
    let mut w1 = CanonWriter::new();
    w1.write_bytes(b"brix.soc.bad-marker");
    w1.write_uint(BUNDLE_VERSION_V1);
    w1.write_str(BUNDLE_PROFILE_V1);
    w1.write_bytes(bundle.context.digest().as_bytes());
    w1.write_list(bundle.entries.iter().map(|e| e.canon_bytes()));
    w1.write_bytes(bundle.final_chain_digest.as_bytes());
    assert_eq!(
        decode_audit_input_bundle_v1(&w1.finish(), &limits),
        Err(BundleDecodeError::BadMarker)
    );

    // 2. Unknown version (0 and 2)
    let mut w2 = CanonWriter::new();
    w2.write_bytes(BUNDLE_MARKER_V1);
    w2.write_uint(2);
    w2.write_str(BUNDLE_PROFILE_V1);
    w2.write_bytes(bundle.context.digest().as_bytes());
    w2.write_list(bundle.entries.iter().map(|e| e.canon_bytes()));
    w2.write_bytes(bundle.final_chain_digest.as_bytes());
    assert_eq!(
        decode_audit_input_bundle_v1(&w2.finish(), &limits),
        Err(BundleDecodeError::UnknownVersion(2))
    );

    // 3. Unknown profile
    let mut w3 = CanonWriter::new();
    w3.write_bytes(BUNDLE_MARKER_V1);
    w3.write_uint(BUNDLE_VERSION_V1);
    w3.write_str("brix.soc.other-profile@1");
    w3.write_bytes(bundle.context.digest().as_bytes());
    w3.write_list(bundle.entries.iter().map(|e| e.canon_bytes()));
    w3.write_bytes(bundle.final_chain_digest.as_bytes());
    assert_eq!(
        decode_audit_input_bundle_v1(&w3.finish(), &limits),
        Err(BundleDecodeError::UnknownProfile)
    );
}

// ---------------------------------------------------------------------------
// 6. Hostile decode inputs
// ---------------------------------------------------------------------------

#[test]
fn hostile_truncation_refused() {
    let (bundle, _, _, _) = honest_bundle();
    let limits = AuditDecodeLimits::strict();
    let bytes = bundle.canon_bytes();

    let truncation_points = [0, 5, 15, 27, 30, 45, 60, 100, bytes.len() - 1];
    for &pt in &truncation_points {
        let truncated = &bytes[..pt.min(bytes.len())];
        assert!(
            decode_audit_input_bundle_v1(truncated, &limits).is_err(),
            "truncated at {pt} must be refused"
        );
    }
}

#[test]
fn hostile_nonminimal_lengths_refused() {
    let limits = AuditDecodeLimits::strict();

    // Uint version encoded non-minimally: [2, 0, 1] instead of [1, 1]
    let mut buf = Vec::new();
    // marker: length 27, then bytes
    buf.push(1);
    buf.push(BUNDLE_MARKER_V1.len() as u8);
    buf.extend_from_slice(BUNDLE_MARKER_V1);
    // non-minimal version: length 2, but first magnitude byte is 0
    buf.push(2);
    buf.push(0);
    buf.push(1);

    let err =
        decode_audit_input_bundle_v1(&buf, &limits).expect_err("nonminimal int must be refused");
    assert_eq!(err, BundleDecodeError::NonMinimalInt);
}

#[test]
fn hostile_huge_counts_refused() {
    let (bundle, _, _, _) = honest_bundle();
    let limits = AuditDecodeLimits::strict();

    // Huge step count
    let mut w = CanonWriter::new();
    w.write_bytes(BUNDLE_MARKER_V1);
    w.write_uint(BUNDLE_VERSION_V1);
    w.write_str(BUNDLE_PROFILE_V1);
    w.write_bytes(bundle.context.digest().as_bytes());
    w.write_uint(u64::MAX); // huge count
    assert!(matches!(
        decode_audit_input_bundle_v1(&w.finish(), &limits),
        Err(BundleDecodeError::BadLength
            | BundleDecodeError::StepsExceeded { .. }
            | BundleDecodeError::CountOverflow)
    ));
}

#[test]
fn hostile_ordinals_sparse_decreasing_duplicate_refused() {
    let (bundle, _, _, _) = honest_bundle();
    let limits = AuditDecodeLimits::strict();
    let entry0 = bundle.entries[0].clone();

    // 1. Duplicate ordinals: [0, 0]
    let dup_bundle = SettlementAuditInputBundleV1 {
        context: bundle.context,
        entries: vec![entry0.clone(), entry0.clone()],
        final_chain_digest: bundle.final_chain_digest,
    };
    assert_eq!(
        decode_audit_input_bundle_v1(&dup_bundle.canon_bytes(), &limits),
        Err(BundleDecodeError::OrdinalMismatch {
            expected: 1,
            found: 0,
        })
    );

    // 2. Decreasing ordinals: [1, 0]
    let mut entry_ord1 = entry0.clone();
    entry_ord1.ordinal = 1;
    let dec_bundle = SettlementAuditInputBundleV1 {
        context: bundle.context,
        entries: vec![entry_ord1, entry0.clone()],
        final_chain_digest: bundle.final_chain_digest,
    };
    assert_eq!(
        decode_audit_input_bundle_v1(&dec_bundle.canon_bytes(), &limits),
        Err(BundleDecodeError::OrdinalMismatch {
            expected: 0,
            found: 1,
        })
    );

    // 3. Sparse ordinals: [0, 2]
    let mut entry_ord2 = entry0.clone();
    entry_ord2.ordinal = 2;
    let sparse_bundle = SettlementAuditInputBundleV1 {
        context: bundle.context,
        entries: vec![entry0, entry_ord2],
        final_chain_digest: bundle.final_chain_digest,
    };
    assert_eq!(
        decode_audit_input_bundle_v1(&sparse_bundle.canon_bytes(), &limits),
        Err(BundleDecodeError::OrdinalMismatch {
            expected: 1,
            found: 2,
        })
    );
}

#[test]
fn hostile_config_generator_mismatch_refused() {
    let (bundle, _, _, _) = honest_bundle();
    let limits = AuditDecodeLimits::strict();

    // Mismatch 1: 2 generators, 2 configs (instead of 3)
    let mut b1 = bundle.clone();
    b1.entries[0].step_material.configs.pop();
    assert_eq!(b1.entries[0].step_material.generators.len(), 2);
    assert_eq!(b1.entries[0].step_material.configs.len(), 2);

    let dec_err1 = decode_audit_input_bundle_v1(&b1.canon_bytes(), &limits)
        .expect_err("config generator mismatch must fail decode");
    assert_eq!(
        dec_err1,
        BundleDecodeError::ConfigGeneratorMismatch {
            generators: 2,
            configs: 2,
        }
    );

    // Mismatch 2: 2 generators, 4 configs
    let mut b2 = bundle.clone();
    b2.entries[0]
        .step_material
        .configs
        .push(ConfigId::from_canon(b"extra-config"));
    assert_eq!(b2.entries[0].step_material.generators.len(), 2);
    assert_eq!(b2.entries[0].step_material.configs.len(), 4);

    let dec_err2 = decode_audit_input_bundle_v1(&b2.canon_bytes(), &limits)
        .expect_err("config generator mismatch must fail decode");
    assert_eq!(
        dec_err2,
        BundleDecodeError::ConfigGeneratorMismatch {
            generators: 2,
            configs: 4,
        }
    );
}

#[test]
fn hostile_trailing_bytes_refused() {
    let (bundle, _, _, _) = honest_bundle();
    let limits = AuditDecodeLimits::strict();

    // 1. Outer trailing bytes
    let mut outer = bundle.canon_bytes();
    outer.push(0xff);
    assert_eq!(
        decode_audit_input_bundle_v1(&outer, &limits),
        Err(BundleDecodeError::TrailingBytes)
    );

    // 2. Nested trailing bytes inside entry frame
    let mut ew = CanonWriter::new();
    ew.write_uint(bundle.entries[0].ordinal);
    ew.write_bytes(bundle.entries[0].prefix_digest.as_bytes());
    bundle.entries[0].step_material.canon_write(&mut ew);
    ew.write_bytes(&bundle.entries[0].receipt_bytes);
    // Append trailing byte inside the entry frame
    let mut entry_bytes_tampered = ew.finish();
    entry_bytes_tampered.push(0xaa);

    let mut bw = CanonWriter::new();
    bw.write_bytes(BUNDLE_MARKER_V1);
    bw.write_uint(BUNDLE_VERSION_V1);
    bw.write_str(BUNDLE_PROFILE_V1);
    bw.write_bytes(bundle.context.digest().as_bytes());
    bw.write_uint(1);
    bw.write_bytes(&entry_bytes_tampered);
    bw.write_bytes(bundle.final_chain_digest.as_bytes());

    assert_eq!(
        decode_audit_input_bundle_v1(&bw.finish(), &limits),
        Err(BundleDecodeError::TrailingBytesInEntry)
    );
}

// ---------------------------------------------------------------------------
// 7. Exact boundary and one-over for ALL EIGHT limits
// ---------------------------------------------------------------------------

#[test]
fn exact_boundary_and_one_over_all_eight_limits() {
    let (bundle, registry, semantics, _) = honest_bundle();
    let bytes = bundle.canon_bytes();

    // 1. max_total_bundle_bytes
    {
        let total_bytes = bytes.len();
        let mut lim = AuditDecodeLimits::strict();
        lim.max_total_bundle_bytes = total_bytes;
        assert!(decode_audit_input_bundle_v1(&bytes, &lim).is_ok());
        assert!(check_audit_input_bundle_v1(&bundle, &registry, &semantics, &lim).is_ok());

        lim.max_total_bundle_bytes = total_bytes - 1;
        assert_eq!(
            decode_audit_input_bundle_v1(&bytes, &lim),
            Err(BundleDecodeError::TotalBundleBytesExceeded {
                limit: total_bytes - 1,
                found: total_bytes,
            })
        );
        assert_eq!(
            check_audit_input_bundle_v1(&bundle, &registry, &semantics, &lim),
            Err(BundleCheckError::LimitExceeded(
                BundleDecodeError::TotalBundleBytesExceeded {
                    limit: total_bytes - 1,
                    found: total_bytes,
                }
            ))
        );
    }

    // 2. max_steps
    {
        let steps = bundle.entries.len();
        let mut lim = AuditDecodeLimits::strict();
        lim.max_steps = steps;
        assert!(decode_audit_input_bundle_v1(&bytes, &lim).is_ok());
        assert!(check_audit_input_bundle_v1(&bundle, &registry, &semantics, &lim).is_ok());

        lim.max_steps = steps - 1;
        assert_eq!(
            decode_audit_input_bundle_v1(&bytes, &lim),
            Err(BundleDecodeError::StepsExceeded {
                limit: steps - 1,
                found: steps,
            })
        );
        assert_eq!(
            check_audit_input_bundle_v1(&bundle, &registry, &semantics, &lim),
            Err(BundleCheckError::LimitExceeded(
                BundleDecodeError::StepsExceeded {
                    limit: steps - 1,
                    found: steps,
                }
            ))
        );
    }

    // 3. max_entry_bytes
    {
        let entry_bytes_len = bundle.entries[0].canon_bytes().len();
        let mut lim = AuditDecodeLimits::strict();
        lim.max_entry_bytes = entry_bytes_len;
        assert!(decode_audit_input_bundle_v1(&bytes, &lim).is_ok());
        assert!(check_audit_input_bundle_v1(&bundle, &registry, &semantics, &lim).is_ok());

        lim.max_entry_bytes = entry_bytes_len - 1;
        assert_eq!(
            decode_audit_input_bundle_v1(&bytes, &lim),
            Err(BundleDecodeError::EntryBytesExceeded {
                limit: entry_bytes_len - 1,
                found: entry_bytes_len,
            })
        );
        assert_eq!(
            check_audit_input_bundle_v1(&bundle, &registry, &semantics, &lim),
            Err(BundleCheckError::LimitExceeded(
                BundleDecodeError::EntryBytesExceeded {
                    limit: entry_bytes_len - 1,
                    found: entry_bytes_len,
                }
            ))
        );
    }

    // 4. max_receipt_bytes
    {
        let receipt_len = bundle.entries[0].receipt_bytes.len();
        let mut lim = AuditDecodeLimits::strict();
        lim.max_receipt_bytes = receipt_len;
        assert!(decode_audit_input_bundle_v1(&bytes, &lim).is_ok());
        assert!(check_audit_input_bundle_v1(&bundle, &registry, &semantics, &lim).is_ok());

        lim.max_receipt_bytes = receipt_len - 1;
        assert_eq!(
            decode_audit_input_bundle_v1(&bytes, &lim),
            Err(BundleDecodeError::ReceiptBytesExceeded {
                limit: receipt_len - 1,
                found: receipt_len,
            })
        );
        assert_eq!(
            check_audit_input_bundle_v1(&bundle, &registry, &semantics, &lim),
            Err(BundleCheckError::LimitExceeded(
                BundleDecodeError::ReceiptBytesExceeded {
                    limit: receipt_len - 1,
                    found: receipt_len,
                }
            ))
        );
    }

    // 5. max_generators_per_step
    {
        let g_count = bundle.entries[0].step_material.generators.len();
        let mut lim = AuditDecodeLimits::strict();
        lim.max_generators_per_step = g_count;
        assert!(decode_audit_input_bundle_v1(&bytes, &lim).is_ok());
        assert!(check_audit_input_bundle_v1(&bundle, &registry, &semantics, &lim).is_ok());

        lim.max_generators_per_step = g_count - 1;
        assert_eq!(
            decode_audit_input_bundle_v1(&bytes, &lim),
            Err(BundleDecodeError::GeneratorsPerStepExceeded {
                limit: g_count - 1,
                found: g_count,
            })
        );
        assert_eq!(
            check_audit_input_bundle_v1(&bundle, &registry, &semantics, &lim),
            Err(BundleCheckError::LimitExceeded(
                BundleDecodeError::GeneratorsPerStepExceeded {
                    limit: g_count - 1,
                    found: g_count,
                }
            ))
        );
    }

    // 6. max_configs_per_step
    {
        let c_count = bundle.entries[0].step_material.configs.len();
        let mut lim = AuditDecodeLimits::strict();
        lim.max_configs_per_step = c_count;
        assert!(decode_audit_input_bundle_v1(&bytes, &lim).is_ok());
        assert!(check_audit_input_bundle_v1(&bundle, &registry, &semantics, &lim).is_ok());

        lim.max_configs_per_step = c_count - 1;
        assert_eq!(
            decode_audit_input_bundle_v1(&bytes, &lim),
            Err(BundleDecodeError::ConfigsPerStepExceeded {
                limit: c_count - 1,
                found: c_count,
            })
        );
        assert_eq!(
            check_audit_input_bundle_v1(&bundle, &registry, &semantics, &lim),
            Err(BundleCheckError::LimitExceeded(
                BundleDecodeError::ConfigsPerStepExceeded {
                    limit: c_count - 1,
                    found: c_count,
                }
            ))
        );
    }

    // 7. max_cumulative_links
    {
        let total_links: usize = bundle
            .entries
            .iter()
            .map(|e| e.step_material.generators.len())
            .sum();
        let mut lim = AuditDecodeLimits::strict();
        lim.max_cumulative_links = total_links;
        assert!(decode_audit_input_bundle_v1(&bytes, &lim).is_ok());
        assert!(check_audit_input_bundle_v1(&bundle, &registry, &semantics, &lim).is_ok());

        lim.max_cumulative_links = total_links - 1;
        assert_eq!(
            decode_audit_input_bundle_v1(&bytes, &lim),
            Err(BundleDecodeError::CumulativeLinksExceeded {
                limit: total_links - 1,
                found: total_links,
            })
        );
        assert_eq!(
            check_audit_input_bundle_v1(&bundle, &registry, &semantics, &lim),
            Err(BundleCheckError::LimitExceeded(
                BundleDecodeError::CumulativeLinksExceeded {
                    limit: total_links - 1,
                    found: total_links,
                }
            ))
        );
    }

    // 8. max_cumulative_framed_bytes
    {
        // Compute exact cumulative framed bytes
        let mut exact_framed = BUNDLE_MARKER_V1.len() + BUNDLE_PROFILE_V1.len() + 32;
        for entry in &bundle.entries {
            exact_framed += entry.canon_bytes().len();
            exact_framed += 32 * 3; // prefix, tiebreak, judgement
            exact_framed += entry.step_material.generators.len() * (34 + 32);
            exact_framed += entry.step_material.configs.len() * (34 + 32);
            exact_framed += 96; // src, dst, witness
            exact_framed += entry.receipt_bytes.len();
        }
        exact_framed += 32; // final_chain_digest

        let mut lim = AuditDecodeLimits::strict();
        lim.max_cumulative_framed_bytes = exact_framed;
        assert!(decode_audit_input_bundle_v1(&bytes, &lim).is_ok());
        assert!(check_audit_input_bundle_v1(&bundle, &registry, &semantics, &lim).is_ok());

        lim.max_cumulative_framed_bytes = exact_framed - 1;
        assert_eq!(
            decode_audit_input_bundle_v1(&bytes, &lim),
            Err(BundleDecodeError::CumulativeFramedBytesExceeded {
                limit: exact_framed - 1,
                found: exact_framed,
            })
        );
        assert_eq!(
            check_audit_input_bundle_v1(&bundle, &registry, &semantics, &lim),
            Err(BundleCheckError::LimitExceeded(
                BundleDecodeError::CumulativeFramedBytesExceeded {
                    limit: exact_framed - 1,
                    found: exact_framed,
                }
            ))
        );
    }
}

// ---------------------------------------------------------------------------
// 8. Typed construction cannot bypass validation & limit-aware producer
// ---------------------------------------------------------------------------

#[test]
fn typed_construction_cannot_bypass_limits_in_checker_and_encoder() {
    let (bundle, registry, semantics, _) = honest_bundle();

    // Verify each limit failure on a typed bundle passed to check and encode:
    let mut lim = AuditDecodeLimits::strict();
    lim.max_steps = 0;
    assert_eq!(
        check_audit_input_bundle_v1(&bundle, &registry, &semantics, &lim),
        Err(BundleCheckError::LimitExceeded(
            BundleDecodeError::StepsExceeded {
                limit: 0,
                found: bundle.entries.len(),
            }
        ))
    );
    assert_eq!(
        encode_audit_input_bundle_v1(&bundle, &lim),
        Err(BundleDecodeError::StepsExceeded {
            limit: 0,
            found: bundle.entries.len(),
        })
    );

    let mut lim = AuditDecodeLimits::strict();
    lim.max_generators_per_step = 0;
    assert_eq!(
        check_audit_input_bundle_v1(&bundle, &registry, &semantics, &lim),
        Err(BundleCheckError::LimitExceeded(
            BundleDecodeError::GeneratorsPerStepExceeded { limit: 0, found: 2 }
        ))
    );

    let mut lim = AuditDecodeLimits::strict();
    lim.max_configs_per_step = 0;
    assert_eq!(
        check_audit_input_bundle_v1(&bundle, &registry, &semantics, &lim),
        Err(BundleCheckError::LimitExceeded(
            BundleDecodeError::ConfigsPerStepExceeded { limit: 0, found: 3 }
        ))
    );

    let mut lim = AuditDecodeLimits::strict();
    lim.max_cumulative_links = 0;
    assert_eq!(
        check_audit_input_bundle_v1(&bundle, &registry, &semantics, &lim),
        Err(BundleCheckError::LimitExceeded(
            BundleDecodeError::CumulativeLinksExceeded { limit: 0, found: 2 }
        ))
    );

    let mut lim = AuditDecodeLimits::strict();
    lim.max_entry_bytes = 0;
    assert!(matches!(
        check_audit_input_bundle_v1(&bundle, &registry, &semantics, &lim),
        Err(BundleCheckError::LimitExceeded(
            BundleDecodeError::EntryBytesExceeded { .. }
        ))
    ));

    let mut lim = AuditDecodeLimits::strict();
    lim.max_receipt_bytes = 0;
    assert!(matches!(
        check_audit_input_bundle_v1(&bundle, &registry, &semantics, &lim),
        Err(BundleCheckError::LimitExceeded(
            BundleDecodeError::ReceiptBytesExceeded { .. }
        ))
    ));

    let mut lim = AuditDecodeLimits::strict();
    lim.max_total_bundle_bytes = 0;
    assert!(matches!(
        check_audit_input_bundle_v1(&bundle, &registry, &semantics, &lim),
        Err(BundleCheckError::LimitExceeded(
            BundleDecodeError::TotalBundleBytesExceeded { .. }
        ))
    ));

    let mut lim = AuditDecodeLimits::strict();
    lim.max_cumulative_framed_bytes = 0;
    assert!(matches!(
        check_audit_input_bundle_v1(&bundle, &registry, &semantics, &lim),
        Err(BundleCheckError::LimitExceeded(
            BundleDecodeError::CumulativeFramedBytesExceeded { .. }
        ))
    ));
}

#[test]
fn limit_aware_producer_refuses_when_limits_exceeded() {
    let (_, context) = committed_fixture_journal();
    let (journal, _) = committed_fixture_journal();
    let registry = registry_with(&[gen1(), gen2()]);
    let semantics = honest_semantics();

    // Producer with max_steps = 0
    let mut lim = AuditDecodeLimits::strict();
    lim.max_steps = 0;
    let err =
        produce_audit_input_bundle_with_limits_v1(&journal, context, &registry, &semantics, &lim)
            .expect_err("producer must enforce limits");
    assert_eq!(
        err,
        BundleProducerError::LimitExceeded(BundleDecodeError::StepsExceeded { limit: 0, found: 1 })
    );

    // Producer with max_generators_per_step = 1
    let mut lim2 = AuditDecodeLimits::strict();
    lim2.max_generators_per_step = 1;
    let err2 =
        produce_audit_input_bundle_with_limits_v1(&journal, context, &registry, &semantics, &lim2)
            .expect_err("producer must enforce generators limit");
    assert_eq!(
        err2,
        BundleProducerError::LimitExceeded(BundleDecodeError::GeneratorsPerStepExceeded {
            limit: 1,
            found: 2,
        })
    );
}
