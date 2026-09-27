//! ADR-0035: exact signed integer division, rounding, and modulo.
//!
//! The literal cases below come from the specification. The property section
//! is the part that actually carries the weight: it re-derives each operation
//! from its **defining identity** in `i128`, independently of how the
//! evaluator computes it, so a sign-handling bug cannot satisfy the test by
//! agreeing with itself. Hand-computed expectations alone would only pin the
//! implementation that produced them.

use brix_lower::l3_v2::{eval, DivModOpV2, EvalEnv, EvalFault, L3ExprV2, L3ValueV2};

fn apply(op: DivModOpV2, a: i64, b: i64) -> Result<i64, EvalFault> {
    let expr = L3ExprV2::IntDivMod(op, Box::new(L3ExprV2::Int(a)), Box::new(L3ExprV2::Int(b)));
    match eval(&expr, &EvalEnv::new()) {
        Ok(L3ValueV2::Int(n)) => Ok(n),
        Ok(other) => panic!("expected an Int, got {other:?}"),
        Err(e) => Err(e),
    }
}

fn ok(op: DivModOpV2, a: i64, b: i64) -> i64 {
    apply(op, a, b).unwrap_or_else(|e| panic!("{}({a}, {b}) faulted: {e}", op.name()))
}

/// Operands chosen for sign coverage and for both overflow edges, not for
/// volume: every interesting case for these operations is a sign or a bound.
const GRID: [i64; 15] = [
    i64::MIN,
    i64::MIN + 1,
    -1000,
    -7,
    -3,
    -2,
    -1,
    0,
    1,
    2,
    3,
    7,
    1000,
    i64::MAX - 1,
    i64::MAX,
];

// ---------------------------------------------------------------------------
// Specified literal cases
// ---------------------------------------------------------------------------

#[test]
fn specified_examples_hold() {
    use DivModOpV2::*;

    // Negative quotients are where truncation and flooring diverge, which is
    // the whole reason these are named operations rather than one `/`.
    assert_eq!(ok(DivFloor, -7, 2), -4);
    assert_eq!(ok(DivCeil, -7, 2), -3);
    assert_eq!(ok(DivFloor, 7, 2), 3);
    assert_eq!(ok(DivCeil, 7, 2), 4);

    // Exact division agrees everywhere; no rounding rule may perturb it.
    for op in [DivFloor, DivCeil, DivHalfEven] {
        assert_eq!(ok(op, 6, 3), 2, "{}", op.name());
        assert_eq!(ok(op, -6, 3), -2, "{}", op.name());
        assert_eq!(ok(op, 6, -3), -2, "{}", op.name());
        assert_eq!(ok(op, -6, -3), 2, "{}", op.name());
    }

    // Half-even ties resolve toward the even neighbour in both directions —
    // not "away from zero", which would give 3 and -3 here.
    assert_eq!(ok(DivHalfEven, 5, 2), 2); // 2.5 -> 2
    assert_eq!(ok(DivHalfEven, -5, 2), -2); // -2.5 -> -2
    assert_eq!(ok(DivHalfEven, 7, 2), 4); // 3.5 -> 4
    assert_eq!(ok(DivHalfEven, -7, 2), -4); // -3.5 -> -4

    // Non-ties round to the genuinely nearer integer.
    assert_eq!(ok(DivHalfEven, 7, 3), 2); // 2.33
    assert_eq!(ok(DivHalfEven, 8, 3), 3); // 2.67
    assert_eq!(ok(DivHalfEven, -7, 3), -2);
    assert_eq!(ok(DivHalfEven, -8, 3), -3);

    // Euclidean remainder is non-negative regardless of either sign — note
    // `mod_euclid(7, -3) == 1`, which is *not* the floored remainder.
    assert_eq!(ok(ModEuclid, 7, 3), 1);
    assert_eq!(ok(ModEuclid, 7, -3), 1);
    assert_eq!(ok(ModEuclid, -7, 3), 2);
    assert_eq!(ok(ModEuclid, -7, -3), 2);
}

