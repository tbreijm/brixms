//! Stage B gate for divergence-sensitive saturation (ADR-0014 §9, #61).
//!
//! Covers #61's acceptance criteria 2 (a quiescence certificate replays and is
//! checkable by an independent path) and 3 (an exhausted or diverging
//! administrative path yields `Unknown` with an explicit reason, **never** a
//! certificate and **never** `Refuted`), plus `Build_Plan_v3_SOC.md` Step 8's
//! named divergence-sensitivity conformance test: a terminal state and an
//! infinitely-searching state must be *distinguished*.
//!
//! The single most important assertion in this file is
//! [`a_terminal_state_and_an_infinitely_searching_state_are_distinguished`].
//! Everything else is the machinery that makes it mean something.

use std::collections::{BTreeMap, BTreeSet};

use brix_canon::{CanonWriter, Canonical, Digest, Domain};
use brix_semantic::{ConfigId, ContextId, Decomposition, GeneratorId, Outcome};
use soc_core::adm::AdmAll;
use soc_core::calendar::Key;
use soc_core::commit::{CommitError, SettlementRegime};
use soc_core::exec::ExecConfig;
use soc_core::history::History;
use soc_core::intern::{Handle, Interner};
use soc_core::regime::{Candidate, Regime};
use soc_core::saturate::{
    check_divergence_certificate, check_quiescence_certificate, decode_divergence_v1,
    decode_quiescence_v1, encode_divergence_v1, encode_quiescence_v1, quiescence_certificate_id,
    sat_step, validate_quiescence_v1, AssumptionId, AssumptionMode, CertEnvelopeError,
    CertificateCheck, CertificateCheckError, DeclaredAssumptions, DivergenceCertificateV1,
    GeneratorPartitionProfile, ObservationProfile, PresentationIdV1, PresentationV1,
    QuiescenceCertificateV1, SaturatedStep, SaturationBudget, SaturationUnknown,
    CERTIFICATE_FORMAT_V1, QUIESCENCE_MARKER, SATURATION_PROFILE_V1,
};

// ---------------------------------------------------------------------------
// Fixture — the edge-table regime idiom, as in `saturation_labels.rs`.
// ---------------------------------------------------------------------------

struct Edge {
    witness: Handle,
    successor: Handle,
    generators: Vec<GeneratorId>,
}

struct ChainRegime {
    id: Handle,
    edges: BTreeMap<Handle, Edge>,
    configs: BTreeMap<Handle, ConfigId>,
}

impl Regime for ChainRegime {
    fn candidates(&self, e: &ExecConfig) -> Vec<Candidate> {
        self.edges
            .get(&e.world)
            .map(|edge| {
                vec![Candidate {
                    regime: self.id,
                    witness: edge.witness,
                    successor: edge.successor,
                }]
            })
            .unwrap_or_default()
    }
}

impl SettlementRegime for ChainRegime {
    fn try_decompose(&self, e: &ExecConfig, c: &Candidate) -> Result<Decomposition, CommitError> {
        let edge = self
            .edges
            .get(&e.world)
            .expect("try_decompose on a live edge");
        let src = self.configs[&e.world];
        let dst = self.configs[&c.successor];
        let mut configs = vec![src];
        for _ in 1..edge.generators.len() {
            configs.push(src);
        }
        configs.push(dst);
        Ok(Decomposition::recorded(edge.generators.clone(), configs).expect("well-formed chain"))
    }
}

/// A regime that **reads `e.history`** — the exact thing P1 forbids.
///
/// From `w0` it offers an edge to `w1` on a pristine history and an edge to
/// `w2` on any other, so returning to `w0` around the loop offers a *different*
/// candidate set at the *same* observable state. That is precisely the
/// falsification the bounded P1 check is looking for.
struct HistoryPeekingRegime {
    id: Handle,
    w0: Handle,
    w1: Handle,
    w2: Handle,
    witness: Handle,
    configs: BTreeMap<Handle, ConfigId>,
    generators: Vec<GeneratorId>,
}

impl Regime for HistoryPeekingRegime {
    fn candidates(&self, e: &ExecConfig) -> Vec<Candidate> {
        let successor = if e.world == self.w0 {
            if e.history == History::empty().digest() {
                self.w1
            } else {
                self.w2
            }
        } else if e.world == self.w1 {
            self.w0
        } else {
            return Vec::new();
        };
        vec![Candidate {
            regime: self.id,
            witness: self.witness,
            successor,
        }]
    }
}

