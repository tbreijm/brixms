//! Compiler-grounded implementation of the public `brix test` gate.
//!
//! The current compiler parses scenario declarations, but deliberately skips
//! them during lowering (`BRX-LOW-0002`) and has no scenario executor. This
//! module therefore runs the complete static compiler check, discovers parsed
//! scenarios for useful evidence, and then fails closed with a structured
//! diagnostic. A static check is evidence, but it is not test execution.

use std::collections::BTreeMap;

use brix_ast::ast::Decl;
use brix_ast::parse_file;
use brix_diag::{CanonValue, Diagnostic, Diagnostics};

use crate::build::{self, BuildError, DiagnosticReport};
use crate::package;

/// Test execution is unavailable in this compiler revision.
pub const EXECUTION_UNAVAILABLE: &str = "BRX-TEST-0001";

/// Evidence returned once a future runtime can execute the selected tests.
///
/// The type is public now so callers do not need a second API change when the
/// fail-closed implementation is replaced by an executor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestOutcome {
    pub source_path: String,
    pub selected: Vec<String>,
    pub passed: usize,
}

/// Check a package with the real compiler, then attempt to run its tests.
///
/// `selectors` are retained as opaque names for the future runner. They are
/// included in the structured diagnostic, but are not guessed to mean only
/// scenarios: later revisions may also select examples, properties, or
/// contracts.
pub fn run(operand: &str, selectors: &[String]) -> Result<TestOutcome, BuildError> {
    build::check(operand)?;

    let located = package::locate(operand).map_err(BuildError::Locate)?;
    let source = std::fs::read_to_string(&located.source_path)?;
    let (file, parse_diagnostics) = parse_file(&source);
    if parse_diagnostics.has_errors() {
        // The source may have changed after the compiler check. Never report
        // stale success evidence in that race.
        return Err(BuildError::Diagnostics(DiagnosticReport {
            source,
            path: located.source_path.to_string(),
            diagnostics: parse_diagnostics,
        }));
    }

    let scenarios = file
        .decls
        .iter()
        .filter_map(|declaration| match declaration {
            Decl::Scenario(scenario) => Some(scenario.name.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();

    let mut structure = BTreeMap::new();
    structure.insert("compiler_check_passed".into(), CanonValue::Bool(true));
    structure.insert("execution_available".into(), CanonValue::Bool(false));
    structure.insert(
        "discovered_scenarios".into(),
        CanonValue::List(scenarios.into_iter().map(CanonValue::String).collect()),
    );
    structure.insert(
        "selectors".into(),
        CanonValue::List(selectors.iter().cloned().map(CanonValue::String).collect()),
    );

    let diagnostic = Diagnostic::error(
        EXECUTION_UNAVAILABLE,
        file.package
            .as_ref()
            .map_or(file.span, |package| package.span),
        "test execution is not yet implemented: scenario declarations are parsed but skipped by compiler lowering, so no test result can be established",
    )
    .with_structure(CanonValue::Object(structure));

    Err(BuildError::Diagnostics(DiagnosticReport {
        source,
        path: located.source_path.to_string(),
        diagnostics: Diagnostics::from_items(vec![diagnostic]),
    }))
}
