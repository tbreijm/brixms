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

// --- issue #154: the `pub derive` orphan gate (BRX-LOW-0019) -----------------
//
// A downstream `impl Trait for Head` may extend a head owned by a dependency
// only when that dependency exported the head `pub derive`. Bare `pub`/`pub
// read` re-exports the head for *reference* but seals it against extension.

fn lower_with_dep(dep_src: &str, root_src: &str) -> Vec<brix_diag::Diagnostic> {
    let (dep_file, dep_diags) = parse_file(dep_src);
    let (root_file, root_diags) = parse_file(root_src);
    let deps = vec![DepPackage {
        name_segments: vec!["dep".to_string()],
        file: &dep_file,
        parse_diags: &dep_diags,
    }];
    lower_graph(&root_file, &root_diags, &deps).diags
}

const ROOT_EXTENDS_FOREIGN: &str = "package root @ 1.0.0\n\
use dep.{Order, Canonical}\n\
impl Canonical for Order { type Item = String }\n";

#[test]
fn extending_a_pub_derive_dependency_head_is_allowed() {
    let dep = "package dep @ 1.0.0\n\
pub derive entity Order { key ref: String }\n\
pub trait Canonical { type Item }\n";
    let diags = lower_with_dep(dep, ROOT_EXTENDS_FOREIGN);
    assert!(
        diags.iter().all(|d| d.code != "BRX-LOW-0019"),
        "a `pub derive` head must be downstream-extensible: {diags:#?}"
    );
}

#[test]
fn extending_a_bare_pub_dependency_head_is_sealed() {
    // Bare `pub` on a relation is `pub read` (least privilege) — read-only, not
    // extensible. The same foreign impl as above must now be rejected.
    let dep = "package dep @ 1.0.0\n\
pub entity Order { key ref: String }\n\
pub trait Canonical { type Item }\n";
    let diags = lower_with_dep(dep, ROOT_EXTENDS_FOREIGN);
    let sealed: Vec<_> = diags.iter().filter(|d| d.code == "BRX-LOW-0019").collect();
    assert_eq!(
        sealed.len(),
        1,
        "extending a non-`pub derive` foreign head must be one BRX-LOW-0019: {diags:#?}"
    );
}

#[test]
fn a_local_trait_may_extend_a_read_only_foreign_head() {
    // The orphan rule mirrors trait coherence: a *local* trait impl'd for a
    // foreign head is always allowed, `pub derive` or not.
    let dep = "package dep @ 1.0.0\n\
pub entity Order { key ref: String }\n";
    let root = "package root @ 1.0.0\n\
use dep.{Order}\n\
trait Local { type Item }\n\
impl Local for Order { type Item = String }\n";
    let diags = lower_with_dep(dep, root);
    assert!(
        diags.iter().all(|d| d.code != "BRX-LOW-0019"),
        "a local trait may extend any foreign head: {diags:#?}"
    );
}

// --- issue #154: the `pub derive` rule-head gate (BRX-LOW-0020) --------------
//
// `pub derive` is "extensible by a downstream package's rules" (errata 0003), so
// a downstream `derive` rule producing tuples into a foreign relation needs that
// relation exported `pub derive`; a `pub read` relation is queryable but sealed
// against downstream rules.

#[test]
fn deriving_into_a_pub_derive_dependency_relation_is_allowed() {
    let dep = "package dep @ 1.0.0\n\
pub read rel Reading { x: I64 } key(x)\n\
pub derive rel Derived { x: I64 } key(x)\n";
    let root = "package root @ 1.0.0\n\
use dep.{Reading, Derived}\n\
derive Fill: Derived(x: x) from { Reading(x: x) }\n";
    let diags = lower_with_dep(dep, root);
    assert!(
        diags.iter().all(|d| d.code != "BRX-LOW-0020"),
        "a `pub derive` relation must accept downstream rules: {diags:#?}"
    );
}

#[test]
fn deriving_into_a_read_only_dependency_relation_is_sealed() {
    let dep = "package dep @ 1.0.0\n\
pub read rel Reading { x: I64 } key(x)\n";
    let root = "package root @ 1.0.0\n\
use dep.{Reading}\n\
rel Local { x: I64 } key(x)\n\
derive Bad: Reading(x: x) from { Local(x: x) }\n";
    let diags = lower_with_dep(dep, root);
    let sealed: Vec<_> = diags.iter().filter(|d| d.code == "BRX-LOW-0020").collect();
    assert_eq!(
        sealed.len(),
        1,
        "deriving into a non-`pub derive` foreign relation must be one BRX-LOW-0020: {diags:#?}"
    );
}

// --- issue #154: the `pub write` gate (BRX-LOW-0021) ------------------------
//
// `write` = "assertable" (errata 0003): a scenario transaction that directly
// asserts into a foreign relation needs that relation exported `pub write`. This
// is a static check over the scenario's write surface — scenarios remain a v0
// defer-line skip for *execution*.

const ROOT_ASSERTS_INTO: &str = "package root @ 1.0.0\n\
use dep.{Ledger}\n\
scenario S {\n\
  seed 1\n\
  setup {\n\
    assert Ledger(x: 1)\n\
  }\n\
}\n";

