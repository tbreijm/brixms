//! Compiler-grounded entry point for the public `brix quality` gate.
//!
//! Contract: [`QUALITY_PROFILES.md`](../QUALITY_PROFILES.md).

use std::collections::BTreeMap;

use brix_ast::ast::Decl;
use brix_diag::{CanonValue, Diagnostic, Diagnostics, Span};

use crate::build::{self, BuildError, DiagnosticReport};
use crate::lowering::{self, local_files};
use crate::package;
use crate::scenario::{classify, ScenarioClass};

/// All required rules passed.
pub const ALL_PASSED: &str = "BRX-QUALITY-0000";
/// At least one required rule failed.
pub const RULE_FAILED: &str = "BRX-QUALITY-0002";
/// No rule failed, but required evidence is unavailable.
pub const EVIDENCE_UNAVAILABLE: &str = "BRX-QUALITY-0003";

/// Standard quality profiles from BrixMS v9 Part VIII §8.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QualityProfile {
    Prototype,
    Standard,
    Production,
    Critical,
}

impl QualityProfile {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "prototype" => Some(Self::Prototype),
            "standard" => Some(Self::Standard),
            "production" => Some(Self::Production),
            "critical" => Some(Self::Critical),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prototype => "prototype",
            Self::Standard => "standard",
            Self::Production => "production",
            Self::Critical => "critical",
        }
    }
}

#[derive(Debug)]
pub struct QualityOutcome {
    pub source_path: camino::Utf8PathBuf,
    pub profile: QualityProfile,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RuleStatus {
    Passed,
    Failed,
    Unavailable,
}

impl RuleStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Clone, Debug)]
struct RuleResult {
    id: &'static str,
    min_profile: QualityProfile,
    status: RuleStatus,
    detail: String,
}

/// One local file's source, AST, canonical formatting, and lowering
/// diagnostics — a rule is evaluated across every entry in this list, never
/// just `world.brix` (issue #42: a `brix.math` submodule's coverage or
/// formatting is exactly as load-bearing as the entry's).
struct FileCtx<'a> {
    source: &'a str,
    file: &'a brix_ast::File,
    formatted: String,
    diags: &'a [Diagnostic],
}

/// Check the package with the compiler and evaluate the selected quality gate.
pub fn evaluate(operand: &str, profile: QualityProfile) -> Result<QualityOutcome, BuildError> {
    let checked = build::check(operand)?;
    let located = package::locate(operand).map_err(BuildError::Locate)?;
    let entry_label = located.source_path.to_string();
    let parsed = lowering::parse(&located)?;
    let lowered = lowering::lower(&parsed, &entry_label);

    let entry_source = parsed.entry.source.clone();
    let files: Vec<FileCtx> = local_files(&parsed, &entry_label)
        .into_iter()
        .map(|(label, pf)| {
            let diags = lowered
                .reports
                .iter()
                .find(|r| r.label == label)
                .map(|r| r.diagnostics.as_slice())
                .unwrap_or(&[]);
            FileCtx {
                source: &pf.source,
                file: &pf.file,
                formatted: brix_ast::format_file(&pf.file),
                diags,
            }
        })
        .collect();

    let rules = evaluate_rules(profile, &located, &files);

    let any_failed = rules.iter().any(|rule| rule.status == RuleStatus::Failed);
    let any_unavailable = rules
        .iter()
        .any(|rule| rule.status == RuleStatus::Unavailable);

    if any_failed {
        return Err(quality_diagnostic(
            &entry_source,
            &checked.source_path,
            profile,
            "failed",
            RULE_FAILED,
            "at least one required quality rule failed",
            &rules,
        ));
    }
    if any_unavailable {
        return Err(quality_diagnostic(
            &entry_source,
            &checked.source_path,
            profile,
            "unavailable",
            EVIDENCE_UNAVAILABLE,
            "required quality evidence is unavailable",
            &rules,
        ));
    }

    Ok(QualityOutcome {
        source_path: checked.source_path,
        profile,
    })
}

fn evaluate_rules(
    profile: QualityProfile,
    located: &package::LocatedPackage,
    files: &[FileCtx],
) -> Vec<RuleResult> {
    RULES
        .iter()
        .filter(|(_, min_profile)| profile_includes(*min_profile, profile))
        .map(|(id, min_profile)| RuleResult {
            id,
            min_profile: *min_profile,
            status: evaluate_rule(id, located, files),
            detail: rule_detail(id, located, files),
        })
        .collect()
}

