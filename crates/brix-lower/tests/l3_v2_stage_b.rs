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

#[test]
fn boolean_operators_truth_table_and_not() {
    let p = plan(
        "rule tt() = true && true\n\
         rule tf() = true && false\n\
         rule ft() = false && true\n\
         rule ff() = false && false\n\
         rule or_tt() = true || true\n\
         rule or_tf() = true || false\n\
         rule or_ft() = false || true\n\
         rule or_ff() = false || false\n\
         rule not_t() = !true\n\
         rule not_f() = !false\n\
         rule not_not_t() = !(!true)\n",
    );
    let env = EvalEnv::new();
    assert_eq!(eval(rule_body(&p, "tt"), &env), Ok(L3ValueV2::Bool(true)));
    assert_eq!(eval(rule_body(&p, "tf"), &env), Ok(L3ValueV2::Bool(false)));
    assert_eq!(eval(rule_body(&p, "ft"), &env), Ok(L3ValueV2::Bool(false)));
    assert_eq!(eval(rule_body(&p, "ff"), &env), Ok(L3ValueV2::Bool(false)));
    assert_eq!(
        eval(rule_body(&p, "or_tt"), &env),
        Ok(L3ValueV2::Bool(true))
    );
    assert_eq!(
        eval(rule_body(&p, "or_tf"), &env),
        Ok(L3ValueV2::Bool(true))
    );
    assert_eq!(
        eval(rule_body(&p, "or_ft"), &env),
        Ok(L3ValueV2::Bool(true))
    );
    assert_eq!(
        eval(rule_body(&p, "or_ff"), &env),
        Ok(L3ValueV2::Bool(false))
    );
    assert_eq!(
        eval(rule_body(&p, "not_t"), &env),
        Ok(L3ValueV2::Bool(false))
    );
    assert_eq!(
        eval(rule_body(&p, "not_f"), &env),
        Ok(L3ValueV2::Bool(true))
    );
    assert_eq!(
        eval(rule_body(&p, "not_not_t"), &env),
        Ok(L3ValueV2::Bool(true))
    );
}

#[test]
fn boolean_operators_short_circuit_on_faults() {
    let p = plan(
        "let big = 9223372036854775807\n\
         rule and_short() = false && (big + big > 0)\n\
         rule or_short() = true || (big + big > 0)\n\
         rule and_no_short() = true && (big + big > 0)\n\
         rule or_no_short() = false || (big + big > 0)\n",
    );
    let env = EvalEnv::new().with_let("big", L3ValueV2::Int(i64::MAX));

    // false && <fault> does NOT evaluate RHS
    assert_eq!(
        eval(rule_body(&p, "and_short"), &env),
        Ok(L3ValueV2::Bool(false))
    );
    // true || <fault> does NOT evaluate RHS
    assert_eq!(
        eval(rule_body(&p, "or_short"), &env),
        Ok(L3ValueV2::Bool(true))
    );

    // true && <fault> evaluates RHS and faults
    assert_eq!(
        eval(rule_body(&p, "and_no_short"), &env),
        Err(EvalFault::Overflow(ArithOpV2::Add))
    );
    // false || <fault> evaluates RHS and faults
    assert_eq!(
        eval(rule_body(&p, "or_no_short"), &env),
        Err(EvalFault::Overflow(ArithOpV2::Add))
    );
}

#[test]
fn boolean_operators_require_boolean_operands() {
    let p = plan(
        "rule bad_and_left() = 1 && true\n\
         rule bad_and_right() = true && 1\n\
         rule bad_or_left() = 1 || false\n\
         rule bad_or_right() = false || 1\n\
         rule bad_not() = !1\n",
    );
    let env = EvalEnv::new();

    assert_eq!(
        eval(rule_body(&p, "bad_and_left"), &env),
        Err(EvalFault::OperandShape(
            "logical AND requires Bool operands"
        ))
    );
    assert_eq!(
        eval(rule_body(&p, "bad_and_right"), &env),
        Err(EvalFault::OperandShape(
            "logical AND requires Bool operands"
        ))
    );
    assert_eq!(
        eval(rule_body(&p, "bad_or_left"), &env),
        Err(EvalFault::OperandShape("logical OR requires Bool operands"))
    );
    assert_eq!(
        eval(rule_body(&p, "bad_or_right"), &env),
        Err(EvalFault::OperandShape("logical OR requires Bool operands"))
    );
    assert_eq!(
        eval(rule_body(&p, "bad_not"), &env),
        Err(EvalFault::OperandShape("logical NOT requires Bool operand"))
    );
}

