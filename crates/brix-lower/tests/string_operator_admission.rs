use std::collections::BTreeMap;

use brix_lower::module_graph::{ModuleGraph, ModuleLoaderLimits};
use brix_lower::relation_dag::lower_relations;
use brix_lower::{lower_finite_decision_plan, FINITE_DECISION_PROFILE};
use brix_syntax::parse;

fn linked(source: &str) -> brix_lower::module_graph::LinkedProgram {
    let sources = BTreeMap::from([("root".to_string(), source.to_string())]);
    ModuleGraph::load(
        "root",
        &|name: &str| sources.get(name).cloned(),
        ModuleLoaderLimits::default(),
    )
    .unwrap()
    .link()
    .unwrap()
}

#[test]
fn finite_decision_rejects_string_add_and_less_during_lowering() {
    let prefix = r#"
config Result = Ok
propose keep() priority 1 when true = Ok
commit results from (keep)
show check
"#;
    for expression in ["\"left\" + \"right\"", "\"left\" < \"right\""] {
        let source = prefix.replace("show check", &format!("show {expression}"));
        let module = parse(&source).unwrap();
        let error = lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE).unwrap_err();
        assert!(
            error.to_string().contains("unsupported for Str operands"),
            "{error}"
        );
    }
}

#[test]
fn finite_decision_checks_helper_body_with_callsite_types_but_keeps_int_and_equality() {
    let bad = r#"
input value: Str
fn hidden(a, b) = a + b
config Result = Ok
propose keep() priority 1 when true = Ok
commit results from (keep)
show hidden(value, "x")
"#;
    let error =
        lower_finite_decision_plan(&parse(bad).unwrap(), FINITE_DECISION_PROFILE).unwrap_err();
    assert!(error.to_string().contains("operator '+'"), "{error}");

    let good = r#"
input value: Int
fn hidden(a, b) = a + b
config Result = Ok
propose keep() priority 1 when value < 10 = Ok
commit results from (keep)
show hidden(value, 2)
show value == 3
show value != 4
show "same" == "same"
show "left" != "right"
"#;
    lower_finite_decision_plan(&parse(good).unwrap(), FINITE_DECISION_PROFILE)
        .expect("integer arithmetic/ordering and string-safe equality remain admitted");
}

#[test]
fn finite_decision_defers_unknown_untyped_helpers_until_a_callsite() {
    let dead = r#"
fn dormant_add(value) = value + "!"
fn dormant_less(value) = value < "z"
config Result = Ok
propose keep() priority 1 when true = Ok
commit results from (keep)
"#;
    lower_finite_decision_plan(&parse(dead).unwrap(), FINITE_DECISION_PROFILE)
        .expect("uncalled untyped helpers have no String call-site shape");

    let int_call = r#"
input value: Int
fn mixed(value) = value + "!"
config Result = Ok
propose keep() priority 1 when true = Ok
commit results from (keep)
show mixed(value)
"#;
    lower_finite_decision_plan(&parse(int_call).unwrap(), FINITE_DECISION_PROFILE)
        .expect("an Int call-site must not be treated as Str + Str");

    let str_call = r#"
input value: Str
fn append(value) = value + "!"
config Result = Ok
propose keep() priority 1 when true = Ok
commit results from (keep)
show append(value)
"#;
    let error = lower_finite_decision_plan(&parse(str_call).unwrap(), FINITE_DECISION_PROFILE)
        .expect_err("the concrete String call-site must inspect the helper body");
    assert!(error.to_string().contains("operator '+'"), "{error}");
}

#[test]
fn finite_decision_rejects_literal_string_ops_inside_unused_untyped_helpers() {
    let source = r#"
fn dormant(value) = "left" + "right"
config Result = Ok
propose keep() priority 1 when true = Ok
commit results from (keep)
"#;
    let error = lower_finite_decision_plan(&parse(source).unwrap(), FINITE_DECISION_PROFILE)
        .expect_err("literal Str+Str is statically known even without a call-site");
    assert!(error.to_string().contains("operator '+'"), "{error}");
}

#[test]
fn finite_decision_recursion_rechecks_changed_argument_shapes() {
    let source = r#"
config Mode = Go | Stop
config Result = Done
fn recurse(mode, left, right) = match mode {
  Go => recurse(Stop, "left", "right")
  Stop => left + right
}
propose keep() priority 1 when true = Done
commit results from (keep)
show recurse(Go, 1, 2)
"#;
    let error = lower_finite_decision_plan(&parse(source).unwrap(), FINITE_DECISION_PROFILE)
        .expect_err("recursive Int-to-Str argument changes must be analyzed");
    assert!(error.to_string().contains("operator '+'"), "{error}");
}

#[test]
fn world_admission_rejects_string_ops_through_fields_and_untyped_helpers() {
    for bad_expression in ["e.name + \"!\"", "e.name < \"z\"", "hidden(e.name, \"!\")"] {
        let helper = if bad_expression.starts_with("hidden") {
            "fn hidden(a, b) = a + b\n"
        } else {
            ""
        };
        let source = format!(
            "rel input entries: {{ id: Str, name: Str }} key id\n{helper}rel derived result = select {{ value: {bad_expression} }} from e in entries"
        );
        let error = lower_relations(&linked(&source)).unwrap_err();
        assert!(
            error.to_string().contains("unsupported for Str operands"),
            "{error}"
        );
    }
}