impl SettlementRegime for HistoryPeekingRegime {
    fn try_decompose(&self, e: &ExecConfig, c: &Candidate) -> Result<Decomposition, CommitError> {
        Ok(Decomposition::recorded(
            self.generators.clone(),
            vec![self.configs[&e.world], self.configs[&c.successor]],
        )
        .expect("well-formed decomposition"))
    }
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

fn gen_tau() -> GeneratorId {
    GeneratorId::named("saturation-cert-fixture.tau@1")
}
fn gen_realizing() -> GeneratorId {
    GeneratorId::named("saturation-cert-fixture.realizing@1")
}

fn hiding_profile() -> GeneratorPartitionProfile {
    GeneratorPartitionProfile::new(
        [gen_tau()].into_iter().collect(),
        [gen_realizing()].into_iter().collect(),
    )
    .expect("disjoint partitions")
}

struct Fixture {
    interner: Interner,
    regime: ChainRegime,
    worlds: BTreeMap<&'static str, Handle>,
    policy: Handle,
}

fn build_fixture(spec: &[(&'static str, &'static str, Vec<GeneratorId>)]) -> Fixture {
    let mut interner = Interner::new();
    let mut worlds: BTreeMap<&'static str, Handle> = BTreeMap::new();
    for (from, to, _) in spec {
        for name in [from, to] {
            if !worlds.contains_key(name) {
                worlds.insert(name, tag(&mut interner, name));
            }
        }
    }
    let policy = tag(&mut interner, "policy");
    let regime_id = tag(&mut interner, "regime.chain");

    let mut configs = BTreeMap::new();
    for handle in worlds.values() {
        configs.insert(*handle, ConfigId(interner.resolve(*handle)));
    }

    let mut edges = BTreeMap::new();
    for (from, to, generators) in spec {
        let witness = tag(&mut interner, &format!("witness.{from}->{to}"));
        edges.insert(
            worlds[from],
            Edge {
                witness,
                successor: worlds[to],
                generators: generators.clone(),
            },
        );
    }

    Fixture {
        interner,
        regime: ChainRegime {
            id: regime_id,
            edges,
            configs,
        },
        worlds,
        policy,
    }
}

impl Fixture {
    fn exec_at(&self, world: &str) -> ExecConfig {
        ExecConfig::new(self.worlds[world], self.policy, History::empty().digest())
    }
}

const PRESENTATION_SEED: &[u8] = b"saturation-cert-fixture@1";
const REGIME_SET_SEED: &[u8] = b"saturation-cert-fixture.regime-set";
const ADM_SEED: &[u8] = b"saturation-cert-fixture.adm-all";

fn presentation<'a>(
    regimes: &'a [&'a dyn SettlementRegime],
    profile: &'a dyn ObservationProfile,
    interner: &'a Interner,
    assumptions: DeclaredAssumptions,
) -> PresentationV1<'a> {
    PresentationV1 {
        id: PresentationIdV1::from_canon(PRESENTATION_SEED),
        regimes,
        regime_set: Digest::of(Domain::Value, REGIME_SET_SEED),
        adm: &AdmAll,
        adm_id: Digest::of(Domain::Value, ADM_SEED),
        profile,
        interner,
        context: ContextId::root(),
        assumptions,
    }
}

fn keyer() -> impl FnMut(&Candidate, u64) -> Key {
    |c: &Candidate, phase: u64| Key::new(phase, 0, tiebreak_of(c))
}

/// A keyer whose *priority* is the phase — a direct P6 violation.
fn phase_dependent_keyer() -> impl FnMut(&Candidate, u64) -> Key {
    |c: &Candidate, phase: u64| Key::new(phase, phase, tiebreak_of(c))
}

fn budget() -> SaturationBudget {
    SaturationBudget::uniform(64)
}

/// `w0 -τ-> w1 -τ-> w2`, with `w2` terminal — the quiescence case.
fn terminating_fixture() -> Fixture {
    build_fixture(&[("w0", "w1", vec![gen_tau()]), ("w1", "w2", vec![gen_tau()])])
}

/// `w0 -τ-> w1 -τ-> w0` — the two-step administrative loop.
fn looping_fixture() -> Fixture {
    build_fixture(&[("w0", "w1", vec![gen_tau()]), ("w1", "w0", vec![gen_tau()])])
}

// ---------------------------------------------------------------------------
// AC-2 — a quiescence certificate replays and is independently checkable.
// ---------------------------------------------------------------------------

#[test]
fn a_terminal_state_certifies_and_the_certificate_re_derives() {
    let fx = terminating_fixture();
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(regimes, &profile, &fx.interner, DeclaredAssumptions::all());
    let mut k = keyer();

    let (step, consumed, _) = sat_step(&pres, &fx.exec_at("w0"), 0, &mut k, budget());

    let SaturatedStep::Quiescent(cert) = step else {
        panic!("expected quiescence after a finite τ prefix, got {step:?}");
    };
    assert_eq!(cert.hidden.len(), 2, "both τ steps are recorded");
    assert_eq!(cert.grade, Outcome::Derived);

    // The terminal configuration the checker re-enumerates at. Its history is
    // whatever the run produced; the checker binds it by *digest*, not by
    // history, which is the whole point of the projection.
    let terminal = ExecConfig::new(fx.worlds["w2"], fx.policy, History::empty().digest());

    let check = check_quiescence_certificate(&cert, &pres, &terminal, &consumed);
    assert_eq!(
        check,
        CertificateCheck::Verified {
            certificate_id: quiescence_certificate_id(&cert)
        },
        "a certificate minted by sat_step must re-derive"
    );
}

#[test]
fn the_certificate_round_trips_through_the_frozen_envelope() {
    let cert = mint_quiescence();
    let bytes = encode_quiescence_v1(&cert);

    assert_eq!(decode_quiescence_v1(&bytes), Ok(cert.clone()));

    let id = validate_quiescence_v1(
        &bytes,
        cert.context,
        cert.profile,
        PresentationIdV1::from_canon(PRESENTATION_SEED),
    )
    .expect("the envelope binds to its own presentation");
    assert_eq!(id, quiescence_certificate_id(&cert));
}

#[test]
fn the_envelope_opens_with_the_frozen_marker_version_and_saturation_profile() {
    let bytes = encode_quiescence_v1(&mint_quiescence());
    let mut r = brix_canon::CanonReader::new(&bytes);
    assert_eq!(r.read_bytes().unwrap(), QUIESCENCE_MARKER);
    assert_eq!(r.read_uint().unwrap(), CERTIFICATE_FORMAT_V1);
    assert_eq!(r.read_bytes().unwrap(), SATURATION_PROFILE_V1.as_bytes());
}

