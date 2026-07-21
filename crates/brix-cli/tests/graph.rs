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
use brixc::lower::RuntimeRelationKind;
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

// --- Issue #42 Slice 2: ambiguous / colliding imports -----------------

const LIB_A: &str = "package a @ 1.0.0\nrel Widget { id: Int } key(id)\n";
const LIB_B: &str = "package b @ 1.0.0\nrel Widget { id: Int } key(id)\n";

fn dep(name: &str, src: &'static str) -> (brix_ast::File, brix_ast::Diagnostics) {
    let (file, diags) = parse_file(src);
    assert!(!diags.has_errors(), "{name} fixture must parse cleanly");
    (file, diags)
}

fn has_code(lowered: &brixc::Lowered, code: &str) -> bool {
    lowered.diags.iter().any(|d| d.code == code)
}

/// Two dependencies (`a`, `b`) both export a relation named `Widget`; the
/// root imports both bare names via two separate `use` items. Neither `use`
/// silently wins — the ambiguity is reported with the stable `BRX-LOW-0014`
/// code (issue #42 Slice 2), not resolved to whichever happened to be
/// processed last.
#[test]
fn two_dependencies_exporting_the_same_bare_name_is_an_ambiguous_import() {
    let (a_file, a_diags) = dep("a", LIB_A);
    let (b_file, b_diags) = dep("b", LIB_B);
    let app_src = "package app @ 0.1.0\nuse a.{Widget}\nuse b.{Widget}\n";
    let (app_file, app_diags) = parse_file(app_src);
    assert!(!app_diags.has_errors());

    let lowered = lower_graph(
        &app_file,
        &app_diags,
        &[
            DepPackage {
                name_segments: vec!["a".to_string()],
                file: &a_file,
                parse_diags: &a_diags,
            },
            DepPackage {
                name_segments: vec!["b".to_string()],
                file: &b_file,
                parse_diags: &b_diags,
            },
        ],
    );
    assert!(
        has_code(&lowered, "BRX-LOW-0014"),
        "expected an ambiguous-import diagnostic, got: {:#?}",
        lowered.diags
    );
}

/// A `use` that imports a bare name already declared locally (the
/// "duplicate export" case) is also flagged — importing `Widget` from `a`
/// while the root itself declares a local `rel Widget` is a name collision,
/// not a silently-shadowed import.
#[test]
fn import_colliding_with_a_local_declaration_is_flagged() {
    let (a_file, a_diags) = dep("a", LIB_A);
    let app_src = "package app @ 0.1.0\nuse a.{Widget}\nrel Widget { id: Int } key(id)\n";
    let (app_file, app_diags) = parse_file(app_src);
    assert!(!app_diags.has_errors());

    let lowered = lower_graph(
        &app_file,
        &app_diags,
        &[DepPackage {
            name_segments: vec!["a".to_string()],
            file: &a_file,
            parse_diags: &a_diags,
        }],
    );
    assert!(
        has_code(&lowered, "BRX-LOW-0014"),
        "expected an ambiguous/duplicate-export diagnostic, got: {:#?}",
        lowered.diags
    );
}

/// Regression guard: a single, non-colliding cross-package import still
/// resolves and lowers cleanly (this is exactly `cross_package_graph_lowers_
/// and_resolves_qualified_symbols`'s app/lib fixture, re-asserted here so the
/// ambiguous-import checks above can never be read as "everything is now
/// flagged").
#[test]
fn non_colliding_import_still_resolves_cleanly() {
    let lowered = lower_app_over_lib();
    assert!(
        !lowered.has_errors(),
        "a non-ambiguous cross-package import must still lower cleanly: {:#?}",
        lowered.diags
    );
    assert!(
        !has_code(&lowered, "BRX-LOW-0014"),
        "a non-colliding import must never be flagged ambiguous: {:#?}",
        lowered.diags
    );
}