#[test]
fn asserting_into_a_pub_write_dependency_relation_is_allowed() {
    let dep = "package dep @ 1.0.0\n\
pub write rel Ledger { x: I64 } key(x)\n";
    let diags = lower_with_dep(dep, ROOT_ASSERTS_INTO);
    assert!(
        diags.iter().all(|d| d.code != "BRX-LOW-0021"),
        "a `pub write` relation must accept downstream assertions: {diags:#?}"
    );
}

#[test]
fn asserting_into_a_read_only_dependency_relation_is_sealed() {
    let dep = "package dep @ 1.0.0\n\
pub read rel Ledger { x: I64 } key(x)\n";
    let diags = lower_with_dep(dep, ROOT_ASSERTS_INTO);
    let sealed: Vec<_> = diags.iter().filter(|d| d.code == "BRX-LOW-0021").collect();
    assert_eq!(
        sealed.len(),
        1,
        "asserting into a non-`pub write` foreign relation must be one BRX-LOW-0021: {diags:#?}"
    );
}

// --- issue #172: `retract`/`supersede` under the same gate -------------------
//
// Those two forms carry their target in an *expression*, not a head path, so the
// gate reaches them through the `let` binding the expression names. Every target
// reachable that way was bound by an `assert`/`set` the gate already checked, so
// the gate deliberately *suppresses* at the retraction site when the binding site
// already reported — it must never double-report one root cause. A `ClaimRef`
// arriving from anywhere other than a local write stays unpinned (that needs
// `ClaimRef<R>` type resolution; scenario bodies are never typed).

const READ_ONLY_LEDGER: &str = "package dep @ 1.0.0\n\
pub read rel Ledger { x: I64 } key(x)\n";

fn sealed_writes(dep: &str, root: &str) -> usize {
    lower_with_dep(dep, root)
        .iter()
        .filter(|d| d.code == "BRX-LOW-0021")
        .count()
}

#[test]
fn retracting_a_read_only_dependency_claim_reports_once_not_twice() {
    let root = "package root @ 1.0.0\n\
use dep.{Ledger}\n\
scenario S {\n\
  seed 1\n\
  setup {\n\
    let c = assert Ledger(x: 1)\n\
    retract c\n\
  }\n\
}\n";
    assert_eq!(
        sealed_writes(READ_ONLY_LEDGER, root),
        1,
        "the assert reports; the `retract` of the same binding must stay silent"
    );
}

#[test]
fn retracting_a_pub_write_dependency_claim_is_allowed() {
    let dep = "package dep @ 1.0.0\n\
pub write rel Ledger { x: I64 } key(x)\n";
    let root = "package root @ 1.0.0\n\
use dep.{Ledger}\n\
scenario S {\n\
  seed 1\n\
  setup {\n\
    let c = assert Ledger(x: 1)\n\
    retract c\n\
  }\n\
}\n";
    assert_eq!(
        sealed_writes(dep, root),
        0,
        "`pub write` grants retraction of a claim the scenario itself asserted"
    );
}

#[test]
fn superseding_read_only_dependency_claims_reports_once_per_assert() {
    let root = "package root @ 1.0.0\n\
use dep.{Ledger}\n\
scenario S {\n\
  seed 1\n\
  setup {\n\
    let a = assert Ledger(x: 1)\n\
    let b = assert Ledger(x: 2)\n\
    supersede a over b\n\
  }\n\
}\n";
    assert_eq!(
        sealed_writes(READ_ONLY_LEDGER, root),
        2,
        "one per `assert`; the `supersede` adds nothing for either operand"
    );
}

#[test]
fn retracting_an_unbound_reference_is_skipped() {
    let root = "package root @ 1.0.0\n\
use dep.{Ledger}\n\
scenario S {\n\
  seed 1\n\
  setup {\n\
    retract someRef\n\
  }\n\
}\n";
    assert_eq!(
        sealed_writes(READ_ONLY_LEDGER, root),
        0,
        "a target the binding map cannot resolve is skipped, not guessed at"
    );
}

#[test]
fn a_pub_fn_in_retract_position_is_not_read_as_a_relation() {
    // `export_caps` carries *every* public dependency symbol, and a bare `pub fn`
    // normalizes to `read` — so resolving a retract operand's head through the
    // resolver (rather than the binding map) would report `helper` as a sealed
    // relation. It is a function; there is no write here at all.
    let dep = "package dep @ 1.0.0\n\
pub read rel Ledger { x: I64 } key(x)\n\
pub fn helper(x: I64) -> I64 = x\n";
    let root = "package root @ 1.0.0\n\
use dep.{Ledger, helper}\n\
scenario S {\n\
  seed 1\n\
  setup {\n\
    retract helper(1)\n\
  }\n\
}\n";
    assert_eq!(
        sealed_writes(dep, root),
        0,
        "a `pub fn` in retract position is not a write target"
    );
}

#[test]
fn retracting_a_root_local_claim_is_unaffected() {
    let root = "package root @ 1.0.0\n\
rel Local { x: I64 } key(x)\n\
scenario S {\n\
  seed 1\n\
  setup {\n\
    let c = assert Local(x: 1)\n\
    retract c\n\
  }\n\
}\n";
    assert_eq!(
        sealed_writes(READ_ONLY_LEDGER, root),
        0,
        "a package retracting its own claim is never gated"
    );
}
