//! ADR-0039 identity rule: a single-commit module's preimage is unchanged by
//! the multiple-commit extension; extra pools live in a trailing tagged
//! section that only multi-pool modules write.

use brix_lower::finite_decision::{
    finite_decision_program_id, finite_decision_program_preimage, lower_finite_decision_plan,
    FINITE_DECISION_PROFILE,
};

const TAG: &[u8] = b"brix.l3.finite-decision.commits@2";

fn preimage(src: &str) -> Vec<u8> {
    let module = brix_syntax::parse(src).expect("parses");
    let plan = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).expect("lowers");
    finite_decision_program_preimage(&plan)
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

const ONE: &str = "config D = A | B\nrule x() = 1\npropose a priority 1 when x > 0 = A\npropose b priority 2 when true = B\ncommit d from (a, b)\n";
const TWO: &str = "config D = A | B\nrule x() = 1\npropose a priority 1 when x > 0 = A\npropose b priority 2 when true = B\ncommit d from (a)\ncommit e from (b)\n";

#[test]
fn single_commit_preimage_has_no_extra_pool_section() {
    assert!(!contains(&preimage(ONE), TAG));
}

#[test]
fn extra_pools_are_appended_under_their_own_tag() {
    let two = preimage(TWO);
    assert!(contains(&two, TAG));
    let one_id = finite_decision_program_id(
        &lower_finite_decision_plan(&brix_syntax::parse(ONE).unwrap(), FINITE_DECISION_PROFILE)
            .unwrap(),
    );
    let two_id = finite_decision_program_id(
        &lower_finite_decision_plan(&brix_syntax::parse(TWO).unwrap(), FINITE_DECISION_PROFILE)
            .unwrap(),
    );
    assert_ne!(one_id, two_id);
}

#[test]
fn shipping_example_keeps_its_published_pin() {
    let src = include_str!("../../../examples/shipping.brix");
    let mut module = brix_syntax::parse(src).unwrap();
    module
        .items
        .retain(|i| !matches!(i, brix_syntax::ast::Item::Show(_)));
    let plan = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap();
    assert_eq!(
        finite_decision_program_id(&plan).0.to_hex(),
        "3a815590c807a8af7e7756d8f0edef99a4938e15830de282b24949fe88ba0d5e"
    );
}
