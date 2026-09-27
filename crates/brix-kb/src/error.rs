//! Unified error type for `brix-kb` operations (ADR-0041).
//!
//! Mirrors the shape of `brix-cli`'s `CliInputError` (`code`, `message`,
//! `status`, `exit_code`) so the CLI layer (`brix-cli/src/commands/kb.rs`) can
//! render a `KbError` with the exact same human/JSON conventions as `brix
//! run`/`brix audit`/`brix verify`, without `brix-kb` depending on `brix-cli`.
//!
//! Exit code convention (matches `crates/brix-cli/src/cli.rs`):
//! - 0 success
//! - 1 rejected / unknown / verification failure (`status` `"rejected"` or `"unknown"`)
//! - 2 usage or IO error (`status` `"usage-error"` or `"io-error"`)

use std::fmt;

use brix_lower::finite_decision::FiniteDecisionLowerError;
use brix_lower::imports::ImportError;
use brix_lower::input::{InputError, InputValidationError};
use brix_lower::FiniteDecisionBuildError;

pub const EXIT_SUCCESS: u8 = 0;
pub const EXIT_REJECTED_OR_UNKNOWN: u8 = 1;
pub const EXIT_USAGE_OR_IO: u8 = 2;

/// A `brix-kb` operation error, carrying enough structure for the CLI layer to
/// render both human and `--json` output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KbError {
    pub code: &'static str,
    pub message: String,
    pub status: &'static str,
    pub exit_code: u8,
}

impl KbError {
    pub fn new(
        code: &'static str,
        message: impl Into<String>,
        status: &'static str,
        exit_code: u8,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            status,
            exit_code,
        }
    }

    pub fn usage(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(code, message, "usage-error", EXIT_USAGE_OR_IO)
    }

    pub fn io(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(code, message, "io-error", EXIT_USAGE_OR_IO)
    }

    pub fn rejected(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(code, message, "rejected", EXIT_REJECTED_OR_UNKNOWN)
    }

    pub fn unknown(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(code, message, "unknown", EXIT_REJECTED_OR_UNKNOWN)
    }

    pub fn diagnostic(&self) -> String {
        format!("{}: {}", self.code, self.message)
    }
}

impl fmt::Display for KbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.status, self.message)
    }
}

impl std::error::Error for KbError {}

impl From<InputError> for KbError {
    fn from(err: InputError) -> Self {
        match err {
            InputError::Decode(brix_lower::input::InputDecodeError::IoError { path, message }) => {
                Self::io(
                    "input-io-error",
                    format!("failed to read input file '{path}': {message}"),
                )
            }
            InputError::Decode(brix_lower::input::InputDecodeError::NotARegularFile(path)) => {
                Self::io(
                    "input-io-error",
                    format!("input path is not a regular file: {path}"),
                )
            }
            other => Self::rejected("input-decode-error", other.to_string()),
        }
    }
}

impl From<InputValidationError> for KbError {
    fn from(err: InputValidationError) -> Self {
        Self::rejected("input-validation-error", err.to_string())
    }
}

impl From<FiniteDecisionBuildError> for KbError {
    fn from(err: FiniteDecisionBuildError) -> Self {
        match err {
            FiniteDecisionBuildError::InputValidation(iv) => Self::from(iv),
            FiniteDecisionBuildError::MissingProposal { candidate } => Self::rejected(
                "missing-proposal",
                format!("candidate '{candidate}' in commit was not found in declared proposals"),
            ),
        }
    }
}

impl From<FiniteDecisionLowerError> for KbError {
    fn from(err: FiniteDecisionLowerError) -> Self {
        Self::rejected("lowering-error", format!("lowering error: {err}"))
    }
}

impl From<ImportError> for KbError {
    fn from(err: ImportError) -> Self {
        Self::rejected("import-error", format!("import error: {err:?}"))
    }
}
