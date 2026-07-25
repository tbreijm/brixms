//! Cross-package trait coherence (issue #111): the §28.3 orphan rule is
//! package-graph-global, so an `impl Trait for Head` in one package that
//! overlaps an impl for the same `(trait, head)` in another must fail closed
//! with BRX-LOW-0017 — the same rule `brixc/src/lower/schema.rs` enforces
//! within a single package, now folded across the dependency graph in
//! `lower_graph`.

use brix_ast::parse_file;
use brixc::{lower_graph, DepPackage};

const DEP: &str = "package dep @ 1.0.0\n\
entity Order { key ref: String }\n\
trait Canonical { type Item }\n\
impl Canonical for Order { type Item = String }\n";

#[test]
fn a_root_impl_overlapping_a_dependency_impl_is_a_cross_package_coherence_error() {
    let root_src = "package root @ 1.0.0\n\
entity Order { key ref: String }\n\
trait Canonical { type Item }\n\
impl Canonical for Order { type Item = String }\n";
    let (dep_file, dep_diags) = parse_file(DEP);
    let (root_file, root_diags) = parse_file(root_src);
    let deps = vec![DepPackage {
        name_segments: vec!["dep".to_string()],
        file: &dep_file,
        parse_diags: &dep_diags,
    }];
    let lowered = lower_graph(&root_file, &root_diags, &deps);
    let coherence: Vec<_> = lowered
        .diags
        .iter()
        .filter(|d| d.code == "BRX-LOW-0017")
        .collect();
    assert_eq!(
        coherence.len(),
        1,
        "root impl overlapping a dependency impl must be one BRX-LOW-0017: {:#?}",
        lowered.diags
    );
}

#[test]
fn distinct_heads_across_packages_are_coherent() {
    // Root implements the same trait for a *different* head — no overlap.
    let root_src = "package root @ 1.0.0\n\
entity Invoice { key ref: String }\n\
trait Canonical { type Item }\n\
impl Canonical for Invoice { type Item = String }\n";
    let (dep_file, dep_diags) = parse_file(DEP);
    let (root_file, root_diags) = parse_file(root_src);
    let deps = vec![DepPackage {
        name_segments: vec!["dep".to_string()],
        file: &dep_file,
        parse_diags: &dep_diags,
    }];
    let lowered = lower_graph(&root_file, &root_diags, &deps);
    assert!(
        lowered.diags.iter().all(|d| d.code != "BRX-LOW-0017"),
        "distinct heads across packages must not collide: {:#?}",
        lowered.diags
    );
}