/// A certificate is bound to *its own* boundary. Offering it against a
/// different context, profile, or revision must not be a near-miss that the
/// reader repairs — it must be a rejection, because that is exactly the
/// silent-reinterpretation failure SOC-LAW-10's domain clause exists to stop.
#[test]
fn validation_refuses_a_certificate_from_another_boundary() {
    let cert = mint_quiescence();
    let bytes = encode_quiescence_v1(&cert);
    let presentation_id = PresentationIdV1::from_canon(PRESENTATION_SEED);

    assert_eq!(
        validate_quiescence_v1(
            &bytes,
            ContextId::from_canon(b"some.other.context"),
            cert.profile,
            presentation_id
        ),
        Err(CertEnvelopeError::ContextMismatch)
    );
    assert_eq!(
        validate_quiescence_v1(
            &bytes,
            cert.context,
            soc_core::saturate::ObservationProfileId::from_canon(b"some.other.profile"),
            presentation_id
        ),
        Err(CertEnvelopeError::ObservationProfileMismatch)
    );
    assert_eq!(
        validate_quiescence_v1(
            &bytes,
            cert.context,
            cert.profile,
            PresentationIdV1::from_canon(b"some.other.revision")
        ),
        Err(CertEnvelopeError::PresentationMismatch)
    );
}

// ---------------------------------------------------------------------------
// Malformed envelopes — fail closed, never best-effort.
// ---------------------------------------------------------------------------

/// Every quiescence field as a mutable byte-level record, so a test can tamper
/// with exactly one and rebuild. Written with primitive `CanonWriter` calls in
/// the frozen field order.
struct QuiescenceBytes {
    marker: Vec<u8>,
    version: u64,
    saturation_profile: Vec<u8>,
    profile: Digest,
    context: Digest,
    presentation: Digest,
    policy: Digest,
    src_world: Digest,
    terminal_world: Digest,
    hidden: Vec<Digest>,
    declared_hidden_count: u64,
    prefix_chain: Digest,
    regime_set: Digest,
    adm_id: Digest,
    enumeration_ordinal: u64,
    grade_ordinal: u64,
    judgement: Digest,
}

impl QuiescenceBytes {
    fn of(cert: &QuiescenceCertificateV1) -> Self {
        Self {
            marker: QUIESCENCE_MARKER.to_vec(),
            version: CERTIFICATE_FORMAT_V1,
            saturation_profile: SATURATION_PROFILE_V1.as_bytes().to_vec(),
            profile: cert.profile.digest(),
            context: cert.context.digest(),
            presentation: cert.presentation.digest(),
            policy: cert.policy.digest(),
            src_world: cert.src_world.digest(),
            terminal_world: cert.terminal_world.digest(),
            hidden: cert.hidden.clone(),
            declared_hidden_count: cert.hidden.len() as u64,
            prefix_chain: cert.prefix_chain,
            regime_set: cert.regime_set,
            adm_id: cert.adm_id,
            enumeration_ordinal: 0,
            grade_ordinal: 2, // Outcome::Derived
            judgement: cert.judgement.digest(),
        }
    }

    fn encode(&self) -> Vec<u8> {
        let mut w = CanonWriter::new();
        w.write_bytes(&self.marker);
        w.write_uint(self.version);
        w.write_bytes(&self.saturation_profile);
        for digest in [
            self.profile,
            self.context,
            self.presentation,
            self.policy,
            self.src_world,
            self.terminal_world,
        ] {
            w.write_bytes(digest.as_bytes());
        }
        w.write_uint(self.declared_hidden_count);
        for digest in &self.hidden {
            w.write_bytes(digest.as_bytes());
        }
        for digest in [self.prefix_chain, self.regime_set, self.adm_id] {
            w.write_bytes(digest.as_bytes());
        }
        w.write_uint(self.enumeration_ordinal);
        w.write_uint(self.grade_ordinal);
        w.write_bytes(self.judgement.as_bytes());
        w.finish()
    }
}

#[test]
fn the_tamper_builder_agrees_with_the_production_encoder() {
    // Otherwise every rejection test below could be passing for the wrong
    // reason — rejecting a shape the encoder never produces.
    let cert = mint_quiescence();
    assert_eq!(
        QuiescenceBytes::of(&cert).encode(),
        encode_quiescence_v1(&cert)
    );
}

#[test]
fn decode_rejects_a_foreign_marker() {
    let mut raw = QuiescenceBytes::of(&mint_quiescence());
    raw.marker = b"brix.soc.not-a-certificate".to_vec();
    assert_eq!(
        decode_quiescence_v1(&raw.encode()),
        Err(CertEnvelopeError::BadMarker)
    );
}

/// The divergence marker in a quiescence reader is the sharpest confusion this
/// format could suffer: fields 1–8 are shared, so only the marker separates
/// "nothing left to do" from "never finishes".
#[test]
fn a_divergence_envelope_is_not_readable_as_quiescence() {
    let (cert, _, _) = mint_divergence();
    assert_eq!(
        decode_quiescence_v1(&encode_divergence_v1(&cert)),
        Err(CertEnvelopeError::BadMarker)
    );
    assert_eq!(
        decode_divergence_v1(&encode_quiescence_v1(&mint_quiescence())),
        Err(CertEnvelopeError::BadMarker)
    );
}

#[test]
fn decode_rejects_an_unknown_version() {
    let mut raw = QuiescenceBytes::of(&mint_quiescence());
    raw.version = 2;
    assert_eq!(
        decode_quiescence_v1(&raw.encode()),
        Err(CertEnvelopeError::UnknownVersion(2))
    );
}

#[test]
fn decode_rejects_an_unknown_saturation_profile() {
    let mut raw = QuiescenceBytes::of(&mint_quiescence());
    raw.saturation_profile = b"brix.soc.saturation@2".to_vec();
    assert_eq!(
        decode_quiescence_v1(&raw.encode()),
        Err(CertEnvelopeError::UnknownSaturationProfile)
    );
}

/// The load-bearing honesty field. A future engine that enumerated only part of
/// the frontier must not be able to mint a v1 certificate that a v1 reader
/// accepts — it has to mint v2 (ADR-0014 §6.2, risk 1).
#[test]
fn decode_rejects_an_incomplete_enumeration_ordinal() {
    let mut raw = QuiescenceBytes::of(&mint_quiescence());
    raw.enumeration_ordinal = 1;
    assert_eq!(
        decode_quiescence_v1(&raw.encode()),
        Err(CertEnvelopeError::UnknownEnumerationOrdinal(1))
    );
}

