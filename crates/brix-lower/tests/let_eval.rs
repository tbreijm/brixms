//! The `let` lane computes values (ADR-0042): `evaluate_let_module` lowers
//! and evaluates every top-level `let`/`witness` binding through the same
//! evaluator the finite-decision lane uses.

use brix_lower::l3_v2::L3ValueV2;
use brix_lower::{check_module, evaluate_let_module, LetEvalOutcome};
use brix_syntax::parse;

/// Evaluate `source` and return the `(name, outcome)` pairs, asserting they
/// line up 1:1 with `check_module`'s own binding count (the contract
/// `evaluate_let_module`'s doc comment promises).
fn eval_bindings(source: &str) -> Vec<(String, LetEvalOutcome)> {
    let module = parse(source).expect("fixture parses");
    let checked = check_module(&module);
    let evaluated = evaluate_let_module(&module);
    assert_eq!(
        checked.len(),
        evaluated.len(),
        "evaluate_let_module must produce one outcome per check_module result"
    );
    evaluated
}

fn value_of(source: &str, name: &str) -> L3ValueV2 {
    let bindings = eval_bindings(source);
    let (_, outcome) = bindings
        .into_iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("no binding named '{name}'"));
    match outcome {
        LetEvalOutcome::Value(v) => v,
        LetEvalOutcome::NotEvaluated(reason) => {
            panic!("expected '{name}' to evaluate, got NotEvaluated({reason})")
        }
    }
}

fn not_evaluated_reason(source: &str, name: &str) -> String {
    let bindings = eval_bindings(source);
    let (_, outcome) = bindings
        .into_iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("no binding named '{name}'"));
    match outcome {
        LetEvalOutcome::Value(v) => panic!("expected '{name}' to be NotEvaluated, got {v:?}"),
        LetEvalOutcome::NotEvaluated(reason) => reason,
    }
}

// ---------------------------------------------------------------------------
// Literals
// ---------------------------------------------------------------------------

#[test]
fn test_int_and_string_literals_evaluate() {
    let source = "let x = 42\nlet s = \"hi\"";
    assert_eq!(value_of(source, "x"), L3ValueV2::Int(42));
    assert_eq!(value_of(source, "s"), L3ValueV2::Str("hi".to_string()));
}

#[test]
fn test_id_fixture_evaluates_to_42() {
    let source = include_str!("fixtures/id.brix");
    assert_eq!(value_of(source, "r"), L3ValueV2::Int(42));
}

#[test]
fn test_arithmetic_evaluates() {
    let source = "let c = 1 + 2";
    assert_eq!(value_of(source, "c"), L3ValueV2::Int(3));
}

// ---------------------------------------------------------------------------
// Records and field access
// ---------------------------------------------------------------------------

#[test]
fn test_record_and_field_projection_evaluate() {
    let source = r#"
config Item = { a: Int, b: Int }
let p = Item { a: 1, b: 2 }
let v = p.a
"#;
    assert_eq!(
        value_of(source, "p"),
        L3ValueV2::Record {
            nominal_config: "Item".to_string(),
            fields: vec![
                ("a".to_string(), L3ValueV2::Int(1)),
                ("b".to_string(), L3ValueV2::Int(2)),
            ],
        }
    );
    assert_eq!(value_of(source, "v"), L3ValueV2::Int(1));
}

// ---------------------------------------------------------------------------
// Sums and match
// ---------------------------------------------------------------------------

#[test]
fn test_sum_constructor_and_match_evaluate() {
    let source = r#"
config Decision = Approved | Rejected(Int)
let d = Rejected(7)
let code = match d {
  Approved => 0
  Rejected(n) => n
}
"#;
    assert_eq!(
        value_of(source, "d"),
        L3ValueV2::Ctor {
            nominal_sum: "Decision".to_string(),
            variant: "Rejected".to_string(),
            args: vec![L3ValueV2::Int(7)],
        }
    );
    assert_eq!(value_of(source, "code"), L3ValueV2::Int(7));
}

