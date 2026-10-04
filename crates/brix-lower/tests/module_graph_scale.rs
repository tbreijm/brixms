//! Scale and qualification test suite for module graph, bounded linker, and invalidation (ADR-0046 P2).

use std::collections::BTreeMap;

use brix_lower::module_graph::{
    compute_affected_modules, ModuleGraph, ModuleLinkError, ModuleLoaderLimits, QualifiedName,
    SizedLoader,
};

#[test]
fn test_100_modules_link_without_flat_name_collisions() {
    // 100 modules that all declare the exact same unexported helper and schema names.
    // They each export a module-specific function.
    let mut sources: BTreeMap<String, String> = BTreeMap::new();
    let mut root_source = String::new();
    let mut calls = Vec::new();

    for i in 0..100 {
        let mod_name = format!("mod_{i}");
        root_source.push_str(&format!("use {mod_name}\n"));
        calls.push(format!("mod_{i}::compute_{i}(x)"));

        // Every module has identical unexported helper & config names:
        let mod_src = format!(
            r#"
config InternalMeta = {{ tag: Str, value: Int }}
fn internal_helper(x: Int): Int = InternalMeta {{ tag: "ok", value: x }}.value + 10
export fn compute_{i}(x: Int): Int = internal_helper(x) + {i}
"#
        );
        sources.insert(mod_name, mod_src);
    }

    root_source.push_str(&format!(
        "fn root_entry(x: Int): Int = {}\n",
        calls.join(" + ")
    ));
    sources.insert("root".to_string(), root_source);

    let loader = |name: &str| sources.get(name).cloned();
    let limits = ModuleLoaderLimits {
        max_import_depth: 32,
        max_import_modules: 256,
        max_module_source_bytes: 1024 * 1024,
        max_total_source_bytes: 8 * 1024 * 1024,
    };

    let graph = ModuleGraph::load("root", &loader, limits).expect("100 modules must load cleanly");
    assert_eq!(graph.modules.len(), 101); // 100 modules + 1 root

    let manifest = graph.manifest("brix.world@1");
    assert_eq!(manifest.modules.len(), 101);

    let linked = graph
        .link()
        .expect("100 modules must link without flat-name collisions");

    // All 100 modules are reachable from root_entry:
    // Verify root function, 100 exported compute functions, 100 private internal_helpers,
    // and 100 private InternalMeta configs coexist without flat-name collisions!
    assert_eq!(linked.functions.len(), 201); // 100 compute + 100 internal_helper + 1 root_entry
    assert_eq!(linked.configs.len(), 100); // 100 InternalMeta configs

    for i in 0..100 {
        let mod_name = format!("mod_{i}");
        assert!(linked
            .functions
            .contains_key(&QualifiedName::new(&mod_name, format!("compute_{i}"))));
        // Verify private helper coexists under its qualified name for each module:
        assert!(linked
            .functions
            .contains_key(&QualifiedName::new(&mod_name, "internal_helper")));
        // Verify private config coexists under its qualified name for each module:
        assert!(linked
            .configs
            .contains_key(&QualifiedName::new(&mod_name, "InternalMeta")));
    }
}

#[test]
fn test_1000_helpers_and_512_schemas_validate() {
    let mut src = String::new();

    // 512 schemas
    for i in 0..512 {
        src.push_str(&format!("config Schema_{i} = {{ id: Int, label: Str }}\n"));
    }

    // 1,000 helpers
    for i in 0..1000 {
        src.push_str(&format!("fn helper_{i}(x: Int): Int = x + {i}\n"));
    }

    src.push_str("fn run(): Int = helper_0(1) + helper_999(2)\n");

    let mut sources = BTreeMap::new();
    sources.insert("large_module".to_string(), src);

    let loader = |name: &str| sources.get(name).cloned();
    let limits = ModuleLoaderLimits {
        max_import_depth: 16,
        max_import_modules: 16,
        max_module_source_bytes: 4 * 1024 * 1024,
        max_total_source_bytes: 8 * 1024 * 1024,
    };

    let graph = ModuleGraph::load("large_module", &loader, limits)
        .expect("1000 helpers and 512 schemas must load");
    let manifest = graph.manifest("brix.world@1");
    assert_eq!(manifest.modules.len(), 1);

    // Call link() to validate contracts across all 1000 helpers and 512 schemas
    let linked = graph
        .link()
        .expect("1000 helpers and 512 schemas must link and validate contracts");
    assert_eq!(linked.functions.len(), 1001); // 1000 helpers + 1 run
    assert_eq!(linked.configs.len(), 512); // 512 schemas

    // Explicitly verify contract validation
    linked
        .validate_contracts()
        .expect("contracts must validate cleanly");
}