#[test]
fn decode_rejects_a_grade_that_is_not_derived() {
    let mut raw = QuiescenceBytes::of(&mint_quiescence());
    raw.grade_ordinal = 0; // Outcome::Proven — a settlement certificate is never a theorem.
    assert_eq!(
        decode_quiescence_v1(&raw.encode()),
        Err(CertEnvelopeError::QuiescenceGradeNotDerived)
    );

    raw.grade_ordinal = 1; // Outcome::Refuted — and never a refutation either.
    assert_eq!(
        decode_quiescence_v1(&raw.encode()),
        Err(CertEnvelopeError::QuiescenceGradeNotDerived)
    );

    raw.grade_ordinal = 99;
    assert_eq!(
        decode_quiescence_v1(&raw.encode()),
        Err(CertEnvelopeError::UnknownOutcomeOrdinal(99))
    );
}

/// A prefix count that disagrees with the digests present shifts every later
/// field. The guarantee here is **rejection**, not a particular error: an
/// over-count runs off the end, while an under-count reads the next digest as
/// the chain and cascades until some field fails to typecheck — for this
/// layout, the enumeration ordinal, which sees a digest's 32-byte length
/// prefix. What matters is that no shifted read ever decodes.
#[test]
fn decode_rejects_a_prefix_count_that_lies_about_its_digests() {
    let mut over = QuiescenceBytes::of(&mint_quiescence());
    over.declared_hidden_count += 1;
    assert!(
        decode_quiescence_v1(&over.encode()).is_err(),
        "a count larger than the digests present must not silently short-read"
    );

    let mut under = QuiescenceBytes::of(&mint_quiescence());
    under.declared_hidden_count -= 1;
    assert_eq!(
        decode_quiescence_v1(&under.encode()),
        Err(CertEnvelopeError::UnknownEnumerationOrdinal(32)),
        "an under-count shifts the tail; the ordinal field catches it"
    );
}

#[test]
fn decode_rejects_trailing_bytes() {
    let mut bytes = encode_quiescence_v1(&mint_quiescence());
    bytes.push(0);
    assert_eq!(
        decode_quiescence_v1(&bytes),
        Err(CertEnvelopeError::TrailingBytes)
    );
}

#[test]
fn decode_rejects_every_truncated_prefix() {
    let bytes = encode_quiescence_v1(&mint_quiescence());
    for n in 0..bytes.len() {
        assert!(
            decode_quiescence_v1(&bytes[..n]).is_err(),
            "prefix of length {n} must be rejected"
        );
    }
}

#[test]
fn decode_rejects_a_non_minimal_integer() {
    // Marker, then the version 1 written non-minimally as [len=2, 0x00, 0x01].
    // The codec admits exactly one encoding per value; a second one would give
    // the same certificate two identities.
    let mut bytes = vec![0x01, QUIESCENCE_MARKER.len() as u8];
    bytes.extend_from_slice(QUIESCENCE_MARKER);
    bytes.extend_from_slice(&[0x02, 0x00, 0x01]);
    assert_eq!(
        decode_quiescence_v1(&bytes),
        Err(CertEnvelopeError::NonMinimalInt)
    );
}

#[test]
fn decode_rejects_a_lasso_with_no_cycle() {
    let (cert, _, _) = mint_divergence();
    let mut bytes = DivergenceBytes::of(&cert);
    bytes.cycle = 0;
    assert_eq!(
        decode_divergence_v1(&bytes.encode()),
        Err(CertEnvelopeError::ZeroCycleLength)
    );
}

#[test]
fn decode_rejects_a_divergence_grade_that_is_not_unknown() {
    let (cert, _, _) = mint_divergence();
    let mut bytes = DivergenceBytes::of(&cert);
    bytes.grade_ordinal = 2; // Outcome::Derived
    assert_eq!(
        decode_divergence_v1(&bytes.encode()),
        Err(CertEnvelopeError::DivergenceGradeNotUnknown)
    );
}

#[test]
fn decode_rejects_an_unknown_assumption_mode() {
    let (cert, _, _) = mint_divergence();
    let mut bytes = DivergenceBytes::of(&cert);
    bytes.assumption_ordinal = 7;
    assert_eq!(
        decode_divergence_v1(&bytes.encode()),
        Err(CertEnvelopeError::UnknownAssumptionOrdinal(7))
    );
}

struct DivergenceBytes {
    marker: Vec<u8>,
    prelude: [Digest; 5],
    stem: u64,
    cycle: u64,
    lasso: Vec<Digest>,
    cycle_world: Digest,
    cycle_policy: Digest,
    assumption_ordinal: u64,
    grade_ordinal: u64,
}

impl DivergenceBytes {
    fn of(cert: &DivergenceCertificateV1) -> Self {
        Self {
            marker: b"brix.soc.divergence".to_vec(),
            prelude: [
                cert.profile.digest(),
                cert.context.digest(),
                cert.presentation.digest(),
                cert.policy.digest(),
                cert.src_world.digest(),
            ],
            stem: cert.stem,
            cycle: cert.cycle,
            lasso: cert.lasso.clone(),
            cycle_world: cert.cycle_world.digest(),
            cycle_policy: cert.cycle_policy.digest(),
            assumption_ordinal: 0,
            grade_ordinal: 4, // Outcome::Unknown
        }
    }

    fn encode(&self) -> Vec<u8> {
        let mut w = CanonWriter::new();
        w.write_bytes(&self.marker);
        w.write_uint(CERTIFICATE_FORMAT_V1);
        w.write_bytes(SATURATION_PROFILE_V1.as_bytes());
        for digest in self.prelude {
            w.write_bytes(digest.as_bytes());
        }
        w.write_uint(self.stem);
        w.write_uint(self.cycle);
        for digest in &self.lasso {
            w.write_bytes(digest.as_bytes());
        }
        w.write_bytes(self.cycle_world.as_bytes());
        w.write_bytes(self.cycle_policy.as_bytes());
        w.write_uint(self.assumption_ordinal);
        w.write_uint(self.grade_ordinal);
        w.finish()
    }
}

