//! Compiler-grounded implementation of the public `brix test` gate.
//!
//! Contract: [`TEST_SCENARIOS.md`](../TEST_SCENARIOS.md).

use std::collections::BTreeMap;

use brix_ast::parse_file;
use brix_diag::{CanonValue, Diagnostic, Diagnostics, Span};

use crate::build::{self, BuildError, DiagnosticReport};
use crate::package;
use crate::scenario::{
    self, resolve_selectors, scenario_evidence, scenarios_in_source_order, ScenarioRun,
    SelectorError, SUPPORTED_SUBSET_VERSION,
};

/// Selected scenario semantics are unavailable.
pub const EXECUTION_UNAVAILABLE: &str = "BRX-TEST-0001";
/// A supported assertion evaluated to false.
pub const ASSERTION_FAILED: &str = "BRX-TEST-0002";
/// Selectors are unknown or scenario names are ambiguous.
pub const SELECTOR_ERROR: &str = "BRX-TEST-0003";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestOutcome {
    pub source_path: String,
    pub selected: Vec<String>,
    pub passed: usize,
}

/// Check a package with the real compiler, then execute selected scenarios.
pub fn run(operand: &str, selectors: &[String]) -> Result<TestOutcome, BuildError> {
    build::check(operand, false)?;

    let located = package::locate(operand).map_err(BuildError::Locate)?;
    let source = std::fs::read_to_string(&located.source_path)?;
    let (file, parse_diagnostics) = parse_file(&source);
    if parse_diagnostics.has_errors() {
        return Err(BuildError::Diagnostics(DiagnosticReport::single(
            source,
            located.source_path.to_string(),
            parse_diagnostics,
        )));
    }

    let scenarios = scenarios_in_source_order(&file);
    let selected = match resolve_selectors(scenarios, selectors) {
        Ok(selected) => selected,
        Err(SelectorError::Unknown(name)) => {
            return Err(selector_diagnostic(
                &source,
                &located.source_path,
                &file,
                selectors,
                SELECTOR_ERROR,
                format!("unknown test selector `{name}`"),
            ));
        }
        Err(SelectorError::Ambiguous) => {
            return Err(selector_diagnostic(
                &source,
                &located.source_path,
                &file,
                selectors,
                SELECTOR_ERROR,
                "scenario names are ambiguous or duplicated".into(),
            ));
        }
    };

    let mut runs = Vec::new();
    let mut assertion_failed = false;
    let mut unavailable = false;
    for scenario in &selected {
        let run = scenario::execute(scenario);
        match &run {
            ScenarioRun::Executed { assertions } => {
                if assertions.iter().any(|assert| !assert.passed) {
                    assertion_failed = true;
                }
            }
            ScenarioRun::Unavailable { .. } => unavailable = true,
        }
        runs.push((scenario.name.text.clone(), run));
    }

    if assertion_failed {
        return Err(test_diagnostic(
            &source,
            &located.source_path,
            selectors,
            "failed",
            ASSERTION_FAILED,
            "a supported assertion evaluated to false",
            &runs,
        ));
    }
    if unavailable {
        return Err(test_diagnostic(
            &source,
            &located.source_path,
            selectors,
            "unavailable",
            EXECUTION_UNAVAILABLE,
            "selected scenario semantics are unavailable",
            &runs,
        ));
    }

    Ok(TestOutcome {
        source_path: located.source_path.to_string(),
        selected: runs.iter().map(|(name, _)| name.clone()).collect(),
        passed: runs.len(),
    })
}

fn selector_diagnostic(
    source: &str,
    path: &camino::Utf8Path,
    file: &brix_ast::File,
    selectors: &[String],
    code: &'static str,
    message: String,
) -> BuildError {
    BuildError::Diagnostics(DiagnosticReport::single(
        source.to_owned(),
        path.to_string(),
        Diagnostics::from_items(vec![Diagnostic::error(
            code,
            file.package
                .as_ref()
                .map_or(file.span, |package| package.span),
            message,
        )
        .with_structure(test_structure(selectors, "failed", &[]))]),
    ))
}

fn test_diagnostic(
    source: &str,
    path: &camino::Utf8Path,
    selectors: &[String],
    status: &str,
    code: &'static str,
    message: &str,
    runs: &[(String, ScenarioRun)],
) -> BuildError {
    BuildError::Diagnostics(DiagnosticReport::single(
        source.to_owned(),
        path.to_string(),
        Diagnostics::from_items(vec![Diagnostic::error(code, Span::new(0, 0), message)
            .with_structure(test_structure(selectors, status, runs))]),
    ))
}

fn test_structure(
    selectors: &[String],
    status: &str,
    runs: &[(String, ScenarioRun)],
) -> CanonValue {
    CanonValue::Object(BTreeMap::from([
        ("status".into(), CanonValue::String(status.into())),
        (
            "selectors".into(),
            CanonValue::List(selectors.iter().cloned().map(CanonValue::String).collect()),
        ),
        (
            "supported_subset_version".into(),
            CanonValue::String(SUPPORTED_SUBSET_VERSION.into()),
        ),
        (
            "scenarios".into(),
            CanonValue::List(
                runs.iter()
                    .map(|(name, run)| scenario_evidence(name, run))
                    .collect(),
            ),
        ),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use camino::Utf8PathBuf;

    fn tmp_source(name: &str, contents: &str) -> Utf8PathBuf {
        let path = Utf8PathBuf::from(format!(
            "{}/brix-test-{name}-{}",
            std::env::temp_dir().display(),
            std::process::id()
        ));
        fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn runs_executable_scenario_successfully() {
        let path = tmp_source(
            "pass",
            "package smoke.test @ 0.1.0\n\
             rel Input { value: I64 } key(value)\n\
             scenario Smoke {\n\
               seed 1\n\
               assert at end { true }\n\
             }\n",
        );
        let outcome = run(path.as_str(), &["Smoke".into()]).unwrap();
        assert_eq!(outcome.passed, 1);
        fs::remove_file(path).ok();
    }

    #[test]
    fn reports_false_assertion_as_brx_test_0002() {
        let path = tmp_source(
            "fail",
            "package smoke.test @ 0.1.0\n\
             scenario Smoke {\n\
               seed 1\n\
               assert at end { false }\n\
             }\n",
        );
        let err = run(path.as_str(), &["Smoke".into()]).unwrap_err();
        match err {
            BuildError::Diagnostics(report) => {
                assert_eq!(
                    report.diagnostics.iter().next().unwrap().code,
                    ASSERTION_FAILED
                );
            }
            other => panic!("unexpected error: {other}"),
        }
        fs::remove_file(path).ok();
    }
}
