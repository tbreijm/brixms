//! Adversarial cases at the concrete-schema input and construction boundaries.
use brix_lower::input::{canonicalize_input_shards, decode_input_shard, InputLimits};
use brix_lower::{lower_finite_decision_plan, FiniteDecisionRuntime, FINITE_DECISION_PROFILE};

#[test]
fn explicit_generic_ascription_specializes_an_internal_constructor() {
    let source = r#"
config Box<T> = Wrap(T)
config Decision = Pass
rule good() = len([Wrap(7)]: List<Box<Int>>) == 1
propose p priority 1 when good = Pass
commit d from (p)
"#;
    let plan = lower_finite_decision_plan(
        &brix_syntax::parse(source).unwrap(),
        FINITE_DECISION_PROFILE,
    )
    .unwrap();
    let run = FiniteDecisionRuntime::build(&plan).unwrap().run();
    assert!(
        run.is_selected(),
        "valid contextual construction failed: {:?}",
        run.stop
    );
}

#[test]
fn empty_annotation_must_resolve_its_type_before_erasure() {
    let source = r#"
config Decision = Pass
rule good() = len([]: List<Missing<Int>>) == 0
propose p priority 1 when good = Pass
commit d from (p)
"#;
    let plan = lower_finite_decision_plan(
        &brix_syntax::parse(source).unwrap(),
        FINITE_DECISION_PROFILE,
    );
    assert!(
        plan.is_err(),
        "annotation naming an undeclared generic type was accepted"
    );
}

#[test]
fn external_nested_generic_values_must_name_the_concrete_instantiation() {
    let source = r#"
config Box<T> = Empty | Wrap(T)
config Decision = Same | Different
input ints: List<Box<Int>> max 1
input strs: List<Box<Str>> max 1
rule same() = ints == strs
propose same_value priority 2 when same = Same
propose different otherwise = Different
commit d from (same_value, different)
"#;
    let plan = lower_finite_decision_plan(
        &brix_syntax::parse(source).unwrap(),
        FINITE_DECISION_PROFILE,
    )
    .unwrap();
    let bytes = br#"{"schema":"brix.input@5","values":{
        "ints":{"type":"list","items":[{"type":"sum","nominal":"Box","variant":"Empty","args":[]}]},
        "strs":{"type":"list","items":[{"type":"sum","nominal":"Box","variant":"Empty","args":[]}]}
    }}"#;
    let limits = InputLimits::default();
    let shard = decode_input_shard(bytes, &limits).unwrap();
    let snapshot = canonicalize_input_shards(vec![shard], &limits).unwrap();
    let result = FiniteDecisionRuntime::build_with_inputs(&plan, &snapshot);
    if let Ok(runtime) = &result {
        panic!(
            "erased external nominals admitted into two concrete schemas: {:?}",
            runtime.run().decision
        );
    }
}

#[test]
fn concrete_generic_return_constructs_and_preserves_its_identity() {
    let source = r#"
config Box<T> = Empty | Wrap(T)
config Decision = Pass
fn make(): Box<Int> = Wrap(7)
rule good() = match make() { Box::Wrap(n) => n == 7 _ => false }
propose p priority 1 when good = Pass
commit d from (p)
"#;
    let plan = lower_finite_decision_plan(
        &brix_syntax::parse(source).unwrap(),
        FINITE_DECISION_PROFILE,
    )
    .unwrap();
    let run = FiniteDecisionRuntime::build(&plan).unwrap().run();
    assert!(
        run.is_selected(),
        "generic return construction failed: {:?}",
        run.stop
    );
}

#[test]
fn nested_generic_ascription_uses_structural_type_arguments() {
    let source = r#"
config Box<T> = Wrap(T)
config Decision = Pass
fn make(): Box<List<Int>> = Wrap([7])
rule good() = len([make()]: List<Box<List<Int>>>) == 1
propose p priority 1 when good = Pass
commit d from (p)
"#;
    let plan = lower_finite_decision_plan(
        &brix_syntax::parse(source).unwrap(),
        FINITE_DECISION_PROFILE,
    )
    .unwrap();
    let run = FiniteDecisionRuntime::build(&plan).unwrap().run();
    assert!(
        run.is_selected(),
        "nested generic ascription failed: {:?}",
        run.stop
    );
}
