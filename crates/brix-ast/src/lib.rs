//! brix-ast — Lexer (logos), hand-written recursive-descent parser, CST, AST, spans, fmt v0.
//! Ring 0 lane owner: see OWNER.md. Spec: ../../spec/BrixMS_v9_0.md
//!
//! Pipeline: [`lexer::lex`] → [`parser::parse_file`] → [`ast::File`] → [`fmt::format_file`].
//! The parser is total (never `Err`): it always returns a tree plus a
//! [`diag::Diagnostics`] list, recovering over errors so Ring 1 always has a
//! tree and a diagnostic stream to work from. Grammar gaps in Appendix D are
//! documented in `spec/errata/` rather than guessed (see the delivery report).

pub mod ast;
pub mod diag;
pub mod fmt;
pub mod lexer;
pub mod parser;
pub mod span;

pub use ast::File;
pub use diag::{Diagnostic, Diagnostics, Severity};
pub use fmt::format_file;
pub use parser::parse_file;
pub use span::{LineIndex, Span};

/// Parse then canonically format `src`. Convenience for `brix fmt`.
pub fn format_source(src: &str) -> (String, Diagnostics) {
    let (file, diags) = parse_file(src);
    (format_file(&file), diags)
}
