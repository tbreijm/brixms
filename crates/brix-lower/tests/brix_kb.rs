//! Deliberately names this external test crate `brix_kb`: type-name strings are
//! not unique Rust type identities or a substitute for a private constructor.

mod world {
    pub mod decision_evidence {
        pub struct WorldVerifierWitness;
    }
}

#[test]
fn external_same_named_type_cannot_impersonate_the_world_verifier() {
    use brix_canon::{Digest, Domain};
    use brix_lower::evidence::{
        Authority, Receipt, VerifiedDecisionClaim, VerifiedDecisionEvidence,
    };
    use brix_lower::l3_v2::DocumentaryRegistry;
    use world::decision_evidence::WorldVerifierWitness;

    assert_eq!(
        std::any::type_name::<WorldVerifierWitness>(),
        "brix_kb::world::decision_evidence::WorldVerifierWitness"
    );
    let digest = Digest::of(Domain::Value, b"invented context with no replay");
    let claim = VerifiedDecisionClaim {
        world: digest,
        program: digest,
        revision: 1,
        revision_digest: digest,
        decide: "attacker::plan".into(),
        entity: "q1".into(),
        candidate: "approve".into(),
        value_digest: digest,
        caller: Some("attacker".into()),
        grants: vec!["admin:all".into()],
        release: "invented".into(),
    };
    let receipt = Receipt::issue(
        "invented receipt",
        "q1",
        digest.to_hex(),
        Authority::RuntimeCheck,
        claim.grants.clone(),
        None,
    );
    let result = VerifiedDecisionEvidence::__issue_from_world_verifier_private(
        claim,
        receipt,
        &WorldVerifierWitness,
    );
    if let Ok(handle) = result {
        assert!(
            DocumentaryRegistry::new()
                .admit_runtime_evidence(handle)
                .is_err(),
            "a same-named external type must not mint verifier authority without replay"
        );
    }
}
