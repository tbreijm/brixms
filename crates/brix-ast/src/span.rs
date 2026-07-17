//! Byte-offset spans shared by the CST, AST, and diagnostics.
//!
//! Spans are plain `[start, end)` byte offsets into the source text. Line/column
//! information is derived on demand (see [`LineIndex`]) rather than carried on
//! every node, so span arithmetic (merging, shrinking) stays cheap.

/// A half-open `[start, end)` byte range into one source file.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub const fn new(start: u32, end: u32) -> Self {
        Span { start, end }
    }

    /// An empty span at `at`, used for synthesized/missing nodes during
    /// error recovery.
    pub const fn empty(at: u32) -> Self {
        Span { start: at, end: at }
    }

    /// The smallest span covering both `self` and `other`.
    pub fn to(self, other: Span) -> Span {
        Span {
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

impl std::fmt::Debug for Span {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}..{}", self.start, self.end)
    }
}

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
