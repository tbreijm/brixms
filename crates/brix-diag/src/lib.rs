//! The one diagnostic channel for the BrixMS toolchain.
//!
//! Diagnostics are deliberately data first: the same stable [`Diagnostic`]
//! produces a human report, a deterministic JSON document for tools, and a
//! SARIF 2.1.0 projection for code-scanning UIs.  This crate owns the common
//! source span so frontend and semantic crates never need a private diagnostic
//! type.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use miette::{
    Diagnostic as MietteDiagnostic, GraphicalReportHandler, LabeledSpan, NamedSource,
    Severity as MietteSeverity, SourceCode, SourceSpan,
};
use serde::Serialize;

/// A half-open byte range `[start, end)` into a source file.
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub const fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    pub const fn empty(at: u32) -> Self {
        Self { start: at, end: at }
    }

    pub fn to(self, other: Self) -> Self {
        Self {
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    pub fn len(self) -> u32 {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(self) -> bool {
        self.end <= self.start
    }

    pub fn as_range(self) -> std::ops::Range<usize> {
        self.start as usize..self.end as usize
    }
}

impl fmt::Debug for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}

/// A stable tool-facing Brix diagnostic identifier (`BRX0xxx` through
/// `BRX8xxx`).  The parser and lowering currently use more specific textual
/// families such as `BRX-AST-0001`; those remain accepted during the migration
/// and are normalized only by a future published code registry.
pub type BrxCode = &'static str;

/// Severity shared by compiler, CLI, and machine-readable renderers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Note,
}

/// Public rendering formats accepted by CLI and editor-facing integrations.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DiagnosticFormat {
    #[default]
    Human,
    Json,
    Sarif,
}

impl DiagnosticFormat {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "human" => Some(Self::Human),
            "json" => Some(Self::Json),
            "sarif" => Some(Self::Sarif),
            _ => None,
        }
    }
}

impl fmt::Display for DiagnosticFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Human => "human",
            Self::Json => "json",
            Self::Sarif => "sarif",
        })
    }
}

impl Severity {
    fn as_miette(self) -> MietteSeverity {
        match self {
            Self::Error => MietteSeverity::Error,
            Self::Warning => MietteSeverity::Warning,
            Self::Note => MietteSeverity::Advice,
        }
    }
}

/// A deterministic, JSON-safe structural payload.  It is intentionally not a
/// second semantic serializer: it carries diagnostic facts only, and object
/// keys are ordered so its projections are byte-stable.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(untagged)]
pub enum CanonValue {
    #[default]
    Null,
    Bool(bool),
    Integer(i64),
    String(String),
    List(Vec<Self>),
    Object(BTreeMap<String, Self>),
}

/// An optional secondary location, such as the opening delimiter paired with
/// an unclosed delimiter error.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Label {
    pub span: Span,
    pub message: String,
}

/// A map from source path/identifier to source code content for multi-source diagnostic rendering.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceMap {
    sources: BTreeMap<String, String>,
}

impl SourceMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn single(path: impl Into<String>, source: impl Into<String>) -> Self {
        let mut map = Self::new();
        map.insert(path, source);
        map
    }

    pub fn insert(&mut self, path: impl Into<String>, source: impl Into<String>) -> &mut Self {
        self.sources.insert(path.into(), source.into());
        self
    }

    pub fn get(&self, path: &str) -> Option<&str> {
        self.sources.get(path).map(|s| s.as_str())
    }

    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }
}

/// The shared diagnostic data model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub code: BrxCode,
    pub severity: Severity,
    pub span: Span,
    pub message: String,
    pub structure: CanonValue,
    pub labels: Vec<Label>,
    pub source_id: Option<String>,
}

impl Diagnostic {
    pub fn error(code: BrxCode, span: Span, message: impl Into<String>) -> Self {
        Self::new(code, Severity::Error, span, message)
    }

    pub fn warning(code: BrxCode, span: Span, message: impl Into<String>) -> Self {
        Self::new(code, Severity::Warning, span, message)
    }