#[test]
fn boolean_operators_precedence_in_evaluator() {
    let p = plan(
        "rule p1() = 1 < 2 && 3 < 4\n\
         rule p2() = false && false || true\n\
         rule p3() = true || false && false\n\
         rule p4() = !false && true\n\
         rule p5() = !(false && true)\n",
    );
    let env = EvalEnv::new();
    assert_eq!(eval(rule_body(&p, "p1"), &env), Ok(L3ValueV2::Bool(true)));
    assert_eq!(eval(rule_body(&p, "p2"), &env), Ok(L3ValueV2::Bool(true)));
    assert_eq!(eval(rule_body(&p, "p3"), &env), Ok(L3ValueV2::Bool(true)));
    assert_eq!(eval(rule_body(&p, "p4"), &env), Ok(L3ValueV2::Bool(true)));
    assert_eq!(eval(rule_body(&p, "p5"), &env), Ok(L3ValueV2::Bool(true)));
}

/// Unary minus (`-e`) has no dedicated `L3ExprV2` variant: for any operand
/// that is not a bare numeral, the parser desugars it to `0 - e` (`ast::
/// BinOp::Sub`), so at this layer it is just `L3ExprV2::Arith(ArithOpV2::Sub,
/// Int(0), e)` — proven here by matching the lowered shape directly, not only
/// its evaluated result. (A bare numeral like `-5` instead folds straight to
/// `L3ExprV2::Int(-5)`, covered separately below.)
#[test]
fn unary_minus_desugars_to_arith_sub_of_zero() {
    let p = plan("rule x() = 5\nrule r(x) = -x\n");
    match rule_body(&p, "r") {
        L3ExprV2::Arith(ArithOpV2::Sub, a, b) => {
            assert_eq!(**a, L3ExprV2::Int(0));
            assert_eq!(**b, L3ExprV2::RuleFact("x".to_string()));
        }
        other => panic!("expected Arith(Sub, 0, RuleFact(x)), got {other:?}"),
    }
}

/// `-a * b` is `(-a) * b`: unary minus binds tighter than every binary
/// operator, evaluated end to end here (the parser-level shape is pinned
/// separately in brix-syntax's `parse_fixtures.rs`).
#[test]
fn unary_minus_precedence_evaluates_tighter_than_multiplication() {
    let p = plan("rule a() = 3\nrule b() = 4\nrule r(a, b) = -a * b\n");
    let env = EvalEnv::new()
        .with_fact("a", L3ValueV2::Int(3))
        .with_fact("b", L3ValueV2::Int(4));
    // (-3) * 4 = -12, not -(3 * 4) which would coincidentally also be -12 —
    // use asymmetric operands to make the parenthesisation load-bearing.
    assert_eq!(eval(rule_body(&p, "r"), &env), Ok(L3ValueV2::Int(-12)));

    let p2 = plan("rule a() = 3\nrule b() = 4\nrule r(a, b) = -(a * b)\n");
    assert_eq!(eval(rule_body(&p2, "r"), &env), Ok(L3ValueV2::Int(-12)));
}

/// A literal at `i64::MIN` is directly representable (`-9223372036854775808`
/// folds to a literal at parse time, never going through `0 - n`), and
/// evaluates without faulting.
#[test]
fn unary_minus_on_i64_min_literal_evaluates_without_faulting() {
    let p = plan("rule r() = -9223372036854775808\n");
    assert_eq!(
        eval(rule_body(&p, "r"), &EvalEnv::new()),
        Ok(L3ValueV2::Int(i64::MIN))
    );
}

/// Negating `i64::MIN` itself is NOT representable: `0 - i64::MIN` overflows
/// `i64::MAX` by one, and checked arithmetic refuses it rather than wrapping
/// (ADR-0027 §9.7). `-(Int::MIN)` reaches exactly this path because the
/// literal is parenthesised, so it cannot take the literal-folding shortcut —
/// it desugars to `0 - e` and the fold happens one level down, on `e` alone.
#[test]
fn negating_i64_min_faults_instead_of_wrapping() {
    let p = plan("rule r() = -(-9223372036854775808)\n");
    assert_eq!(
        eval(rule_body(&p, "r"), &EvalEnv::new()),
        Err(EvalFault::Overflow(ArithOpV2::Sub))
    );
}

/// The same fault reachable through a bound fact, not just a literal: an
/// ordinary program computing `-x` for `x = i64::MIN` must fault, never wrap
/// to `i64::MIN` again or produce some other value.
#[test]
fn negating_a_fact_bound_to_i64_min_faults() {
    let p = plan("rule x() = 1\nrule r(x) = -x\n");
    let env = EvalEnv::new().with_fact("x", L3ValueV2::Int(i64::MIN));
    assert_eq!(
        eval(rule_body(&p, "r"), &env),
        Err(EvalFault::Overflow(ArithOpV2::Sub))
    );
}
