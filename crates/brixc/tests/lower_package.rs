//! Issue #42: multi-file packages. `lower_package` merges a package's entry
//! file with local `src/<name>.brix` submodules into one checked program,
//! with each submodule's decls qualified under its file stem.

use brix_ast::parse_file;
use brixc::lower::{lower_package, SubmoduleInput};

const ENTRY: &str = "package brix.mathtest @ 0.1.0\nmodule MathTest\n";

const ORDER: &str = "fn min(a: Int, b: Int) -> Int = if a < b then a else b\n\
fn max(a: Int, b: Int) -> Int = if a > b then a else b\n\
fn clamp(x: Int, lo: Int, hi: Int) -> Int = min(max(x, lo), hi)\n";

const INTERP: &str = "fn mix(a: Int, b: Int, t: Int) -> Int = clamp(t, a, b)\n";

fn parse(src: &str) -> (brix_ast::File, brix_ast::Diagnostics) {
    parse_file(src)
}

fn diag_codes(lowered: &brixc::lower::PackageLowered) -> Vec<&'static str> {
    lowered
        .reports
        .iter()
        .flat_map(|r| r.diagnostics.iter().map(|d| d.code))
        .collect()
}

#[test]
fn submodule_decls_are_qualified_and_cross_module_calls_resolve() {
    let (entry_file, entry_diags) = parse(ENTRY);
    let (order_file, order_diags) = parse(ORDER);
    let (interp_file, interp_diags) = parse(INTERP);

    let submodules = vec![
        SubmoduleInput {
            qualifier: "order".to_string(),
            file: &order_file,
            parse_diags: &order_diags,
        },
        SubmoduleInput {
            qualifier: "interp".to_string(),
            file: &interp_file,
            parse_diags: &interp_diags,
        },
    ];

    let lowered = lower_package(&entry_file, &entry_diags, "src/world.brix", &submodules);
    for report in &lowered.reports {
        for d in &report.diagnostics {
            eprintln!("{}: {} {}", report.label, d.code, d.message);
        }
    }
    assert!(!lowered.has_errors(), "expected a clean multi-file lower");

    let names: Vec<String> = lowered
        .source
        .functions
        .iter()
        .map(|f| f.name.to_string())
        .collect();
    assert!(names.contains(&"order.min".to_string()));
    assert!(names.contains(&"order.max".to_string()));
    assert!(names.contains(&"order.clamp".to_string()));
    // `interp.mix`'s body calls bare `clamp(...)` — the auto-imported alias
    // to `order.clamp` — and lowers to a real, checked function.
    assert!(names.contains(&"interp.mix".to_string()));
}

#[test]
fn reordering_submodules_does_not_change_the_result() {
    let (entry_file, entry_diags) = parse(ENTRY);
    let (order_file, order_diags) = parse(ORDER);
    let (interp_file, interp_diags) = parse(INTERP);

    let forward = vec![
        SubmoduleInput {
            qualifier: "order".to_string(),
            file: &order_file,
            parse_diags: &order_diags,
        },
        SubmoduleInput {
            qualifier: "interp".to_string(),
            file: &interp_file,
            parse_diags: &interp_diags,
        },
    ];
    let backward = vec![
        SubmoduleInput {
            qualifier: "interp".to_string(),
            file: &interp_file,
            parse_diags: &interp_diags,
        },
        SubmoduleInput {
            qualifier: "order".to_string(),
            file: &order_file,
            parse_diags: &order_diags,
        },
    ];

    let a = lower_package(&entry_file, &entry_diags, "src/world.brix", &forward);
    let b = lower_package(&entry_file, &entry_diags, "src/world.brix", &backward);
    assert!(!a.has_errors());
    assert!(!b.has_errors());

    let names = |p: &brixc::lower::PackageLowered| -> Vec<String> {
        let mut v: Vec<String> = p
            .source
            .functions
            .iter()
            .map(|f| f.name.to_string())
            .collect();
        v.sort();
        v
    };
    assert_eq!(names(&a), names(&b));
}