#[test]
fn test_invalid_contract_rejected_at_link() {
    // Schema with duplicate field
    let mut sources = BTreeMap::new();
    sources.insert(
        "bad_schema".to_string(),
        r#"
config Bad = { a: Int, a: Str }
fn run(): Int = 1
"#
        .to_string(),
    );
    let loader = |name: &str| sources.get(name).cloned();
    let graph = ModuleGraph::load("bad_schema", &loader, ModuleLoaderLimits::default()).unwrap();
    let err = graph.link().expect_err("duplicate field must be rejected");
    assert!(matches!(err, ModuleLinkError::InvalidSchema { .. }));

    // Helper with duplicate parameter
    let mut sources2 = BTreeMap::new();
    sources2.insert(
        "bad_helper".to_string(),
        r#"
fn bad(x: Int, x: Int): Int = x
fn run(): Int = bad(1, 2)
"#
        .to_string(),
    );
    let loader2 = |name: &str| sources2.get(name).cloned();
    let graph2 = ModuleGraph::load("bad_helper", &loader2, ModuleLoaderLimits::default()).unwrap();
    let err2 = graph2
        .link()
        .expect_err("duplicate parameter must be rejected");
    assert!(matches!(err2, ModuleLinkError::InvalidHelper { .. }));
}

#[test]
fn test_interface_based_invalidation() {
    let mut sources_v1 = BTreeMap::new();
    sources_v1.insert(
        "lib_b".to_string(),
        r#"
fn internal_secret(x: Int): Int = x * 2
export fn add_one(x: Int): Int = x + 1
"#
        .to_string(),
    );
    sources_v1.insert(
        "unrelated_c".to_string(),
        r#"
export fn independent(): Int = 42
"#
        .to_string(),
    );
    sources_v1.insert(
        "app_a".to_string(),
        r#"
use lib_b
use unrelated_c
fn call_b(x: Int): Int = lib_b::add_one(x) + unrelated_c::independent()
"#
        .to_string(),
    );

    let loader_v1 = |name: &str| sources_v1.get(name).cloned();
    let limits = ModuleLoaderLimits::default();

    let graph_v1 = ModuleGraph::load("app_a", &loader_v1, limits).expect("v1 loads");
    let manifest_v1 = graph_v1.manifest("brix.world@1");

    // Case 1: Internal implementation edit in lib_b
    // Signature of add_one(x: Int): Int is unchanged. Internal secret changes.
    let mut sources_v2 = sources_v1.clone();
    sources_v2.insert(
        "lib_b".to_string(),
        r#"
fn internal_secret(x: Int): Int = x * 3 + 7
export fn add_one(x: Int): Int = x + 1
"#
        .to_string(),
    );

    let loader_v2 = |name: &str| sources_v2.get(name).cloned();
    let graph_v2 = ModuleGraph::load("app_a", &loader_v2, limits).expect("v2 loads");
    let manifest_v2 = graph_v2.manifest("brix.world@1");

    let affected_v2 = compute_affected_modules(&manifest_v1, &manifest_v2);
    // lib_b's source changed, but its interface did NOT change.
    // app_a must NOT be affected! unrelated_c must NOT be affected!
    assert_eq!(affected_v2.len(), 1);
    assert!(affected_v2.contains("lib_b"));
    assert!(!affected_v2.contains("app_a"));
    assert!(!affected_v2.contains("unrelated_c"));

    // Case 2: Interface signature change in lib_b
    // Change parameter count of add_one
    let mut sources_v3 = sources_v1.clone();
    sources_v3.insert(
        "lib_b".to_string(),
        r#"
fn internal_secret(x: Int): Int = x * 2
export fn add_one(x: Int, y: Int): Int = x + y + 1
"#
        .to_string(),
    );

    let loader_v3 = |name: &str| sources_v3.get(name).cloned();
    let graph_v3 = ModuleGraph::load("app_a", &loader_v3, limits).expect("v3 loads");
    let manifest_v3 = graph_v3.manifest("brix.world@1");

    let affected_v3 = compute_affected_modules(&manifest_v1, &manifest_v3);
    // Now lib_b (source + interface) and app_a (downstream dependent) are affected,
    // but unrelated_c is STILL NOT affected!
    assert_eq!(affected_v3.len(), 2);
    assert!(affected_v3.contains("lib_b"));
    assert!(affected_v3.contains("app_a"));
    assert!(!affected_v3.contains("unrelated_c"));
}

#[test]
fn test_unused_library_exports_pruned_from_active_plan() {
    let mut sources = BTreeMap::new();
    let mut big_lib_src = String::new();

    // Export 50 functions and 20 configs
    for i in 0..50 {
        big_lib_src.push_str(&format!("export fn unused_fn_{i}(x: Int): Int = x + {i}\n"));
    }
    for i in 0..20 {
        big_lib_src.push_str(&format!("export config UnusedConfig_{i} = {{ id: Int }}\n"));
    }
    big_lib_src.push_str("export fn target_fn(x: Int): Int = x * 2\n");
    sources.insert("big_lib".to_string(), big_lib_src);

    let root_src = r#"
use big_lib
fn main_entry(x: Int): Int = big_lib::target_fn(x)
"#;
    sources.insert("root".to_string(), root_src.to_string());

    let loader = |name: &str| sources.get(name).cloned();
    let graph = ModuleGraph::load("root", &loader, ModuleLoaderLimits::default()).expect("loads");
    let linked = graph.link().expect("links");

    // The active plan must ONLY contain root::main_entry and big_lib::target_fn,
    // and NONE of the 50 unused functions or 20 configs from big_lib!
    assert_eq!(linked.configs.len(), 0);
    assert_eq!(linked.functions.len(), 2);
    assert!(linked
        .functions
        .contains_key(&QualifiedName::new("root", "main_entry")));
    assert!(linked
        .functions
        .contains_key(&QualifiedName::new("big_lib", "target_fn")));
    assert!(!linked
        .functions
        .contains_key(&QualifiedName::new("big_lib", "unused_fn_0")));
}

