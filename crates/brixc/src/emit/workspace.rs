//! Assemble a standalone, on-disk-ready Cargo workspace around
//! [`super::emit_crate_root`]'s output — the file map `brix-cli`'s `build`/
//! `run` (issue #9) writes to disk and shells `cargo` against.
//!
//! `emit_crate_root` only produces one Rust source string (relation/rule
//! modules under a determinism header); it has no `Cargo.toml` and no
//! `main` (every `delta_from_*` body is still `todo!()`, so there is no
//! real driving logic to call). This module owns exactly the three files
//! that turn that string into something `cargo build`/`cargo run` can act
//! on — codegen shape stays brixc's concern, same as everything else in
//! `emit`; writing the map to disk and invoking `cargo` is `brix-cli`'s.

use std::collections::{BTreeMap, BTreeSet};

use camino::Utf8PathBuf;

use super::{emit_crate_root, RelationDesc, RuleDesc};

/// Sanitize a BrixMS package name (dotted, e.g. `demo.logistics`) into a
/// valid, filesystem- and Cargo-safe crate name. Exposed so a caller (e.g.
/// `brix-cli`) can independently compute the binary path `cargo build`
/// will produce without re-deriving `Cargo.toml`'s content.
pub fn sanitize_crate_name(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() || !out.chars().next().unwrap().is_ascii_alphabetic() {
        out.insert_str(0, "pkg_");
    }
    out
}

/// Build the in-memory file map for a standalone generated crate: a
/// `Cargo.toml` authored fresh (declaring its own empty `[workspace]` so it
/// is never absorbed by an ancestor workspace — the repo root, in
/// particular, is itself `[workspace] members = [...]`), `emit_crate_root`'s
/// output prefixed with placeholder aliases for every scaffold type name it
/// used that has no real Rust definition yet (`rust_type.rs`'s own doc
/// comment: "plausible... even though the store/runtime types don't exist
/// yet" — this is what makes that plausible enough to actually compile) as
/// `src/generated.rs`, and a small `src/main.rs` harness that links every
/// generated relation `Store` and prints a fixed marker line, proving the
/// generated code compiles and runs without calling into any `todo!()`
/// delta body.
pub fn assemble_workspace(
    package_name: &str,
    relations: &[RelationDesc],
    rules: &[RuleDesc],
) -> BTreeMap<Utf8PathBuf, String> {
    let crate_name = sanitize_crate_name(package_name);
    let mut files = BTreeMap::new();

    let generated = insert_after_header(
        &emit_crate_root(relations, rules),
        &scaffold_type_defs(relations),
    );

    files.insert(Utf8PathBuf::from("Cargo.toml"), cargo_toml(&crate_name));
    files.insert(Utf8PathBuf::from("src").join("generated.rs"), generated);
    files.insert(Utf8PathBuf::from("src").join("main.rs"), main_rs(relations));

    files
}

/// Splice `insertion` in right after `emit_crate_root`'s `#![deny(...)]`
/// determinism header — those are inner attributes, which Rust requires to
/// be the very first items in the file, so they can never move, but
/// anything module-level is free to follow them.
fn insert_after_header(crate_root: &str, insertion: &str) -> String {
    const HEADER_END: &str = "#![deny(clippy::float_arithmetic)]\n";
    match crate_root.find(HEADER_END) {
        Some(i) => {
            let split_at = i + HEADER_END.len();
            let (header, body) = crate_root.split_at(split_at);
            format!("{header}{insertion}{body}")
        }
        None => format!("{insertion}{crate_root}"),
    }
}

/// Fixed scaffold type names `rust_type.rs` can produce that have no real
/// Rust representation yet, mapped to a trivially real stand-in. Every
/// scaffold domain here is numeric-shaped, so an integer alias always
/// works (never a `float_arithmetic`-denying-lint trap, and always usable
/// as a `BTreeMap` key when a role needs one).
const FIXED_SCAFFOLD_ALIASES: &[(&str, &str)] = &[
    ("Decimal", "i128"),
    ("Instant", "i64"),
    ("Duration", "i64"),
    ("Date", "i64"),
    ("TimeOfDay", "i64"),
    ("TimeZone", "i64"),
    ("Quantity", "i64"),
    ("Money", "i64"),
    ("Probability", "i64"),
    ("EventId", "u64"),
    ("BigInt", "i128"),
    ("BigNat", "u128"),
];

