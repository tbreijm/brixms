//! Regression tests for evidence soundness and profile compatibility (ADR-0048).
//! These assert the required behavior; unresolved review findings fail normally.

use brix_kernel::Budget;
use brix_lower::evidence::{
    self, Authority, CertificateArtifact, Disposition, ExpectedTarget, Reason, PROFILE,
};
use brix_lower::finite_decision::{finite_decision_program_id, lower_finite_decision_plan};
use brix_syntax::parse;

#[test]
fn symbolic_inequality_cannot_be_proved_for_arbitrary_inputs() {
    let module = brix_syntax::parse("fn bad(x: Int, y: Int) -> Int ensures x != y = 0\n").unwrap();
    let report = evidence::check_module(&module);
    assert!(!report.ok, "accepted false universal contract: {report:?}");
}

#[test]
fn refined_return_must_hold_on_both_match_arms() {
    let module = brix_syntax::parse("type Positive = { x: Int | x > 0 }\nfn bad(flag: Bool) -> Positive = match flag { true => 1 false => 0 }\n").unwrap();
    let report = evidence::check_module(&module);
    assert!(!report.ok, "accepted invalid false branch: {report:?}");
}

#[test]
fn certificate_must_not_substitute_another_subject() {
    let original = evidence::integer_predicate_certificate(
        "x",
        "x > 0",
        2,
        PROFILE,
        vec![],
        Authority::RuntimeCheck,
    )
    .unwrap();
    let mut changed = original.clone();
    changed.subject = "3".into();
    if let CertificateArtifact::IntegerPredicate { value, .. } = &mut changed.artifact {
        *value = 3;
    }
    let result = evidence::verify_certificate(
        &changed,
        ExpectedTarget::Predicate {
            proposition: &original.proposition,
            context: PROFILE,
            binder: "x",
            subject: "2",
            assumptions: &[],
        },
        Budget::new(100, 100),
    );
    assert!(
        !result.authorizes(),
        "consumer expecting original claim accepted different subject: {result:?}"
    );
}

#[test]
fn certificate_cannot_choose_its_own_assumptions() {
    let certificate = evidence::integer_predicate_certificate(
        "x",
        "trusted",
        0,
        PROFILE,
        vec!["trusted".into()],
        Authority::RuntimeCheck,
    )
    .unwrap();
    let result = evidence::verify_certificate(
        &certificate,
        ExpectedTarget::Predicate {
            proposition: "trusted",
            context: PROFILE,
            binder: "x",
            subject: "0",
            assumptions: &[],
        },
        Budget::new(100, 100),
    );
    assert!(
        !result.authorizes(),
        "unconditional consumer accepted self-declared assumption: {result:?}"
    );
}

#[test]
fn assuming_a_cannot_establish_not_a() {
    let module = brix_syntax::parse(
        "assume a\ntype Negative = { x: Int | !a }\nlet bad: Negative given a = 1\n",
    )
    .unwrap();
    let report = evidence::check_module(&module);
    assert!(
        !report.ok,
        "accepted negation under positive assumption: {report:?}"
    );
}

#[test]
fn empty_record_type_remains_valid() {
    assert!(brix_syntax::parse("config Empty = {}\n").is_ok());
}

#[test]
fn validation_outcome_cannot_escape_as_a_satisfied_refined_value() {
    let module =
        parse("type Positive = { x: Int | x > 0 }\nlet bad = validate<Positive>(0)\n").unwrap();
    let report = evidence::check_module(&module);
    assert!(
        !report.bindings.iter().any(|binding| {
            binding.path == "bad" && binding.ty == "Positive" && binding.disposition.authorizes()
        }),
        "an unchecked validation outcome became a satisfied Positive: {report:?}"
    );
}

#[test]
fn identity_conversion_cannot_erase_required_assumptions() {
    let module = parse(
        "assume a\nnewtype Ticket = Int\nfn produce() -> Int ensures a = 1\nlet bad: Ticket = convert(identity, produce())\n",
    )
    .unwrap();
    let report = evidence::check_module(&module);
    assert!(
        !report.ok,
        "identity conversion made a conditional value unconditional: {report:?}"
    );
}

#[test]
fn finite_decision_rejects_unsupported_function_contracts() {
    let module = brix_syntax::parse(
        r#"
        config Decision = Ship
        fn positive(x: Int): Int requires x > 0 ensures result > 0 = x
        let value = positive(0)
        propose p() priority 10 when value == 0 = Ship
        commit c from (p)
        "#,
    )
    .unwrap();
    let result =
        brix_lower::lower_finite_decision_plan(&module, brix_lower::FINITE_DECISION_PROFILE);
    assert!(
        result.is_err(),
        "finite-decision lowering silently accepted unsupported contracts"
    );
}