// ---------------------------------------------------------------------------
// The semantic checker re-derives; it does not trust.
// ---------------------------------------------------------------------------

/// A certificate whose *envelope* is impeccable but whose *claim* is false. The
/// distinction between decoding and checking is the whole reason both exist.
#[test]
fn a_well_formed_certificate_over_a_live_frontier_is_refused() {
    let fx = terminating_fixture();
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(regimes, &profile, &fx.interner, DeclaredAssumptions::all());
    let mut k = keyer();

    // Quiesce at w2, then re-point the whole claim at w0 — which is emphatically
    // not quiescent — keeping the envelope perfectly well-formed.
    let (step, _, _) = sat_step(&pres, &fx.exec_at("w2"), 0, &mut k, budget());
    let SaturatedStep::Quiescent(mut cert) = step else {
        panic!("w2 is terminal");
    };
    let w0 = ConfigId(fx.interner.resolve(fx.worlds["w0"]));
    cert.src_world = w0;
    cert.terminal_world = w0;
    cert.judgement = soc_core::saturate::quiescence_judgement(&cert);

    assert!(
        decode_quiescence_v1(&encode_quiescence_v1(&cert)).is_ok(),
        "the tampered certificate is still structurally valid — that is the point"
    );

    let check = check_quiescence_certificate(&cert, &pres, &fx.exec_at("w0"), &[]);
    assert_eq!(
        check,
        CertificateCheck::Unknown(CertificateCheckError::FrontierNotEmpty { candidates: 1 }),
        "re-enumeration must catch a claim the envelope cannot"
    );
}

#[test]
fn a_prefix_containing_a_realizing_step_invalidates_the_certificate() {
    // w0 -o-> w1, and w1 is terminal. Saturating from w0 gives a *realizing*
    // step; forcing that step into a quiescence prefix must be refused.
    let fx = build_fixture(&[("w0", "w1", vec![gen_realizing()])]);
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(regimes, &profile, &fx.interner, DeclaredAssumptions::all());
    let mut k = keyer();

    let (step, realizing_steps, _) = sat_step(&pres, &fx.exec_at("w0"), 0, &mut k, budget());
    assert!(matches!(step, SaturatedStep::Realizing { .. }));

    // Now certify quiescence at w1 and splice the realizing step in as if it
    // had been hidden.
    let (terminal_step, _, _) = sat_step(&pres, &fx.exec_at("w1"), 0, &mut k, budget());
    let SaturatedStep::Quiescent(mut cert) = terminal_step else {
        panic!("w1 is terminal");
    };
    cert.src_world = ConfigId(fx.interner.resolve(fx.worlds["w0"]));
    cert.hidden = realizing_steps
        .iter()
        .map(|s| s.canon_digest(Domain::Value))
        .collect();
    cert.prefix_chain = *soc_core::journal::Journal::replay_chain(&realizing_steps)
        .last()
        .expect("one step");
    cert.judgement = soc_core::saturate::quiescence_judgement(&cert);

    assert_eq!(
        check_quiescence_certificate(&cert, &pres, &fx.exec_at("w1"), &realizing_steps),
        CertificateCheck::Unknown(CertificateCheckError::PrefixNotAdministrative { at_step: 0 }),
        "one realizing step in the prefix invalidates the whole hiding claim"
    );
}

#[test]
fn a_certificate_whose_steps_do_not_replay_is_refused() {
    let fx = terminating_fixture();
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(regimes, &profile, &fx.interner, DeclaredAssumptions::all());
    let mut k = keyer();
    let (step, consumed, _) = sat_step(&pres, &fx.exec_at("w0"), 0, &mut k, budget());
    let SaturatedStep::Quiescent(mut cert) = step else {
        panic!("expected quiescence");
    };
    let terminal = ExecConfig::new(fx.worlds["w2"], fx.policy, History::empty().digest());

    // Same steps, a chain digest from somewhere else.
    cert.prefix_chain = Digest::of(Domain::Value, b"not the chain");
    cert.judgement = soc_core::saturate::quiescence_judgement(&cert);
    assert_eq!(
        check_quiescence_certificate(&cert, &pres, &terminal, &consumed),
        CertificateCheck::Unknown(CertificateCheckError::ChainDigestMismatch)
    );

    // Fewer steps than recorded.
    let (step, _, _) = sat_step(&pres, &fx.exec_at("w0"), 0, &mut k, budget());
    let SaturatedStep::Quiescent(cert) = step else {
        panic!("expected quiescence");
    };
    assert_eq!(
        check_quiescence_certificate(&cert, &pres, &terminal, &consumed[..1]),
        CertificateCheck::Unknown(CertificateCheckError::StepCountMismatch {
            declared: 2,
            supplied: 1
        })
    );
}

#[test]
fn a_certificate_bound_to_another_governance_policy_is_refused() {
    let fx = terminating_fixture();
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(regimes, &profile, &fx.interner, DeclaredAssumptions::all());
    let mut k = keyer();
    let (step, consumed, _) = sat_step(&pres, &fx.exec_at("w0"), 0, &mut k, budget());
    let SaturatedStep::Quiescent(mut cert) = step else {
        panic!("expected quiescence");
    };
    let terminal = ExecConfig::new(fx.worlds["w2"], fx.policy, History::empty().digest());

    cert.adm_id = Digest::of(Domain::Value, b"a stricter policy");
    cert.judgement = soc_core::saturate::quiescence_judgement(&cert);
    assert_eq!(
        check_quiescence_certificate(&cert, &pres, &terminal, &consumed),
        CertificateCheck::Unknown(CertificateCheckError::AdmMismatch),
        "tightening Adm changes which configurations are quiescent, so the \
         identity is load-bearing"
    );
}

