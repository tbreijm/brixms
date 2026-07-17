//! Byte-offset spans shared by the CST, AST, and diagnostics.
//!
//! Spans are plain `[start, end)` byte offsets into the source text. Line/column
//! information is derived on demand (see [`LineIndex`]) rather than carried on
//! every node, so span arithmetic (merging, shrinking) stays cheap.

pub use brix_diag::Span;

/// Maps byte offsets to 1-based (line, column) pairs for human-facing
/// diagnostics. Built once per source file.
pub struct LineIndex {
    /// Byte offset of the start of each line.
    line_starts: Vec<u32>,
}

impl LineIndex {
    pub fn new(src: &str) -> Self {
        let mut line_starts = vec![0u32];
        for (i, b) in src.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i as u32 + 1);
            }
        }
        LineIndex { line_starts }
    }

    /// 1-based (line, column) for a byte offset. Column counts UTF-8 bytes
    /// since the start of the line (good enough for v0 diagnostics; a
    /// grapheme-aware column is a fmt/diag-lane follow-up, not a parser
    /// concern).
    pub fn line_col(&self, offset: u32) -> (u32, u32) {
        let line = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let col = offset - self.line_starts[line];
        (line as u32 + 1, col + 1)
    }
}