    pub fn note(code: BrxCode, span: Span, message: impl Into<String>) -> Self {
        Self::new(code, Severity::Note, span, message)
    }

    pub fn new(code: BrxCode, severity: Severity, span: Span, message: impl Into<String>) -> Self {
        Self {
            code,
            severity,
            span,
            message: message.into(),
            structure: CanonValue::Null,
            labels: Vec::new(),
            source_id: None,
        }
    }

    pub fn with_source_id(mut self, source_id: impl Into<String>) -> Self {
        self.source_id = Some(source_id.into());
        self
    }

    pub fn with_label(mut self, span: Span, message: impl Into<String>) -> Self {
        self.labels.push(Label {
            span,
            message: message.into(),
        });
        self.labels.sort();
        self
    }

    pub fn with_structure(mut self, structure: CanonValue) -> Self {
        self.structure = structure;
        self
    }
}

/// A deterministic diagnostic collection.  Insertion is sorted by source
/// location and stable tie-breakers, so diagnostics emitted by separate passes
/// retain one public order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Diagnostics {
    items: Vec<Diagnostic>,
}

impl Diagnostics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, diagnostic: Diagnostic) {
        let at = self
            .items
            .binary_search_by(|existing| diagnostic_key(existing).cmp(&diagnostic_key(&diagnostic)))
            .unwrap_or_else(|at| at);
        self.items.insert(at, diagnostic);
    }

    pub fn extend(&mut self, diagnostics: impl IntoIterator<Item = Diagnostic>) {
        for diagnostic in diagnostics {
            self.push(diagnostic);
        }
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
        self.items
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error)
    }

    pub fn from_items(items: Vec<Diagnostic>) -> Self {
        let mut diagnostics = Self::new();
        diagnostics.extend(items);
        diagnostics
    }

    /// The compact deterministic human rendering used where a full source
    /// report is not available (for example lowering diagnostics).
    pub fn render(&self, source: &str, path: &str) -> String {
        self.render_compact(source, path)
    }

    pub fn render_compact(&self, source: &str, path: &str) -> String {
        let mut map = SourceMap::new();
        map.insert(path, source);
        self.render_compact_map(&map, path)
    }

    pub fn render_compact_map(&self, sources: &SourceMap, default_path: &str) -> String {
        let default_source = sources.get(default_path).unwrap_or("");
        let mut output = String::new();
        for diagnostic in &self.items {
            let file_path = diagnostic.source_id.as_deref().unwrap_or(default_path);
            let source = sources.get(file_path).unwrap_or(default_source);
            let (line, column) = line_col(source, diagnostic.span.start);
            output.push_str(&format!(
                "{file_path}:{line}:{column}: {severity}[{code}]: {message}\n",
                severity = severity_name(diagnostic.severity),
                code = diagnostic.code,
                message = diagnostic.message,
            ));
            for label in &diagnostic.labels {
                let (line, column) = line_col(source, label.span.start);
                output.push_str(&format!(
                    "    {file_path}:{line}:{column}: {}\n",
                    label.message
                ));
            }
        }
        output
    }

    /// Render source-labelled reports through miette.  This is intentionally a
    /// separate opt-in renderer because CLI modes use compact, JSON, or SARIF
    /// output depending on their consumer.
    pub fn render_miette(&self, source: &str, path: &str) -> Result<String, fmt::Error> {
        let mut map = SourceMap::new();
        map.insert(path, source);
        self.render_miette_map(&map, path)
    }

    pub fn render_miette_map(
        &self,
        sources: &SourceMap,
        default_path: &str,
    ) -> Result<String, fmt::Error> {
        let handler = GraphicalReportHandler::new();
        let mut output = String::new();
        let default_source = sources.get(default_path).unwrap_or("");
        for diagnostic in &self.items {
            let file_path = diagnostic.source_id.as_deref().unwrap_or(default_path);
            let source = sources.get(file_path).unwrap_or(default_source);
            handler.render_report(
                &mut output,
                &MietteReport::new(diagnostic, source, file_path),
            )?;
        }
        Ok(output)
    }

    /// Canonical compact JSON for editor, agent, and test consumers.
    pub fn render_json(&self, source: &str, path: &str) -> String {
        let mut map = SourceMap::new();
        map.insert(path, source);
        self.render_json_map(&map, path)
    }

    pub fn render_json_map(&self, sources: &SourceMap, default_path: &str) -> String {
        let default_source = sources.get(default_path).unwrap_or("");
        let diagnostics = self
            .items
            .iter()
            .map(|diagnostic| {
                let file_path = diagnostic.source_id.as_deref().unwrap_or(default_path);
                let source = sources.get(file_path).unwrap_or(default_source);
                JsonDiagnostic::from_diagnostic(diagnostic, source, file_path)
            })
            .collect::<Vec<_>>();
        serde_json::to_string(&JsonOutput { diagnostics }).expect("diagnostics are serializable")
    }

    /// SARIF 2.1.0 projection for CI code scanning.  The projection has one
    /// deterministic run and emits a result for every diagnostic, including
    /// warnings and notes.
    pub fn render_sarif(&self, source: &str, path: &str) -> String {
        let mut map = SourceMap::new();
        map.insert(path, source);
        self.render_sarif_map(&map, path)
    }

    pub fn render_sarif_map(&self, sources: &SourceMap, default_path: &str) -> String {
        let default_source = sources.get(default_path).unwrap_or("");
        let rules = self
            .items
            .iter()
            .map(|diagnostic| {
                serde_json::json!({
                    "id": diagnostic.code,
                    "shortDescription": { "text": diagnostic.message },
                })
            })
            .collect::<Vec<_>>();
        let results = self
            .items
            .iter()
            .map(|diagnostic| {
                let file_path = diagnostic.source_id.as_deref().unwrap_or(default_path);
                let source = sources.get(file_path).unwrap_or(default_source);
                let (start_line, start_column) = line_col(source, diagnostic.span.start);
                let (end_line, end_column) = line_col(source, diagnostic.span.end);
                serde_json::json!({
                    "ruleId": diagnostic.code,
                    "level": sarif_level(diagnostic.severity),
                    "message": { "text": diagnostic.message },
                    "locations": [{
                        "physicalLocation": {
                            "artifactLocation": { "uri": file_path },
                            "region": {
                                "startLine": start_line,
                                "startColumn": start_column,
                                "endLine": end_line,
                                "endColumn": end_column,
                            }
                        }
                    }],
                    "properties": { "structure": diagnostic.structure },
                })
            })
            .collect::<Vec<_>>();
        serde_json::json!({
            "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
            "version": "2.1.0",
            "runs": [{
                "tool": { "driver": { "name": "brix", "rules": rules } },
                "results": results,
            }],
        })
        .to_string()
    }

    pub fn render_format(&self, format: DiagnosticFormat, source: &str, path: &str) -> String {
        let mut map = SourceMap::new();
        map.insert(path, source);
        self.render_format_map(format, &map, path)
    }

    pub fn render_format_map(
        &self,
        format: DiagnosticFormat,
        sources: &SourceMap,
        default_path: &str,
    ) -> String {
        match format {
            DiagnosticFormat::Human => self.render_compact_map(sources, default_path),
            DiagnosticFormat::Json => self.render_json_map(sources, default_path),
            DiagnosticFormat::Sarif => self.render_sarif_map(sources, default_path),
        }
    }
}

