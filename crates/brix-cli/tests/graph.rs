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
use brixc::{lower_file, lower_graph, merge_files, AstPhase, DepPackage};

const LIB: &str = "package lib @ 1.0.0\n\
pub rel Widget { id: Int; n: Int } key(id)\n\
pub fn scale(x: Int) -> Int = x + x\n";

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

const LIB_A: &str = "package a @ 1.0.0\npub rel Widget { id: Int } key(id)\n";
const LIB_B: &str = "package b @ 1.0.0\npub rel Widget { id: Int } key(id)\n";

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

// --- Issue #42 Slice 3: broader cross-package exports ------------------
//
// Slice 1 only re-exported single-name relations + total fns. A dependency's
// enums, `type` aliases, and protocol-synth relations (dotted names) now also
// cross the package boundary under package-qualified names, so a root can
// `use dep.{Colour, Meters, Assign, ...}` and get real nominal types + a
// protocol.

const LIB_BROAD: &str = "package lib @ 1.0.0\n\
pub enum Colour { Red; Green }\n\
pub type Meters = Int\n\
pub rel Widget { id: Int; c: Colour; len: Meters } key(id)\n\
pub protocol Assign { request { id: Int } key(id) outcome Chosen { who: Int } }\n\
pub fn scale(x: Meters) -> Meters = x + x\n";

const APP_BROAD: &str = "package app @ 0.1.0\n\
use lib.{Widget, Colour, Meters, Assign, scale}\n\
rel Out { id: Int; c: Colour; v: Meters } key(id)\n\
derive R: Out(id: i, c: k, v: y) from { Widget(id: i, c: k, len: x); let y = scale(x) }\n";