// ---------------------------------------------------------------------------
// Defining identities, checked independently in i128
// ---------------------------------------------------------------------------

/// `div_floor` is characterised by its remainder taking the **sign of the
/// divisor**: `a = b*q + r` with `0 <= r < b` for positive `b`, mirrored for
/// negative `b`.
#[test]
fn div_floor_satisfies_its_defining_identity() {
    for a in GRID {
        for b in GRID {
            if b == 0 {
                continue;
            }
            let Ok(q) = apply(DivModOpV2::DivFloor, a, b) else {
                continue; // overflow cases are pinned separately
            };
            let (a128, b128, q128) = (i128::from(a), i128::from(b), i128::from(q));
            let r = a128 - b128 * q128;
            if b > 0 {
                assert!(r >= 0 && r < b128, "div_floor({a}, {b}) = {q}, r = {r}");
            } else {
                assert!(r <= 0 && r > b128, "div_floor({a}, {b}) = {q}, r = {r}");
            }
        }
    }
}

/// `div_ceil` is the mirror image: its remainder takes the sign **opposite**
/// the divisor.
#[test]
fn div_ceil_satisfies_its_defining_identity() {
    for a in GRID {
        for b in GRID {
            if b == 0 {
                continue;
            }
            let Ok(q) = apply(DivModOpV2::DivCeil, a, b) else {
                continue;
            };
            let (a128, b128, q128) = (i128::from(a), i128::from(b), i128::from(q));
            let r = a128 - b128 * q128;
            if b > 0 {
                assert!(r <= 0 && r > -b128, "div_ceil({a}, {b}) = {q}, r = {r}");
            } else {
                assert!(r >= 0 && r < -b128, "div_ceil({a}, {b}) = {q}, r = {r}");
            }
        }
    }
}

/// Round-half-even is fully characterised by two conditions on the residual:
/// it is never more than half a divisor away, and an exact half lands on an
/// even quotient. Checking both is what distinguishes it from half-up,
/// half-away-from-zero, and plain truncation.
#[test]
fn div_half_even_satisfies_its_defining_identity() {
    for a in GRID {
        for b in GRID {
            if b == 0 {
                continue;
            }
            let Ok(q) = apply(DivModOpV2::DivHalfEven, a, b) else {
                continue;
            };
            let (a128, b128, q128) = (i128::from(a), i128::from(b), i128::from(q));
            let r = a128 - b128 * q128;
            let twice = 2 * r.abs();
            let mag = b128.abs();
            assert!(
                twice <= mag,
                "div_half_even({a}, {b}) = {q} is not a nearest integer (r = {r})"
            );
            if twice == mag {
                assert_eq!(
                    q128 % 2,
                    0,
                    "div_half_even({a}, {b}) = {q} broke an exact tie toward an odd quotient"
                );
            }
        }
    }
}