const DECL_SKIPPED: &str = "BRX-LOW-0002";

const RULES: &[(&str, QualityProfile)] = &[
    ("compiler.validity", QualityProfile::Prototype),
    ("source.canonical_format", QualityProfile::Standard),
    ("package.identity", QualityProfile::Standard),
    ("compiler.semantic_coverage", QualityProfile::Standard),
    ("package.explicit_manifest", QualityProfile::Production),
    ("test.execution", QualityProfile::Production),
    ("architecture.ownership", QualityProfile::Production),
    ("architecture.capabilities", QualityProfile::Production),
    ("test.mutation", QualityProfile::Critical),
    ("conformance.result", QualityProfile::Critical),
    ("supply_chain.signatures", QualityProfile::Critical),
];

fn profile_includes(required: QualityProfile, selected: QualityProfile) -> bool {
    matches!(
        (required, selected),
        (QualityProfile::Prototype, _)
            | (
                QualityProfile::Standard,
                QualityProfile::Standard | QualityProfile::Production | QualityProfile::Critical
            )
            | (
                QualityProfile::Production,
                QualityProfile::Production | QualityProfile::Critical
            )
            | (QualityProfile::Critical, QualityProfile::Critical)
    )
}

fn evaluate_rule(id: &str, located: &package::LocatedPackage, files: &[FileCtx]) -> RuleStatus {
    match id {
        "compiler.validity" => RuleStatus::Passed,
        "source.canonical_format" => {
            if files.iter().all(|f| f.formatted == f.source) {
                RuleStatus::Passed
            } else {
                RuleStatus::Failed
            }
        }
        "package.identity" => {
            // The entry file (always first, see `local_files`) is the only
            // one allowed to carry a `package` decl.
            let Some(package_decl) = files.first().and_then(|f| f.file.package.as_ref()) else {
                return RuleStatus::Failed;
            };
            let source_name = package_decl
                .name
                .segments
                .iter()
                .map(|segment| segment.text.as_str())
                .collect::<Vec<_>>()
                .join(".");
            if located
                .manifest
                .check_matches_source_decl(&source_name, &package_decl.version.text)
                .is_ok()
            {
                RuleStatus::Passed
            } else {
                RuleStatus::Failed
            }
        }
        "compiler.semantic_coverage" => semantic_coverage_status(files),
        "package.explicit_manifest" => {
            if located.explicit_manifest {
                RuleStatus::Passed
            } else {
                RuleStatus::Failed
            }
        }
        "test.execution"
        | "architecture.ownership"
        | "architecture.capabilities"
        | "test.mutation"
        | "conformance.result"
        | "supply_chain.signatures" => RuleStatus::Unavailable,
        _ => RuleStatus::Unavailable,
    }
}

fn rule_detail(id: &str, located: &package::LocatedPackage, files: &[FileCtx]) -> String {
    match id {
        "compiler.validity" => "parse, lowering, type/effect, and phase checks passed".into(),
        "source.canonical_format" => {
            if files.iter().all(|f| f.formatted == f.source) {
                "source matches canonical formatter output".into()
            } else {
                "source differs from canonical formatter output".into()
            }
        }
        "package.identity" => {
            if evaluate_rule(id, located, files) == RuleStatus::Passed {
                "manifest identity matches source package declaration".into()
            } else {
                "manifest identity does not match source package declaration".into()
            }
        }
        "compiler.semantic_coverage" => match semantic_coverage_status(files) {
            RuleStatus::Passed => {
                "every skipped declaration is an executable scenario covered by `brix test`".into()
            }
            RuleStatus::Failed => "semantic coverage rule failed".into(),
            RuleStatus::Unavailable => {
                "skipped declarations include unsupported constructs or non-executable scenarios"
                    .into()
            }
        },
        "package.explicit_manifest" => {
            if located.explicit_manifest {
                "on-disk brix.toml present".into()
            } else {
                "package relies on synthesized manifest metadata".into()
            }
        }
        "test.execution" => "test-run evidence is not bound into quality evaluation yet".into(),
        "architecture.ownership" => {
            "resolved ownership analysis is not available in this toolchain revision".into()
        }
        "architecture.capabilities" => {
            "resolved capability analysis is not available in this toolchain revision".into()
        }
        "test.mutation" => "mutation testing is not available in this toolchain revision".into(),
        "conformance.result" => {
            "package conformance results are not available in this toolchain revision".into()
        }
        "supply_chain.signatures" => {
            "verified provenance or signature results are not available in this toolchain revision"
                .into()
        }
        _ => "unknown rule".into(),
    }
}

