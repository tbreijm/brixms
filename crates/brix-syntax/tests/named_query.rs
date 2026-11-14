use brix_syntax::{ast::Item, parse};

#[test]
fn parses_typed_named_query_as_additive_item() {
    let module = parse(
        r#"
        rel input catalogue_entries: { id: Str, kind: Str } key id
        query catalogue(kind: Str) = select { id: c.id, kind: c.kind }
          from c in catalogue_entries where c.kind == kind
    "#,
    )
    .expect("named query parses");
    let Item::Query(query) = &module.items[1] else {
        panic!("query item expected")
    };
    assert_eq!(query.name, "catalogue");
    assert_eq!(query.params.len(), 1);
    assert_eq!(query.params[0].name, "kind");
    assert_eq!(query.query.from[0].relation, "catalogue_entries");
    assert!(parse("query q(kind: Str, kind: Int) = select {x: a.x} from a").is_err());
}

#[test]
fn named_query_params_cannot_shadow_bindings_or_host_names() {
    assert!(parse("query q(a: Str) = select {x: a.x} from a in rows").is_err());
    assert!(parse(
        "query q(blocked: Str) = select {x: a.x} from a in rows where not exists blocked in rows where blocked.id == a.id"
    )
    .is_err());
    let reserved = [
        "parent",
        "program",
        "world",
        "namespace",
        "tenant",
        "principal",
        "capability_view",
        "scenario_mode",
        "branch",
        "valid_time_start",
        "valid_time_end",
        "transaction_time",
        "correction_time",
        "simulation_time",
        "observation_profile",
        "redaction_profile",
        "kernel_profile",
        "regime_profile",
        "axiom_profile",
        "validity_start",
        "validity_end",
        "step_budget",
        "depth_budget",
    ];
    for name in reserved {
        let query = format!("query q({name}: Str) = select {{x: a.x}} from a in rows");
        assert!(
            parse(&query).is_err(),
            "reserved context name accepted: {name}"
        );
    }
}
