//! ADR-0027 Stage B gates: the evaluator, its faults, and exhaustiveness.

use brix_lower::l3_v2::{
    check_exhaustive, eval, lower_l3_plan_v2, ArithOpV2, EvalEnv, EvalFault, L3ExprV2,
    L3PlanItemV2, L3PlanV2, L3V2LowerError, L3ValueV2, L3_PROFILE_MARKER_V2,
};
use brix_syntax::parse;

fn plan(src: &str) -> L3PlanV2 {
    let module = parse(src).expect("fixture parses");
    lower_l3_plan_v2(&module, L3_PROFILE_MARKER_V2).expect("fixture lowers")
}

fn rule_body<'a>(p: &'a L3PlanV2, name: &str) -> &'a L3ExprV2 {
    p.items
        .iter()
        .find_map(|i| match i {
            L3PlanItemV2::Rule { name: n, body, .. } if n == name => Some(body),
            _ => None,
        })
        .expect("rule present")
}

/// **The end-to-end shape of v2.** A rule computes from an earlier rule's
/// committed fact — the thing v1 cannot express at all.
#[test]
fn a_rule_derives_from_a_committed_fact() {
    let p = plan("rule base() = 1500\nrule boosted(base) = base + 500\n");
    let env = EvalEnv::new().with_fact("base", L3ValueV2::Int(1500));

    assert_eq!(
        eval(rule_body(&p, "boosted"), &env),
        Ok(L3ValueV2::Int(2000))
    );
}

/// Eligibility is exactly ⟨D-DERIVE⟩'s condition: a rule is a candidate only
/// once every dependency has committed.
#[test]
fn a_rule_is_ineligible_until_its_dependency_commits() {
    let p = plan("rule base() = 1\nrule derived(base) = base\n");
    let deps = p
        .items
        .iter()
        .find_map(|i| match i {
            L3PlanItemV2::Rule {
                name, depends_on, ..
            } if name == "derived" => Some(depends_on),
            _ => None,
        })
        .expect("rule present");

    let empty = EvalEnv::new();
    assert!(!empty.satisfies(deps), "not eligible before base commits");
    assert_eq!(
        eval(rule_body(&p, "derived"), &empty),
        Err(EvalFault::Unbound("base".to_string())),
        "and evaluating anyway is a refusal, never a default value"
    );

    let ready = EvalEnv::new().with_fact("base", L3ValueV2::Int(1));
    assert!(ready.satisfies(deps));
}

/// Arithmetic is **checked**. An overflowed fact would claim a value the
/// arithmetic did not produce, so it is a fault — never wrapped, saturated or
/// truncated (ADR-0027 §9.7).
#[test]
fn arithmetic_overflow_is_a_fault_not_a_wrap() {
    let p = plan("let big = 9223372036854775807\nrule r() = big + big\n");
    let env = EvalEnv::new().with_let("big", L3ValueV2::Int(i64::MAX));

    assert_eq!(
        eval(rule_body(&p, "r"), &env),
        Err(EvalFault::Overflow(ArithOpV2::Add))
    );
}

/// Operands evaluate left to right, which fixes *which* fault a program with
/// two faulty operands reports. That ordering is ABI, not an accident.
#[test]
fn operands_evaluate_left_to_right() {
    // Both operands read facts that have not committed, so both would fault.
    // Lowering accepts this — the rules exist — and evaluation decides which
    // fault is reported, which is exactly what the ordering fixes.
    let p = plan("rule a() = 1\nrule b() = 2\nrule r(a, b) = a + b\n");
    match eval(rule_body(&p, "r"), &EvalEnv::new()) {
        Err(EvalFault::Unbound(name)) => {
            assert_eq!(name, "a", "the LEFT operand's fault is the reported one")
        }
        other => panic!("expected an Unbound fault, got {other:?}"),
    }
}