#[test]
fn a_certificate_whose_judgement_does_not_recompute_is_refused() {
    let fx = terminating_fixture();
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(regimes, &profile, &fx.interner, DeclaredAssumptions::all());
    let mut k = keyer();
    let (step, consumed, _) = sat_step(&pres, &fx.exec_at("w0"), 0, &mut k, budget());
    let SaturatedStep::Quiescent(mut cert) = step else {
        panic!("expected quiescence");
    };
    let terminal = ExecConfig::new(fx.worlds["w2"], fx.policy, History::empty().digest());

    cert.judgement = brix_semantic::JudgementId::from_canon(b"a judgement about something else");
    assert_eq!(
        check_quiescence_certificate(&cert, &pres, &terminal, &consumed),
        CertificateCheck::Unknown(CertificateCheckError::JudgementMismatch)
    );
}

#[test]
fn a_certificate_offered_against_the_wrong_configuration_is_refused() {
    let fx = terminating_fixture();
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(regimes, &profile, &fx.interner, DeclaredAssumptions::all());
    let mut k = keyer();
    let (step, consumed, _) = sat_step(&pres, &fx.exec_at("w0"), 0, &mut k, budget());
    let SaturatedStep::Quiescent(cert) = step else {
        panic!("expected quiescence");
    };

    assert_eq!(
        check_quiescence_certificate(&cert, &pres, &fx.exec_at("w1"), &consumed),
        CertificateCheck::Unknown(CertificateCheckError::ConfigMismatch),
        "the checker must prove the config it re-enumerates at is the one named"
    );
}

// ---------------------------------------------------------------------------
// AC-3 — divergence, exhaustion, and the things that are neither.
// ---------------------------------------------------------------------------

#[test]
fn a_two_step_administrative_loop_certifies_divergence() {
    let (cert, lasso, _) = mint_divergence();
    assert_eq!(cert.stem, 0, "the cycle starts at the initial state");
    assert_eq!(cert.cycle, 2, "w0 -> w1 -> w0");
    assert_eq!(cert.lasso.len(), 2);
    assert_eq!(cert.assumptions, AssumptionMode::DeclaredP1P6);
    assert_eq!(
        cert.grade,
        Outcome::Unknown,
        "divergence is Unknown for the completion question — never Refuted, \
         never Derived, never the 1 summand"
    );
    assert_ne!(cert.grade, Outcome::Refuted);
    assert_eq!(
        lasso.len(),
        2,
        "both τ steps are still committed and journaled"
    );
}

#[test]
fn the_divergence_certificate_re_derives_and_round_trips() {
    let fx = looping_fixture();
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(regimes, &profile, &fx.interner, DeclaredAssumptions::all());
    let mut k = keyer();
    let (step, lasso, _) = sat_step(&pres, &fx.exec_at("w0"), 0, &mut k, budget());
    let SaturatedStep::Divergent(cert) = step else {
        panic!("expected certified divergence, got {step:?}");
    };

    assert_eq!(
        decode_divergence_v1(&encode_divergence_v1(&cert)),
        Ok((*cert).clone())
    );
    assert!(
        check_divergence_certificate(&cert, &pres, &lasso).is_verified(),
        "a certificate minted by sat_step must re-derive"
    );
}

#[test]
fn a_divergence_certificate_whose_cycle_does_not_close_is_refused() {
    let fx = looping_fixture();
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(regimes, &profile, &fx.interner, DeclaredAssumptions::all());
    let mut k = keyer();
    let (step, lasso, _) = sat_step(&pres, &fx.exec_at("w0"), 0, &mut k, budget());
    let SaturatedStep::Divergent(mut cert) = step else {
        panic!("expected certified divergence");
    };

    cert.cycle_world = ConfigId(fx.interner.resolve(fx.worlds["w1"]));
    assert_eq!(
        check_divergence_certificate(&cert, &pres, &lasso),
        CertificateCheck::Unknown(CertificateCheckError::CycleEntryMismatch)
    );
}

/// Without P1 the engine has *noticed* a repeated observable state but may not
/// conclude anything from it: a regime that reads `history` could behave
/// differently on the second pass. Noticing is not knowing.
#[test]
fn a_loop_without_declared_history_independence_yields_undeclared_not_divergence() {
    let fx = looping_fixture();
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(
        regimes,
        &profile,
        &fx.interner,
        DeclaredAssumptions {
            history_independent: false,
            phase_stable_keying: true,
        },
    );
    let mut k = keyer();
    let (step, _, _) = sat_step(&pres, &fx.exec_at("w0"), 0, &mut k, budget());

    assert_eq!(
        step,
        SaturatedStep::Unknown(SaturationUnknown::UndeclaredAssumption(
            AssumptionId::HistoryIndependence
        ))
    );
}

#[test]
fn a_loop_without_declared_phase_stable_keying_yields_undeclared_not_divergence() {
    let fx = looping_fixture();
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(
        regimes,
        &profile,
        &fx.interner,
        DeclaredAssumptions {
            history_independent: true,
            phase_stable_keying: false,
        },
    );
    let mut k = keyer();
    let (step, _, _) = sat_step(&pres, &fx.exec_at("w0"), 0, &mut k, budget());

    assert_eq!(
        step,
        SaturatedStep::Unknown(SaturationUnknown::UndeclaredAssumption(
            AssumptionId::PhaseStableKeying
        ))
    );
}