#[test]
fn finite_decision_rejects_each_unsupported_clause() {
    for (clause, item) in [
        (
            "requires",
            "fn positive(x: Int): Int\n  requires x > 0\n  = x",
        ),
        (
            "ensures",
            "fn positive(x: Int): Int\n  ensures result > 0\n  = x",
        ),
        (
            "converts",
            "fn positive(x: Int): Int\n  converts identity\n  = x",
        ),
        ("type parameters", "fn positive<T>(x: Int): Int = x"),
        (
            "requires",
            "rule positive(x: Int)\n  requires x > 0\n  = x > 0",
        ),
        (
            "ensures",
            "rule positive(x: Int)\n  ensures x > 0\n  = x > 0",
        ),
        (
            "converts",
            "rule positive(x: Int)\n  converts identity\n  = x > 0",
        ),
    ] {
        let src = format!("config Decision = Ship\n{item}\n");
        let error =
            lower_finite_decision_plan(&parse(&src).expect(clause), PROFILE_FD).expect_err(clause);
        let text = error.to_string();
        assert!(
            text.contains(clause),
            "expected {clause} in {text} for {item}"
        );
    }
}

const PROFILE_FD: &str = brix_lower::FINITE_DECISION_PROFILE;

#[test]
fn ordinary_finite_decision_program_keeps_its_identity() {
    let src = r#"
        config Decision = Ship
        fn id(x: Int): Int = x
        let value = id(0)
        propose p() priority 10 when value == 0 = Ship
        commit c from (p)
    "#;
    let module = parse(src).unwrap();
    let first = lower_finite_decision_plan(&module, PROFILE_FD).expect("ordinary program");
    let second = lower_finite_decision_plan(&module, PROFILE_FD).expect("ordinary program");
    assert_eq!(
        finite_decision_program_id(&first),
        finite_decision_program_id(&second)
    );
}

#[test]
fn reflexive_and_bounded_comparisons_stay_sound() {
    let same =
        evidence::check_module(&parse("fn same(x: Int) -> Int ensures result == x = x\n").unwrap());
    assert!(same.ok, "x == x should hold: {same:?}");
    assert_eq!(
        same.bindings[0].disposition.authority(),
        Some(Authority::BoundedFragment)
    );

    let proved = evidence::check_module(&parse("fn apart() -> Int ensures 1 != 0 = 0\n").unwrap());
    assert!(proved.ok, "{proved:?}");

    let bounded = evidence::check_module(
        &parse("fn tight(x: Int) -> Int requires x > 2 ensures x != 0 = 0\n").unwrap(),
    );
    assert!(bounded.ok, "bounds should prove x != 0: {bounded:?}");

    let open =
        evidence::check_module(&parse("fn maybe(x: Int) -> Int ensures x == 0 = 0\n").unwrap());
    assert!(!open.ok, "unbounded x == 0 must not be proved");
    assert!(
        open.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic.disposition,
            Disposition::Unknown {
                reason: Reason::MissingEvidence
            }
        )),
        "uncertain equality became a refutation: {open:?}"
    );
}

#[test]
fn refined_return_accepts_uniform_arms_and_ignores_unreachable_zeros() {
    let same = evidence::check_module(
        &parse(
            "type Positive = { x: Int | x > 0 }\nfn ok(flag: Bool) -> Positive = match flag { true => 1 false => 1 }\n",
        )
        .unwrap(),
    );
    assert!(same.ok, "{same:?}");

    let reversed = evidence::check_module(
        &parse(
            "type Positive = { x: Int | x > 0 }\nfn bad(flag: Bool) -> Positive = match flag { false => 0 true => 1 }\n",
        )
        .unwrap(),
    );
    assert!(!reversed.ok, "reversed arm order still returns 0");

    let nested = evidence::check_module(
        &parse(
            "type Positive = { x: Int | x > 0 }\nfn bad(flag: Bool, inner: Bool) -> Positive = match flag { true => match inner { true => 1 false => 0 } false => 1 }\n",
        )
        .unwrap(),
    );
    assert!(!nested.ok, "nested match hid a zero");

    let unreachable = evidence::check_module(
        &parse(
            "type Positive = { x: Int | x > 0 }\nfn ok() -> Positive = match true { true => 1 false => 0 }\n",
        )
        .unwrap(),
    );
    assert!(
        unreachable.ok,
        "unreachable zero must not reject the function: {unreachable:?}"
    );
}

