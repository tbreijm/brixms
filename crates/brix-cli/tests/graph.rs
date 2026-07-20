//! Issue #42 Slice 1: a locked 2-package graph compiles with cross-package
//! name resolution and executes identically on the oracle and the generated
//! runtime. The root `app` imports a relation and a total function from a
//! dependency `lib` (`use lib.{Widget, scale}`); `lower_graph` merges them into
//! one program with the dependency's exports package-qualified (`lib.Widget`,
//! `lib.scale`), and the dependency's compiled `scale` body runs from source on
//! both engines — no hand-registration.

use brix_ast::parse_file;
use brix_oracle::dsl::row;
use brix_oracle::dump::dump_bytes;
use brix_oracle::frontend::{program_from_source, FnLibrary, KindTable};
use brix_oracle::program::RelKind;
use brix_oracle::store::Store as OracleStore;
use brix_oracle::txn::Transaction as OracleTxn;
use brix_oracle::value::Value;
use brixc::lower::{RuntimeRelationKind, SubmoduleInput};
use brixc::pipeline::PhaseAssign;
use brixc::{lower_graph, AstPhase, DepPackage};

const LIB: &str = "package lib @ 1.0.0\n\
rel Widget { id: Int; n: Int } key(id)\n\
fn scale(x: Int) -> Int = x + x\n";

const APP: &str = "package app @ 0.1.0\n\
use lib.{Widget, scale}\n\
rel Out { id: Int; v: Int } key(id)\n\
derive R: Out(id: i, v: y) from { Widget(id: i, n: x); let y = scale(x) }\n";

