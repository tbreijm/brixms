//! Acceptance test (issue #8): `emit` produces a Rust workspace from
//! genuinely lowered + phased Core IR, not hand-built descriptors.
//!
//! `flagship_generated_workspace_snapshot` runs the full chain — parse ->
//! lower -> type-check (folded into lowering, see brixc::lower::lower_file)
//! -> phase-assign -> emit — and captures the complete generated workspace
//! as a reviewable `insta` snapshot. The other two tests are the issue's
//! explicit regression requirement: a source-level change only perturbs the
//! generated module it actually affects, and unchanged input reproduces
//! byte-identical output.

use brix_ast::parse_file;
use brixc::pipeline::PhaseAssign;
use brixc::{emit, lower_file, AstPhase};

const FLAGSHIP_SRC: &str =
    include_str!("../../brix-ast/tests/fixtures/spec/0001-part-i-the-flagship-program.brix");

#[test]
fn flagship_generated_workspace_snapshot() {
    let (file, parse_diags) = parse_file(FLAGSHIP_SRC);
    assert!(!parse_diags.has_errors(), "flagship must parse cleanly");

    let lowered = lower_file(&file, &parse_diags);
    assert!(
        !lowered.has_errors(),
        "flagship must lower cleanly: {:#?}",
        lowered.diags
    );

    let phased = AstPhase
        .assign_phases(lowered)
        .expect("flagship must be well-stratified");

    let (relations, rules) = emit::project(&phased.lowered);
    let workspace = emit::emit_crate_root(&relations, &rules);

    insta::assert_snapshot!("flagship_generated_workspace", workspace);
}

const BASE_SRC: &str = r#"
package t @ 1.0.0

rel Input { value: Int } key(value)
rel Output { value: Int } key(value)
derive R: Output(value: value) from { Input(value) }
"#;

const MUTATED_SRC: &str = r#"
package t @ 1.0.0

rel Input { value: Int; extra: String } key(value)
rel Output { value: Int } key(value)
derive R: Output(value: value) from { Input(value) }
"#;

fn generate(src: &str) -> String {
    let (file, parse_diags) = parse_file(src);
    assert!(!parse_diags.has_errors(), "fixture must parse cleanly");
    let lowered = lower_file(&file, &parse_diags);
    assert!(
        !lowered.has_errors(),
        "fixture must lower cleanly: {:#?}",
        lowered.diags
    );
    let (relations, rules) = emit::project(&lowered);
    emit::emit_crate_root(&relations, &rules)
}

/// Extract one `mod <name> { ... }` block's full text via brace matching —
/// good enough for prettyplease-formatted output, which never puts braces
/// inside a string literal in these generated modules.
fn module_text<'a>(full: &'a str, mod_name: &str) -> &'a str {
    let needle = format!("mod {mod_name} {{");
    let start = full
        .find(&needle)
        .unwrap_or_else(|| panic!("module `{mod_name}` not found in:\n{full}"));
    let mut depth = 0i32;
    for (i, ch) in full[start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &full[start..start + i + 1];
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced braces looking for module `{mod_name}`");
}

#[test]
fn source_level_change_only_perturbs_the_relevant_relation_module() {
    let base = generate(BASE_SRC);
    let mutated = generate(MUTATED_SRC);
    assert_ne!(
        base, mutated,
        "adding a role must change the generated text"
    );

    // Input gained a role -> its module changes.
    assert_ne!(
        module_text(&base, "rel_input"),
        module_text(&mutated, "rel_input")
    );
    // Output and the rule module never touch Input's roles -> unaffected.
    assert_eq!(
        module_text(&base, "rel_output"),
        module_text(&mutated, "rel_output")
    );
    assert_eq!(
        module_text(&base, "rule_r"),
        module_text(&mutated, "rule_r")
    );
}

#[test]
fn unchanged_input_produces_byte_identical_output() {
    assert_eq!(generate(BASE_SRC), generate(BASE_SRC));
}