#[test]
fn negation_keeps_its_premise_through_connectives() {
    let double = evidence::check_module(
        &parse("assume a\ntype Still = { x: Int | !!a }\nlet ok: Still given a = 1\n").unwrap(),
    );
    assert!(double.ok, "{double:?}");
    let Disposition::Satisfied(receipt) = &double.bindings[0].disposition else {
        panic!("{:?}", double.bindings[0]);
    };
    assert_eq!(receipt.assumptions, vec!["a".to_string()]);

    let erased = evidence::check_module(
        &parse("assume a\ntype Still = { x: Int | !!a }\nlet bad: Still = 1\n").unwrap(),
    );
    assert!(!erased.ok, "double negation erased the dependency on a");

    let contradiction = evidence::check_module(
        &parse("assume a\ntype Empty = { x: Int | a && !a }\nlet bad: Empty given a = 1\n")
            .unwrap(),
    );
    assert!(contradiction
        .diagnostics
        .iter()
        .any(|diagnostic| { matches!(diagnostic.disposition, Disposition::Violated { .. }) }));

    let excluded = evidence::check_module(
        &parse("assume a\ntype Covered = { x: Int | a || !a }\nlet ok: Covered given a = 1\n")
            .unwrap(),
    );
    assert!(excluded.ok, "{excluded:?}");
    let Disposition::Satisfied(receipt) = &excluded.bindings[0].disposition else {
        panic!("{:?}", excluded.bindings[0]);
    };
    assert_eq!(receipt.assumptions, vec!["a".to_string()]);

    let concrete = evidence::check_module(
        &parse("type No = { x: Int | !true }\ntype Yes = { x: Int | !false }\nlet bad: No = 1\nlet ok: Yes = 1\n")
            .unwrap(),
    );
    assert!(concrete.diagnostics.iter().any(|diagnostic| {
        diagnostic.item == "bad" && matches!(diagnostic.disposition, Disposition::Violated { .. })
    }));
    let ok = concrete
        .bindings
        .iter()
        .find(|binding| binding.path == "ok")
        .expect("concrete negation");
    let Disposition::Satisfied(receipt) = &ok.disposition else {
        panic!("{ok:?}");
    };
    assert!(receipt.assumptions.is_empty());
    assert_eq!(receipt.authority, Authority::RuntimeCheck);
}

#[test]
fn conditional_checks_do_not_issue_unconditional_receipts() {
    let report = evidence::check_module(
        &parse(
            "assume a\nfn produce() -> Int ensures a = 1\nlet bare = produce()\nlet covered: Int given a = produce()\n",
        )
        .unwrap(),
    );
    let function = report
        .bindings
        .iter()
        .find(|binding| binding.path == "produce")
        .expect("function binding");
    let Disposition::Satisfied(receipt) = &function.disposition else {
        panic!("{function:?}");
    };
    assert_eq!(receipt.assumptions, vec!["a".to_string()]);
    assert!(report.diagnostics.iter().any(|diagnostic| {
        diagnostic.item == "bare"
            && matches!(
                diagnostic.disposition,
                Disposition::Unknown {
                    reason: Reason::AssumptionRequired { .. }
                }
            )
    }));
    let covered = report
        .bindings
        .iter()
        .find(|binding| binding.path == "covered")
        .expect("covered let");
    let Disposition::Satisfied(receipt) = &covered.disposition else {
        panic!("{covered:?}");
    };
    assert_eq!(receipt.assumptions, vec!["a".to_string()]);
}

#[test]
fn certificate_matches_the_expected_subject_after_a_round_trip() {
    let certificate = evidence::integer_predicate_certificate(
        "x",
        "x > 0",
        2,
        PROFILE,
        Vec::new(),
        Authority::RuntimeCheck,
    )
    .unwrap();
    let encoded = evidence::encode_integer_certificate(&certificate).unwrap();
    let decoded = evidence::decode_integer_certificate(&encoded).unwrap();
    let verified = evidence::verify_certificate(
        &decoded,
        ExpectedTarget::Predicate {
            proposition: &certificate.proposition,
            context: PROFILE,
            binder: "x",
            subject: "2",
            assumptions: &[],
        },
        Budget::new(10, 10),
    );
    assert!(verified.authorizes(), "{verified:?}");
}

#[test]
fn authorized_certificate_assumptions_stay_conditional() {
    let certificate = evidence::integer_predicate_certificate(
        "x",
        "trusted",
        0,
        PROFILE,
        vec!["trusted".into()],
        Authority::RuntimeCheck,
    )
    .unwrap();
    let authorized = evidence::verify_certificate(
        &certificate,
        ExpectedTarget::Predicate {
            proposition: "trusted",
            context: PROFILE,
            binder: "x",
            subject: "0",
            assumptions: &["trusted".to_string()],
        },
        Budget::new(100, 100),
    );
    let Disposition::Satisfied(receipt) = authorized else {
        panic!("authorized premise should verify: {authorized:?}");
    };
    assert_eq!(receipt.authority, Authority::RuntimeCheck);
    assert_eq!(receipt.assumptions, vec!["trusted".to_string()]);
    assert!(receipt.kernel_certificate.is_none());

    let spare = evidence::verify_certificate(
        &certificate,
        ExpectedTarget::Predicate {
            proposition: "trusted",
            context: PROFILE,
            binder: "x",
            subject: "0",
            assumptions: &["trusted".to_string(), "spare".to_string()],
        },
        Budget::new(100, 100),
    );
    let Disposition::Satisfied(receipt) = spare else {
        panic!("{spare:?}");
    };
    assert_eq!(receipt.assumptions, vec!["trusted".to_string()]);
}