fn kinds(lowered: &brixc::Lowered) -> KindTable {
    let mut table = KindTable::new();
    for relation in lowered.resolver.relations() {
        if relation.derived {
            continue;
        }
        let kind = match lowered.resolver.relation_kind(&relation.name) {
            RuntimeRelationKind::Entity => RelKind::Entity,
            RuntimeRelationKind::Ground => RelKind::Ground,
            RuntimeRelationKind::State => RelKind::State,
            RuntimeRelationKind::Event => RelKind::Event,
        };
        table.insert(relation.name.to_string(), kind);
    }
    table
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn lower_app_over_lib() -> brixc::Lowered {
    let (app_file, app_diags) = parse_file(APP);
    let (lib_file, lib_diags) = parse_file(LIB);
    assert!(
        !app_diags.has_errors() && !lib_diags.has_errors(),
        "fixtures parse"
    );
    lower_graph(
        &app_file,
        &app_diags,
        &[DepPackage {
            name_segments: vec!["lib".to_string()],
            file: &lib_file,
            parse_diags: &lib_diags,
            submodules: &[],
        }],
    )
}

#[test]
fn cross_package_graph_lowers_and_resolves_qualified_symbols() {
    let lowered = lower_app_over_lib();
    assert!(
        !lowered.has_errors(),
        "app must lower + check cleanly against lib: {:#?}",
        lowered.diags
    );
    // The dependency's relation and function are present under package-qualified
    // names, and its `scale` body was carried into the graph.
    assert!(
        lowered
            .resolver
            .relations()
            .any(|r| r.name.to_string() == "lib.Widget"),
        "lib.Widget must be a qualified relation in the merged resolver"
    );
    assert!(
        lowered
            .source
            .functions
            .iter()
            .any(|f| f.name.to_string() == "lib.scale"),
        "lib.scale's compiled body must be merged into the graph, got {:?}",
        lowered
            .source
            .functions
            .iter()
            .map(|f| f.name.to_string())
            .collect::<Vec<_>>()
    );
}

#[test]
fn cross_package_fn_executes_matching_oracle() {
    let lowered = lower_app_over_lib();
    assert!(!lowered.has_errors(), "{:#?}", lowered.diags);
    let phased = AstPhase
        .assign_phases(lowered)
        .expect("graph must be well-stratified");

    // Oracle: empty FnLibrary — `lib.scale` runs from its compiled body.
    let oracle_program = program_from_source(
        &phased.lowered.source,
        &phased.lowered.resolver,
        &kinds(&phased.lowered),
        FnLibrary::new(),
    )
    .expect("graph adapts to the oracle");
    let mut store = OracleStore::new(oracle_program).expect("stratified");
    let settled = store
        .commit(&OracleTxn::new(b"brix-stdin-0".to_vec()).assert(
            "lib.Widget",
            row(&[("id", Value::Int(5)), ("n", Value::Int(3))]),
        ))
        .expect("commits");
    let oracle_hex = hex(&dump_bytes(settled));

    // Generated runtime: same compiled `lib.scale`, driven over the qualified
    // relation name.
    let rt_program = brixc::emit::project_program(&phased);
    let out = brix_rt::engine::run_text(rt_program, "assert lib.Widget id=int:5,n=int:3\n")
        .expect("runtime runs the cross-package graph");
    let rt_hex = out
        .lines()
        .next()
        .and_then(|line| line.split_ascii_whitespace().nth(2))
        .expect("dump line");

    assert_eq!(
        rt_hex, oracle_hex,
        "a cross-package compiled fn must settle identically on both engines"
    );
}

// -- `reimport`: a multi-file dependency's package-root facade --------------
//
// `lib` is itself multi-file (issue #42): its entry declares only `Widget`
// and re-exports `ops`'s whole surface via `reimport ops` (issue-93-style
// redesign), instead of copying `scale`'s body into `world.brix`. `app`
// then reaches `scale` through the *flat* package-root path
// (`use lib.{Widget, scale}`) exactly like a single-file dependency would —
// the nested path (`lib.ops.scale`) stays reachable too, since `reimport`
// only ever promotes, never removes.

const LIB_ROOT_WITH_REIMPORT: &str =
    "package lib @ 1.0.0\nreimport ops\nrel Widget { id: Int; n: Int } key(id)\n";
const LIB_OPS: &str = "fn scale(x: Int) -> Int = x + x\n";

const APP_FLAT_OVER_REIMPORT: &str = "package app @ 0.1.0\n\
use lib.{Widget, scale}\n\
rel Out { id: Int; v: Int } key(id)\n\
derive R: Out(id: i, v: y) from { Widget(id: i, n: x); let y = scale(x) }\n";

#[test]
fn reimport_publishes_a_submodule_export_at_the_dependency_package_root() {
    let (app_file, app_diags) = parse_file(APP_FLAT_OVER_REIMPORT);
    let (lib_file, lib_diags) = parse_file(LIB_ROOT_WITH_REIMPORT);
    let (ops_file, ops_diags) = parse_file(LIB_OPS);
    assert!(
        !app_diags.has_errors() && !lib_diags.has_errors() && !ops_diags.has_errors(),
        "fixtures parse"
    );

    let submodules = vec![SubmoduleInput {
        qualifier: "ops".to_string(),
        file: &ops_file,
        parse_diags: &ops_diags,
    }];
    let lowered = lower_graph(
        &app_file,
        &app_diags,
        &[DepPackage {
            name_segments: vec!["lib".to_string()],
            file: &lib_file,
            parse_diags: &lib_diags,
            submodules: &submodules,
        }],
    );

    assert!(
        !lowered.has_errors(),
        "app must resolve `scale` through lib's reimport facade: {:#?}",
        lowered.diags
    );
    let names: Vec<String> = lowered
        .source
        .functions
        .iter()
        .map(|f| f.name.to_string())
        .collect();
    assert!(
        names.contains(&"lib.scale".to_string()),
        "the reimported package-root name must be published, got {names:?}"
    );
    assert!(
        names.contains(&"lib.ops.scale".to_string()),
        "the nested path must remain reachable too — reimport promotes, never removes, got {names:?}"
    );
}
