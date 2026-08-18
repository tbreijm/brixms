//! Frozen saturation-certificate vectors (ADR-0014 §6.2, ⟨D-QCERT⟩ and
//! ⟨D-OBS⟩, ratified 2026-08-03).
//!
//! Three certificates — quiescence with no administrative prefix, quiescence
//! after a two-step prefix, and a two-step administrative lasso — are minted by
//! `sat_step`, encoded through the production v1 envelopes, and frozen in
//! `vectors/soc_quiescence_v1.json` and `vectors/soc_divergence_v1.json`
//! together with their bytes and certificate ids.
//!
//! Three consumers guard the manifests:
//!
//! 1. `*_vectors_are_frozen` — the production encoders must keep reproducing
//!    the committed bytes (regenerate with `BLESS_VECTORS=1`);
//! 2. `*_vectors_reproduced_by_primitive_canon_writes` — a second construction
//!    path spelling out every pinned field with primitive `CanonWriter`
//!    operations, which never calls the production encoder, so a vector cannot
//!    be vacuously satisfied by the code it guards;
//! 3. `the_observation_profile_id_is_reproduced_from_its_frozen_preimage` —
//!    the same treatment for ⟨D-OBS⟩'s profile identity, which every
//!    certificate embeds and which no envelope test would otherwise pin.
//!
//! After the freeze these manifests are append-only: an existing case may never
//! change without a new envelope format version (ADR-0014 §6.2).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use brix_canon::{CanonWriter, Digest, Domain};
use brix_semantic::{
    ConfigId, ContextId, Decomposition, Evidence, GeneratorId, JudgementId, Outcome, Quiescent,
};
use soc_core::adm::AdmAll;
use soc_core::calendar::Key;
use soc_core::commit::{CommitError, SettlementWitnessProvider};
use soc_core::exec::ExecConfig;
use soc_core::history::History;
use soc_core::intern::{Handle, Interner};
use soc_core::saturate::{
    divergence_certificate_id, encode_divergence_v1, encode_quiescence_v1,
    quiescence_certificate_id, sat_step, DeclaredAssumptions, DivergenceCertificateV1,
    GeneratorPartitionProfile, ObservationProfile, ObservationProfileId, PresentationIdV1,
    PresentationV1, QuiescenceCertificateV1, SaturatedStep, SaturationBudget,
    CERTIFICATE_FORMAT_V1, SATURATION_PROFILE_V1,
};
use soc_core::witness_provider::{Candidate, WitnessProvider};

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

struct Edge {
    witness: Handle,
    successor: Handle,
    generators: Vec<GeneratorId>,
}

struct ChainRegime {
    edges: BTreeMap<Handle, Edge>,
    configs: BTreeMap<Handle, ConfigId>,
}

impl WitnessProvider for ChainRegime {
    fn candidates(&self, e: &ExecConfig) -> Vec<Candidate> {
        self.edges
            .get(&e.world)
            .map(|edge| {
                vec![Candidate {
                    witness: edge.witness,
                    successor: edge.successor,
                }]
            })
            .unwrap_or_default()
    }
}

impl SettlementWitnessProvider for ChainRegime {
    fn try_decompose(&self, e: &ExecConfig, c: &Candidate) -> Result<Decomposition, CommitError> {
        let edge = self
            .edges
            .get(&e.world)
            .expect("try_decompose on a live edge");
        Ok(Decomposition::recorded(
            edge.generators.clone(),
            vec![self.configs[&e.world], self.configs[&c.successor]],
        )
        .expect("well-formed decomposition"))
    }
}

const VECTOR_PRESENTATION_SEED: &[u8] = b"soc.saturation.vectors@1";
const VECTOR_REGIME_SET_SEED: &[u8] = b"soc.saturation.vectors.regime-set@1";
const VECTOR_ADM_SEED: &[u8] = b"soc.saturation.vectors.adm-all@1";

fn tau_generator() -> GeneratorId {
    GeneratorId::named("soc.saturation.vectors.tau@1")
}

fn realizing_generator() -> GeneratorId {
    GeneratorId::named("soc.saturation.vectors.realizing@1")
}

