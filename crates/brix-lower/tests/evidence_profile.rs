//! ADR-0048 evidence profile. These tests are the acceptance checks for the
//! bounded fragment: nominal types, refinements, authority, and revision
//! applicability. A passing predicate is not a kernel proof.

use brix_kernel::{acceptance, Budget, ExplicitTerm, Prop, TermKind, Var, Verdict};
use brix_lower::evidence::{
    admit_candidate, check_module, check_module_with_budget, decode_integer_certificate,
    encode_integer_certificate, eval_predicate, integer_predicate_certificate, kernel_certificate,
    module_opts_in, verify_certificate, verify_certificate_with_steps, Authority, Candidate,
    Disposition, ExpectedTarget, History, Reason, Snapshot, CERTIFICATE_PROFILE, PROFILE,
};
use brix_lower::finite_decision::{
    lower_finite_decision_plan, FiniteDecisionLowerError, FINITE_DECISION_PROFILE,
};
use brix_semantic::{ContextId, PropositionId};
use brix_syntax::ast::Expr;
use brix_syntax::parse;

fn predicate_target<'a>(
    proposition: &'a str,
    binder: &'a str,
    subject: &'a str,
    assumptions: &'a [String],
) -> ExpectedTarget<'a> {
    ExpectedTarget::Predicate {
        proposition,
        context: PROFILE,
        binder,
        subject,
        assumptions,
    }
}

fn check(src: &str) -> brix_lower::evidence::EvidenceReport {
    let module = parse(src).unwrap_or_else(|err| panic!("parse: {err}"));
    check_module(&module)
}

fn satisfied<'a>(report: &'a brix_lower::evidence::EvidenceReport, path: &str) -> &'a Disposition {
    report
        .bindings
        .iter()
        .find(|binding| binding.path == path)
        .unwrap_or_else(|| panic!("missing binding {path}: {report:?}"))
        .disposition
        .as_ref_disposition()
}

trait DispExt {
    fn as_ref_disposition(&self) -> &Disposition;
}

impl DispExt for Disposition {
    fn as_ref_disposition(&self) -> &Disposition {
        self
    }
}

fn authority(disposition: &Disposition) -> Authority {
    disposition.authority().expect("satisfied")
}