fn semantic_coverage_status(files: &[FileCtx]) -> RuleStatus {
    for f in files {
        let skipped_spans = f
            .diags
            .iter()
            .filter(|diag| diag.code == DECL_SKIPPED)
            .map(|diag| diag.span)
            .collect::<Vec<_>>();
        if skipped_spans.is_empty() {
            continue;
        }
        for decl in &f.file.decls {
            if !skipped_spans.iter().any(|span| decl.span() == *span) {
                continue;
            }
            match decl {
                Decl::Scenario(scenario) => {
                    if !matches!(classify(scenario), ScenarioClass::Executable) {
                        return RuleStatus::Unavailable;
                    }
                }
                _ => return RuleStatus::Unavailable,
            }
        }
    }
    RuleStatus::Passed
}

fn quality_diagnostic(
    source: &str,
    path: &camino::Utf8Path,
    profile: QualityProfile,
    status: &str,
    code: &'static str,
    message: &str,
    rules: &[RuleResult],
) -> BuildError {
    BuildError::Diagnostics(DiagnosticReport {
        source: source.to_owned(),
        path: path.to_string(),
        diagnostics: Diagnostics::from_items(vec![Diagnostic::error(
            code,
            Span::new(0, 0),
            message,
        )
        .with_structure(quality_structure(profile, status, rules))]),
    })
}

fn quality_structure(profile: QualityProfile, status: &str, rules: &[RuleResult]) -> CanonValue {
    CanonValue::Object(BTreeMap::from([
        (
            "profile".into(),
            CanonValue::String(profile.as_str().into()),
        ),
        ("status".into(), CanonValue::String(status.into())),
        (
            "rules".into(),
            CanonValue::List(
                rules
                    .iter()
                    .map(|rule| {
                        CanonValue::Object(BTreeMap::from([
                            ("id".into(), CanonValue::String(rule.id.into())),
                            (
                                "min_profile".into(),
                                CanonValue::String(rule.min_profile.as_str().into()),
                            ),
                            (
                                "status".into(),
                                CanonValue::String(rule.status.as_str().into()),
                            ),
                            ("detail".into(), CanonValue::String(rule.detail.clone())),
                        ]))
                    })
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
            "{}/brix-quality-{name}-{}",
            std::env::temp_dir().display(),
            std::process::id()
        ));
        fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn parses_only_standard_profiles() {
        assert_eq!(
            QualityProfile::parse("prototype"),
            Some(QualityProfile::Prototype)
        );
        assert_eq!(
            QualityProfile::parse("standard"),
            Some(QualityProfile::Standard)
        );
        assert_eq!(
            QualityProfile::parse("production"),
            Some(QualityProfile::Production)
        );
        assert_eq!(
            QualityProfile::parse("critical"),
            Some(QualityProfile::Critical)
        );
        assert_eq!(QualityProfile::parse("serve"), None);
    }

    #[test]
    fn prototype_passes_on_valid_source() {
        let path = tmp_source(
            "prototype",
            "package smoke.quality @ 0.1.0\n\nrel Input {\n  value: I64\n} key(value)\n",
        );
        let outcome = evaluate(path.as_str(), QualityProfile::Prototype).unwrap();
        assert_eq!(outcome.profile, QualityProfile::Prototype);
        fs::remove_file(path).ok();
    }

    #[test]
    fn production_is_unavailable_without_explicit_manifest() {
        let path = tmp_source(
            "production",
            "package smoke.quality @ 0.1.0\n\nrel Input {\n  value: I64\n} key(value)\n",
        );
        let err = evaluate(path.as_str(), QualityProfile::Production).unwrap_err();
        match err {
            BuildError::Diagnostics(report) => {
                let code = report.diagnostics.iter().next().unwrap().code;
                assert!(
                    code == RULE_FAILED || code == EVIDENCE_UNAVAILABLE,
                    "{code}"
                );
            }
            other => panic!("unexpected error: {other}"),
        }
        fs::remove_file(path).ok();
    }
}