fn administrative_partition() -> BTreeSet<GeneratorId> {
    [tau_generator()].into_iter().collect()
}

fn realizing_partition() -> BTreeSet<GeneratorId> {
    [realizing_generator()].into_iter().collect()
}

fn vector_profile() -> GeneratorPartitionProfile {
    GeneratorPartitionProfile::new(administrative_partition(), realizing_partition())
        .expect("disjoint partitions")
}

fn tag(i: &mut Interner, s: &str) -> Handle {
    i.intern(Digest::of(Domain::Value, s.as_bytes()))
}

fn tiebreak_of(c: &Candidate) -> Digest {
    let mut w = CanonWriter::new();
    w.write_uint(c.witness.raw() as u64);
    w.write_uint(c.successor.raw() as u64);
    w.digest(Domain::Value)
}

struct Fixture {
    interner: Interner,
    regime: ChainRegime,
    worlds: BTreeMap<&'static str, Handle>,
    policy: Handle,
}

fn build_fixture(spec: &[(&'static str, &'static str)]) -> Fixture {
    let mut interner = Interner::new();
    let mut worlds: BTreeMap<&'static str, Handle> = BTreeMap::new();
    for (from, to) in spec {
        for name in [from, to] {
            if !worlds.contains_key(name) {
                worlds.insert(name, tag(&mut interner, name));
            }
        }
    }
    let policy = tag(&mut interner, "vector.policy");
    let _presentation_handle = tag(&mut interner, "vector.regime");

    let configs = worlds
        .values()
        .map(|h| (*h, ConfigId(interner.resolve(*h))))
        .collect();

    let mut edges = BTreeMap::new();
    for (from, to) in spec {
        let witness = tag(&mut interner, &format!("vector.witness.{from}->{to}"));
        edges.insert(
            worlds[from],
            Edge {
                witness,
                successor: worlds[to],
                generators: vec![tau_generator()],
            },
        );
    }

    Fixture {
        interner,
        regime: ChainRegime { edges, configs },
        worlds,
        policy,
    }
}

impl Fixture {
    fn exec_at(&self, world: &str) -> ExecConfig {
        ExecConfig::new(self.worlds[world], self.policy, History::empty().digest())
    }
}

fn presentation<'a>(
    regimes: &'a [&'a dyn SettlementWitnessProvider],
    profile: &'a dyn ObservationProfile,
    interner: &'a Interner,
) -> PresentationV1<'a> {
    PresentationV1 {
        id: PresentationIdV1::from_canon(VECTOR_PRESENTATION_SEED),
        regimes,
        regime_set: Digest::of(Domain::Value, VECTOR_REGIME_SET_SEED),
        adm: &AdmAll,
        adm_id: Digest::of(Domain::Value, VECTOR_ADM_SEED),
        profile,
        interner,
        context: ContextId::root(),
        assumptions: DeclaredAssumptions::all(),
    }
}

fn keyer() -> impl FnMut(&Candidate, u64) -> Key {
    |c: &Candidate, phase: u64| Key::new(phase, 0, tiebreak_of(c))
}

// ---------------------------------------------------------------------------
// The frozen cases
// ---------------------------------------------------------------------------

struct QuiescenceFixture {
    name: &'static str,
    description: &'static str,
    certificate: QuiescenceCertificateV1,
}

fn quiescence_fixtures() -> Vec<QuiescenceFixture> {
    vec![
        QuiescenceFixture {
            name: "terminal_without_prefix",
            description: "quiescence at a terminal world with no administrative prefix; \
                          the prefix chain is the empty history",
            certificate: mint_quiescence("w2"),
        },
        QuiescenceFixture {
            name: "terminal_after_two_administrative_steps",
            description: "quiescence at w2 after hiding the τ prefix w0 -> w1 -> w2",
            certificate: mint_quiescence("w0"),
        },
    ]
}