fn lower_over_lib(lib_src: &str) -> brixc::Lowered {
    let (app_file, app_diags) = parse_file(APP_BROAD);
    let (lib_file, lib_diags) = parse_file(lib_src);
    assert!(
        !app_diags.has_errors() && !lib_diags.has_errors(),
        "broad-export fixtures parse"
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
fn dependency_enum_alias_and_protocol_are_exported_qualified() {
    let lowered = lower_over_lib(LIB_BROAD);
    assert!(
        !lowered.has_errors(),
        "app must lower cleanly using the dependency's enum/alias/protocol: {:#?}",
        lowered.diags
    );

    // Enum + alias arrive under package-qualified names (Slice 1 dropped these
    // entirely — only relations/fns crossed the boundary).
    assert!(
        lowered
            .resolver
            .enums()
            .any(|(n, _)| n.to_string() == "lib.Colour"),
        "lib.Colour must be a qualified enum in the merged resolver"
    );
    assert!(
        lowered
            .resolver
            .aliases()
            .any(|(n, _)| n.to_string() == "lib.Meters"),
        "lib.Meters must be a qualified alias in the merged resolver"
    );

    // Protocol-synth relations (dotted names) are now qualified, not skipped.
    for want in ["lib.Assign.request", "lib.Assign.Chosen"] {
        assert!(
            lowered
                .resolver
                .relations()
                .any(|r| r.name.to_string() == want),
            "{want} must be a qualified protocol relation, got {:?}",
            lowered
                .resolver
                .relations()
                .map(|r| r.name.to_string())
                .collect::<Vec<_>>()
        );
    }
}

/// Determinism (issue acceptance): reordering a dependency's declarations must
/// not change which qualified symbols the graph exports.
#[test]
fn broader_exports_are_order_independent() {
    let reordered = "package lib @ 1.0.0\n\
pub fn scale(x: Meters) -> Meters = x + x\n\
pub protocol Assign { request { id: Int } key(id) outcome Chosen { who: Int } }\n\
pub type Meters = Int\n\
pub rel Widget { id: Int; c: Colour; len: Meters } key(id)\n\
pub enum Colour { Red; Green }\n";

    let names = |lowered: &brixc::Lowered| -> Vec<String> {
        let mut out: Vec<String> = lowered
            .resolver
            .relations()
            .map(|r| r.name.to_string())
            .chain(lowered.resolver.enums().map(|(n, _)| n.to_string()))
            .chain(lowered.resolver.aliases().map(|(n, _)| n.to_string()))
            .collect();
        out.sort();
        out
    };

    let a = lower_over_lib(LIB_BROAD);
    let b = lower_over_lib(reordered);
    assert!(!a.has_errors() && !b.has_errors());
    assert_eq!(
        names(&a),
        names(&b),
        "reordering dependency decls must not change the exported symbol set"
    );
}

// --- Issue #42 Slice 4: multi-file / multi-module packages -------------
//
// A package may span several `src/**/*.brix` files, each with its own `module`
// header. In the flat-namespace model the files share one declaration space:
// `merge_files` concatenates them (caller sorts by path for determinism) and a
// nominal decl declared in two files is a duplicate export (BRX-LOW-0015).

const MOD_CORE: &str = "package multi @ 0.1.0\n\
module Core\n\
rel Widget { id: Int; n: Int } key(id)\n";

const MOD_API: &str = "package multi @ 0.1.0\n\
module Api\n\
rel Out { id: Int; v: Int } key(id)\n\
derive R: Out(id: i, v: x) from { Widget(id: i, n: x) }\n";

fn parse_ok(src: &str) -> (brix_ast::File, brix_ast::Diagnostics) {
    let (file, diags) = parse_file(src);
    assert!(!diags.has_errors(), "fixture parses: {:#?}", diags);
    (file, diags)
}

#[test]
fn multi_file_package_shares_a_flat_namespace() {
    // `Api` references `Widget` declared in `Core`; after merge they lower as
    // one program with no imports needed.
    let (core, _) = parse_ok(MOD_CORE);
    let (api, _) = parse_ok(MOD_API);
    let (merged, dups) = merge_files(&[&core, &api]);
    assert!(dups.is_empty(), "no duplicates expected: {:#?}", dups);

    let (_, parse_diags) = parse_file(MOD_CORE);
    let lowered = lower_file(&merged, &parse_diags);
    assert!(
        !lowered.has_errors(),
        "a cross-file reference must resolve in the flat namespace: {:#?}",
        lowered.diags
    );
    assert!(
        lowered
            .resolver
            .relations()
            .any(|r| r.name.to_string() == "Widget"),
        "Widget from the other file must be in the merged program"
    );
}

#[test]
fn merge_is_independent_of_file_order() {
    let (core, _) = parse_ok(MOD_CORE);
    let (api, _) = parse_ok(MOD_API);
    let a = merge_files(&[&core, &api]).0;
    let b = merge_files(&[&api, &core]).0;
    // Same decl set regardless of order (the caller's path sort fixes the
    // canonical order; here we just assert order-independence of the set).
    let names = |f: &brix_ast::File| {
        let mut n: Vec<String> = f.decls.iter().map(|d| format!("{:?}", d.span())).collect();
        n.sort();
        n
    };
    assert_eq!(names(&a), names(&b));
}

#[test]
fn duplicate_nominal_decl_across_files_is_flagged() {
    let (core, _) = parse_ok(MOD_CORE);
    let dup = "package multi @ 0.1.0\nmodule Other\nrel Widget { id: Int; n: Int } key(id)\n";
    let (other, _) = parse_ok(dup);
    let (_, dups) = merge_files(&[&core, &other]);
    assert!(
        dups.iter().any(|d| d.code == "BRX-LOW-0015"),
        "a nominal decl declared in two files must be flagged: {:#?}",
        dups
    );
    // Both occurrences are flagged (deterministic, one per site).
    assert_eq!(
        dups.iter().filter(|d| d.code == "BRX-LOW-0015").count(),
        2,
        "every occurrence of the duplicate name is reported"
    );
}

#[test]
fn function_overload_across_files_is_not_a_duplicate() {
    let a = "package m @ 0.1.0\nmodule A\nfn f(x: Int) -> Int = x\n";
    let b = "package m @ 0.1.0\nmodule B\nfn f(x: Float) -> Float = x\n";
    let (fa, _) = parse_ok(a);
    let (fb, _) = parse_ok(b);
    let (_, dups) = merge_files(&[&fa, &fb]);
    assert!(
        dups.is_empty(),
        "a repeated `fn` name is an overload, not a duplicate export: {:#?}",
        dups
    );
}

// --- Issue #42 Slice 5: richer / attributed diagnostics ---------------

#[test]
fn function_type_error_carries_a_real_span() {
    // `f` takes one argument; calling it with two is a function-named type
    // error. render_type_error maps the function back to its decl span, so the
    // diagnostic no longer degrades to 0:0.
    let src = "package t @ 0.1.0\n\
rel In { id: Int; n: Int } key(id)\n\
rel Out { id: Int; v: Int } key(id)\n\
fn f(x: Int) -> Int = x\n\
derive R: Out(id: i, v: y) from { In(id: i, n: x); let y = f(x, x) }\n";
    let (file, diags) = parse_file(src);
    assert!(!diags.has_errors(), "fixture parses: {:#?}", diags);
    let lowered = lower_file(&file, &diags);
    let type_err = lowered
        .diags
        .iter()
        .find(|d| d.code == "BRX-IR-0005")
        .expect("an arity/overload type error is expected");
    assert!(
        !(type_err.span.start == 0 && type_err.span.end == 0),
        "a function-named type error must carry a real span, got {:?}",
        type_err.span
    );
}

#[test]
fn dependency_diagnostics_are_attributed_to_their_package() {
    let lib_src = "package lib @ 1.0.0\nrel Bad { x: Nonexistent } key(x)\n";
    let (lib_file, lib_diags) = parse_file(lib_src);
    assert!(
        !lib_diags.has_errors(),
        "dep parses cleanly (error is semantic)"
    );
    let app_src = "package app @ 0.1.0\nrel Out { id: Int } key(id)\n";
    let (app_file, app_diags) = parse_file(app_src);
    assert!(!app_diags.has_errors());

    let lowered = lower_graph(
        &app_file,
        &app_diags,
        &[DepPackage {
            name_segments: vec!["lib".to_string()],
            file: &lib_file,
            parse_diags: &lib_diags,
        }],
    );
    let dep_diag = lowered
        .diags
        .iter()
        .find(|d| d.source_id.as_deref() == Some("lib"))
        .expect("dependency diagnostic carried source_id `lib`");
    assert!(
        dep_diag.span.start > 0,
        "dependency diagnostic retains its real span inside lib"
    );

    let mut sources = brix_diag::SourceMap::new();
    sources.insert("app.brix", app_src);
    sources.insert("lib", lib_src);

    let rendered = brix_diag::Diagnostics::from_items(lowered.diags)
        .render_compact_map(&sources, "app.brix");
    assert!(
        rendered.contains("lib:2:14: error"),
        "multi-source rendering formats carets against dependency source line:col, got:\n{rendered}"
    );
}

// --- Issue #108: pub/visibility tests ----------------------------------

#[test]
fn private_declarations_in_dependency_are_not_exported_and_cause_brx_low_0016() {
    let lib_src = "package lib @ 1.0.0\n\
rel Secret { id: Int } key(id)\n\
pub rel PublicWidget { id: Int } key(id)\n";
    let app_src = "package app @ 0.1.0\nuse lib.{Secret}\n";
    let (lib_file, lib_diags) = parse_file(lib_src);
    let (app_file, app_diags) = parse_file(app_src);
    assert!(!lib_diags.has_errors() && !app_diags.has_errors());

    let lowered = lower_graph(
        &app_file,
        &app_diags,
        &[DepPackage {
            name_segments: vec!["lib".to_string()],
            file: &lib_file,
            parse_diags: &lib_diags,
        }],
    );
    assert!(
        has_code(&lowered, "BRX-LOW-0016"),
        "importing private declaration `Secret` must trigger BRX-LOW-0016, got: {:#?}",
        lowered.diags
    );
}

#[test]
fn relation_visibility_qualifiers_parse_and_format() {
    use brix_ast::format_file;
    let src = "package p @ 1.0.0\n\
pub read rel R { id: Int } key(id)\n\
pub write rel W { id: Int } key(id)\n\
pub derive rel D { id: Int } key(id)\n";
    let (file, diags) = parse_file(src);
    assert!(!diags.has_errors());
    let formatted = format_file(&file);
    let (file2, diags2) = parse_file(&formatted);
    assert!(!diags2.has_errors());
    assert_eq!(format_file(&file2), formatted);
}

// --- Issue #110: cross-package entity-typed roles ----------------------

#[test]
fn cross_package_entity_typed_roles_resolve_and_check_cleanly() {
    let lib_src = "package lib @ 1.0.0\n\
pub entity Account { id: Int }\n\
pub rel Balance { acc: Account; amount: Int } key(acc)\n";
    let app_src = "package app @ 0.1.0\n\
use lib.{Account, Balance}\n\
rel HighBalance { acc: Account; amount: Int } key(acc)\n\
derive R: HighBalance(acc: a, amount: v) from { Balance(acc: a, amount: v); when v > 100 }\n";
    let (lib_file, lib_diags) = parse_file(lib_src);
    let (app_file, app_diags) = parse_file(app_src);
    assert!(!lib_diags.has_errors() && !app_diags.has_errors());

    let lowered = lower_graph(
        &app_file,
        &app_diags,
        &[DepPackage {
            name_segments: vec!["lib".to_string()],
            file: &lib_file,
            parse_diags: &lib_diags,
        }],
    );
    assert!(
        !lowered.has_errors(),
        "cross-package entity-typed role must lower cleanly: {:#?}",
        lowered.diags
    );
}