#[test]
fn typed_overloads_sharing_one_bare_name_in_a_single_submodule_are_not_a_duplicate_export() {
    let (entry_file, entry_diags) = parse(ENTRY);
    // Two `min` overloads (Int and Float) in the *same* submodule file — a
    // bare-name claim is per-file, not per-decl, so this must lower clean,
    // not trip `BRX-PKG-0002` against itself.
    const OVERLOADED: &str = "fn min(a: Int, b: Int) -> Int = if a < b then a else b\n\
fn min(a: Float, b: Float) -> Float = if a < b then a else b\n";
    let (order_file, order_diags) = parse(OVERLOADED);

    let submodules = vec![SubmoduleInput {
        qualifier: "order".to_string(),
        file: &order_file,
        parse_diags: &order_diags,
    }];

    let lowered = lower_package(&entry_file, &entry_diags, "src/world.brix", &submodules);
    assert!(
        !lowered.has_errors(),
        "same-file overloads must not self-collide: {:?}",
        lowered
            .reports
            .iter()
            .flat_map(|r| r.diagnostics.iter().map(|d| d.code))
            .collect::<Vec<_>>()
    );
    let names: Vec<String> = lowered
        .source
        .functions
        .iter()
        .map(|f| f.name.to_string())
        .collect();
    assert!(names.contains(&"order.min".to_string()));
    assert_eq!(
        names.iter().filter(|n| n.as_str() == "order.min").count(),
        2,
        "both overloads must lower, not just the first"
    );
}

#[test]
fn duplicate_export_across_modules_is_a_stable_diagnostic() {
    let (entry_file, entry_diags) = parse(ENTRY);
    let (order_file, order_diags) = parse(ORDER);
    // A second module that also declares `clamp` — colliding with `order`'s.
    const CLASH: &str = "fn clamp(x: Int, lo: Int, hi: Int) -> Int = x\n";
    let (clash_file, clash_diags) = parse(CLASH);

    let submodules = vec![
        SubmoduleInput {
            qualifier: "order".to_string(),
            file: &order_file,
            parse_diags: &order_diags,
        },
        SubmoduleInput {
            qualifier: "zzz".to_string(),
            file: &clash_file,
            parse_diags: &clash_diags,
        },
    ];

    let lowered = lower_package(&entry_file, &entry_diags, "src/world.brix", &submodules);
    assert!(lowered.has_errors());
    let zzz_report = lowered
        .reports
        .iter()
        .find(|r| r.label == "src/zzz.brix")
        .expect("zzz report present");
    assert!(zzz_report
        .diagnostics
        .iter()
        .any(|d| d.code == "BRX-PKG-0002"));
}

#[test]
fn package_decl_outside_the_entry_file_is_rejected() {
    let (entry_file, entry_diags) = parse(ENTRY);
    const BAD: &str = "package sneaky @ 0.1.0\nfn identity(x: Int) -> Int = x\n";
    let (bad_file, bad_diags) = parse(BAD);

    let submodules = vec![SubmoduleInput {
        qualifier: "bad".to_string(),
        file: &bad_file,
        parse_diags: &bad_diags,
    }];

    let lowered = lower_package(&entry_file, &entry_diags, "src/world.brix", &submodules);
    assert!(lowered.has_errors());
    let bad_report = lowered
        .reports
        .iter()
        .find(|r| r.label == "src/bad.brix")
        .expect("bad report present");
    assert!(bad_report
        .diagnostics
        .iter()
        .any(|d| d.code == "BRX-PKG-0001"));
}

// -- `reimport` (redesign: reimport + use-as aliases) -----------------------

const ENTRY_WITH_REIMPORT_ALL: &str =
    "package brix.mathtest @ 0.1.0\nmodule MathTest\nreimport order\n";
const ENTRY_WITH_REIMPORT_SOME: &str =
    "package brix.mathtest @ 0.1.0\nmodule MathTest\nreimport order.{min, clamp}\n";

#[test]
fn bare_reimport_promotes_every_export_of_the_target_submodule() {
    let (entry_file, entry_diags) = parse(ENTRY_WITH_REIMPORT_ALL);
    let (order_file, order_diags) = parse(ORDER);
    let submodules = vec![SubmoduleInput {
        qualifier: "order".to_string(),
        file: &order_file,
        parse_diags: &order_diags,
    }];

    let lowered = lower_package(&entry_file, &entry_diags, "src/world.brix", &submodules);
    assert!(!lowered.has_errors(), "{:?}", diag_codes(&lowered));
    assert_eq!(
        lowered.reexports.get("min").map(|q| q.to_string()),
        Some("order.min".to_string())
    );
    assert_eq!(
        lowered.reexports.get("max").map(|q| q.to_string()),
        Some("order.max".to_string())
    );
    assert_eq!(
        lowered.reexports.get("clamp").map(|q| q.to_string()),
        Some("order.clamp".to_string())
    );
}