/// `w0 -τ-> w1 -τ-> w2`, with `w2` terminal.
fn mint_quiescence(from: &str) -> QuiescenceCertificateV1 {
    let fx = build_fixture(&[("w0", "w1"), ("w1", "w2")]);
    let profile = vector_profile();
    let regime: &dyn SettlementWitnessProvider = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(regimes, &profile, &fx.interner);
    let mut k = keyer();
    let (step, _, _) = sat_step(
        &pres,
        &fx.exec_at(from),
        0,
        &mut k,
        SaturationBudget::uniform(64),
    );
    match step {
        SaturatedStep::Quiescent(cert) => *cert,
        other => panic!("expected quiescence from {from}, got {other:?}"),
    }
}

struct DivergenceFixture {
    name: &'static str,
    description: &'static str,
    certificate: DivergenceCertificateV1,
}

fn divergence_fixtures() -> Vec<DivergenceFixture> {
    vec![DivergenceFixture {
        name: "two_step_administrative_loop",
        description: "the administrative orbit w0 -> w1 -> w0 closes a lasso with stem 0 \
                      and cycle 2, under declared P1 and P6",
        certificate: mint_divergence(),
    }]
}

/// `w0 -τ-> w1 -τ-> w0`.
fn mint_divergence() -> DivergenceCertificateV1 {
    let fx = build_fixture(&[("w0", "w1"), ("w1", "w0")]);
    let profile = vector_profile();
    let regime: &dyn SettlementWitnessProvider = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(regimes, &profile, &fx.interner);
    let mut k = keyer();
    let (step, _, _) = sat_step(
        &pres,
        &fx.exec_at("w0"),
        0,
        &mut k,
        SaturationBudget::uniform(64),
    );
    match step {
        SaturatedStep::Divergent(cert) => *cert,
        other => panic!("expected divergence, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// The independent construction path.
//
// Deliberately spells out the markers, version, saturation-profile string, and
// every field's framing with primitive `CanonWriter` calls, repeating the
// frozen literals rather than importing the constants, so a typo'd constant in
// `certificate.rs` cannot silently agree with itself. It MUST NOT call
// `encode_quiescence_v1`, `encode_divergence_v1`, or either `*_certificate_id`.
//
// `Quiescent`, `Evidence`, and `Judgement` keep their own `Canonical` impls
// where the judgement identity is rebuilt: those are separately frozen
// `brix-semantic` ABIs with their own independent-reproduction tests, and
// re-deriving them here would test that crate rather than this envelope.
// ---------------------------------------------------------------------------

/// Rebuild the ⟨D-OBS⟩ generator-partition preimage from first principles,
/// including `write_set`'s sort-and-deduplicate rule, spelled out by hand.
fn independent_profile_preimage(
    administrative: &BTreeSet<GeneratorId>,
    realizing: &BTreeSet<GeneratorId>,
) -> Vec<u8> {
    fn write_generator_set(w: &mut CanonWriter, set: &BTreeSet<GeneratorId>) {
        let mut elements: Vec<Vec<u8>> = set
            .iter()
            .map(|g| {
                let mut e = CanonWriter::new();
                e.write_bytes(g.digest().as_bytes());
                e.finish()
            })
            .collect();
        elements.sort();
        elements.dedup();
        w.write_uint(elements.len() as u64);
        for element in elements {
            w.write_bytes(&element);
        }
    }

    let mut w = CanonWriter::new();
    w.write_bytes(b"brix.soc.obs-profile");
    w.write_uint(1);
    w.write_str("brix.soc.obs-profile.generator-partition@1");
    write_generator_set(&mut w, administrative);
    write_generator_set(&mut w, realizing);
    w.finish()
}

fn independent_quiescence_envelope(cert: &QuiescenceCertificateV1) -> Vec<u8> {
    let mut w = CanonWriter::new();
    w.write_bytes(b"brix.soc.quiescence");
    w.write_uint(1);
    w.write_str("brix.soc.saturation@1");
    w.write_bytes(cert.profile.digest().as_bytes());
    w.write_bytes(cert.context.digest().as_bytes());
    w.write_bytes(cert.presentation.digest().as_bytes());
    w.write_bytes(cert.policy.digest().as_bytes());
    w.write_bytes(cert.src_world.digest().as_bytes());
    w.write_bytes(cert.terminal_world.digest().as_bytes());
    w.write_uint(cert.hidden.len() as u64);
    for digest in &cert.hidden {
        w.write_bytes(digest.as_bytes());
    }
    w.write_bytes(cert.prefix_chain.as_bytes());
    w.write_bytes(cert.regime_set.as_bytes());
    w.write_bytes(cert.adm_id.as_bytes());
    w.write_uint(0); // EnumerationCompleteness::Complete
    w.write_uint(2); // Outcome::Derived
    w.write_bytes(cert.judgement.digest().as_bytes());
    w.finish()
}

fn independent_divergence_envelope(cert: &DivergenceCertificateV1) -> Vec<u8> {
    let mut w = CanonWriter::new();
    w.write_bytes(b"brix.soc.divergence");
    w.write_uint(1);
    w.write_str("brix.soc.saturation@1");
    w.write_bytes(cert.profile.digest().as_bytes());
    w.write_bytes(cert.context.digest().as_bytes());
    w.write_bytes(cert.presentation.digest().as_bytes());
    w.write_bytes(cert.policy.digest().as_bytes());
    w.write_bytes(cert.src_world.digest().as_bytes());
    w.write_uint(cert.stem);
    w.write_uint(cert.cycle);
    for digest in &cert.lasso {
        w.write_bytes(digest.as_bytes());
    }
    w.write_bytes(cert.cycle_world.digest().as_bytes());
    w.write_bytes(cert.cycle_policy.digest().as_bytes());
    w.write_uint(0); // AssumptionMode::DeclaredP1P6
    w.write_uint(4); // Outcome::Unknown
    w.finish()
}

/// Rebuild the quiescence judgement identity without going through
/// `quiescence_judgement`, so the certificate's judgement field is pinned to a
/// statement about the terminal world rather than to whatever the encoder
/// happened to compute.
fn independent_quiescence_judgement(cert: &QuiescenceCertificateV1) -> Digest {
    let proposition = Quiescent::new(
        cert.terminal_world,
        cert.policy,
        cert.regime_set,
        cert.adm_id,
    )
    .proposition_id();
    let evidence = Evidence::SettlementReplay {
        body: cert.prefix_chain,
    }
    .id();
    JudgementId::recompute(cert.context, proposition, Outcome::Derived, evidence).digest()
}

// ---------------------------------------------------------------------------
// Manifest rendering. Hand-built ASCII JSON, matching `brix-canon`'s and
// `brix-kernel`'s vector tests — `soc-core` carries no dependencies beyond
// `brix-canon`/`brix-semantic` and gains none for its tests.
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

fn manifest_header(marker: &str, note: &str) -> String {
    let mut out = String::new();
    out.push_str("{\n");
    out.push_str(&format!("  \"format\": {},\n", json_str(marker)));
    out.push_str(&format!("  \"version\": {CERTIFICATE_FORMAT_V1},\n"));
    out.push_str(&format!(
        "  \"saturation_profile\": {},\n",
        json_str(SATURATION_PROFILE_V1)
    ));
    out.push_str(&format!(
        "  \"observation_profile_id\": \"{}\",\n",
        vector_profile().id().to_hex()
    ));
    out.push_str(&format!("  \"note\": {},\n", json_str(note)));
    out.push_str("  \"cases\": [\n");
    out
}

fn build_quiescence_manifest() -> String {
    let mut out = manifest_header(
        "brix.soc.quiescence",
        "Frozen saturation quiescence-certificate vectors (ADR-0014 §6.2, ⟨D-QCERT⟩ \
         ratified 2026-08-03). Append-only: an existing case may never change without a \
         new envelope format version. Regenerate with BLESS_VECTORS=1.",
    );

    let cases = quiescence_fixtures();
    for (index, fixture) in cases.iter().enumerate() {
        let cert = &fixture.certificate;
        let envelope = encode_quiescence_v1(cert);
        out.push_str("    {\n");
        out.push_str(&format!("      \"name\": {},\n", json_str(fixture.name)));
        out.push_str(&format!(
            "      \"description\": {},\n",
            json_str(fixture.description)
        ));
        out.push_str(&format!(
            "      \"context_id\": \"{}\",\n",
            cert.context.to_hex()
        ));
        out.push_str(&format!(
            "      \"presentation_id\": \"{}\",\n",
            cert.presentation.digest().to_hex()
        ));
        out.push_str(&format!(
            "      \"policy\": \"{}\",\n",
            cert.policy.to_hex()
        ));
        out.push_str(&format!(
            "      \"src_world\": \"{}\",\n",
            cert.src_world.to_hex()
        ));
        out.push_str(&format!(
            "      \"terminal_world\": \"{}\",\n",
            cert.terminal_world.to_hex()
        ));
        out.push_str(&format!("      \"hidden_steps\": {},\n", cert.hidden.len()));
        out.push_str("      \"hidden\": [");
        for (i, digest) in cert.hidden.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push_str(&format!("\"{}\"", digest.to_hex()));
        }
        out.push_str("],\n");
        out.push_str(&format!(
            "      \"prefix_chain\": \"{}\",\n",
            cert.prefix_chain.to_hex()
        ));
        out.push_str(&format!(
            "      \"regime_set\": \"{}\",\n",
            cert.regime_set.to_hex()
        ));
        out.push_str(&format!("      \"adm\": \"{}\",\n", cert.adm_id.to_hex()));
        out.push_str("      \"enumeration\": \"Complete\",\n");
        out.push_str("      \"grade\": \"Derived\",\n");
        out.push_str(&format!(
            "      \"judgement\": \"{}\",\n",
            cert.judgement.to_hex()
        ));
        out.push_str(&format!(
            "      \"envelope_hex\": \"{}\",\n",
            to_hex(&envelope)
        ));
        out.push_str(&format!(
            "      \"certificate_id\": \"{}\"\n",
            quiescence_certificate_id(cert).to_hex()
        ));
        out.push_str("    }");
        out.push_str(if index + 1 == cases.len() {
            "\n"
        } else {
            ",\n"
        });
    }

    out.push_str("  ]\n}\n");
    out
}

fn build_divergence_manifest() -> String {
    let mut out = manifest_header(
        "brix.soc.divergence",
        "Frozen saturation divergence-certificate vectors (ADR-0014 §6.2, ⟨D-QCERT⟩ \
         ratified 2026-08-03). A divergence certificate is Unknown-graded for the \
         completion question — never Refuted, never the 1 summand. Append-only; \
         regenerate with BLESS_VECTORS=1.",
    );

    let cases = divergence_fixtures();
    for (index, fixture) in cases.iter().enumerate() {
        let cert = &fixture.certificate;
        let envelope = encode_divergence_v1(cert);
        out.push_str("    {\n");
        out.push_str(&format!("      \"name\": {},\n", json_str(fixture.name)));
        out.push_str(&format!(
            "      \"description\": {},\n",
            json_str(fixture.description)
        ));
        out.push_str(&format!(
            "      \"context_id\": \"{}\",\n",
            cert.context.to_hex()
        ));
        out.push_str(&format!(
            "      \"presentation_id\": \"{}\",\n",
            cert.presentation.digest().to_hex()
        ));
        out.push_str(&format!(
            "      \"policy\": \"{}\",\n",
            cert.policy.to_hex()
        ));
        out.push_str(&format!(
            "      \"src_world\": \"{}\",\n",
            cert.src_world.to_hex()
        ));
        out.push_str(&format!("      \"stem\": {},\n", cert.stem));
        out.push_str(&format!("      \"cycle\": {},\n", cert.cycle));
        out.push_str("      \"lasso\": [");
        for (i, digest) in cert.lasso.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push_str(&format!("\"{}\"", digest.to_hex()));
        }
        out.push_str("],\n");
        out.push_str(&format!(
            "      \"cycle_world\": \"{}\",\n",
            cert.cycle_world.to_hex()
        ));
        out.push_str(&format!(
            "      \"cycle_policy\": \"{}\",\n",
            cert.cycle_policy.to_hex()
        ));
        out.push_str("      \"assumptions\": \"DeclaredP1P6\",\n");
        out.push_str("      \"grade\": \"Unknown\",\n");
        out.push_str(&format!(
            "      \"envelope_hex\": \"{}\",\n",
            to_hex(&envelope)
        ));
        out.push_str(&format!(
            "      \"certificate_id\": \"{}\"\n",
            divergence_certificate_id(cert).to_hex()
        ));
        out.push_str("    }");
        out.push_str(if index + 1 == cases.len() {
            "\n"
        } else {
            ",\n"
        });
    }

    out.push_str("  ]\n}\n");
    out
}

fn manifest_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("vectors")
        .join(name)
}