#[test]
fn nominal_types_are_not_interchangeable() {
    let report = check(
        r#"
        newtype CustomerId = Int
        newtype OrderId = Int
        let customer: CustomerId = convert(identity, 1)
        let order: OrderId = customer
        let converted: OrderId = convert(identity, customer)
        "#,
    );
    assert!(report.opted_in);
    assert!(!report.ok);
    assert!(matches!(
        authority(satisfied(&report, "customer")),
        Authority::RuntimeCheck
    ));
    assert!(satisfied(&report, "customer").authority().is_some());
    match &satisfied(&report, "customer") {
        Disposition::Satisfied(receipt) => {
            assert!(receipt.authentic());
            assert!(receipt.kernel_certificate.is_none());
        }
        other => panic!("{other:?}"),
    }
    assert!(report.diagnostics.iter().any(|d| {
        d.item == "order"
            && matches!(
                d.disposition,
                Disposition::Unknown {
                    reason: Reason::NominalMismatch { .. }
                }
            )
    }));
    match satisfied(&report, "converted") {
        Disposition::Satisfied(receipt) => {
            assert!(receipt.proposition.contains("convert(identity)"));
            assert!(receipt.proposition.contains("OrderId"));
            assert_eq!(receipt.authority, Authority::RuntimeCheck);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn refinement_admits_valid_constants_and_reports_the_failed_predicate() {
    let good = check(
        r#"
        type PositiveInt = { x: Int | x > 0 }
        let ok: PositiveInt = 2
        "#,
    );
    assert!(good.ok, "{good:?}");
    match satisfied(&good, "ok") {
        Disposition::Satisfied(receipt) => {
            assert_eq!(receipt.authority, Authority::RuntimeCheck);
            assert!(receipt.kernel_certificate.is_none());
            assert!(receipt.authentic());
        }
        other => panic!("{other:?}"),
    }

    let bad = check(
        r#"
        type PositiveInt = { x: Int | x > 0 }
        let bad: PositiveInt = 0
        "#,
    );
    assert!(!bad.ok);
    assert!(matches!(
        &bad.diagnostics[0].disposition,
        Disposition::Violated { .. }
    ));
    assert!(!bad.diagnostics[0].disposition.is_logical_refutation());
}

#[test]
fn validate_gives_the_refined_value_only_to_the_satisfied_arm() {
    let report = check(
        r#"
        type PositiveInt = { x: Int | x > 0 }
        fn take(raw: Int) -> Int
          = match validate<PositiveInt>(raw) {
              Satisfied(value, receipt) => value
              Violated(_) => 0
              Unknown(_) => 0
            }
        "#,
    );
    assert!(report.ok, "{report:?}");
    let arm = report
        .arms
        .iter()
        .find(|arm| arm.path.ends_with("Satisfied/value"))
        .expect("satisfied binder");
    assert_eq!(arm.ty, "PositiveInt");
    assert!(report
        .arms
        .iter()
        .all(|arm| !arm.path.contains("Violated/")));
    assert!(satisfied(&report, "take")
        .authority()
        .is_some_and(|a| a != Authority::ProofKernel));
}

#[test]
fn overflow_is_definedness_not_a_false_predicate() {
    let report = check(
        r#"
        type Any = { x: Int | x == x }
        let wrapped: Any = 9223372036854775807 + 1
        "#,
    );
    assert!(!report.ok);
    assert!(matches!(
        &report.diagnostics[0].disposition,
        Disposition::Unknown {
            reason: Reason::Definedness { .. }
        }
    ));
    assert!(!report.diagnostics[0].disposition.is_logical_refutation());
}

#[test]
fn postcondition_failure_on_one_return_path_is_rejected() {
    let report = check(
        r#"
        fn bad(flag: Bool) -> Int
          ensures result > 0
          = match flag {
              true => 1
              false => 0
            }
        "#,
    );
    assert!(!report.ok, "{report:?}");
    assert!(matches!(
        &report.diagnostics[0].disposition,
        Disposition::Violated { .. }
    ));
}

#[test]
fn defined_increment_meets_its_postcondition_without_a_kernel_proof() {
    let report = check(
        r#"
        fn increment(x: Int) -> Int
          requires x < 9223372036854775807
          ensures result == x + 1
          = x + 1
        "#,
    );
    assert!(report.ok, "{report:?}");
    match satisfied(&report, "increment") {
        Disposition::Satisfied(receipt) => {
            assert_eq!(receipt.authority, Authority::BoundedFragment);
            assert!(receipt.kernel_certificate.is_none());
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn generic_instantiation_reports_a_missing_index_obligation() {
    let report = check(
        r#"
        newtype Quantity<Unit> = Int
        fn same<U>(a: Quantity<U>, b: Quantity<U>) -> Quantity<U>
          = a
        let meters: Quantity<Meter> = convert(identity, 1)
        let seconds: Quantity<Second> = convert(identity, 1)
        let mixed = same(meters, seconds)
        "#,
    );
    assert!(!report.ok, "{report:?}");
    assert!(report.diagnostics.iter().any(|d| {
        d.item == "mixed"
            && matches!(
                d.disposition,
                Disposition::Unknown {
                    reason: Reason::MissingObligation { .. }
                }
            )
    }));
}

#[test]
fn branch_join_keeps_a_refinement_only_when_every_reaching_arm_has_it() {
    let report = check(
        r#"
        type PositiveInt = { x: Int | x > 0 }
        fn pick(raw: Int) -> PositiveInt
          = match validate<PositiveInt>(raw) {
              Satisfied(value, receipt) => value
              Violated(_) => 0
              Unknown(_) => 0
            }
        "#,
    );
    assert!(!report.ok, "{report:?}");
    assert!(report.diagnostics.iter().any(|d| d.item == "pick"));
}

#[test]
fn proven_annotation_does_not_mint_a_kernel_handle() {
    let report = check(
        r#"
        type PositiveInt = { x: Int | x > 0 }
        let claimed: PositiveInt @ Proven = 2
        "#,
    );
    assert!(!report.ok);
    assert!(matches!(
        &report.diagnostics[0].disposition,
        Disposition::Unknown {
            reason: Reason::InsufficientAuthority { .. }
        }
    ));
    assert!(report.bindings.iter().all(|b| b.path != "claimed"));
}

#[test]
fn assumption_is_visible_and_does_not_satisfy_an_unconditional_consumer() {
    let report = check(
        r#"
        assume external_stock
        type Enough = { x: Int | x > 0 && external_stock }
        let conditional: Enough given external_stock = 1
        let unconditional: Enough = conditional
        "#,
    );
    assert!(!report.ok, "{report:?}");
    match satisfied(&report, "conditional") {
        Disposition::Satisfied(receipt) => {
            assert_eq!(receipt.assumptions, vec!["external_stock".to_string()]);
            assert!(receipt.kernel_certificate.is_none());
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        &report
            .diagnostics
            .iter()
            .find(|d| d.item == "unconditional")
            .unwrap()
            .disposition,
        Disposition::Unknown {
            reason: Reason::AssumptionRequired { .. }
        }
    ));
}

#[test]
fn budget_exhaustion_is_not_a_violation() {
    let module = parse(
        r#"
        type PositiveInt = { x: Int | x > 0 }
        let ok: PositiveInt = 2
        "#,
    )
    .unwrap();
    let report = check_module_with_budget(&module, 1);
    assert!(!report.ok);
    assert!(matches!(
        &report.diagnostics[0].disposition,
        Disposition::Unknown {
            reason: Reason::BudgetExhausted
        }
    ));
    assert!(!report.diagnostics[0].disposition.is_logical_refutation());
    assert!(!report.diagnostics[0].disposition.authorizes());
}

#[test]
fn predicate_evaluation_agrees_with_refinement_checking() {
    for value in [1, 2, 0, -3] {
        let direct = eval_predicate(
            &Expr::Bin {
                op: brix_syntax::ast::BinOp::Gt,
                lhs: Box::new(Expr::Var("x".into())),
                rhs: Box::new(Expr::Num("0".into())),
            },
            "x",
            value,
        );
        let via_type = check(&format!(
            "type PositiveInt = {{ x: Int | x > 0 }}\nlet v: PositiveInt = {value}\n"
        ));
        let via = if via_type.ok {
            satisfied(&via_type, "v").clone()
        } else {
            via_type.diagnostics[0].disposition.clone()
        };
        assert_eq!(
            std::mem::discriminant(&direct),
            std::mem::discriminant(&via),
            "value {value}: direct {direct:?} via {via:?}"
        );
        assert!(!direct.is_logical_refutation());
    }
}

#[test]
fn forged_label_and_mismatched_certificate_are_refused() {
    let forged = admit_candidate(Candidate::ForgedLabel("Proven"), Budget::new(10, 10));
    assert!(matches!(
        forged,
        Disposition::Unknown {
            reason: Reason::InsufficientAuthority { .. }
        }
    ));
    assert!(!forged.authorizes());
    assert!(!forged.is_logical_refutation());

    let ctx = ContextId::from_canon(b"evidence-ctx");
    let other = ContextId::from_canon(b"other-ctx");
    let prop = Prop::Atom(PropositionId::from_canon(b"P"));
    let goal = Prop::Impl(Box::new(prop.clone()), Box::new(prop.clone()));
    let term = ExplicitTerm::new(
        ctx,
        TermKind::Lam {
            var_name: Some("h".into()),
            body: Box::new(TermKind::Hyp(Var::Index(0))),
        },
    );
    let accepted = admit_candidate(
        Candidate::Term {
            term: &term,
            proposition: &goal,
            expected_proposition: &goal,
            expected_context: &ctx,
        },
        Budget::new(100, 100),
    );
    match accepted {
        Disposition::Satisfied(receipt) => {
            assert_eq!(receipt.authority, Authority::ProofKernel);
            assert!(receipt.kernel_certificate.is_some());
            assert!(receipt.authentic());
        }
        other => panic!("{other:?}"),
    }

    let mismatched = admit_candidate(
        Candidate::Term {
            term: &term,
            proposition: &goal,
            expected_proposition: &prop,
            expected_context: &ctx,
        },
        Budget::new(100, 100),
    );
    assert!(matches!(
        mismatched,
        Disposition::Unknown {
            reason: Reason::ClaimMismatch { .. }
        }
    ));
    assert!(!mismatched.is_logical_refutation());

    let wrong_ctx = admit_candidate(
        Candidate::Term {
            term: &term,
            proposition: &goal,
            expected_proposition: &goal,
            expected_context: &other,
        },
        Budget::new(100, 100),
    );
    assert!(matches!(
        wrong_ctx,
        Disposition::Unknown {
            reason: Reason::ContextMismatch { .. }
        }
    ));

    let rejected = admit_candidate(
        Candidate::Term {
            term: &ExplicitTerm::new(ctx, TermKind::Hyp(Var::Index(0))),
            proposition: &goal,
            expected_proposition: &goal,
            expected_context: &ctx,
        },
        Budget::new(100, 100),
    );
    assert!(matches!(
        rejected,
        Disposition::Unknown {
            reason: Reason::CandidateRejected
        }
    ));
    assert!(!rejected.authorizes());

    let exhausted = admit_candidate(
        Candidate::Term {
            term: &term,
            proposition: &goal,
            expected_proposition: &goal,
            expected_context: &ctx,
        },
        Budget::new(0, 0),
    );
    assert!(matches!(
        exhausted,
        Disposition::Unknown {
            reason: Reason::BudgetExhausted
        }
    ));
    assert!(!exhausted.is_logical_refutation());
}

#[test]
fn stale_dispatch_evidence_stays_historical() {
    let mut history = History::default();
    history
        .record_snapshot(Snapshot {
            revision: 42,
            world: "rev-42".into(),
            quantities: [("o1".into(), 10), ("o2".into(), 3)].into(),
        })
        .expect("snapshot");
    let established = history.establish_can_dispatch(42, "o1");
    let Disposition::Satisfied(receipt) = established.clone() else {
        panic!("{established:?}");
    };
    assert_eq!(receipt.authority, Authority::RuntimeCheck);
    assert!(receipt.kernel_certificate.is_none());

    let current = Snapshot {
        revision: 43,
        world: "rev-43".into(),
        quantities: [("o1".into(), 10), ("o2".into(), 9)].into(),
    };
    let reused = history.dispatch(&current, "o1", &receipt);
    assert!(reused.authorizes(), "{reused:?}");

    history
        .record_snapshot(Snapshot {
            revision: 43,
            world: "rev-43".into(),
            quantities: [("o1".into(), 0), ("o2".into(), 9)].into(),
        })
        .expect("snapshot");
    let stale = history.dispatch(
        history
            .claims()
            .first()
            .map(|_| Snapshot {
                revision: 43,
                world: "rev-43".into(),
                quantities: [("o1".into(), 0), ("o2".into(), 9)].into(),
            })
            .as_ref()
            .unwrap(),
        "o1",
        &receipt,
    );
    assert!(matches!(
        stale,
        Disposition::Unknown {
            reason: Reason::ContextMismatch { .. }
        }
    ));
    assert_eq!(history.claims().len(), 1);
    let historical = history.recheck_historical(&receipt);
    assert!(historical.authorizes(), "{historical:?}");
    assert!(!stale.is_logical_refutation());
}

#[test]
fn composition_names_the_incompatible_obligation() {
    let report = check(
        r#"
        fn produce(x: Int) -> Int
          requires x > 0
          ensures result > 0
          = x
        fn needs(y: Int) -> Int
          requires y > 10
          ensures result > 10
          = y
        fn pipe(x: Int) -> Int
          requires x > 0
          = needs(produce(x))
        "#,
    );
    assert!(!report.ok, "{report:?}");
    assert!(report.diagnostics.iter().any(|d| {
        d.item == "pipe"
            && matches!(
                &d.disposition,
                Disposition::Unknown {
                    reason: Reason::IncompatibleComposition { obligation }
                } if obligation.contains('>')
            )
    }));
}

#[test]
fn unannotated_program_does_not_enter_the_profile() {
    let module = parse("let x = 1\n").unwrap();
    assert!(!module_opts_in(&module));
    let report = check_module(&module);
    assert!(!report.opted_in);
    assert!(report.ok);
    assert_eq!(PROFILE, "brix.evidence@1");
}

#[test]
fn contextual_words_remain_names() {
    let module = parse("let newtype = 1\nlet type = 2\n").unwrap();
    assert!(!module_opts_in(&module));
}

#[test]
fn finite_decision_profile_rejects_evidence_items() {
    let module = parse("newtype CustomerId = Int\n").unwrap();
    let err = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
    assert!(matches!(err, FiniteDecisionLowerError::ItemNotAllowed(_)));
}

#[test]
fn record_types_still_parse() {
    let module = parse("config Order = { id: Int, units: Int }\n").unwrap();
    assert!(!module_opts_in(&module));
}

#[test]
fn portable_predicate_certificate_is_rechecked_from_its_source() {
    let certificate = integer_predicate_certificate(
        "x",
        "x > 0",
        2,
        PROFILE,
        Vec::new(),
        Authority::RuntimeCheck,
    )
    .unwrap();
    assert_eq!(certificate.profile, CERTIFICATE_PROFILE);
    let encoded = encode_integer_certificate(&certificate).unwrap();
    let decoded = decode_integer_certificate(&encoded).unwrap();
    let verified = verify_certificate(
        &decoded,
        predicate_target(&certificate.proposition, "x", "2", &[]),
        Budget::new(10, 10),
    );
    match verified {
        Disposition::Satisfied(receipt) => {
            assert_eq!(receipt.authority, Authority::RuntimeCheck);
            assert!(receipt.kernel_certificate.is_none());
            assert!(receipt.authentic());
        }
        other => panic!("{other:?}"),
    }

    let mut forged = decoded.clone();
    forged.claimed_authority = Authority::ProofKernel;
    let forged_text = encode_integer_certificate(&forged).unwrap();
    let forged = decode_integer_certificate(&forged_text).unwrap();
    let refused = verify_certificate(
        &forged,
        predicate_target(&certificate.proposition, "x", "2", &[]),
        Budget::new(10, 10),
    );
    assert!(matches!(
        refused,
        Disposition::Unknown {
            reason: Reason::InsufficientAuthority { .. }
        }
    ));
    assert!(!refused.authorizes());

    let mut tampered = decode_integer_certificate(&encoded).unwrap();
    tampered.artifact = brix_lower::evidence::CertificateArtifact::IntegerPredicate {
        binder: "x".into(),
        source: "x > 1".into(),
        value: 2,
    };
    let mismatched = verify_certificate(
        &tampered,
        predicate_target(&certificate.proposition, "x", "2", &[]),
        Budget::new(10, 10),
    );
    assert!(matches!(
        mismatched,
        Disposition::Unknown {
            reason: Reason::ClaimMismatch { .. }
        }
    ));

    let mut invalid = decode_integer_certificate(&encoded).unwrap();
    invalid.subject = "0".into();
    invalid.artifact = brix_lower::evidence::CertificateArtifact::IntegerPredicate {
        binder: "x".into(),
        source: "x > 0".into(),
        value: 0,
    };
    let violated = verify_certificate(
        &invalid,
        predicate_target(&certificate.proposition, "x", "0", &[]),
        Budget::new(10, 10),
    );
    assert!(matches!(violated, Disposition::Violated { .. }));
    assert!(!violated.is_logical_refutation());

    let exhausted = verify_certificate_with_steps(
        &decoded,
        predicate_target(&certificate.proposition, "x", "2", &[]),
        Budget::new(10, 10),
        0,
    );
    assert!(matches!(
        exhausted,
        Disposition::Unknown {
            reason: Reason::BudgetExhausted
        }
    ));
    assert!(!exhausted.authorizes());
}

#[test]
fn portable_kernel_certificate_matches_direct_acceptance() {
    let ctx = ContextId::from_canon(b"evidence-ctx");
    let prop = Prop::Atom(PropositionId::from_canon(b"P"));
    let goal = Prop::Impl(Box::new(prop.clone()), Box::new(prop));
    let term = ExplicitTerm::new(
        ctx,
        TermKind::Lam {
            var_name: Some("h".into()),
            body: Box::new(TermKind::Hyp(Var::Index(0))),
        },
    );
    let Verdict::Accepted(direct) = acceptance(&ctx, &goal, &term, Budget::new(100, 100)) else {
        panic!("kernel rejected a valid term");
    };
    let certificate = kernel_certificate(term, goal.clone(), Authority::ProofKernel);
    let verified = verify_certificate(
        &certificate,
        ExpectedTarget::Kernel {
            proposition: &goal,
            context: &ctx,
        },
        Budget::new(100, 100),
    );
    match verified {
        Disposition::Satisfied(receipt) => {
            assert_eq!(receipt.authority, Authority::ProofKernel);
            assert_eq!(
                receipt.kernel_certificate.as_deref(),
                Some(direct.certificate_id.to_hex().as_str())
            );
        }
        other => panic!("{other:?}"),
    }

    let other = ContextId::from_canon(b"other-ctx");
    let wrong_ctx = verify_certificate(
        &certificate,
        ExpectedTarget::Kernel {
            proposition: &goal,
            context: &other,
        },
        Budget::new(100, 100),
    );
    assert!(matches!(
        wrong_ctx,
        Disposition::Unknown {
            reason: Reason::ContextMismatch { .. }
        }
    ));
    assert!(!wrong_ctx.authorizes());
}

#[test]
fn exported_dispatch_claim_is_rechecked_from_the_snapshot() {
    let mut history = History::default();
    history
        .record_snapshot(Snapshot {
            revision: 42,
            world: String::new(),
            quantities: [("o1".into(), 10)].into(),
        })
        .expect("snapshot");
    let established = history.establish_can_dispatch(42, "o1");
    assert!(established.authorizes());
    let exported = history.export_dispatch("o1").expect("exported claim");
    let rechecked = history.recheck_dispatch_certificate(&exported);
    match rechecked {
        Disposition::Satisfied(receipt) => {
            assert_eq!(receipt.authority, Authority::RuntimeCheck);
            assert!(receipt.kernel_certificate.is_none());
        }
        other => panic!("{other:?}"),
    }

    let mut tampered = exported.clone();
    tampered.quantity = 0;
    let disagreed = history.recheck_dispatch_certificate(&tampered);
    assert!(matches!(
        disagreed,
        Disposition::Unknown {
            reason: Reason::Malformed { .. }
        }
    ));
    assert!(!disagreed.authorizes());
    assert_eq!(history.claims().len(), 1);

    let from_snapshot = brix_lower::evidence::DispatchCertificate {
        order: "o1".into(),
        policy: "qty>=1".into(),
        revision: 42,
        world: String::new(),
        quantity: 10,
    };
    assert!(history
        .recheck_dispatch_certificate(&from_snapshot)
        .authorizes());
}

#[test]
fn persistent_policy_decision_is_bound_to_its_snapshot() {
    use brix_lower::evidence::{check_persisted_dispatch, DISPATCH_POLICY};

    let mut history = History::default();
    history
        .record_snapshot(Snapshot {
            revision: 42,
            world: "digest-42".into(),
            quantities: [("o1".into(), 10), ("o2".into(), 3)].into(),
        })
        .expect("snapshot");
    let requested = history.establish_can_dispatch(42, "o1");
    assert!(requested.authorizes(), "{requested:?}");
    let certificate = history.export_dispatch("o1").expect("policy certificate");
    assert_eq!(certificate.policy, DISPATCH_POLICY);
    assert!(history
        .use_dispatch(&certificate, "o1", DISPATCH_POLICY, 42)
        .authorizes());

    let wrong_order = history.use_dispatch(&certificate, "o2", DISPATCH_POLICY, 42);
    assert!(matches!(
        wrong_order,
        Disposition::Unknown {
            reason: Reason::ClaimMismatch { .. }
        }
    ));
    assert!(!wrong_order.authorizes());

    let wrong_policy = history.use_dispatch(&certificate, "o1", "qty>=10", 42);
    assert!(matches!(
        wrong_policy,
        Disposition::Unknown {
            reason: Reason::ContextMismatch { .. }
        }
    ));
    assert!(!wrong_policy.authorizes());

    history
        .record_snapshot(Snapshot {
            revision: 43,
            world: "digest-43".into(),
            quantities: [("o1".into(), 10), ("o2".into(), 9)].into(),
        })
        .expect("snapshot");
    let wrong_revision = history.use_dispatch(&certificate, "o1", DISPATCH_POLICY, 43);
    assert!(matches!(
        wrong_revision,
        Disposition::Unknown {
            reason: Reason::ContextMismatch { .. }
        }
    ));
    let transferred = history.transfer_dispatch(&certificate, 43);
    assert!(
        transferred.authorizes(),
        "unchanged quantity should transfer: {transferred:?}"
    );
    assert_eq!(history.claims().len(), 2);
    assert!(history
        .use_dispatch(&certificate, "o1", DISPATCH_POLICY, 42)
        .authorizes());
    let moved = history
        .export_dispatch_at("o1", 43)
        .expect("transferred claim");
    assert!(history
        .use_dispatch(&moved, "o1", DISPATCH_POLICY, 43)
        .authorizes());

    history
        .record_snapshot(Snapshot {
            revision: 44,
            world: "digest-44".into(),
            quantities: [("o1".into(), 0), ("o2".into(), 9)].into(),
        })
        .expect("snapshot");
    let stale = history.transfer_dispatch(&certificate, 44);
    assert!(matches!(
        stale,
        Disposition::Unknown {
            reason: Reason::ContextMismatch { .. }
        }
    ));
    assert!(!stale.is_logical_refutation());
    assert_eq!(history.claims().len(), 2);
    assert!(history
        .recheck_dispatch_certificate(&certificate)
        .authorizes());

    let mut substituted = certificate.clone();
    substituted.order = "o2".into();
    substituted.quantity = 3;
    assert!(!history
        .use_dispatch(&substituted, "o1", DISPATCH_POLICY, 42)
        .authorizes());

    let encoded = history.encode_journal().expect("journal");
    let decoded = History::decode_journal(&encoded).expect("reload");
    assert_eq!(decoded.claims().len(), 2);
    assert_eq!(
        decoded.encode_journal().expect("canonical"),
        encoded,
        "journal round trip"
    );
    let independent = check_persisted_dispatch(&encoded, &certificate, "o1", DISPATCH_POLICY, 42);
    match independent {
        Disposition::Satisfied(receipt) => {
            assert_eq!(receipt.authority, Authority::RuntimeCheck);
            assert!(receipt.kernel_certificate.is_none());
        }
        other => panic!("independent check: {other:?}"),
    }
    assert!(
        !check_persisted_dispatch(&encoded, &certificate, "o2", DISPATCH_POLICY, 42).authorizes()
    );
    assert!(!check_persisted_dispatch(&encoded, &certificate, "o1", "qty>=10", 42).authorizes());
    assert!(
        !check_persisted_dispatch(&encoded, &certificate, "o1", DISPATCH_POLICY, 43).authorizes()
    );
    let mut other_value = certificate.clone();
    other_value.quantity = 4;
    assert!(
        !check_persisted_dispatch(&encoded, &other_value, "o1", DISPATCH_POLICY, 42).authorizes()
    );

    let lied = encoded.replace("qty=10", "qty=0");
    assert_ne!(lied, encoded);
    assert!(
        check_persisted_dispatch(&lied, &certificate, "o1", DISPATCH_POLICY, 42).authorizes(),
        "a rewritten claim must not replace the snapshot"
    );
    let decoded_lie = History::decode_journal(&lied).expect("lied journal still parses");
    assert_eq!(decoded_lie.claims().len(), 2);
    assert!(!decoded_lie
        .recheck_historical(decoded_lie.claims().first().expect("old claim"))
        .authorizes());
}