/// A *declared* hypothesis that the bounded check falsifies is worse than an
/// undeclared one: the presentation asserted something false. Fail closed.
#[test]
fn a_declared_p1_that_the_bounded_check_falsifies_yields_assumption_violated() {
    let mut interner = Interner::new();
    let w0 = tag(&mut interner, "w0");
    let w1 = tag(&mut interner, "w1");
    let w2 = tag(&mut interner, "w2");
    let witness = tag(&mut interner, "witness");
    let policy = tag(&mut interner, "policy");
    let regime_id = tag(&mut interner, "regime.history-peeking");
    let configs = [w0, w1, w2]
        .into_iter()
        .map(|h| (h, ConfigId(interner.resolve(h))))
        .collect();

    let peeking = HistoryPeekingRegime {
        id: regime_id,
        w0,
        w1,
        w2,
        witness,
        configs,
        generators: vec![gen_tau()],
    };
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &peeking;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(regimes, &profile, &interner, DeclaredAssumptions::all());
    let mut k = keyer();

    let start = ExecConfig::new(w0, policy, History::empty().digest());
    let (step, _, _) = sat_step(&pres, &start, 0, &mut k, budget());

    assert_eq!(
        step,
        SaturatedStep::Unknown(SaturationUnknown::AssumptionViolated {
            assumption: AssumptionId::HistoryIndependence,
            at_step: 2,
        }),
        "the same observable state offered different candidates on the second \
         visit, so the presentation's P1 declaration is false"
    );
}

#[test]
fn a_declared_p6_that_the_bounded_check_falsifies_yields_assumption_violated() {
    let fx = looping_fixture();
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(regimes, &profile, &fx.interner, DeclaredAssumptions::all());
    let mut k = phase_dependent_keyer();
    let (step, _, _) = sat_step(&pres, &fx.exec_at("w0"), 0, &mut k, budget());

    assert_eq!(
        step,
        SaturatedStep::Unknown(SaturationUnknown::AssumptionViolated {
            assumption: AssumptionId::PhaseStableKeying,
            at_step: 2,
        }),
        "a keyer whose priority is the phase can reorder the frontier between \
         visits, so the orbit is not a cycle"
    );
}

/// The ADR's Stage B exhaustion fixture: a τ-chain longer than its budget must
/// produce **no certificate of either kind**.
#[test]
fn an_exhausted_budget_produces_neither_certificate() {
    let fx = build_fixture(&[
        ("w0", "w1", vec![gen_tau()]),
        ("w1", "w2", vec![gen_tau()]),
        ("w2", "w3", vec![gen_tau()]),
        ("w3", "w4", vec![gen_tau()]),
        ("w4", "w5", vec![gen_tau()]),
        ("w5", "w6", vec![gen_tau()]),
        ("w6", "w7", vec![gen_tau()]),
        ("w7", "w8", vec![gen_tau()]),
        ("w8", "w9", vec![gen_tau()]),
        ("w9", "w10", vec![gen_realizing()]),
    ]);
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(regimes, &profile, &fx.interner, DeclaredAssumptions::all());
    let mut k = keyer();
    let (step, _, _) = sat_step(
        &pres,
        &fx.exec_at("w0"),
        0,
        &mut k,
        SaturationBudget {
            max_hidden_steps: 3,
            max_administrative_states: 64,
            max_visible_steps: 64,
        },
    );

    match step {
        SaturatedStep::Unknown(SaturationUnknown::AdministrativeBudgetExhausted { .. }) => {}
        other => panic!("expected budget exhaustion, got {other:?}"),
    }
}