#[test]
fn world_admission_checks_nested_match_and_lambda_helper_bodies() {
    let source = r#"
rel input entries: { id: Str, label: Str } key id
config Opt = None | Some(Str)
fn hidden(value) = match Some(value) { Some(x) => x + "!" }
rel derived result = select { value: hidden(r.label) } from r in entries
"#;
    let error = lower_relations(&linked(source)).unwrap_err();
    assert!(error.to_string().contains("operator '+'"), "{error}");

    let lambda = r#"
rel input entries: { id: Str, label: Str } key id
rel derived result = select { values: map([e.label], x => x + "!") }
  from e in entries
"#;
    let error = lower_relations(&linked(lambda)).unwrap_err();
    assert!(error.to_string().contains("operator '+'"), "{error}");

    let mixed_match = r#"
rel input entries: { id: Str, label: Str } key id
config Opt = None | Some(Str)
fn hidden(value) = match Some(value) {
  Some(x) => x
  None => 0
} + "!"
rel derived result = select { value: hidden(e.label) } from e in entries
"#;
    let error = lower_relations(&linked(mixed_match)).unwrap_err();
    assert!(error.to_string().contains("operator '+'"), "{error}");
}

#[test]
fn world_admission_preserves_int_ops_and_string_equality() {
    let source = r#"
rel input entries: { id: Str, value: Int } key id
fn twice(x) = x + x
rel derived result = select { id: r.id, value: twice(r.value) }
  from r in entries where r.id == "ok" and r.value < 10
"#;
    lower_relations(&linked(source)).expect("supported arithmetic and comparisons remain admitted");
}

#[test]
fn world_decision_admission_checks_scoped_string_expressions() {
    let source = r#"
rel input entries: { id: Str } key id
decide choice for e in entries per id {
  propose keep priority 1 when true = e.id + "!"
}
"#;
    let error = lower_relations(&linked(source)).unwrap_err();
    assert!(error.to_string().contains("operator '+'"), "{error}");
}

#[test]
fn imported_helper_uses_its_own_module_for_nested_string_calls() {
    let sources = BTreeMap::from([
        ("root".to_string(), "use helpers\nrel input entries: { id: Str } key id\nrel derived result = select { value: helpers::outer(e.id) } from e in entries".to_string()),
        ("helpers".to_string(), "fn hidden(a) = a + a\nexport fn outer(a) = hidden(a)".to_string()),
    ]);
    let program = ModuleGraph::load(
        "root",
        &|name: &str| sources.get(name).cloned(),
        ModuleLoaderLimits::default(),
    )
    .unwrap()
    .link()
    .unwrap();
    let error = lower_relations(&program).unwrap_err();
    assert!(
        error.to_string().contains("unsupported for Str operands"),
        "{error}"
    );
}

#[test]
fn world_string_analysis_follows_long_helpers_and_changed_recursive_argument_types() {
    let mut chain = "rel input entries: { id: Str } key id\n".to_string();
    for index in 0..70 {
        chain.push_str(&format!("fn f{index}(x) = f{}(x)\n", index + 1));
    }
    chain.push_str(
        "fn f70(x) = x\nrel derived result = select { value: f0(e.id) + \"!\" } from e in entries",
    );
    let error = lower_relations(&linked(&chain)).unwrap_err();
    assert!(
        error.to_string().contains("unsupported for Str operands"),
        "{error}"
    );

    let recursive = r#"
rel input entries: { id: Str } key id
fn changed(x) = match true { true => changed("bad") false => x + x }
rel derived result = select { value: changed(1) } from e in entries
"#;
    let error = lower_relations(&linked(recursive)).unwrap_err();
    assert!(
        error.to_string().contains("unsupported for Str operands"),
        "{error}"
    );
}

#[test]
fn finite_string_analysis_checks_changed_recursive_argument_types() {
    let source = r#"
config Result = Ok
fn changed(x) = match true { true => changed("bad") false => x + x }
propose keep priority 1 when true = Ok
commit result from (keep)
show changed(1)
"#;
    let result = lower_finite_decision_plan(&parse(source).unwrap(), FINITE_DECISION_PROFILE);
    assert!(
        result.is_err(),
        "recursive argument changed to Str escaped admission"
    );
}

#[test]
fn shared_numeric_schema_dag_does_not_expand_during_string_discovery() {
    let mut source = "config Level0 = { value: Int }\n".to_string();
    for index in 1..28 {
        source.push_str(&format!(
            "config Level{index} = {{ left: Level{}, right: Level{} }}\n",
            index - 1,
            index - 1
        ));
    }
    source.push_str("input tree: Level27\nconfig Result = Ok\npropose keep priority 1 when true = Ok\ncommit result from (keep)");
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let module = parse(&source).unwrap();
        sender
            .send(
                lower_finite_decision_plan(&module, FINITE_DECISION_PROFILE)
                    .map(|_| ())
                    .map_err(|error| error.to_string()),
            )
            .unwrap();
    });
    receiver
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("shared schemas must be visited without exponential expansion")
        .unwrap();
}