fn assert_frozen(name: &str, generated: String) {
    let path = manifest_path(name);
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
        "saturation certificate vectors drifted from {}.\n\
         The v1 envelope is frozen ABI (⟨D-QCERT⟩) — this is a compatibility \
         break, not a refresh. If the change is intended and versioned, \
         regenerate with BLESS_VECTORS=1 and review the diff by hand.",
        path.display()
    );
}

#[test]
fn quiescence_vectors_are_frozen() {
    assert_frozen("soc_quiescence_v1.json", build_quiescence_manifest());
}

#[test]
fn divergence_vectors_are_frozen() {
    assert_frozen("soc_divergence_v1.json", build_divergence_manifest());
}

#[test]
fn quiescence_vectors_reproduced_by_primitive_canon_writes() {
    for fixture in quiescence_fixtures() {
        let produced = encode_quiescence_v1(&fixture.certificate);
        let independent = independent_quiescence_envelope(&fixture.certificate);
        assert_eq!(
            to_hex(&independent),
            to_hex(&produced),
            "{}: independent envelope bytes differ from the production encoder",
            fixture.name
        );

        assert_eq!(
            Digest::of(Domain::Value, &independent).to_hex(),
            quiescence_certificate_id(&fixture.certificate).to_hex(),
            "{}: independent certificate id differs from the production id",
            fixture.name
        );

        assert_eq!(
            independent_quiescence_judgement(&fixture.certificate).to_hex(),
            fixture.certificate.judgement.to_hex(),
            "{}: the certificate's judgement is not the Quiescent statement about \
             its own terminal world",
            fixture.name
        );
    }
}

