//! `parse_bounded_with_source_map` must return the exact same [`Module`] as
//! `parse_bounded` for every real `.brix` source in the repository — the
//! sidecar source map is purely additive bookkeeping alongside the same
//! parse, never a second, possibly-diverging code path (see
//! `crates/brix-syntax/src/source_map.rs`'s module doc).

use std::path::PathBuf;

use brix_syntax::{parse_bounded, parse_bounded_with_source_map, ParseLimits};

fn repo_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .expect("crates parent")
        .parent()
        .expect("repo root")
        .to_path_buf()
}

fn assert_same_module(label: &str, source: &str) {
    let limits = ParseLimits::strict();
    let plain = parse_bounded(source, limits).unwrap_or_else(|e| {
        panic!("{label}: parse_bounded failed: {e}");
    });
    let (with_map, source_map) =
        parse_bounded_with_source_map(source, limits).unwrap_or_else(|e| {
            panic!("{label}: parse_bounded_with_source_map failed: {e}");
        });
    assert_eq!(
        plain, with_map,
        "{label}: parse_bounded and parse_bounded_with_source_map must return equal modules"
    );
    // The source map must record exactly one entry per top-level item.
    assert_eq!(
        source_map.items.len(),
        plain.items.len(),
        "{label}: source map item count must match module item count"
    );
}

#[test]
fn parse_bounded_and_with_source_map_agree_on_every_example() {
    let examples_dir = repo_root().join("examples");
    let mut checked = 0;
    for entry in std::fs::read_dir(&examples_dir).expect("read examples dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("brix") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("read example source");
        assert_same_module(&path.display().to_string(), &source);
        checked += 1;
    }
    assert!(checked > 0, "expected at least one examples/*.brix file");
}

#[test]
fn parse_bounded_and_with_source_map_agree_on_brix_soc() {
    let soc_path = repo_root().join("packages/brix.soc/src/soc.brix");
    let source = std::fs::read_to_string(&soc_path).expect("read packages/brix.soc/src/soc.brix");
    assert_same_module("packages/brix.soc/src/soc.brix", &source);
}

#[test]
fn source_map_resolves_a_propose_dependency_identifier() {
    let source = "config Decision = A\n\nrule x() = 1\n\npropose a(y) priority 1 when true = A\n\ncommit c from (a)\n";
    // `y` is not an earlier rule, so this is malformed as a *program*, but
    // parsing (and the source map) must still succeed — the source map is
    // built directly from tokens, independent of whether lowering would
    // later accept the module.
    let (_module, source_map) =
        parse_bounded_with_source_map(source, ParseLimits::strict()).expect("parses");
    let item = source_map
        .find_item("a")
        .expect("propose item 'a' recorded");
    assert_eq!(item.kind, "propose");
    let dep = item.find_ident("y").expect("dependency 'y' recorded");
    assert_eq!(dep.line, 5);
    // `propose a(y) ...`: 'y' is the 11th character (1-based).
    assert_eq!(dep.col, 11);
}

#[test]
fn source_map_prefers_the_last_occurrence_of_a_duplicated_name() {
    let source = "config Decision = A\n\npropose a() priority 1 when true = A\npropose a() priority 2 when true = A\n\ncommit c from (a)\n";
    let (_module, source_map) =
        parse_bounded_with_source_map(source, ParseLimits::strict()).expect("parses");
    let item = source_map
        .find_item("a")
        .expect("propose item 'a' recorded");
    // The second `propose a` declaration starts on line 4.
    assert_eq!(item.span.start_line, 4);
}