#[test]
fn selective_reimport_promotes_only_the_listed_names() {
    let (entry_file, entry_diags) = parse(ENTRY_WITH_REIMPORT_SOME);
    let (order_file, order_diags) = parse(ORDER);
    let submodules = vec![SubmoduleInput {
        qualifier: "order".to_string(),
        file: &order_file,
        parse_diags: &order_diags,
    }];

    let lowered = lower_package(&entry_file, &entry_diags, "src/world.brix", &submodules);
    assert!(!lowered.has_errors(), "{:?}", diag_codes(&lowered));
    assert_eq!(
        lowered.reexports.get("min").map(|q| q.to_string()),
        Some("order.min".to_string())
    );
    assert_eq!(
        lowered.reexports.get("clamp").map(|q| q.to_string()),
        Some("order.clamp".to_string())
    );
    assert!(
        !lowered.reexports.contains_key("max"),
        "`max` was not listed, so it must not be published at the package root"
    );
}

#[test]
fn reimport_of_an_unknown_submodule_is_a_clean_diagnostic() {
    const ENTRY_BAD: &str = "package brix.mathtest @ 0.1.0\nmodule MathTest\nreimport bogus\n";
    let (entry_file, entry_diags) = parse(ENTRY_BAD);
    let (order_file, order_diags) = parse(ORDER);
    let submodules = vec![SubmoduleInput {
        qualifier: "order".to_string(),
        file: &order_file,
        parse_diags: &order_diags,
    }];

    let lowered = lower_package(&entry_file, &entry_diags, "src/world.brix", &submodules);
    assert!(lowered.has_errors());
    let entry_report = &lowered.reports[0];
    assert!(entry_report
        .diagnostics
        .iter()
        .any(|d| d.code == "BRX-PKG-0003"));
    assert!(
        lowered.reexports.is_empty(),
        "an unknown target must not publish anything"
    );
}

#[test]
fn reimport_of_an_unknown_item_is_rejected_but_the_known_items_still_publish() {
    const ENTRY_MIXED: &str =
        "package brix.mathtest @ 0.1.0\nmodule MathTest\nreimport order.{min, nope}\n";
    let (entry_file, entry_diags) = parse(ENTRY_MIXED);
    let (order_file, order_diags) = parse(ORDER);
    let submodules = vec![SubmoduleInput {
        qualifier: "order".to_string(),
        file: &order_file,
        parse_diags: &order_diags,
    }];

    let lowered = lower_package(&entry_file, &entry_diags, "src/world.brix", &submodules);
    assert!(lowered.has_errors());
    let entry_report = &lowered.reports[0];
    assert!(entry_report
        .diagnostics
        .iter()
        .any(|d| d.code == "BRX-PKG-0003"));
    assert_eq!(
        lowered.reexports.get("min").map(|q| q.to_string()),
        Some("order.min".to_string()),
        "the one valid item in the same reimport must still publish"
    );
    assert!(!lowered.reexports.contains_key("nope"));
}

#[test]
fn reimport_outside_the_entry_file_is_rejected() {
    let (entry_file, entry_diags) = parse(ENTRY);
    const SNEAKY_REIMPORT: &str = "reimport order\nfn identity(x: Int) -> Int = x\n";
    let (sneaky_file, sneaky_diags) = parse(SNEAKY_REIMPORT);
    let (order_file, order_diags) = parse(ORDER);

    let submodules = vec![
        SubmoduleInput {
            qualifier: "order".to_string(),
            file: &order_file,
            parse_diags: &order_diags,
        },
        SubmoduleInput {
            qualifier: "sneaky".to_string(),
            file: &sneaky_file,
            parse_diags: &sneaky_diags,
        },
    ];

    let lowered = lower_package(&entry_file, &entry_diags, "src/world.brix", &submodules);
    assert!(lowered.has_errors());
    let sneaky_report = lowered
        .reports
        .iter()
        .find(|r| r.label == "src/sneaky.brix")
        .expect("sneaky report present");
    assert!(sneaky_report
        .diagnostics
        .iter()
        .any(|d| d.code == "BRX-PKG-0004"));
}

#[test]
fn reimport_colliding_with_the_entry_files_own_declaration_is_a_hard_error() {
    const ENTRY_CLASHING: &str =
        "package brix.mathtest @ 0.1.0\nmodule MathTest\nreimport order.{min}\nfn min(x: Int) -> Int = x\n";
    let (entry_file, entry_diags) = parse(ENTRY_CLASHING);
    let (order_file, order_diags) = parse(ORDER);
    let submodules = vec![SubmoduleInput {
        qualifier: "order".to_string(),
        file: &order_file,
        parse_diags: &order_diags,
    }];

    let lowered = lower_package(&entry_file, &entry_diags, "src/world.brix", &submodules);
    assert!(lowered.has_errors());
    let entry_report = &lowered.reports[0];
    assert!(entry_report
        .diagnostics
        .iter()
        .any(|d| d.code == "BRX-PKG-0005"));
    assert!(
        !lowered.reexports.contains_key("min"),
        "a rejected reimport must not publish"
    );
}