// ---------------------------------------------------------------------------
// Functions and recursion, including a user generic config (ADR-0042's
// erasure) — deliberately named `Stack<T>`, not `List<T>`, to avoid the
// built-in collection type the relations agent is adding to the
// finite-decision lane.
// ---------------------------------------------------------------------------

#[test]
fn test_function_call_evaluates() {
    let source = "fn double(x) = x + x\nlet r = double(2)";
    assert_eq!(value_of(source, "r"), L3ValueV2::Int(4));
}

#[test]
fn test_recursive_function_over_generic_stack_evaluates() {
    let source = r#"
config Stack<T> = SNil | SCons(T, Stack<T>)

fn length(s) = match s {
  SNil => 0
  SCons(x, rest) => 1 + length(rest)
}

let xs = SCons(1, SCons(2, SCons(3, SNil)))
let n = length(xs)
"#;
    assert_eq!(value_of(source, "n"), L3ValueV2::Int(3));
}

#[test]
fn test_generic_config_head_or_matches_readme_example() {
    // The corrected README example (see the final report): `head_or(Cons(42,
    // Nil), 0)` must print `= 42`, exercised here against a user config named
    // `Stack<T>` rather than `List<T>`.
    let source = r#"
config Stack<T> = Nil | Cons(T, Stack<T>)

fn head_or(s, default) = match s {
  Nil => default
  Cons(x, rest) => x
}

let r = head_or(Cons(42, Nil), 0)
"#;
    assert_eq!(value_of(source, "r"), L3ValueV2::Int(42));
}

// ---------------------------------------------------------------------------
// "Not evaluated": outside the exact executable fragment, never a guess.
// ---------------------------------------------------------------------------

#[test]
fn test_float_literal_is_not_evaluated() {
    let reason = not_evaluated_reason("let f = 3.14", "f");
    assert!(reason.contains("Float"), "reason: {reason}");
}

#[test]
fn test_float_arithmetic_is_not_evaluated() {
    let reason = not_evaluated_reason("let mixed = 1 + 2.5", "mixed");
    assert!(reason.contains("Float"), "reason: {reason}");
}

#[test]
fn test_division_is_not_evaluated_and_names_the_replacements() {
    let reason = not_evaluated_reason("let ratio = 7 / 2", "ratio");
    assert!(reason.contains("div_floor"), "reason: {reason}");
    assert!(reason.contains("Float"), "reason: {reason}");
}

#[test]
fn test_witness_composition_is_not_evaluated() {
    let source = "witness w1 = 1\nwitness w2 = 2\nwitness w3 = w1 then w2";
    let reason = not_evaluated_reason(source, "w3");
    assert!(reason.contains("composition"), "reason: {reason}");
}

#[test]
fn test_prove_is_not_evaluated() {
    let source = "let x = 1\nlet p = prove x";
    let reason = not_evaluated_reason(source, "p");
    assert!(reason.contains("prove"), "reason: {reason}");
}

#[test]
fn test_wildcard_match_arm_is_not_evaluated() {
    let source = r#"
config Decision = Approved | Rejected
let d = Approved
let v = match d {
  Approved => 1
  _ => 0
}
"#;
    let reason = not_evaluated_reason(source, "v");
    assert!(
        reason.contains("wildcard") || reason.contains("catch-all"),
        "reason: {reason}"
    );
}

#[test]
fn test_binding_depending_on_not_evaluated_binding_is_also_not_evaluated() {
    // `doubled` itself is entirely within the fragment, but it depends on
    // `ratio`, which is not — so it must not be silently treated as 0 or
    // guessed in any other way.
    let source = "let ratio = 7 / 2\nlet doubled = ratio";
    let reason = not_evaluated_reason(source, "doubled");
    assert!(reason.contains("ratio"), "reason: {reason}");
}

#[test]
fn test_evaluation_never_changes_the_grade() {
    use brix_semantic::Outcome;
    let module = parse("let ratio = 7 / 2").expect("parses");
    let checked = check_module(&module);
    let cr = checked.into_iter().next().unwrap().expect("type-checks");
    // `/` type-checks fine (Int/Int -> Float) even though it is not
    // evaluated; the grade is unaffected by evaluability.
    assert_eq!(cr.outcome, Outcome::Audited);
}
