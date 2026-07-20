//! `use ... as Alias` (redesign: reimport + use-as aliases). Two
//! dependencies, `a` and `b`, each declare their own `min` — genuinely
//! different bodies — so the root package must bind each under a distinct
//! local alias instead of colliding on the bare name (`use a.{min} as A`,
//! `use b.{min} as B`, or the prefix form `use a as X`). Proven by
//! inspecting each call's *resolved* Core IR target, not just a clean
//! lower: if aliasing collapsed onto the wrong dependency, the call target
//! would silently point at the wrong package.

use brix_ast::parse_file;
use brix_ir::core::ExprKind;
use brixc::{lower_graph, DepPackage, Lowered};

const DEP_A: &str = "package a @ 0.1.0\nfn min(x: Int, y: Int) -> Int = x + 1000\n";
const DEP_B: &str = "package b @ 0.1.0\nfn min(x: Int, y: Int) -> Int = y + 2000\n";

fn deps<'a>(
    a_file: &'a brix_ast::File,
    a_diags: &'a brix_ast::Diagnostics,
    b_file: &'a brix_ast::File,
    b_diags: &'a brix_ast::Diagnostics,
) -> Vec<DepPackage<'a>> {
    vec![
        DepPackage {
            name_segments: vec!["a".to_string()],
            file: a_file,
            parse_diags: a_diags,
            submodules: &[],
        },
        DepPackage {
            name_segments: vec!["b".to_string()],
            file: b_file,
            parse_diags: b_diags,
            submodules: &[],
        },
    ]
}

fn lower(app_src: &str) -> Lowered {
    let (app_file, app_diags) = parse_file(app_src);
    let (a_file, a_diags) = parse_file(DEP_A);
    let (b_file, b_diags) = parse_file(DEP_B);
    lower_graph(
        &app_file,
        &app_diags,
        &deps(&a_file, &a_diags, &b_file, &b_diags),
    )
}

fn call_target(lowered: &Lowered, fn_name: &str) -> String {
    let f = lowered
        .source
        .functions
        .iter()
        .find(|f| f.name.to_string() == fn_name)
        .unwrap_or_else(|| panic!("`{fn_name}` must lower into a compiled function"));
    match f.body.kind.as_ref() {
        ExprKind::Call { func, .. } => func.to_string(),
        _ => panic!("expected `{fn_name}`'s body to be a direct call"),
    }
}

#[test]
fn selective_use_as_lets_identical_bare_names_coexist_under_distinct_aliases() {
    const APP: &str = "package app @ 0.1.0\n\
use a.{min} as A\n\
use b.{min} as B\n\
fn use_a(x: Int) -> Int = A.min(x, x)\n\
fn use_b(x: Int) -> Int = B.min(x, x)\n";

    let lowered = lower(APP);
    assert!(!lowered.has_errors(), "{:#?}", lowered.diags);
    assert_eq!(call_target(&lowered, "use_a"), "a.min");
    assert_eq!(call_target(&lowered, "use_b"), "b.min");
}

#[test]
fn prefix_use_as_renames_the_whole_module_alias() {
    const APP: &str = "package app @ 0.1.0\n\
use a as X\n\
use b as Y\n\
fn use_a(x: Int) -> Int = X.min(x, x)\n\
fn use_b(x: Int) -> Int = Y.min(x, x)\n";

    let lowered = lower(APP);
    assert!(!lowered.has_errors(), "{:#?}", lowered.diags);
    assert_eq!(call_target(&lowered, "use_a"), "a.min");
    assert_eq!(call_target(&lowered, "use_b"), "b.min");
}

#[test]
fn bare_selective_use_without_as_is_unchanged() {
    // No aliasing involved at all — the pre-existing v0 behavior (bare
    // selective import claims the bare name directly) must still work.
    const APP: &str = "package app @ 0.1.0\nuse a.{min}\nfn use_a(x: Int) -> Int = min(x, x)\n";

    let lowered = lower(APP);
    assert!(!lowered.has_errors(), "{:#?}", lowered.diags);
    assert_eq!(call_target(&lowered, "use_a"), "a.min");
}

#[test]
fn duplicate_use_alias_in_one_file_is_a_hard_error() {
    const APP: &str = "package app @ 0.1.0\n\
use a.{min} as Same\n\
use b.{min} as Same\n\
fn f(x: Int) -> Int = Same.min(x, x)\n";

    let lowered = lower(APP);
    assert!(lowered.has_errors());
    assert!(
        lowered.diags.iter().any(|d| d.code == "BRX-LOW-0014"),
        "{:#?}",
        lowered.diags
    );
}

#[test]
fn use_as_alias_colliding_with_a_bare_use_last_segment_is_a_hard_error() {
    // `use a` alone would default-alias to `a`; `use b as a` then collides
    // with that same local name — one of the two must win deterministically
    // and the other must be flagged, never silently overwritten.
    const APP: &str = "package app @ 0.1.0\n\
use a\n\
use b as a\n\
fn f(x: Int) -> Int = a.min(x, x)\n";

    let lowered = lower(APP);
    assert!(lowered.has_errors());
    assert!(
        lowered.diags.iter().any(|d| d.code == "BRX-LOW-0014"),
        "{:#?}",
        lowered.diags
    );
}