/// Rust's own real primitive/std generic names — `rust_type.rs` may
/// produce these directly, and they never need a scaffold alias.
fn is_known_type_head(head: &str) -> bool {
    matches!(
        head,
        "bool"
            | "char"
            | "String"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "f32"
            | "f64"
            | "Option"
            | "Vec"
            | "Result"
            | "Estimate"
    )
}

/// Every bare identifier token in `rust_type` — nested arbitrarily deep
/// inside `Option<...>`/`Vec<...>`/etc — except fully-qualified path
/// segments (`std::collections::BTreeMap`) and the `compile_error!(...)`
/// sentinel, which name nothing to alias.
fn collect_type_heads(rust_type: &str, heads: &mut BTreeSet<String>) {
    if rust_type.starts_with("compile_error!") {
        return;
    }
    for token in rust_type.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == ':')) {
        if token.is_empty() || token.contains("::") || is_known_type_head(token) {
            continue;
        }
        heads.insert(token.to_string());
    }
}

/// Placeholder `pub type` aliases for every scaffold type name actually
/// used by `relations`, so the generated crate compiles standalone. Two
/// sources: the fixed list above, and per-entity identity names
/// (`{Entity}Id`/`{Entity}EdgeId`/`{Entity}ClaimId`) that vary by program
/// and so must be discovered by scanning, aliased to `u64` — a plausible
/// stand-in for a hash-based identity domain (Appendix G), same posture as
/// every other name `rust_type.rs` invents.
fn scaffold_type_defs(relations: &[RelationDesc]) -> String {
    let mut heads: BTreeSet<String> = BTreeSet::new();
    for rel in relations {
        for col in &rel.columns {
            collect_type_heads(&col.rust_type, &mut heads);
        }
    }

    let mut out = String::from(
        "// @generated by brixc — do not edit. Placeholder scaffold aliases\n\
         // for types with no real runtime representation yet (post-brix-rt\n\
         // work) — never real semantics, only what makes the rest of this\n\
         // file compile standalone.\n\
         pub type Estimate<T> = T;\n",
    );
    for head in &heads {
        let real = FIXED_SCAFFOLD_ALIASES
            .iter()
            .find_map(|(name, real)| (*name == head).then_some(*real))
            .unwrap_or("u64");
        out.push_str(&format!("pub type {head} = {real};\n"));
    }
    out.push('\n');
    out
}

fn cargo_toml(crate_name: &str) -> String {
    format!(
        "# @generated by brixc — do not edit.\n\
         [workspace]\n\
         \n\
         [package]\n\
         name = \"{crate_name}\"\n\
         version = \"0.0.0\"\n\
         edition = \"2021\"\n\
         publish = false\n\
         \n\
         [[bin]]\n\
         name = \"{crate_name}\"\n\
         path = \"src/main.rs\"\n"
    )
}

