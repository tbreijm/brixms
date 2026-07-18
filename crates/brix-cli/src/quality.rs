//! Compiler-grounded entry point for the public `brix quality` gate.
//!
//! The full graph-native quality rule-pack engine described by the language
//! specification is not present in Ring 0 yet.  This module still provides a
//! stable, honest CLI contract: it runs every currently implemented compiler
//! check first and then fails closed with a structured diagnostic identifying
//! the missing gate.  Consumers must never mistake static validity for a
//! quality-profile pass.

use std::collections::BTreeMap;

use brix_diag::{CanonValue, Diagnostic, Diagnostics, Span};

use crate::build::{self, BuildError, DiagnosticReport};

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

/// Reserved successful result for the eventual quality rule-pack engine.
///
/// No current code path constructs this type: `evaluate` intentionally fails
/// closed after compiler validation until all profile gates can be evaluated.
#[derive(Debug)]
pub struct QualityOutcome {
    pub source_path: camino::Utf8PathBuf,
    pub profile: QualityProfile,
}

/// Check the package with the compiler and evaluate the selected quality gate.
///
/// Static compiler failures are returned unchanged.  A statically valid
/// package receives `BRX-QUALITY-0001`, because the quality rule-pack engine is
/// not available and therefore cannot authorize a successful gate result.
pub fn evaluate(operand: &str, profile: QualityProfile) -> Result<QualityOutcome, BuildError> {
    let checked = build::check(operand)?;
    let source = std::fs::read_to_string(&checked.source_path)?;

    let structure = CanonValue::Object(BTreeMap::from([
        ("available".into(), CanonValue::Bool(false)),
        (
            "profile".into(),
            CanonValue::String(profile.as_str().into()),
        ),
        ("static_checks".into(), CanonValue::String("passed".into())),
    ]));
    let diagnostic = Diagnostic::error(
        "BRX-QUALITY-0001",
        Span::new(0, 0),
        format!(
            "quality profile `{}` was not evaluated: the quality rule-pack engine is unavailable",
            profile.as_str()
        ),
    )
    .with_structure(structure);

    Err(BuildError::Diagnostics(DiagnosticReport {
        source,
        path: checked.source_path.to_string(),
        diagnostics: Diagnostics::from_items(vec![diagnostic]),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
