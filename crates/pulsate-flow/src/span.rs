//! Source spans and the diagnostic-rendering source view.
//!
//! Every token, AST node, and value carries a [`Span`] so diagnostics point at
//! the exact `(file, line, column, length)` of a problem
//! (`docs/04-configuration.md#validation--error-reporting`). Spans are
//! byte-offset based; line/column are computed on demand from the [`Source`].

/// A byte range within a single source file, plus its 1-based line/column.
///
/// The line/column are resolved at lex time from the source so downstream
/// consumers (diagnostics, the admin API) never need the original text to
/// report a location.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    /// Byte offset of the first character.
    pub offset: usize,
    /// Length in bytes.
    pub len: usize,
    /// 1-based line of the first character.
    pub line: u32,
    /// 1-based column (in characters) of the first character.
    pub col: u32,
}

impl Span {
    /// Construct a span.
    #[must_use]
    pub const fn new(offset: usize, len: usize, line: u32, col: u32) -> Self {
        Self {
            offset,
            len,
            line,
            col,
        }
    }

    /// The end byte offset (exclusive).
    #[must_use]
    pub const fn end(&self) -> usize {
        self.offset + self.len
    }

    /// A zero-length span at the start of this one (used for "expected X here").
    #[must_use]
    pub const fn collapsed(&self) -> Span {
        Span::new(self.offset, 0, self.line, self.col)
    }
}

/// A named source file with its text, used to render diagnostics with context.
#[derive(Debug, Clone)]
pub struct Source {
    name: String,
    text: String,
    /// Byte offset of the start of each line (line 1 starts at index 0).
    line_starts: Vec<usize>,
}

impl Source {
    /// Build a source from a file name and its contents.
    #[must_use]
    pub fn new(name: impl Into<String>, text: impl Into<String>) -> Self {
        let text = text.into();
        let mut line_starts = vec![0];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        Self {
            name: name.into(),
            text,
            line_starts,
        }
    }

    /// The file name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The full source text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The text of the (1-based) line containing `span`, without its newline.
    #[must_use]
    pub fn line_text(&self, line: u32) -> &str {
        let idx = (line as usize).saturating_sub(1);
        let start = self.line_starts.get(idx).copied().unwrap_or(0);
        let end = self
            .line_starts
            .get(idx + 1)
            .map_or(self.text.len(), |&n| n);
        self.text[start..end].trim_end_matches(['\n', '\r'])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_text_returns_the_right_line() {
        let src = Source::new("t.flow", "a\nbb\nccc\n");
        assert_eq!(src.line_text(1), "a");
        assert_eq!(src.line_text(2), "bb");
        assert_eq!(src.line_text(3), "ccc");
    }

    #[test]
    fn span_end_and_collapse() {
        let s = Span::new(10, 5, 2, 3);
        assert_eq!(s.end(), 15);
        assert_eq!(s.collapsed().len, 0);
    }
}
