//! Parser diagnostics.
//!
//! **Interface gap (see this crate's delivery report):** `brix-diag` currently
//! exports no public items at all — it is a stub crate (doc comment only).
//! The contract for this lane says to "emit diagnostics through brix-diag
//! ... if a type you need isn't there yet, use its current public API and
//! note the gap" — there is no current public API to use, so the minimal
//! `Diagnostic`/`Severity`/`Diagnostics` types below live here as a local
//! stand-in. They are deliberately small and shaped like what brix-diag will
//! almost certainly want (a stable code, a severity, a primary span, a
//! message, optional labeled secondary spans) so migrating call sites to a
//! real `brix_diag::Diagnostic` later should be close to a rename.
//!
//! `brix-ast` still takes a `brix-diag` dependency (see `Cargo.toml`) so that
//! migration is a one-crate, no-new-dependency change whenever brix-diag
//! grows the real type.

use crate::span::Span;

/// Diagnostic severity. Errors block a clean parse of the affected node but
/// never stop the parser from recovering and continuing (diagnostic quality
/// is the product of this lane).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Warning,
    Error,
}

/// One diagnostic. `code` is a stable `BRX-AST-NNNN` identifier so tooling
/// (and Ring 1 consumers) can match on it without parsing the message.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: &'static str,
    pub message: String,
    pub span: Span,
    /// Additional spans with their own short labels, e.g. "unclosed brace
    /// opened here".
    pub labels: Vec<(Span, String)>,
}

impl Diagnostic {
    pub fn error(code: &'static str, span: Span, message: impl Into<String>) -> Self {
        Diagnostic {
            severity: Severity::Error,
            code,
            message: message.into(),
            span,
            labels: Vec::new(),
        }
    }

    /// A non-blocking diagnostic: the node still parsed (structurally), but
    /// is flagged for attention (e.g. a spec-prose placeholder). Warnings do
    /// not count toward [`Diagnostics::has_errors`], so they never block a
    /// clean parse.
    pub fn warning(code: &'static str, span: Span, message: impl Into<String>) -> Self {
        Diagnostic {
            severity: Severity::Warning,
            code,
            message: message.into(),
            span,
            labels: Vec::new(),
        }
    }

    pub fn with_label(mut self, span: Span, label: impl Into<String>) -> Self {
        self.labels.push((span, label.into()));
        self
    }
}

/// A parse's accumulated diagnostics, in emission order (which is source
/// order for a left-to-right recursive-descent parser).
#[derive(Debug, Clone, Default)]
pub struct Diagnostics {
    items: Vec<Diagnostic>,
}

impl Diagnostics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, d: Diagnostic) {
        self.items.push(d);
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Diagnostic> {
        self.items.iter()
    }

    pub fn has_errors(&self) -> bool {
        self.items.iter().any(|d| d.severity == Severity::Error)
    }

    /// Render one diagnostic as `path:line:col: severity[code]: message`,
    /// the minimal human-readable form used by the corpus tests. A richer
    /// `miette`-backed renderer belongs to brix-diag once it exists.
    pub fn render(&self, src: &str, path: &str) -> String {
        let idx = crate::span::LineIndex::new(src);
        let mut out = String::new();
        for d in &self.items {
            let (line, col) = idx.line_col(d.span.start);
            let sev = match d.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
            };
            out.push_str(&format!(
                "{path}:{line}:{col}: {sev}[{code}]: {msg}\n",
                path = path,
                line = line,
                col = col,
                code = d.code,
                msg = d.message
            ));
            for (span, label) in &d.labels {
                let (l, c) = idx.line_col(span.start);
                out.push_str(&format!("    {path}:{l}:{c}: {label}\n", path = path));
            }
        }
        out
    }
}
