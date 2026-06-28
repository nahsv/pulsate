//! `pulsate-flow` — the Flow configuration language.
//!
//! A hand-written lexer and recursive-descent parser turn `pulsate.flow` source
//! into a typed [`Ast`] with span-accurate [`Diagnostic`]s. The crate is
//! deliberately dependency-light and domain-agnostic: it knows Flow's *shape*
//! (blocks, directives, routes, typed values) but not its *meaning* — the
//! keyword set, references, and invariants live in `pulsate-config`, which lowers
//! the AST into the typed config model (`docs/04-configuration.md`).
#![forbid(unsafe_code)]

pub mod ast;
pub mod diag;
pub mod lexer;
pub mod parser;
pub mod span;
pub mod value;

#[doc(inline)]
pub use ast::Ast;
#[doc(inline)]
pub use diag::{Diagnostic, Severity};
#[doc(inline)]
pub use span::{Source, Span};
#[doc(inline)]
pub use value::{RateWindow, Spanned, Value};

/// Lex and parse Flow source text into an [`Ast`].
///
/// This is the entry point `pulsate-config` calls. The `name` is used only for
/// diagnostic rendering; pair the returned error with the [`Source`] to render
/// it (see [`Diagnostic::render`]).
///
/// # Errors
/// Returns the first [`Diagnostic`] produced by lexing or parsing.
pub fn parse(name: &str, text: &str) -> Result<Ast, Diagnostic> {
    let _ = name; // reserved for multi-file diagnostics; kept for a stable API
    let tokens = lexer::lex(text)?;
    parser::parse(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_to_end_parse_of_a_small_config() {
        let src = r#"
            flow_version "1"
            upstream api { target http://127.0.0.1:8080; policy least_conn }
            site app.example.com {
              tls auto
              route /api/* ~> compress ~> proxy(@api)
            }
        "#;
        let ast = parse("pulsate.flow", src).expect("parses");
        assert_eq!(ast.stmts.len(), 3);
    }

    #[test]
    fn a_syntax_error_renders_with_a_span() {
        let src = "site app.example.com {\n  route /api/* ~> proxy(@api\n}";
        let err = parse("pulsate.flow", src).unwrap_err();
        let source = Source::new("pulsate.flow", src);
        let rendered = err.render(&source);
        assert!(rendered.contains("PLS-CFG-0001"));
        assert!(rendered.contains("pulsate.flow:"));
    }
}