#[test]
fn the_visited_state_budget_is_its_own_distinct_unknown() {
    let fx = build_fixture(&[
        ("w0", "w1", vec![gen_tau()]),
        ("w1", "w2", vec![gen_tau()]),
        ("w2", "w3", vec![gen_tau()]),
        ("w3", "w4", vec![gen_tau()]),
    ]);
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(regimes, &profile, &fx.interner, DeclaredAssumptions::all());
    let mut k = keyer();
    let (step, _, _) = sat_step(
        &pres,
        &fx.exec_at("w0"),
        0,
        &mut k,
        SaturationBudget {
            max_hidden_steps: 64,
            max_administrative_states: 2,
            max_visible_steps: 64,
        },
    );

    match step {
        SaturatedStep::Unknown(SaturationUnknown::AdministrativeStateBudgetExhausted {
            budget,
            ..
        }) => assert_eq!(budget, 2),
        other => panic!("expected the visited-state bound to be hit, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// `Build_Plan_v3_SOC.md` Step 8's named conformance test.
// ---------------------------------------------------------------------------

/// Two configurations, one saturation operator, one profile, one budget large
/// enough for both. A terminal state and an infinitely-searching state must
/// come back as **structurally distinct, non-interconvertible** results — which
/// is exactly what the unsaturated engine could not do, and the defect
/// ADR-0014 §1 opens with.
#[test]
fn a_terminal_state_and_an_infinitely_searching_state_are_distinguished() {
    let terminal_fx = terminating_fixture();
    let looping_fx = looping_fixture();
    let profile = hiding_profile();

    let terminal_regime: &dyn SettlementRegime = &terminal_fx.regime;
    let terminal_regimes = std::slice::from_ref(&terminal_regime);
    let terminal_pres = presentation(
        terminal_regimes,
        &profile,
        &terminal_fx.interner,
        DeclaredAssumptions::all(),
    );

    let looping_regime: &dyn SettlementRegime = &looping_fx.regime;
    let looping_regimes = std::slice::from_ref(&looping_regime);
    let looping_pres = presentation(
        looping_regimes,
        &profile,
        &looping_fx.interner,
        DeclaredAssumptions::all(),
    );

    let mut k1 = keyer();
    let (terminal_step, terminal_consumed, _) = sat_step(
        &terminal_pres,
        &terminal_fx.exec_at("w0"),
        0,
        &mut k1,
        budget(),
    );
    let mut k2 = keyer();
    let (looping_step, looping_consumed, _) = sat_step(
        &looping_pres,
        &looping_fx.exec_at("w0"),
        0,
        &mut k2,
        budget(),
    );

    // 1. Different summands.
    let SaturatedStep::Quiescent(quiescence) = &terminal_step else {
        panic!("the terminating world must certify quiescence, got {terminal_step:?}");
    };
    let SaturatedStep::Divergent(divergence) = &looping_step else {
        panic!("the looping world must certify divergence, got {looping_step:?}");
    };
    assert_ne!(terminal_step, looping_step);

    // 2. Different grades, and neither is Refuted. Quiescence decides a
    //    negative; divergence explicitly does not.
    assert_eq!(quiescence.grade, Outcome::Derived);
    assert_eq!(divergence.grade, Outcome::Unknown);
    assert_ne!(quiescence.grade, Outcome::Refuted);
    assert_ne!(divergence.grade, Outcome::Refuted);

    // 3. Non-interconvertible at the byte level: neither envelope can be read
    //    as the other kind, so no downstream consumer can confuse them by
    //    parsing alone.
    let quiescence_bytes = encode_quiescence_v1(quiescence);
    let divergence_bytes = encode_divergence_v1(divergence);
    assert_eq!(
        decode_divergence_v1(&quiescence_bytes),
        Err(CertEnvelopeError::BadMarker)
    );
    assert_eq!(
        decode_quiescence_v1(&divergence_bytes),
        Err(CertEnvelopeError::BadMarker)
    );

    // 4. And each checker refuses the other's evidence.
    let terminal_config = ExecConfig::new(
        terminal_fx.worlds["w2"],
        terminal_fx.policy,
        History::empty().digest(),
    );
    assert!(check_quiescence_certificate(
        quiescence,
        &terminal_pres,
        &terminal_config,
        &terminal_consumed
    )
    .is_verified());
    assert!(
        check_divergence_certificate(divergence, &looping_pres, &looping_consumed).is_verified()
    );

    // The quiescence certificate's own steps, offered as a lasso, do not close.
    assert!(
        !check_quiescence_certificate(
            quiescence,
            &looping_pres,
            &looping_fx.exec_at("w0"),
            &looping_consumed
        )
        .is_verified(),
        "a quiescence claim must not survive being re-checked against a \
         divergent run"
    );
}

/// Two routes to the same terminal world state the **same proposition** but
/// publish **different judgements**, because their evidence differs. That is
/// `Judgement`'s documented semantics (identity includes evidence, excludes
/// search) applied to quiescence, and it is worth pinning: a reader that
/// deduplicated quiescence judgements by proposition would silently merge two
/// distinct pieces of evidence.
#[test]
fn the_same_terminal_world_gives_one_proposition_but_evidence_specific_judgements() {
    let fx = terminating_fixture();
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(regimes, &profile, &fx.interner, DeclaredAssumptions::all());
    let mut k = keyer();

    let (from_w0, _, _) = sat_step(&pres, &fx.exec_at("w0"), 0, &mut k, budget());
    let (from_w2, _, _) = sat_step(&pres, &fx.exec_at("w2"), 0, &mut k, budget());
    let (SaturatedStep::Quiescent(long), SaturatedStep::Quiescent(short)) = (from_w0, from_w2)
    else {
        panic!("both routes must certify quiescence at w2");
    };

    assert_eq!(long.terminal_world, short.terminal_world);
    assert_eq!(
        brix_semantic::Quiescent::new(
            long.terminal_world,
            long.policy,
            long.regime_set,
            long.adm_id
        )
        .proposition_id(),
        brix_semantic::Quiescent::new(
            short.terminal_world,
            short.policy,
            short.regime_set,
            short.adm_id
        )
        .proposition_id(),
        "quiescence at a world is one statement, however you got there"
    );
    assert_ne!(
        long.judgement, short.judgement,
        "…but a two-step replay and an empty one are different evidence"
    );
    assert_ne!(
        quiescence_certificate_id(&long),
        quiescence_certificate_id(&short)
    );
}

// ---------------------------------------------------------------------------
// Determinism.
// ---------------------------------------------------------------------------

#[test]
fn certificates_are_byte_identical_across_two_runs() {
    let a = encode_quiescence_v1(&mint_quiescence());
    let b = encode_quiescence_v1(&mint_quiescence());
    assert_eq!(a, b);

    let (c, _, _) = mint_divergence();
    let (d, _, _) = mint_divergence();
    assert_eq!(encode_divergence_v1(&c), encode_divergence_v1(&d));
}

// ---------------------------------------------------------------------------
// Minting helpers — each builds a fresh fixture so the certificates are
// self-contained values the tamper tests can own.
// ---------------------------------------------------------------------------

fn mint_quiescence() -> QuiescenceCertificateV1 {
    let fx = terminating_fixture();
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(regimes, &profile, &fx.interner, DeclaredAssumptions::all());
    let mut k = keyer();
    let (step, _, _) = sat_step(&pres, &fx.exec_at("w0"), 0, &mut k, budget());
    match step {
        SaturatedStep::Quiescent(cert) => *cert,
        other => panic!("expected quiescence, got {other:?}"),
    }
}

fn mint_divergence() -> (
    DivergenceCertificateV1,
    Vec<soc_core::journal::CommittedStep>,
    BTreeSet<GeneratorId>,
) {
    let fx = looping_fixture();
    let profile = hiding_profile();
    let regime: &dyn SettlementRegime = &fx.regime;
    let regimes = std::slice::from_ref(&regime);
    let pres = presentation(regimes, &profile, &fx.interner, DeclaredAssumptions::all());
    let mut k = keyer();
    let (step, consumed, _) = sat_step(&pres, &fx.exec_at("w0"), 0, &mut k, budget());
    match step {
        SaturatedStep::Divergent(cert) => (*cert, consumed, [gen_tau()].into_iter().collect()),
        other => panic!("expected divergence, got {other:?}"),
    }
}