#[test]
fn divergence_vectors_reproduced_by_primitive_canon_writes() {
    for fixture in divergence_fixtures() {
        let produced = encode_divergence_v1(&fixture.certificate);
        let independent = independent_divergence_envelope(&fixture.certificate);
        assert_eq!(
            to_hex(&independent),
            to_hex(&produced),
            "{}: independent envelope bytes differ from the production encoder",
            fixture.name
        );

        assert_eq!(
            Digest::of(Domain::Value, &independent).to_hex(),
            divergence_certificate_id(&fixture.certificate).to_hex(),
            "{}: independent certificate id differs from the production id",
            fixture.name
        );
    }
}

/// ⟨D-OBS⟩'s preimage is frozen ABI in its own right: every certificate embeds
/// the resulting id, so a drift here would silently re-identify every
/// certificate ever minted.
#[test]
fn the_observation_profile_id_is_reproduced_from_its_frozen_preimage() {
    let profile = vector_profile();
    let independent =
        independent_profile_preimage(&administrative_partition(), &realizing_partition());

    assert_eq!(
        ObservationProfileId::from_canon(&independent),
        profile.id(),
        "the generator-partition preimage drifted from its frozen ⟨D-OBS⟩ layout"
    );
}

/// The two partitions are not interchangeable: swapping them is a different
/// observation boundary and must be a different identity, or a profile that
/// hides everything would collide with one that hides nothing.
#[test]
fn swapping_the_partitions_changes_the_profile_identity() {
    let forward = vector_profile();
    let backward =
        GeneratorPartitionProfile::new(realizing_partition(), administrative_partition())
            .expect("disjoint partitions");
    assert_ne!(forward.id(), backward.id());
}
