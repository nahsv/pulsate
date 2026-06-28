//! Diagnostics: a coded, span-anchored error with an optional fix hint, and a
//! renderer that prints the offending source line with a caret underline.
//!
//! Every diagnostic carries a stable [`Code`] and points at a [`Span`]. The
//! rendered form mirrors the catalog example
//! (`docs/04-configuration.md#validation--error-reporting`).

use std::fmt::Write as _;

use pulsate_core::Code;

use crate::span::{Source, Span};

/// Severity of a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Blocks the config from loading.
    Error,
    /// Loads, but surfaces a problem (e.g. a deprecation).
    Warning,
}

impl Severity {
    fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

/// A single diagnostic: severity, stable code, message, span, and optional help.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    severity: Severity,
    code: Code,
    message: String,
    span: Span,
    help: Option<String>,
}

impl Diagnostic {
    /// Build an error diagnostic.
    #[must_use]
    pub fn error(code: Code, message: impl Into<String>, span: Span) -> Self {
        Self {
            severity: Severity::Error,
            code,
            message: message.into(),
            span,
            help: None,
        }
    }

    /// Build a warning diagnostic.
    #[must_use]
    pub fn warning(code: Code, message: impl Into<String>, span: Span) -> Self {
        Self {
            severity: Severity::Warning,
            code,
            message: message.into(),
            span,
            help: None,
        }
    }

    /// Attach a fix hint (the `= help:` line).
    #[must_use]
    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    /// The stable error code.
    #[must_use]
    pub fn code(&self) -> Code {
        self.code
    }

    /// The severity.
    #[must_use]
    pub fn severity(&self) -> Severity {
        self.severity
    }

    /// The span the diagnostic points at.
    #[must_use]
    pub fn span(&self) -> Span {
        self.span
    }

    /// The message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Render the diagnostic against its source, with a caret underline:
    ///
    /// ```text
    /// error[PLS-CFG-0007]: unknown upstream reference
    ///   ┌─ pulsate.flow:14:31
    ///    │
    /// 14 │   route /api/* ~> proxy(@apii)
    ///    │                         ^^^^^ no upstream named `apii` is defined
    ///    │
    ///    = help: did you mean `@api`?
    /// ```
    #[must_use]
    pub fn render(&self, source: &Source) -> String {
        let mut out = String::new();
        let line_no = self.span.line;
        let line_text = source.line_text(line_no);
        let gutter_w = line_no.to_string().len();
        let pad = " ".repeat(gutter_w);

        let _ = writeln!(
            out,
            "{}[{}]: {}",
            self.severity.as_str(),
            self.code,
            self.message
        );
        let _ = writeln!(
            out,
            "{pad}┌─ {}:{}:{}",
            source.name(),
            line_no,
            self.span.col
        );
        let _ = writeln!(out, "{pad} │");
        let _ = writeln!(out, "{line_no} │ {line_text}");

        // Caret underline: align under the offending column.
        let col = self.span.col.saturating_sub(1) as usize;
        let caret_len = self.span.len.max(1);
        let underline = format!("{}{}", " ".repeat(col), "^".repeat(caret_len));
        let _ = writeln!(out, "{pad} │ {underline}");

        if let Some(help) = &self.help {
            let _ = writeln!(out, "{pad} = help: {help}");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_points_at_the_span() {
        let src = Source::new("pulsate.flow", "route /api/* ~> proxy(@apii)\n");
        // `@apii` starts at column 23 (1-based), length 5.
        let diag = Diagnostic::error(
            Code::CFG_UNKNOWN_REF,
            "unknown upstream reference",
            Span::new(22, 5, 1, 23),
        )
        .with_help("did you mean `@api`?");
        let out = diag.render(&src);
        assert!(out.contains("error[PLS-CFG-0007]: unknown upstream reference"));
        assert!(out.contains("pulsate.flow:1:23"));
        assert!(out.contains("^^^^^"));
        assert!(out.contains("= help: did you mean `@api`?"));
    }
}