/// Field access, comparison and `match` compute what they should.
#[test]
fn the_evaluator_computes_the_v2_fragment() {
    let p = plan(
        "config Card = MkCard { atk: Int }\n\
         let c = MkCard { atk: 1800 }\n\
         rule strong() = c.atk > 1500\n",
    );
    let card = L3ValueV2::Record {
        nominal_config: "MkCard".to_string(),
        fields: vec![("atk".to_string(), L3ValueV2::Int(1800))],
    };
    let env = EvalEnv::new().with_let("c", card);
    assert_eq!(
        eval(rule_body(&p, "strong"), &env),
        Ok(L3ValueV2::Bool(true))
    );
}

/// A boolean `match` dispatches on `Bool`'s two constructors.
#[test]
fn a_boolean_match_dispatches() {
    let p = plan(
        "config R = Win | Lose\n\
         let flag = true\n\
         rule outcome() = match flag { true => Win  false => Lose }\n",
    );
    let env = EvalEnv::new().with_let("flag", L3ValueV2::Bool(true));
    assert_eq!(
        eval(rule_body(&p, "outcome"), &env),
        Ok(L3ValueV2::Ctor {
            nominal_sum: "R".to_string(),
            variant: "Win".to_string(),
            args: vec![],
        })
    );
}

/// A constructor pattern binds its arguments, and the binding is visible in
/// the arm body.
#[test]
fn a_constructor_pattern_binds_its_arguments() {
    let p = plan(
        "config Box = MkBox(Int)\n\
         let b = MkBox(7)\n\
         rule unwrapped() = match b { MkBox(v) => v }\n",
    );
    let env = EvalEnv::new().with_let(
        "b",
        L3ValueV2::Ctor {
            nominal_sum: "Box".to_string(),
            variant: "MkBox".to_string(),
            args: vec![L3ValueV2::Int(7)],
        },
    );
    assert_eq!(
        eval(rule_body(&p, "unwrapped"), &env),
        Ok(L3ValueV2::Int(7))
    );
}

/// Exhaustiveness is required, and now actually checked — Stage A declared no
/// error for it precisely because Stage A could not fire one.
#[test]
fn a_non_exhaustive_match_is_refused() {
    let p = plan(
        "config Z = Hand | Field | Grave\n\
         let z = Hand\n\
         rule r() = match z { Hand => 1  Field => 2 }\n",
    );
    match check_exhaustive(&p) {
        Err(L3V2LowerError::NonExhaustiveMatch { sum, missing }) => {
            assert_eq!(sum, "Z");
            assert_eq!(missing, vec!["Grave".to_string()]);
        }
        other => panic!("expected NonExhaustiveMatch, got {other:?}"),
    }

    // And a covering match passes.
    let ok = plan(
        "config Z = Hand | Field\n\
         let z = Hand\n\
         rule r() = match z { Hand => 1  Field => 2 }\n",
    );
    assert_eq!(check_exhaustive(&ok), Ok(()));
}

/// A boolean match must cover both constructors — `Bool` is a two-variant sum,
/// not a special case.
#[test]
fn a_boolean_match_must_cover_both_cases() {
    let p = plan("let flag = true\nrule r() = match flag { true => 1 }\n");
    match check_exhaustive(&p) {
        Err(L3V2LowerError::NonExhaustiveMatch { sum, missing }) => {
            assert_eq!(sum, "Bool");
            assert_eq!(missing, vec!["false".to_string()]);
        }
        other => panic!("expected NonExhaustiveMatch on Bool, got {other:?}"),
    }
}

/// Evaluation is deterministic: the same expression under the same
/// environment produces the same value, every time.
#[test]
fn evaluation_is_deterministic() {
    let p = plan("rule r() = 2 * 3 + 4\n");
    let env = EvalEnv::new();
    let first = eval(rule_body(&p, "r"), &env);
    for _ in 0..16 {
        assert_eq!(eval(rule_body(&p, "r"), &env), first);
    }
    assert_eq!(first, Ok(L3ValueV2::Int(10)));
}