fn diagnostic_key(
    diagnostic: &Diagnostic,
) -> (Option<&str>, Span, BrxCode, Severity, &str, &CanonValue) {
    (
        diagnostic.source_id.as_deref(),
        diagnostic.span,
        diagnostic.code,
        diagnostic.severity,
        &diagnostic.message,
        &diagnostic.structure,
    )
}

fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Note => "note",
    }
}

fn sarif_level(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Note => "note",
    }
}

fn line_col(source: &str, offset: u32) -> (u32, u32) {
    let bounded = (offset as usize).min(source.len());
    let line = source[..bounded]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count() as u32
        + 1;
    let line_start = source[..bounded].rfind('\n').map(|at| at + 1).unwrap_or(0);
    (line, bounded.saturating_sub(line_start) as u32 + 1)
}

#[derive(Serialize)]
struct JsonOutput {
    diagnostics: Vec<JsonDiagnostic>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonDiagnostic {
    code: BrxCode,
    severity: Severity,
    message: String,
    path: String,
    span: Span,
    start_line: u32,
    start_column: u32,
    end_line: u32,
    end_column: u32,
    structure: CanonValue,
    labels: Vec<Label>,
}

impl JsonDiagnostic {
    fn from_diagnostic(diagnostic: &Diagnostic, source: &str, path: &str) -> Self {
        let (start_line, start_column) = line_col(source, diagnostic.span.start);
        let (end_line, end_column) = line_col(source, diagnostic.span.end);
        Self {
            code: diagnostic.code,
            severity: diagnostic.severity,
            message: diagnostic.message.clone(),
            path: path.to_owned(),
            span: diagnostic.span,
            start_line,
            start_column,
            end_line,
            end_column,
            structure: diagnostic.structure.clone(),
            labels: diagnostic.labels.clone(),
        }
    }
}

#[derive(Debug)]
struct MietteReport {
    diagnostic: Diagnostic,
    source: NamedSource<String>,
}

impl MietteReport {
    fn new(diagnostic: &Diagnostic, source: &str, path: &str) -> Self {
        Self {
            diagnostic: diagnostic.clone(),
            source: NamedSource::new(path, source.to_owned()),
        }
    }
}

impl fmt::Display for MietteReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.diagnostic.message)
    }
}