/// `mod_euclid` is defined by `a = b*q + r` with `0 <= r < |b|` — a condition
/// on `r` alone, with `q` left existentially quantified. That is deliberate:
/// for negative `b` the Euclidean quotient is not the floored one, so pinning
/// a quotient here would pin the wrong pairing.
#[test]
fn mod_euclid_satisfies_its_defining_identity() {
    for a in GRID {
        for b in GRID {
            if b == 0 {
                continue;
            }
            let r = i128::from(ok(DivModOpV2::ModEuclid, a, b));
            let (a128, b128) = (i128::from(a), i128::from(b));
            assert!(
                r >= 0 && r < b128.abs(),
                "mod_euclid({a}, {b}) = {r} is outside [0, |b|)"
            );
            assert_eq!(
                (a128 - r) % b128,
                0,
                "mod_euclid({a}, {b}) = {r} admits no integer quotient"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Faults
// ---------------------------------------------------------------------------

#[test]
fn division_and_modulo_by_zero_fault() {
    for op in DivModOpV2::ALL {
        for a in GRID {
            assert_eq!(
                apply(op, a, 0),
                Err(EvalFault::DivisionByZero(op)),
                "{}({a}, 0)",
                op.name()
            );
        }
    }
}

/// The single quotient that is mathematically defined but unrepresentable.
/// Every other pair in the grid must *not* fault, so this is a real edge and
/// not an over-eager range check.
#[test]
fn only_min_over_negative_one_overflows() {
    for op in [
        DivModOpV2::DivFloor,
        DivModOpV2::DivCeil,
        DivModOpV2::DivHalfEven,
    ] {
        assert_eq!(
            apply(op, i64::MIN, -1),
            Err(EvalFault::DivisionOverflow(op)),
            "{}(MIN, -1)",
            op.name()
        );
        for a in GRID {
            for b in GRID {
                if b == 0 || (a == i64::MIN && b == -1) {
                    continue;
                }
                assert!(
                    apply(op, a, b).is_ok(),
                    "{}({a}, {b}) faulted unexpectedly",
                    op.name()
                );
            }
        }
    }
}

/// `mod_euclid(MIN, -1)` is `0`, and must not inherit the quotient's
/// overflow: the remainder is bounded by `|b|`, so it always fits. Refusing
/// it would refuse a well-defined answer.
#[test]
fn mod_euclid_never_overflows_including_min_over_negative_one() {
    assert_eq!(ok(DivModOpV2::ModEuclid, i64::MIN, -1), 0);
    for a in GRID {
        for b in GRID {
            if b == 0 {
                continue;
            }
            assert!(
                apply(DivModOpV2::ModEuclid, a, b).is_ok(),
                "mod_euclid({a}, {b}) faulted"
            );
        }
    }
}

#[test]
fn operands_must_be_integers() {
    for op in DivModOpV2::ALL {
        let bad_left = L3ExprV2::IntDivMod(
            op,
            Box::new(L3ExprV2::Bool(true)),
            Box::new(L3ExprV2::Int(2)),
        );
        let bad_right = L3ExprV2::IntDivMod(
            op,
            Box::new(L3ExprV2::Int(2)),
            Box::new(L3ExprV2::Bool(true)),
        );
        for expr in [bad_left, bad_right] {
            assert_eq!(
                eval(&expr, &EvalEnv::new()),
                Err(EvalFault::OperandShape(
                    "integer division requires Int operands"
                )),
                "{}",
                op.name()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Lowering: reserved names, arity, and the refusal of bare `/`
// ---------------------------------------------------------------------------

mod lowering {
    use brix_lower::{
        lower_finite_decision_plan, FiniteDecisionLowerError, FiniteDecisionPlan,
        FiniteDecisionRuntime, FINITE_DECISION_PROFILE,
    };
    use brix_syntax::parse;

    fn lower(source: &str) -> Result<FiniteDecisionPlan, FiniteDecisionLowerError> {
        let module = parse(source).expect("fixture parses");
        lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE)
    }

    fn plan(source: &str) -> FiniteDecisionPlan {
        lower(source).expect("fixture lowers")
    }

    const PREAMBLE: &str = "config Decision = Yes | No\n";
    const TAIL: &str = "propose p(r) priority 1 when r > 0 = Yes\n\
                        propose q() priority 10 when true = No\n\
                        commit c from (p, q)\n";

    /// `/` stays refused, and the diagnostic names the replacements. The
    /// point of this milestone is that rounding is *stated*, so silently
    /// picking one for `/` would undo it.
    #[test]
    fn bare_division_operator_is_still_refused_and_says_why() {
        let err = lower(&format!("{PREAMBLE}rule r() = 7 / 2\n{TAIL}"))
            .expect_err("'/' must stay refused");
        let msg = err.to_string();
        assert!(msg.contains("div_floor"), "{msg}");
        assert!(msg.contains("div_half_even"), "{msg}");
    }

    /// A helper may not claim a reserved name. Refused at the declaration, so
    /// the author sees it once, rather than losing every call site silently
    /// to the built-in.
    #[test]
    fn a_helper_may_not_shadow_a_reserved_name() {
        for name in ["div_floor", "div_ceil", "div_half_even", "mod_euclid"] {
            let src = format!(
                "{PREAMBLE}fn {name}(a: Int, b: Int): Int = a\nrule r() = {name}(7, 2)\n{TAIL}"
            );
            match lower(&src) {
                Err(FiniteDecisionLowerError::ReservedOperationName { name: n, kind }) => {
                    assert_eq!(n, name);
                    assert_eq!(kind, "function");
                }
                other => panic!("{name}: expected a reserved-name refusal, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_constructor_may_not_shadow_a_reserved_name() {
        let src = format!("config Weird = div_floor | Other\n{PREAMBLE}rule r() = 1\n{TAIL}");
        match lower(&src) {
            Err(FiniteDecisionLowerError::ReservedOperationName { name, kind }) => {
                assert_eq!(name, "div_floor");
                assert_eq!(kind, "constructor");
            }
            other => panic!("expected a reserved-name refusal, got {other:?}"),
        }
    }

    #[test]
    fn reserved_operations_take_exactly_two_arguments() {
        for call in ["div_floor(7)", "div_floor(7, 2, 3)", "mod_euclid()"] {
            let src = format!("{PREAMBLE}rule r() = {call}\n{TAIL}");
            match lower(&src) {
                Err(FiniteDecisionLowerError::FunctionArityMismatch { expected, .. }) => {
                    assert_eq!(expected, 2, "{call}");
                }
                other => panic!("{call}: expected an arity refusal, got {other:?}"),
            }
        }
    }

    /// End to end through the real runtime, with a negative dividend so the
    /// rounding rule is load-bearing for which candidate wins.
    #[test]
    fn a_decision_can_turn_on_a_rounding_rule() {
        // -7/2 is -4 flooring and -3 ceiling, so the two programs differ only
        // in the operation named and must reach different decisions.
        let src = |op: &str| {
            format!(
                "config Decision = Yes | No\n\
                 rule r() = {op}(-7, 2)\n\
                 propose p(r) priority 1 when r > -4 = Yes\n\
                 propose q() priority 10 when true = No\n\
                 commit c from (p, q)\n"
            )
        };

        let floored = plan(&src("div_floor"));
        let run = FiniteDecisionRuntime::build(&floored)
            .expect("runtime builds")
            .run();
        assert_eq!(run.decision.as_ref().expect("decided").candidate, "q");

        let ceiled = plan(&src("div_ceil"));
        let run = FiniteDecisionRuntime::build(&ceiled)
            .expect("runtime builds")
            .run();
        assert_eq!(run.decision.as_ref().expect("decided").candidate, "p");
    }

    /// Two programs differing only in the named rounding must not share a
    /// program id: the operation is pinned structurally, not by a helper name
    /// that audit could resolve differently.
    #[test]
    fn the_named_rounding_is_part_of_program_identity() {
        use brix_lower::finite_decision_program_id;
        let src = |op: &str| format!("config Decision = Yes | No\nrule r() = {op}(7, 2)\n{TAIL}");
        let ids: Vec<_> = ["div_floor", "div_ceil", "div_half_even", "mod_euclid"]
            .into_iter()
            .map(|op| finite_decision_program_id(&plan(&src(op))))
            .collect();
        for (i, a) in ids.iter().enumerate() {
            assert_eq!(
                *a,
                finite_decision_program_id(&plan(&src([
                    "div_floor",
                    "div_ceil",
                    "div_half_even",
                    "mod_euclid"
                ][i]))),
                "program id must be reproducible"
            );
            for (j, b) in ids.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "distinct roundings must not share a program id");
                }
            }
        }
    }

    /// A fault inside the decision fails closed: no candidate is committed.
    #[test]
    fn a_division_by_zero_commits_nothing() {
        let p = plan(&format!(
            "{PREAMBLE}let d = 0\nrule r() = div_floor(7, d)\n{TAIL}"
        ));
        let run = FiniteDecisionRuntime::build(&p)
            .expect("runtime builds")
            .run();
        assert!(
            run.decision.is_none(),
            "a faulting rule must commit nothing"
        );
    }
}