#[test]
fn test_bounded_loading_depth_limit_refusal() {
    let mut sources = BTreeMap::new();
    sources.insert("m0".to_string(), "use m1".to_string());
    sources.insert("m1".to_string(), "use m2".to_string());
    sources.insert("m2".to_string(), "use m3".to_string());
    sources.insert("m3".to_string(), "fn leaf(): Int = 1".to_string());

    let loader = |name: &str| sources.get(name).cloned();
    let limits = ModuleLoaderLimits {
        max_import_depth: 2, // will refuse before m3
        max_import_modules: 16,
        max_module_source_bytes: 1024,
        max_total_source_bytes: 4096,
    };

    let err = ModuleGraph::load("m0", &loader, limits).expect_err("must refuse on depth");
    match err {
        ModuleLinkError::ImportDepthExceeded {
            limit,
            found,
            chain,
        } => {
            assert_eq!(limit, 2);
            assert_eq!(found, 3);
            assert_eq!(chain, vec!["m0", "m1", "m2"]);
        }
        other => panic!("expected ImportDepthExceeded, got {other:?}"),
    }
}

#[test]
fn test_bounded_loading_module_count_limit_refusal() {
    let mut sources = BTreeMap::new();
    sources.insert("root".to_string(), "use a\nuse b\nuse c".to_string());
    sources.insert("a".to_string(), "fn a(): Int = 1".to_string());
    sources.insert("b".to_string(), "fn b(): Int = 2".to_string());
    sources.insert("c".to_string(), "fn c(): Int = 3".to_string());

    let loader = |name: &str| sources.get(name).cloned();
    let limits = ModuleLoaderLimits {
        max_import_depth: 8,
        max_import_modules: 2, // limit is 2 modules, 4 needed
        max_module_source_bytes: 1024,
        max_total_source_bytes: 4096,
    };

    let err = ModuleGraph::load("root", &loader, limits).expect_err("must refuse on count");
    match err {
        ModuleLinkError::ModuleCountExceeded { limit, found } => {
            assert_eq!(limit, 2);
            assert_eq!(found, 3);
        }
        other => panic!("expected ModuleCountExceeded, got {other:?}"),
    }
}

#[test]
fn test_bounded_loading_module_bytes_limit_refusal() {
    let mut sources = BTreeMap::new();
    sources.insert("huge".to_string(), "a".repeat(500));

    // 1. Pre-allocation refusal via SizedLoader:
    // load_fn asserts that it is NEVER CALLED when size exceeds the limit!
    let sized_loader = SizedLoader::new(
        |name: &str| sources.get(name).map(|s| s.len()),
        |name: &str| {
            if name == "huge" {
                panic!("load_module must not be called when module size exceeds limit (refusal must happen before allocation!)");
            }
            sources.get(name).cloned()
        },
    );

    let limits = ModuleLoaderLimits {
        max_import_depth: 8,
        max_import_modules: 8,
        max_module_source_bytes: 200, // 200 bytes max
        max_total_source_bytes: 4096,
    };

    let err = ModuleGraph::load("huge", &sized_loader, limits)
        .expect_err("must refuse on module bytes before allocation");
    match err {
        ModuleLinkError::ModuleBytesExceeded {
            module,
            limit,
            found,
        } => {
            assert_eq!(module, "huge");
            assert_eq!(limit, 200);
            assert_eq!(found, 500);
        }
        other => panic!("expected ModuleBytesExceeded, got {other:?}"),
    }

    // 2. Fallback refusal for closure loader (post-load check when size is not pre-announced):
    let fallback_loader = |name: &str| sources.get(name).cloned();
    let err2 = ModuleGraph::load("huge", &fallback_loader, limits)
        .expect_err("must refuse on module bytes post-load fallback");
    assert!(matches!(err2, ModuleLinkError::ModuleBytesExceeded { .. }));
}

#[test]
fn test_bounded_loading_cycle_detection() {
    let mut sources = BTreeMap::new();
    sources.insert("a".to_string(), "use b".to_string());
    sources.insert("b".to_string(), "use c".to_string());
    sources.insert("c".to_string(), "use a".to_string());

    let loader = |name: &str| sources.get(name).cloned();
    let err = ModuleGraph::load("a", &loader, ModuleLoaderLimits::default()).expect_err("cycle");
    match err {
        ModuleLinkError::Cycle(chain) => {
            assert_eq!(chain, vec!["a", "b", "c", "a"]);
        }
        other => panic!("expected Cycle, got {other:?}"),
    }
}