impl Error for MietteReport {}

impl MietteDiagnostic for MietteReport {
    fn code<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        Some(Box::new(self.diagnostic.code))
    }

    fn severity(&self) -> Option<MietteSeverity> {
        Some(self.diagnostic.severity.as_miette())
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        let primary = LabeledSpan::new_with_span(
            Some("here".to_owned()),
            SourceSpan::from((
                self.diagnostic.span.start as usize,
                self.diagnostic.span.len() as usize,
            )),
        );
        let labels = std::iter::once(primary).chain(self.diagnostic.labels.iter().map(|label| {
            LabeledSpan::new_with_span(
                Some(label.message.clone()),
                SourceSpan::from((label.span.start as usize, label.span.len() as usize)),
            )
        }));
        Some(Box::new(labels))
    }

    fn source_code(&self) -> Option<&dyn SourceCode> {
        Some(&self.source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projections_are_deterministic_and_sorted() {
        let mut diagnostics = Diagnostics::new();
        diagnostics.push(Diagnostic::warning("BRX1xxx", Span::new(5, 6), "later"));
        diagnostics.push(
            Diagnostic::error("BRX0xxx", Span::new(0, 4), "earlier").with_structure(
                CanonValue::Object(BTreeMap::from([(
                    "path".to_owned(),
                    CanonValue::List(vec![CanonValue::String("a".to_owned())]),
                )])),
            ),
        );

        let json = diagnostics.render_json("oops\nwarn", "test.brix");
        assert_eq!(json, diagnostics.render_json("oops\nwarn", "test.brix"));
        assert!(json.find("BRX0xxx").unwrap() < json.find("BRX1xxx").unwrap());

        let sarif = diagnostics.render_sarif("oops\nwarn", "test.brix");
        assert_eq!(sarif, diagnostics.render_sarif("oops\nwarn", "test.brix"));
        assert!(sarif.contains("https://json.schemastore.org/sarif-2.1.0.json"));
    }

    #[test]
    fn miette_and_compact_renderers_include_stable_code() {
        let mut diagnostics = Diagnostics::new();
        diagnostics.push(Diagnostic::error("BRX0xxx", Span::new(0, 4), "bad input"));
        insta::assert_snapshot!(
            diagnostics.render_compact("oops", "test.brix"),
            @"test.brix:1:1: error[BRX0xxx]: bad input
        "
        );
        let miette = diagnostics.render_miette("oops", "test.brix").unwrap();
        assert!(miette.contains("BRX0xxx"));
        assert!(miette.contains("bad input"));
    }
}