fn main_rs(relations: &[RelationDesc]) -> String {
    let mut checks = String::new();
    for rel in relations {
        let mod_name = super::to_snake(&rel.name);
        checks.push_str(&format!(
            "    assert!(generated::rel_{mod_name}::Store::new().is_empty());\n"
        ));
    }
    format!(
        "// @generated by brixc — do not edit. A harness proving the\n\
         // generated modules compile, link, and run — not a semantic\n\
         // executor (delta bodies are `todo!()` until join plans land).\n\
         mod generated;\n\
         \n\
         fn main() {{\n\
         {checks}\
         \x20\x20\x20\x20println!(\"brix: generated workspace OK\");\n\
         }}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emit::ColumnDesc;

    fn order_status() -> RelationDesc {
        RelationDesc {
            name: "OrderStatus".into(),
            columns: vec![ColumnDesc {
                name: "order".into(),
                rust_type: "NodeId".into(),
            }],
            key: vec!["order".into()],
        }
    }

    #[test]
    fn sanitize_crate_name_lowercases_and_strips_dots() {
        assert_eq!(sanitize_crate_name("demo.logistics"), "demo_logistics");
    }

    #[test]
    fn sanitize_crate_name_never_starts_with_a_digit() {
        assert_eq!(sanitize_crate_name("2fast"), "pkg_2fast");
    }

    #[test]
    fn assemble_workspace_produces_the_three_expected_files() {
        let files = assemble_workspace("demo.logistics", &[order_status()], &[]);
        assert!(files.contains_key(&Utf8PathBuf::from("Cargo.toml")));
        assert!(files.contains_key(&Utf8PathBuf::from("src/generated.rs")));
        assert!(files.contains_key(&Utf8PathBuf::from("src/main.rs")));
    }

    #[test]
    fn cargo_toml_declares_its_own_standalone_workspace() {
        let files = assemble_workspace("demo.logistics", &[], &[]);
        let toml = &files[&Utf8PathBuf::from("Cargo.toml")];
        assert!(toml.contains("[workspace]"));
        assert!(toml.contains("name = \"demo_logistics\""));
    }

    #[test]
    fn main_rs_checks_every_relation_store_and_never_calls_a_delta_fn() {
        let files = assemble_workspace("demo.logistics", &[order_status()], &[]);
        let main = &files[&Utf8PathBuf::from("src/main.rs")];
        assert!(main.contains("generated::rel_order_status::Store::new()"));
        assert!(!main.contains("delta_from"));
        assert!(main.contains("brix: generated workspace OK"));
    }

    #[test]
    fn generated_rs_aliases_every_scaffold_type_it_uses() {
        let files = assemble_workspace("demo.logistics", &[order_status()], &[]);
        let generated = &files[&Utf8PathBuf::from("src/generated.rs")];
        // "NodeId" (order_status's column type) is not a real Rust type —
        // it must be aliased before it's ever referenced.
        assert!(generated.contains("pub type NodeId = u64;"));
        let alias_pos = generated.find("pub type NodeId").unwrap();
        let use_pos = generated.find("pub order: NodeId").unwrap();
        assert!(alias_pos < use_pos, "alias must precede its use");
    }

    #[test]
    fn fixed_scaffold_names_get_their_documented_stand_in() {
        let rel = RelationDesc {
            name: "Delivered".into(),
            columns: vec![ColumnDesc {
                name: "at".into(),
                rust_type: "Instant".into(),
            }],
            key: vec![],
        };
        let files = assemble_workspace("demo.logistics", &[rel], &[]);
        let generated = &files[&Utf8PathBuf::from("src/generated.rs")];
        assert!(generated.contains("pub type Instant = i64;"));
    }

    #[test]
    fn real_rust_types_are_never_aliased() {
        let rel = RelationDesc {
            name: "Flag".into(),
            columns: vec![ColumnDesc {
                name: "on".into(),
                rust_type: "bool".into(),
            }],
            key: vec![],
        };
        let files = assemble_workspace("demo.logistics", &[rel], &[]);
        let generated = &files[&Utf8PathBuf::from("src/generated.rs")];
        assert!(!generated.contains("pub type bool"));
    }

    #[test]
    fn nested_generic_scaffold_types_are_still_found() {
        let rel = RelationDesc {
            name: "Maybe".into(),
            columns: vec![ColumnDesc {
                name: "amount".into(),
                rust_type: "Option<Money>".into(),
            }],
            key: vec![],
        };
        let files = assemble_workspace("demo.logistics", &[rel], &[]);
        let generated = &files[&Utf8PathBuf::from("src/generated.rs")];
        assert!(generated.contains("pub type Money = i64;"));
        assert!(!generated.contains("pub type Option"));
    }
}
